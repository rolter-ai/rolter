//! The dashboard UX event pipeline, end to end against a real ClickHouse
//! (#1728).
//!
//! Every hop of this chain already has tests of its own: `ui/src/lib/ux.test.ts`
//! covers the browser queue against a mocked `fetch`, and the unit tests in
//! `ui_events.rs` cover validation and row construction against no database at
//! all. Neither answers the question the dogfood week depends on — *do the rows
//! arrive, and do they say what the interaction said?* A queue that stamps `ts`
//! correctly and a handler that builds a correct row still capture nothing if
//! the table is missing, the column types disagree, or the insert is accepted
//! and dropped.
//!
//! So this drives the real endpoint with the exact payload `ux.ts` sends, then
//! reads the rows back out of ClickHouse and asserts screen, action and `ts`.
//!
//! It also pins the *failure* modes, because the client deliberately swallows
//! every one of them: what status the server really returns when ClickHouse is
//! down, when a batch is rejected, and when the session is gone. `ux.ts`
//! branches on exactly those statuses — 401/403/404/405 disable the stream for
//! the life of the tab, anything else drops the batch and keeps going — so the
//! statuses asserted here are what decide whether an operator's week is
//! partially or entirely uncaptured.
//!
//! Gated on the `postgres` feature and on both `ROLTER_TEST_DATABASE_URL` and
//! `ROLTER_TEST_CLICKHOUSE_URL`; unset either and the tests self-skip.
#![cfg(feature = "postgres")]

use std::net::SocketAddr;

use chrono::Timelike;

use rolter_store::postgres::test_database;
use rolter_store::postgres::test_schema::TestSchema;
use serde_json::{json, Value};

/// The ClickHouse this suite writes to. A *server* pointer, like
/// `ROLTER_TEST_DATABASE_URL`: unset means "no column store here", not "fail".
const CLICKHOUSE_URL_ENV: &str = "ROLTER_TEST_CLICKHOUSE_URL";

fn clickhouse_url() -> Option<String> {
    std::env::var(CLICKHOUSE_URL_ENV)
        .ok()
        .map(|url| url.trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())
}

macro_rules! skip_without_stack {
    () => {{
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
        match clickhouse_url() {
            Some(url) => url,
            None => {
                eprintln!("skipping: {CLICKHOUSE_URL_ENV} not set");
                return;
            }
        }
    }};
}

/// Apply the shipped `ui_events` DDL, and every migration that has widened it.
///
/// Read from `clickhouse/` rather than repeated here on purpose: a copy would
/// let the table this test writes to drift away from the one a deployment gets,
/// which is precisely the class of failure the test exists to catch. Every
/// statement is `create table if not exists` or an `alter`, so this is also the
/// repair for the failure mode a dogfood stack is most likely to hit — a
/// ClickHouse volume created before #805 landed never ran the init scripts
/// again and has no `ui_events` table at all.
///
/// All of them, not just the `create`: the `action` column is an `Enum8` that
/// `010_*` appended to, and a test that only ran the `create` would reject
/// every value added since while the deployment accepted it (#1731).
async fn ensure_table(client: &reqwest::Client, base: &str) {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../clickhouse"));
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("read the clickhouse migration directory")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_str()?.to_string();
            // the numeric prefix is the order, and it is why these are sorted
            // by file name rather than taken as the directory hands them over
            name.contains("ui_events").then_some((name, path))
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no ui_events DDL found in clickhouse/");

    for (name, path) in files {
        let ddl = std::fs::read_to_string(&path).expect("read the shipped ui_events DDL");
        let response = client
            .post(format!("{base}/"))
            .body(ddl)
            .send()
            .await
            .expect("reach clickhouse");
        assert!(
            response.status().is_success(),
            "{name} failed: {}",
            response.text().await.unwrap_or_default()
        );
    }
}

/// The rows this session wrote, oldest first.
///
/// `session_id` is how these tests isolate from each other and from whatever
/// else shares the ClickHouse — the Postgres answer, a schema per test, has no
/// analogue here because the insert statement names `ui_events` outright. An
/// append-only table tagged per test is the equivalent guarantee: no test can
/// see another's rows, and none of them can be confused by a dogfood stack's.
async fn rows_for_session(client: &reqwest::Client, base: &str, session_id: &str) -> Vec<Value> {
    let sql = "select toString(ts) as ts, event_id, screen, action, target, outcome, \
               duration_ms, from_screen, user_id, app_version \
               from ui_events where session_id = {session:String} order by ts \
               FORMAT JSON";
    let response = client
        .post(format!("{base}/"))
        .query(&[("param_session", session_id)])
        .body(sql)
        .send()
        .await
        .expect("reach clickhouse");
    assert!(response.status().is_success(), "read-back query failed");
    let body: Value = response.json().await.expect("clickhouse json");
    body["data"].as_array().cloned().unwrap_or_default()
}

async fn fresh_db() -> TestSchema {
    let url = test_database::url()
        .await
        .expect("ROLTER_TEST_DATABASE_URL checked by caller");
    TestSchema::create(&url).await
}

async fn serve(app: axum::Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// A live session for a seeded local user, returned as the bearer token the
/// dashboard sends. The digest uses the empty pepper the extractor falls back
/// to when `ROLTER_SESSION_PEPPER` is unset.
async fn seed_session(pool: &sqlx::PgPool, email: &str) -> (uuid::Uuid, String) {
    let user_id: uuid::Uuid = sqlx::query_scalar(
        "insert into users (email, password_hash, is_superadmin) values ($1, null, true) \
         returning id",
    )
    .bind(email)
    .fetch_one(pool)
    .await
    .unwrap();
    let token = format!("rolter_sess_ux_{}", uuid::Uuid::new_v4().simple());
    let token_hash = rolter_auth::hash_key("", &token);
    sqlx::query(
        "insert into sessions (user_id, token_hash, expires_at) \
         values ($1, $2, now() + interval '1 hour')",
    )
    .bind(user_id)
    .bind(&token_hash)
    .execute(pool)
    .await
    .unwrap();
    (user_id, token)
}

/// A distinct session id per test, in the shape `sanitizeKey` accepts.
fn session_id() -> String {
    format!("uxtest-{}", uuid::Uuid::new_v4().simple())
}

/// The whole chain: the payload `ux.ts` builds, through the real endpoint, into
/// ClickHouse and back out again.
///
/// The two events are queued four and a half seconds apart and flushed in one
/// batch, which is the normal shape — and the shape that used to collapse a
/// whole session onto the flush instant (#1224). Asserting the read-back `ts`
/// is the only place that regression is actually observable: a row stamped at
/// ingest is still a valid row, so nothing short of reading the column back
/// catches it.
#[tokio::test]
async fn a_dashboard_batch_lands_in_clickhouse_with_its_own_screen_action_and_ts() {
    let ch_url = skip_without_stack!();
    let http = reqwest::Client::new();
    ensure_table(&http, &ch_url).await;

    let db = fresh_db().await;
    let pool = db.pool().clone();
    // the app runs the migrations, so it is built before anything is seeded
    let app = rolter_control::test_app_with_clickhouse(pool.clone(), &ch_url)
        .await
        .unwrap();
    let addr = serve(app).await;
    let (user_id, token) = seed_session(&pool, "ux-pipeline@example.com").await;

    let session = session_id();
    // relative to the wall clock, not fixed: ingest replaces a ts more than a
    // day behind it as a skewed client clock, so a dated fixture rots in 24h.
    // two distinct sub-second instants, a minute back so they are never ahead
    let base = chrono::Utc::now()
        .with_nanosecond(0)
        .expect("zero nanoseconds is always valid")
        - chrono::Duration::minutes(1);
    let first = base + chrono::Duration::milliseconds(250);
    let second = base + chrono::Duration::milliseconds(4750);
    let wire =
        |t: chrono::DateTime<chrono::Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let stored = |t: chrono::DateTime<chrono::Utc>| t.format("%Y-%m-%d %H:%M:%S%.3f").to_string();
    let batch = json!({"events": [
        {
            "event_id": "ux-e2e-1",
            "ts": wire(first),
            "screen": "providers",
            "action": "screen_view",
            "session_id": session,
            "from_screen": "dashboard",
            "app_version": "0.0.0-test",
        },
        {
            "event_id": "ux-e2e-2",
            "ts": wire(second),
            "screen": "providers",
            "action": "form_submit",
            "target": "provider-sheet",
            "outcome": "ok",
            "duration_ms": 1234,
            "session_id": session,
            "app_version": "0.0.0-test",
        },
    ]});

    let response = http
        .post(format!("http://{addr}/api/v1/ui-events"))
        .bearer_auth(&token)
        .json(&batch)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);

    let rows = rows_for_session(&http, &ch_url, &session).await;
    assert_eq!(rows.len(), 2, "batch did not land: {rows:?}");

    assert_eq!(rows[0]["screen"], "providers");
    assert_eq!(rows[0]["action"], "screen_view");
    assert_eq!(rows[0]["from_screen"], "dashboard");
    // the browser's instant, to the millisecond, not the batch's
    assert_eq!(rows[0]["ts"], stored(first));

    assert_eq!(rows[1]["action"], "form_submit");
    assert_eq!(rows[1]["target"], "provider-sheet");
    assert_eq!(rows[1]["outcome"], "ok");
    assert_eq!(rows[1]["duration_ms"], 1234);
    assert_eq!(rows[1]["ts"], stored(second));
    assert_ne!(
        rows[0]["ts"], rows[1]["ts"],
        "a batch collapsed onto one instant again (#1224)"
    );

    // attribution is the server's, never the payload's — the batch above sends
    // no user id at all
    for row in &rows {
        assert_eq!(row["user_id"], user_id.to_string());
        assert_eq!(row["app_version"], "0.0.0-test");
    }
}

/// Every action the server accepts is an action the `Enum8` accepts.
///
/// This is the round trip that the Rust-side list cannot do on its own:
/// `ACTIONS` in `ui_events.rs` is a mirror of the enum in `clickhouse/`, and
/// nothing but a real insert proves the two agree. Drift is silent and
/// expensive — a value the validator passes and the column rejects is a 500 on
/// a whole batch, in the deployment rather than in CI (#1731).
#[tokio::test]
async fn every_action_the_server_accepts_is_one_clickhouse_stores() {
    let ch_url = skip_without_stack!();
    let http = reqwest::Client::new();
    ensure_table(&http, &ch_url).await;

    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_clickhouse(pool.clone(), &ch_url)
        .await
        .unwrap();
    let addr = serve(app).await;
    let (_user_id, token) = seed_session(&pool, "ux-actions@example.com").await;

    let session = session_id();
    // the struggle signals, which are what `010_*` widened the enum for, plus
    // one of the originals so a wholesale enum replacement cannot pass either
    let actions = [
        "screen_view",
        "form_abandon",
        "retry_submit",
        "refused_click",
        "abandon_dirty",
    ];
    let events: Vec<Value> = actions
        .iter()
        .enumerate()
        .map(|(i, action)| {
            json!({
                "event_id": format!("ux-action-{i}"),
                "screen": "providers",
                "action": action,
                // the shape a refused control emits: the control key joined to
                // the capability that refused it, and nothing else
                "target": "provider-new:provider:create",
                "outcome": "error",
                "session_id": session,
            })
        })
        .collect();

    let response = http
        .post(format!("http://{addr}/api/v1/ui-events"))
        .bearer_auth(&token)
        .json(&json!({ "events": events }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);

    let rows = rows_for_session(&http, &ch_url, &session).await;
    assert_eq!(rows.len(), actions.len(), "an action was refused: {rows:?}");
    let stored: Vec<&str> = rows
        .iter()
        .map(|row| row["action"].as_str().unwrap_or_default())
        .collect();
    for action in actions {
        assert!(
            stored.contains(&action),
            "{action} did not land: {stored:?}"
        );
    }
    // the refusal carries the control and the capability, and nothing a label
    // or a message could have travelled in
    let refusal = rows
        .iter()
        .find(|row| row["action"] == "refused_click")
        .expect("no refused_click row");
    assert_eq!(refusal["target"], "provider-new:provider:create");
    assert_eq!(refusal["outcome"], "error");
}

/// One malformed event costs the whole batch, and nothing partial is written.
///
/// Worth pinning because it is not the behaviour the call sites suggest: the
/// events in a batch are unrelated interactions that merely shared a flush, so
/// a bad key emitted by one screen silently deletes whatever the user did on
/// the others in the same five seconds. `ux.ts` sanitizes keys before queueing
/// precisely to keep this from happening, and this is the assertion that says
/// what it costs when something slips through.
#[tokio::test]
async fn one_bad_event_rejects_the_whole_batch_and_writes_nothing() {
    let ch_url = skip_without_stack!();
    let http = reqwest::Client::new();
    ensure_table(&http, &ch_url).await;

    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve(
        rolter_control::test_app_with_clickhouse(pool.clone(), &ch_url)
            .await
            .unwrap(),
    )
    .await;
    let (_, token) = seed_session(&pool, "ux-reject@example.com").await;

    let session = session_id();
    let response = http
        .post(format!("http://{addr}/api/v1/ui-events"))
        .bearer_auth(&token)
        .json(&json!({"events": [
            {
                "event_id": "ux-good",
                "screen": "routes",
                "action": "screen_view",
                "session_id": session,
            },
            {
                "event_id": "ux-bad",
                "screen": "routes",
                "action": "sudo_make_me_a_sandwich",
                "session_id": session,
            },
        ]}))
        .send()
        .await
        .unwrap();

    // 400, not 404/405 — so `ux.ts` drops this batch and keeps sending the next
    // one rather than disabling itself
    assert_eq!(response.status(), 400);
    let rows = rows_for_session(&http, &ch_url, &session).await;
    assert!(
        rows.is_empty(),
        "a rejected batch wrote a partial row: {rows:?}"
    );
}

/// ClickHouse being unreachable is a 500 and the events are gone.
///
/// The number matters more than it looks: 500 is in `ux.ts`'s "plausibly
/// transient" branch, so the batch is dropped and the *next* flush is still
/// attempted. A capture through a ClickHouse restart therefore loses exactly
/// the events queued during the outage window and resumes by itself — no
/// operator action, and no notification either.
#[tokio::test]
async fn a_dead_clickhouse_is_a_500_and_the_batch_is_dropped_not_queued() {
    let ch_url = skip_without_stack!();
    let http = reqwest::Client::new();
    ensure_table(&http, &ch_url).await;

    let db = fresh_db().await;
    let pool = db.pool().clone();

    // a port nothing is listening on: bind one, learn its number, drop it
    let closed = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap()
    };
    let addr = serve(
        rolter_control::test_app_with_clickhouse(pool.clone(), &format!("http://{closed}"))
            .await
            .unwrap(),
    )
    .await;
    let (_, token) = seed_session(&pool, "ux-down@example.com").await;

    let session = session_id();
    let event = json!({
        "event_id": "ux-outage",
        "screen": "logs",
        "action": "screen_view",
        "session_id": session,
    });
    let response = http
        .post(format!("http://{addr}/api/v1/ui-events"))
        .bearer_auth(&token)
        .json(&json!({"events": [event]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 500);

    // nothing is buffered anywhere: the control plane holds no queue of its
    // own, so when ClickHouse comes back the event is not there
    let healthy = serve(
        rolter_control::test_app_with_clickhouse(pool.clone(), &ch_url)
            .await
            .unwrap(),
    )
    .await;
    assert!(
        rows_for_session(&http, &ch_url, &session).await.is_empty(),
        "the outage batch reappeared after recovery"
    );

    // and the very next batch succeeds, which is why the loss is bounded by the
    // outage rather than by the tab's lifetime
    let resumed = http
        .post(format!("http://{healthy}/api/v1/ui-events"))
        .bearer_auth(&token)
        .json(&json!({"events": [{
            "event_id": "ux-after-outage",
            "screen": "logs",
            "action": "screen_view",
            "session_id": session,
        }]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resumed.status(), 202);
    let rows = rows_for_session(&http, &ch_url, &session).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["event_id"], "ux-after-outage");
}

/// An expired or absent session is a 401, which is the one failure that ends
/// the capture for good.
///
/// `ux.ts` treats 401/403/404/405 as terminal and stops sending for the life of
/// the tab — correctly, since retrying a route that will never accept us costs
/// a request per flush forever. The consequence for a dogfood week is the part
/// worth writing down: a session that lapses while the dashboard is open keeps
/// rendering (the screens reauthenticate) while the UX stream stays off until
/// the tab is reloaded, and nothing on screen says so.
#[tokio::test]
async fn a_lapsed_session_is_a_401_which_disables_the_client_permanently() {
    let ch_url = skip_without_stack!();
    let http = reqwest::Client::new();
    ensure_table(&http, &ch_url).await;

    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve(
        rolter_control::test_app_with_clickhouse(pool.clone(), &ch_url)
            .await
            .unwrap(),
    )
    .await;
    // a live session exists — it is simply not the caller's. without one in the
    // table an endpoint that accepted *any* session would still answer 401 here
    // and the assertions below would prove nothing
    let _valid = seed_session(&pool, "ux-someone-else@example.com").await;

    let session = session_id();
    let batch = json!({"events": [{
        "event_id": "ux-unauthenticated",
        "screen": "models",
        "action": "screen_view",
        "session_id": session,
    }]});

    let anonymous = http
        .post(format!("http://{addr}/api/v1/ui-events"))
        .json(&batch)
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);

    let expired = http
        .post(format!("http://{addr}/api/v1/ui-events"))
        .bearer_auth("rolter_sess_not_a_real_token")
        .json(&batch)
        .send()
        .await
        .unwrap();
    assert_eq!(expired.status(), 401);

    assert!(
        rows_for_session(&http, &ch_url, &session).await.is_empty(),
        "an unauthenticated batch was stored"
    );
}

/// The two statuses an *older* control plane answers with, and both of them are
/// terminal for the client.
///
/// A deployment without the route (a control plane from before #805, or a
/// reverse proxy that does not forward it) answers 404; a proxy that rewrites
/// the method, or a route mounted for a different verb, answers 405. `ux.ts`
/// disables itself on either, so a mid-week rollback to an older control plane
/// silently ends the capture in every tab that was open at the time.
#[tokio::test]
async fn a_missing_route_is_404_and_a_wrong_method_is_405() {
    let ch_url = skip_without_stack!();
    let http = reqwest::Client::new();

    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve(
        rolter_control::test_app_with_clickhouse(pool.clone(), &ch_url)
            .await
            .unwrap(),
    )
    .await;
    let (_, token) = seed_session(&pool, "ux-older@example.com").await;

    let missing = http
        .post(format!("http://{addr}/api/v1/ui-events-that-never-existed"))
        .bearer_auth(&token)
        .json(&json!({"events": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);

    let wrong_method = http
        .get(format!("http://{addr}/api/v1/ui-events"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_method.status(), 405);
}

/// An oversized batch is refused whole, and the client's own cap is what keeps
/// it from ever being sent.
///
/// `UI_EVENTS_MAX_BATCH` in `ui/src/lib/api.ts` mirrors `MAX_BATCH` here, so
/// this only fires when the two drift — at which point every flush from a busy
/// session is a 400 and the busiest sessions are the ones that vanish.
#[tokio::test]
async fn a_batch_over_the_server_limit_is_refused_whole() {
    let ch_url = skip_without_stack!();
    let http = reqwest::Client::new();
    ensure_table(&http, &ch_url).await;

    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve(
        rolter_control::test_app_with_clickhouse(pool.clone(), &ch_url)
            .await
            .unwrap(),
    )
    .await;
    let (_, token) = seed_session(&pool, "ux-oversize@example.com").await;

    let session = session_id();
    let events: Vec<Value> = (0..101)
        .map(|i| {
            json!({
                "event_id": format!("ux-bulk-{i}"),
                "screen": "logs",
                "action": "screen_view",
                "session_id": session,
            })
        })
        .collect();
    let response = http
        .post(format!("http://{addr}/api/v1/ui-events"))
        .bearer_auth(&token)
        .json(&json!({ "events": events }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert!(rows_for_session(&http, &ch_url, &session).await.is_empty());
}
