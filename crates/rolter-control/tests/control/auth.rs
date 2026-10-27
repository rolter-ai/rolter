//! Local sign-in: login, sessions, password changes and resets, and the TOTP second factor.

use crate::support::*;
use serde_json::{json, Value};

/// A superadmin password reset through `PUT /api/v1/users/{id}` ends every
/// session the account holds (#1936): the usual reason for a reset is that the
/// old password is compromised, and whoever signed in with it must not keep a
/// week-long session. A superadmin resetting their own password keeps the
/// session they did it from and loses the rest, and the audit row says how
/// many went.
#[tokio::test]
async fn a_password_reset_revokes_the_accounts_live_sessions() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let admin = seed_user(&pool, "reset-admin@example.com", true).await;
    let admin_here = seed_session(&pool, admin, "resetadminhere").await;
    let admin_elsewhere = seed_session(&pool, admin, "resetadminelsewhere").await;
    let target = seed_user(&pool, "reset-target@example.com", false).await;
    let stolen = seed_session(&pool, target, "resettargetstolen").await;
    let laptop = seed_session(&pool, target, "resettargetlaptop").await;

    let me = |token: String| {
        let client = client.clone();
        let base = base.clone();
        async move {
            client
                .get(format!("{base}/api/v1/auth/me"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .status()
        }
    };
    assert_eq!(me(stolen.clone()).await, 200, "the session starts live");

    // a change that leaves the password alone revokes nothing
    let renamed = client
        .put(format!("{base}/api/v1/users/{target}"))
        .bearer_auth(&admin_here)
        .json(&json!({"email": "reset-target-2@example.com"}))
        .send()
        .await
        .unwrap();
    assert_eq!(renamed.status(), 200);
    assert_eq!(me(laptop.clone()).await, 200);

    let reset = client
        .put(format!("{base}/api/v1/users/{target}"))
        .bearer_auth(&admin_here)
        .json(&json!({"password": random_password()}))
        .send()
        .await
        .unwrap();
    assert_eq!(reset.status(), 200);
    for token in [&stolen, &laptop] {
        assert_eq!(me(token.clone()).await, 401, "{token} outlived the reset");
    }
    assert_eq!(
        me(admin_here.clone()).await,
        200,
        "resetting someone else touches only their sessions"
    );
    let revoked: Option<i64> = sqlx::query_scalar(
        "select (detail->>'sessions_revoked')::bigint from audit_log
         where action = 'user.update' and target_id = $1
           and (detail->>'password_changed')::boolean
         order by at desc limit 1",
    )
    .bind(target)
    .fetch_optional(&pool)
    .await
    .unwrap()
    .flatten();
    assert_eq!(
        revoked,
        Some(2),
        "the audit row counts the revoked sessions"
    );

    // resetting your own password keeps the session you did it from
    let own = client
        .put(format!("{base}/api/v1/users/{admin}"))
        .bearer_auth(&admin_here)
        .json(&json!({"password": random_password()}))
        .send()
        .await
        .unwrap();
    assert_eq!(own.status(), 200);
    assert_eq!(
        me(admin_here.clone()).await,
        200,
        "the caller stays signed in"
    );
    assert_eq!(
        me(admin_elsewhere.clone()).await,
        401,
        "every other session of the caller ends"
    );

    // the admin token is no session, so a reset through it spares none
    let again = seed_session(&pool, target, "resettargetagain").await;
    let by_token = client
        .put(format!("{base}/api/v1/users/{target}"))
        .bearer_auth(admin_token())
        .json(&json!({"password": random_password()}))
        .send()
        .await
        .unwrap();
    assert_eq!(by_token.status(), 200);
    assert_eq!(me(again).await, 401);
}

/// End-to-end local-account login (ROL-32): seed a user with an argon2id hash
/// directly (no signup flow exists yet), then exercise login → `/auth/me` →
/// logout → the now-revoked token is rejected.
#[tokio::test]
async fn login_me_logout_round_trip() {
    skip_without_db!();
    let password = random_password();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // seed a user the way `rolter-seed` does (same argon2id hashing call shape)
    use argon2::password_hash::PasswordHasher;
    let hash = argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .unwrap()
        .to_string();
    sqlx::query("insert into users (email, password_hash, is_superadmin) values ($1, $2, true)")
        .bind("admin@example.com")
        .bind(&hash)
        .execute(&pool)
        .await
        .unwrap();

    // wrong password is rejected
    let denied = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "admin@example.com", "password": "wrong"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    // /auth/me without a token is rejected
    let no_token = client
        .get(format!("{base}/api/v1/auth/me"))
        .send()
        .await
        .unwrap();
    assert_eq!(no_token.status(), 401);

    // a made-up token is rejected too (never matches a stored session hash)
    let bad_token = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth("rolter_sess_not-a-real-token")
        .send()
        .await
        .unwrap();
    assert_eq!(bad_token.status(), 401);

    // correct credentials issue a session token
    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "admin@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().expect("token").to_string();
    assert_eq!(login["user"]["email"], "admin@example.com");

    // the token resolves the current user via the CurrentUser extractor
    let me: Value = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["user"]["email"], "admin@example.com");
    assert!(me["memberships"].is_array());

    // logout revokes the session; a repeat logout is a no-op (idempotent)
    let logout = client
        .post(format!("{base}/api/v1/auth/logout"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), 204);
    let logout_again = client
        .post(format!("{base}/api/v1/auth/logout"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(logout_again.status(), 204);

    // the revoked token no longer resolves a session
    let after_logout = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(after_logout.status(), 401);
}

/// Failed logins are throttled, audited and per-account (#1079).
///
/// Before this, `POST /api/v1/auth/login` answered an unlimited number of
/// guesses and each one bought a full argon2 verification, so the endpoint was
/// both a guessing oracle and a cheap CPU-exhaustion vector. The properties
/// worth pinning are that the lock engages, that it says how long it lasts,
/// that it is scoped to the account rather than the deployment, and that a
/// correct password clears the counter.
#[tokio::test]
async fn failed_logins_are_throttled_per_account_and_audited() {
    skip_without_db!();
    let password = random_password();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    use argon2::password_hash::PasswordHasher;
    let hash_for = |password: &str| {
        argon2::Argon2::default()
            .hash_password(password.as_bytes())
            .unwrap()
            .to_string()
    };
    for email in ["target@example.com", "bystander@example.com"] {
        sqlx::query(
            "insert into users (email, password_hash, is_superadmin) values ($1, $2, true)",
        )
        .bind(email)
        .bind(hash_for(&password))
        .execute(&pool)
        .await
        .unwrap();
    }

    let guess = |email: &'static str| {
        let client = client.clone();
        let base = base.clone();
        async move {
            client
                .post(format!("{base}/api/v1/auth/login"))
                .json(&json!({"email": email, "password": "wrong"}))
                .send()
                .await
                .unwrap()
        }
    };

    // the default budget is five failures; every one of them is an ordinary 401
    for _ in 0..5 {
        assert_eq!(guess("target@example.com").await.status(), 401);
    }

    // the sixth is refused before any password work happens, and says for how long
    let locked = guess("target@example.com").await;
    assert_eq!(locked.status(), 429);
    let retry_after: u64 = locked
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .expect("a lock states how long it lasts")
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        retry_after > 0,
        "retry-after must never invite an instant retry"
    );

    // the lock belongs to the account, not to the deployment: locking one
    // account must not lock everybody out, which would turn the throttle into
    // the denial-of-service it exists to prevent
    assert_eq!(guess("bystander@example.com").await.status(), 401);
    let ok = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({
            "email": "bystander@example.com",
            "password": password
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);

    // and a correct password clears the counter it had started
    for _ in 0..5 {
        assert_eq!(guess("bystander@example.com").await.status(), 401);
    }
    assert_eq!(guess("bystander@example.com").await.status(), 429);

    // the attempts are on the record. the write is spawned off the response
    // path — resolving the org an entry belongs to only costs a query when the
    // account exists, and awaiting that would make a registered address
    // measurably slower to reject than an unregistered one — so wait for the
    // rows themselves. the wait ends on the condition, not on a count of
    // sleeps: a fixed number of polls was a wall-clock bet that lost when a
    // loaded full-suite run delayed the spawned writes (#2768). the timeout is
    // only the backstop that turns a write that never lands into a failure
    let actions: Vec<String> = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let actions: Vec<String> = sqlx::query_scalar(
                "select action from audit_log where action like 'auth.login%' order by at",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            if actions.iter().any(|a| a == "auth.login_locked")
                && actions.iter().any(|a| a == "auth.login_throttled")
            {
                break actions;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the lock and the throttled attempt are both audited");
    assert!(
        actions.iter().any(|a| a == "auth.login_failed"),
        "every rejected guess is recorded: {actions:?}"
    );
    assert!(
        actions.iter().any(|a| a == "auth.login_locked"),
        "the attempt that engaged the lock says so: {actions:?}"
    );
    assert!(
        actions.iter().any(|a| a == "auth.login_throttled"),
        "an attempt refused by the lock is recorded too: {actions:?}"
    );
}

// ---------------------------------------------------------------------------
// changing your own password (#2804)
// ---------------------------------------------------------------------------

/// Seed a local account with a password, optionally a superadmin, and return
/// its id. `seed_local_user` is always a superadmin; the self-service routes
/// are for every role, so these tests mostly want an account that is none.
async fn seed_password_user(
    pool: &sqlx::PgPool,
    email: &str,
    password: &str,
    is_superadmin: bool,
) -> uuid::Uuid {
    use argon2::password_hash::PasswordHasher;
    let hash = argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .unwrap()
        .to_string();
    sqlx::query_scalar(
        "insert into users (email, password_hash, is_superadmin) values ($1, $2, $3)
         returning id",
    )
    .bind(email)
    .bind(&hash)
    .bind(is_superadmin)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// The status a password sign-in answers, without asserting it succeeded.
async fn login_status(client: &reqwest::Client, base: &str, email: &str, password: &str) -> u16 {
    client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// The status `GET /api/v1/auth/me` answers for `token`, which is whether the
/// session behind it is alive.
async fn session_status(client: &reqwest::Client, base: &str, token: &str) -> u16 {
    client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// The whole happy path for an account with no role anywhere: the change takes
/// effect, every *other* session of the account ends, the one it was made from
/// survives, and the audit row says how many ended without ever carrying a
/// password.
#[tokio::test]
async fn changing_your_own_password_ends_the_other_sessions_and_keeps_this_one() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let old_password = random_password();
    let new_password = random_password();
    let user_id = seed_password_user(&pool, "self@example.com", &old_password, false).await;
    // another account's session must be left alone, which a delete that forgot
    // its `where user_id` would not
    let bystander =
        seed_password_user(&pool, "bystander@example.com", &random_password(), false).await;
    let bystander_token = seed_session(&pool, bystander, "bystander").await;

    let here = session_token(&client, &base, "self@example.com", &old_password).await;
    let elsewhere = session_token(&client, &base, "self@example.com", &old_password).await;
    let seeded = seed_session(&pool, user_id, "self-seeded").await;

    let me: Value = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&here)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["has_local_password"], true);
    assert!(
        me["user"].get("password_hash").is_none(),
        "the hash never leaves the server"
    );

    let changed = post_password_change(&client, &base, &here, &old_password, &new_password).await;
    assert_eq!(changed.status(), 200);
    let body: Value = changed.json().await.unwrap();
    assert_eq!(body["sessions_revoked"], 2, "the two other sessions end");

    assert_eq!(
        session_status(&client, &base, &here).await,
        200,
        "this session is kept"
    );
    assert_eq!(session_status(&client, &base, &elsewhere).await, 401);
    assert_eq!(session_status(&client, &base, &seeded).await, 401);
    assert_eq!(
        session_status(&client, &base, &bystander_token).await,
        200,
        "another account's session is not touched"
    );

    assert_eq!(
        login_status(&client, &base, "self@example.com", &old_password).await,
        401,
        "the old password stops working"
    );
    assert_eq!(
        login_status(&client, &base, "self@example.com", &new_password).await,
        200,
        "and the new one signs in"
    );

    let (actor, detail): (Option<uuid::Uuid>, Value) = sqlx::query_as(
        "select actor_user_id, detail from audit_log where action = 'auth.password_changed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(actor, Some(user_id));
    assert_eq!(detail["sessions_revoked"], 2);
    let recorded = detail.to_string();
    assert!(
        !recorded.contains(&old_password) && !recorded.contains(&new_password),
        "a password must never reach the audit log"
    );
}

/// A wrong current password is a refusal that leaves the account as it was, is
/// audited, and counts like a failed sign-in: the lock engages after the
/// budget, says how long it lasts, holds even against the right password, and
/// is the same lock the login screen sees.
#[tokio::test]
async fn a_wrong_current_password_is_refused_audited_and_throttled_like_a_login() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    let wanted = random_password();
    seed_password_user(&pool, "guarded@example.com", &password, false).await;
    let token = session_token(&client, &base, "guarded@example.com", &password).await;

    // the default budget is five failures; each is an ordinary refusal that
    // names the field, and is a 400 rather than a 401 so the dashboard does not
    // read it as a dead session and sign the person out
    for _ in 0..5 {
        let refused =
            post_password_change(&client, &base, &token, &random_password(), &wanted).await;
        assert_eq!(refused.status(), 400);
        let body: Value = refused.json().await.unwrap();
        assert_eq!(body["error"]["code"], "invalid_field");
        assert_eq!(body["error"]["field"], "current_password");
    }

    // the sixth is refused before any password work happens, and says for how
    // long. The right password does not get through a lock either
    let locked = post_password_change(&client, &base, &token, &password, &wanted).await;
    assert_eq!(locked.status(), 429);
    let retry_after: u64 = locked
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .expect("a lock states how long it lasts")
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        retry_after > 0,
        "retry-after must never invite an instant retry"
    );
    let body: Value = locked.json().await.unwrap();
    assert_eq!(body["error"]["code"], "too_many_attempts");

    // the budget is the account's, shared with the login screen: a guess made
    // here and a guess made there are one run, and the lock holds for both
    assert_eq!(
        login_status(&client, &base, "guarded@example.com", &password).await,
        429
    );

    // nothing changed: the same password still opens the account, and this
    // session is intact
    let stored_hash: String =
        sqlx::query_scalar("select password_hash from users where email = 'guarded@example.com'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let parsed = argon2::PasswordHash::new(&stored_hash).unwrap();
    use argon2::password_hash::PasswordVerifier;
    assert!(argon2::Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok());
    assert_eq!(session_status(&client, &base, &token).await, 200);

    let rows: Vec<Value> = sqlx::query_scalar(
        "select detail from audit_log where action = 'auth.password_change_failed' order by at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 5, "every rejected guess is on the record");
    assert!(
        rows[4]["locked_for_secs"].as_u64().is_some(),
        "the guess that engaged the lock says so"
    );
    assert!(
        rows[0].get("locked_for_secs").is_none(),
        "an ordinary miss does not claim a lock"
    );
    let changed: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'auth.password_changed'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(changed, 0);
}

/// A correct current password clears the failures that led up to it, exactly as
/// a successful sign-in does, so an ordinary typo does not carry over into the
/// next change.
#[tokio::test]
async fn a_correct_current_password_forgets_the_earlier_misses() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let first = random_password();
    let second = random_password();
    let third = random_password();
    seed_password_user(&pool, "typo@example.com", &first, false).await;
    let token = session_token(&client, &base, "typo@example.com", &first).await;

    for _ in 0..4 {
        let refused =
            post_password_change(&client, &base, &token, &random_password(), &second).await;
        assert_eq!(refused.status(), 400);
    }
    assert_eq!(
        post_password_change(&client, &base, &token, &first, &second)
            .await
            .status(),
        200
    );
    // four more would have crossed five had the first four still counted
    for _ in 0..4 {
        let refused =
            post_password_change(&client, &base, &token, &random_password(), &third).await;
        assert_eq!(refused.status(), 400, "the counter started over");
    }
    assert_eq!(
        post_password_change(&client, &base, &token, &second, &third)
            .await
            .status(),
        200
    );
}

/// The new password is held to the length every other path that sets one
/// enforces, and a refusal for its shape is not a guess at the current
/// password: it costs the account nothing and runs no verification.
#[tokio::test]
async fn a_new_password_is_checked_for_length_without_counting_as_a_guess() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    seed_password_user(&pool, "short@example.com", &password, false).await;
    let token = session_token(&client, &base, "short@example.com", &password).await;

    // well past the account's failure budget, none of it counted
    for _ in 0..8 {
        let refused = post_password_change(&client, &base, &token, &password, &"s".repeat(7)).await;
        assert_eq!(refused.status(), 400);
        let body: Value = refused.json().await.unwrap();
        assert_eq!(body["error"]["code"], "invalid_field");
        assert_eq!(body["error"]["field"], "new_password");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("at least 8"),
            "the refusal states the minimum"
        );
    }
    // exactly the minimum is long enough
    assert_eq!(
        post_password_change(&client, &base, &token, &password, &"m".repeat(8))
            .await
            .status(),
        200
    );
}

/// Setting the password to what it already is would end every other session for
/// nothing, so it is refused -- and only after the current password is proved,
/// so the answer tells nobody anything.
#[tokio::test]
async fn the_new_password_must_differ_from_the_current_one() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    seed_password_user(&pool, "same@example.com", &password, false).await;
    let token = session_token(&client, &base, "same@example.com", &password).await;
    let elsewhere = session_token(&client, &base, "same@example.com", &password).await;

    let refused = post_password_change(&client, &base, &token, &password, &password).await;
    assert_eq!(refused.status(), 400);
    let body: Value = refused.json().await.unwrap();
    assert_eq!(body["error"]["field"], "new_password");
    assert_eq!(
        session_status(&client, &base, &elsewhere).await,
        200,
        "nothing was revoked"
    );
}

/// The route is authenticated by a session and by nothing else, and it takes
/// exactly the two fields it names.
#[tokio::test]
async fn the_password_route_needs_a_session_and_a_well_formed_body() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    seed_password_user(&pool, "shape@example.com", &password, false).await;
    let token = session_token(&client, &base, "shape@example.com", &password).await;

    let anonymous = client
        .post(format!("{base}/api/v1/auth/password"))
        .json(&json!({"current_password": password, "new_password": random_password()}))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);

    for (shape, body) in [
        json!({"current_password": password}),
        json!({"new_password": random_password()}),
        json!({"current_password": password, "new_password": random_password(), "extra": 1}),
    ]
    .into_iter()
    .enumerate()
    {
        let refused = client
            .post(format!("{base}/api/v1/auth/password"))
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 400, "malformed body #{shape}");
        let json: Value = refused.json().await.unwrap();
        assert!(
            json["error"]["message"].as_str().is_some(),
            "malformed body #{shape} is answered with a message"
        );
    }
}

/// An expired session row is rejected even though the token digest matches,
/// proving `find_active_by_hash`'s `expires_at > now()` bound is doing its job.
#[tokio::test]
async fn expired_session_is_rejected() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let user_id: uuid::Uuid = sqlx::query_scalar(
        "insert into users (email, password_hash, is_superadmin) values ($1, $2, true) returning id",
    )
    .bind("expired@example.com")
    .bind("unused-hash")
    .fetch_one(&pool)
    .await
    .unwrap();

    // insert an already-expired session directly, bypassing login, with a
    // digest matching what the extractor computes for an empty pepper
    let token = "rolter_sess_deadbeef";
    let token_hash = rolter_auth::hash_key("", token);
    sqlx::query(
        "insert into sessions (user_id, token_hash, expires_at) values ($1, $2, now() - interval '1 hour')",
    )
    .bind(user_id)
    .bind(&token_hash)
    .execute(&pool)
    .await
    .unwrap();

    let resp = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

/// The current TOTP code for a base32 secret, as an authenticator app would
/// compute it.
fn current_code(secret_b32: &str) -> String {
    let secret = rolter_auth::totp::base32_decode(secret_b32).expect("secret decodes");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    rolter_auth::totp::code_at_step(&secret, rolter_auth::totp::step_at(now))
}

/// The whole enrolment → step-up → recovery-code path, plus the two properties
/// that make the factor worth having: a code cannot be replayed, and a
/// recovery code is single-use.
#[tokio::test]
async fn totp_enrolment_step_up_and_recovery_codes() {
    skip_without_db!();
    // enrolment seals the secret with the deployment KEK, so a control plane
    // without one must refuse rather than store a bearer credential in clear
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    seed_local_user(&pool, "mfa@example.com", &password).await;

    // sign in the ordinary way: no factor yet, so a session comes straight back
    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "mfa@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().expect("session token").to_string();

    // status before enrolment
    let status: Value = client
        .get(format!("{base}/api/v1/me/mfa"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["enabled"], false);
    assert_eq!(status["policy"], "off");

    // begin enrolment: the secret is shown once
    let enrol: Value = client
        .post(format!("{base}/api/v1/me/mfa/enroll"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = enrol["secret"].as_str().expect("secret").to_string();
    assert!(
        enrol["otpauth_uri"]
            .as_str()
            .unwrap()
            .starts_with("otpauth://totp/"),
        "{enrol}"
    );

    // an unconfirmed factor arms nothing: logging in again must still hand
    // back a session, not a challenge
    let mid: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "mfa@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        mid["token"].is_string(),
        "unconfirmed factor must not gate login: {mid}"
    );

    // a wrong code does not arm it
    let bad = client
        .post(format!("{base}/api/v1/me/mfa/confirm"))
        .bearer_auth(&token)
        .json(&json!({"code": "000000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);

    // the right code does, and returns the recovery batch. bind it: the replay
    // assertion below has to send back this exact code, and reading the clock a
    // second time sends the next step's code across a 30s boundary, asserting
    // the clock rather than the replay rule (#1451)
    let confirming_code = current_code(&secret);
    let confirmed: Value = client
        .post(format!("{base}/api/v1/me/mfa/confirm"))
        .bearer_auth(&token)
        .json(&json!({"code": &confirming_code}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let codes: Vec<String> = confirmed["recovery_codes"]
        .as_array()
        .expect("recovery codes")
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    assert_eq!(codes.len(), 10, "{confirmed}");

    // now login is a challenge, not a session
    let challenge: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "mfa@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(challenge["mfa_required"], true, "{challenge}");
    assert!(
        challenge["token"].is_null(),
        "a challenge is not a session: {challenge}"
    );
    let mfa_token = challenge["mfa_token"]
        .as_str()
        .expect("mfa token")
        .to_string();

    // a wrong code against the challenge is refused
    let wrong = client
        .post(format!("{base}/api/v1/auth/mfa/verify"))
        .json(&json!({"mfa_token": mfa_token, "code": "000000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);

    // the code that confirmed the enrolment has already been spent, so it does
    // not redeem the challenge either -- the replay rule does not care that
    // the earlier use was a legitimate one
    let spent = client
        .post(format!("{base}/api/v1/auth/mfa/verify"))
        .json(&json!({"mfa_token": mfa_token, "code": &confirming_code}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        spent.status(),
        401,
        "the confirming code is spent and must not redeem a challenge"
    );

    // stand in for the 30 seconds a real user waits for the next code, by
    // winding the spent step back one. Winding the clock instead would mean
    // sleeping through a step in every CI run
    sqlx::query("update user_totp_factors set last_used_step = last_used_step - 1")
        .execute(&pool)
        .await
        .unwrap();

    // now the current code redeems the challenge for a real session
    let challenge: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "mfa@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mfa_token = challenge["mfa_token"]
        .as_str()
        .expect("mfa token")
        .to_string();
    let code = current_code(&secret);
    let stepped: Value = client
        .post(format!("{base}/api/v1/auth/mfa/verify"))
        .json(&json!({"mfa_token": mfa_token, "code": code}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session = stepped["token"]
        .as_str()
        .expect("session token")
        .to_string();
    let me = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&session)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200, "the stepped-up session must authenticate");

    // *the* property: that same code, still inside its window, cannot be
    // replayed on a fresh challenge. Without the spent-step check a
    // shoulder-surfed code stays usable for up to 90 seconds
    let replay_challenge: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "mfa@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let replayed = client
        .post(format!("{base}/api/v1/auth/mfa/verify"))
        .json(&json!({
            "mfa_token": replay_challenge["mfa_token"].as_str().unwrap(),
            "code": code,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        replayed.status(),
        401,
        "a spent TOTP step must not verify again"
    );

    // a recovery code gets past the factor exactly once
    let recovery_challenge: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "mfa@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let redeemed = client
        .post(format!("{base}/api/v1/auth/mfa/verify"))
        .json(&json!({
            "mfa_token": recovery_challenge["mfa_token"].as_str().unwrap(),
            "code": codes[0],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        redeemed.status(),
        200,
        "an unspent recovery code must verify"
    );

    let reuse_challenge: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "mfa@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let reused = client
        .post(format!("{base}/api/v1/auth/mfa/verify"))
        .json(&json!({
            "mfa_token": reuse_challenge["mfa_token"].as_str().unwrap(),
            "code": codes[0],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(reused.status(), 401, "a recovery code must be single-use");

    let after: Value = client
        .get(format!("{base}/api/v1/me/mfa"))
        .bearer_auth(&session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after["enabled"], true);
    assert_eq!(after["recovery_codes_remaining"], 9);
}

/// A challenge is spendable a bounded number of times. Without this, a
/// stolen password is six digits and unlimited guesses away from a session.
#[tokio::test]
async fn a_challenge_is_exhausted_by_repeated_wrong_codes() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    seed_local_user(&pool, "attempts@example.com", &password).await;

    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "attempts@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().unwrap().to_string();
    let enrol: Value = client
        .post(format!("{base}/api/v1/me/mfa/enroll"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = enrol["secret"].as_str().unwrap().to_string();
    client
        .post(format!("{base}/api/v1/me/mfa/confirm"))
        .bearer_auth(&token)
        .json(&json!({"code": current_code(&secret)}))
        .send()
        .await
        .unwrap();

    let challenge: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "attempts@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mfa_token = challenge["mfa_token"].as_str().unwrap().to_string();

    for attempt in 0..3 {
        let wrong = client
            .post(format!("{base}/api/v1/auth/mfa/verify"))
            .json(&json!({"mfa_token": mfa_token, "code": "000000"}))
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), 401, "attempt {attempt}");
    }

    // the budget is spent, so even the *correct* code no longer redeems this
    // challenge -- the user has to start over from the password
    let correct = client
        .post(format!("{base}/api/v1/auth/mfa/verify"))
        .json(&json!({"mfa_token": mfa_token, "code": current_code(&secret)}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        correct.status(),
        401,
        "an exhausted challenge must not redeem, right code or not"
    );
}

/// `rolter mfa reset` is the documented way back into an account whose factor
/// is gone. It clears the factor and revokes the sessions that were riding on
/// it, and records why it was run.
#[tokio::test]
async fn break_glass_reset_clears_the_factor_and_revokes_sessions() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    let user_id = seed_local_user(&pool, "locked-out@example.com", &password).await;

    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "locked-out@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().unwrap().to_string();
    let enrol: Value = client
        .post(format!("{base}/api/v1/me/mfa/enroll"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    client
        .post(format!("{base}/api/v1/me/mfa/confirm"))
        .bearer_auth(&token)
        .json(&json!({"code": current_code(enrol["secret"].as_str().unwrap())}))
        .send()
        .await
        .unwrap();
    // a sign-in halfway through its step-up when the reset lands
    let in_flight = login_as(&client, &base, "locked-out@example.com", &password).await;
    assert_eq!(in_flight["mfa_required"], true, "{in_flight}");

    let cleared = rolter_control::mfa::break_glass_reset(&pool, user_id, "lost phone, ticket 42")
        .await
        .unwrap();
    assert!(cleared, "there was a factor to clear");
    // and every challenge in flight goes with the factor. An enrolment token
    // left alive would be refused only while a factor is armed, so the moment
    // the reset cleared it, its holder could arm one of their own
    let challenges: i64 =
        sqlx::query_scalar("select count(*) from mfa_challenges where user_id = $1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(challenges, 0, "reset must drop every challenge in flight");

    // the session that existed before the reset is gone
    let stale = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), 401, "reset must revoke live sessions");

    // and the password alone gets back in, so the user can enrol again
    let back_in: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "locked-out@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(back_in["token"].is_string(), "{back_in}");

    // the reason is on the audit entry, which is the whole point of demanding
    // one on the command line
    let reason: Option<String> = sqlx::query_scalar(
        "select detail->>'reason' from audit_log
         where action = 'auth.mfa_break_glass_reset' and actor_user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(&pool)
    .await
    .unwrap()
    .flatten();
    assert_eq!(reason.as_deref(), Some("lost phone, ticket 42"));
}

/// A `required_all` org no longer refuses an account with no armed factor: the
/// sign-in hands out an enrolment challenge, proving a code from the secret it
/// mints is what issues the session, and the challenge opens nothing else on
/// the way (#1852).
#[tokio::test]
async fn a_required_policy_sends_an_unenrolled_account_through_enrolment() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    // an admin token, so the CRUD api actually checks what it is handed rather
    // than letting every request through as open mode does
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admin-secret".into()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    let (user_id, org_id) = seed_bound_member(
        &pool,
        "unenrolled@example.com",
        &password,
        "required_all",
        "null",
    )
    .await;

    let challenge = login_as(&client, &base, "unenrolled@example.com", &password).await;
    assert_eq!(challenge["mfa_enrolment_required"], true, "{challenge}");
    assert!(
        challenge["token"].is_null(),
        "an enrolment challenge is not a session: {challenge}"
    );
    let enrolment_token = challenge["enrolment_token"]
        .as_str()
        .expect("enrolment token")
        .to_string();

    // the token reads nothing and writes nothing: not as a bearer on the
    // account's own routes, not on the CRUD api, and not on the step-up
    for (method, path) in [
        ("GET", "/api/v1/auth/me"),
        ("GET", "/api/v1/me/mfa"),
        ("POST", "/api/v1/me/mfa/enroll"),
        ("GET", "/api/v1/me/virtual-keys"),
        ("GET", "/api/v1/orgs"),
        ("GET", &format!("/api/v1/orgs/{org_id}/auth-policy")),
    ] {
        let response = client
            .request(method.parse().unwrap(), format!("{base}{path}"))
            .bearer_auth(&enrolment_token)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            401,
            "{method} {path} must not accept an enrolment token"
        );
    }
    let write = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(&enrolment_token)
        .json(&json!({"name": "stolen", "slug": "stolen"}))
        .send()
        .await
        .unwrap();
    assert_eq!(write.status(), 401, "an enrolment token must not write");
    let relax = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .bearer_auth(&enrolment_token)
        .json(&json!({"allow_password_login": true, "allow_sso": true, "mfa_policy": "off"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        relax.status(),
        401,
        "an enrolment token must not relax the policy it is bound by"
    );
    let step_up = client
        .post(format!("{base}/api/v1/auth/mfa/verify"))
        .json(&json!({"mfa_token": enrolment_token, "code": "000000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(step_up.status(), 401, "the step-up must not accept it");

    // what it is for: a secret, shown once
    let enrol: Value = client
        .post(format!("{base}/api/v1/auth/mfa/enroll"))
        .json(&json!({"enrolment_token": enrolment_token}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = enrol["secret"].as_str().expect("secret").to_string();
    assert!(
        enrol["otpauth_uri"]
            .as_str()
            .unwrap()
            .starts_with("otpauth://totp/"),
        "{enrol}"
    );

    // a pending secret still arms nothing, so a code from it does not get
    // past the step-up either -- the one way through is the confirm below
    let still_refused = client
        .post(format!("{base}/api/v1/auth/mfa/verify"))
        .json(&json!({"mfa_token": enrolment_token, "code": current_code(&secret)}))
        .send()
        .await
        .unwrap();
    assert_eq!(still_refused.status(), 401);

    let wrong = client
        .post(format!("{base}/api/v1/auth/mfa/confirm"))
        .json(&json!({"enrolment_token": enrolment_token, "code": "000000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 400, "a wrong code keeps the challenge");

    // stand in for a password step that came straight after a lockout: the
    // session comes out of a later request, and has to learn it from the row
    sqlx::query("update mfa_challenges set after_lock = true where user_id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();

    let signed_in: Value = client
        .post(format!("{base}/api/v1/auth/mfa/confirm"))
        .json(&json!({"enrolment_token": enrolment_token, "code": current_code(&secret)}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session = signed_in["token"]
        .as_str()
        .expect("confirming yields a session")
        .to_string();
    assert_eq!(
        signed_in["recovery_codes"].as_array().map(Vec::len),
        Some(10),
        "{signed_in}"
    );
    assert_eq!(signed_in["user"]["email"], "unenrolled@example.com");
    let me = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&session)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200, "the enrolled session must authenticate");
    let status: Value = client
        .get(format!("{base}/api/v1/me/mfa"))
        .bearer_auth(&session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["enabled"], true, "{status}");

    // single use: the token went with the session it minted. The factor is
    // disarmed behind the api's back first, because `enrolling_user` refuses
    // an armed account on its own and would answer 401 whether or not the
    // token had been consumed
    let disarm = "update user_totp_factors set confirmed_at = null where user_id = $1";
    let rearm = "update user_totp_factors set confirmed_at = now() where user_id = $1";
    sqlx::query(disarm)
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    let again = client
        .post(format!("{base}/api/v1/auth/mfa/confirm"))
        .json(&json!({"enrolment_token": enrolment_token, "code": current_code(&secret)}))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 401, "an enrolment token redeems once");
    sqlx::query(rearm)
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();

    // audited like any other sign-in, and the enrolment says where it happened
    let actions: Vec<(String, Option<bool>, Option<bool>)> = sqlx::query_as(
        "select action, (detail->>'at_sign_in')::boolean, (detail->>'after_lock')::boolean
         from audit_log where actor_user_id = $1 order by at",
    )
    .bind(user_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        actions.iter().any(
            |(action, _, after_lock)| action == "auth.mfa_enrolment_challenge"
                && *after_lock == Some(false)
        ),
        "the challenge row says whether the password step followed a lockout: {actions:?}"
    );
    assert!(
        actions.iter().any(
            |(action, at_sign_in, _)| action == "auth.mfa_enabled" && *at_sign_in == Some(true)
        ),
        "{actions:?}"
    );
    assert!(
        actions
            .iter()
            .any(|(action, _, after_lock)| action == "auth.login" && *after_lock == Some(true)),
        "the session carries the lockout its challenge recorded: {actions:?}"
    );

    // the factor is armed now, so the next sign-in is an ordinary step-up --
    // and that step-up token does not open the enrolment routes either. Again
    // with the factor disarmed underneath, so it is the purpose on the token
    // that refuses rather than the armed factor
    let step_up: Value = login_as(&client, &base, "unenrolled@example.com", &password).await;
    assert_eq!(step_up["mfa_required"], true, "{step_up}");
    sqlx::query(disarm)
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    for route in ["enroll", "confirm"] {
        let reenrol = client
            .post(format!("{base}/api/v1/auth/mfa/{route}"))
            .json(&json!({"enrolment_token": step_up["mfa_token"], "code": current_code(&secret)}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            reenrol.status(),
            401,
            "a step-up token must not reach /auth/mfa/{route}"
        );
    }
}

/// An enrolment challenge is short-lived and bounded: once it expires, or once
/// its codes are spent, it opens nothing, the right code included.
#[tokio::test]
async fn an_enrolment_challenge_expires_and_is_spent_by_wrong_codes() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    seed_bound_member(&pool, "late@example.com", &password, "required_all", "null").await;

    // expiry: a challenge left sitting is dead to both routes
    let stale = login_as(&client, &base, "late@example.com", &password).await;
    let stale_token = stale["enrolment_token"]
        .as_str()
        .expect("token")
        .to_string();
    let enrol: Value = client
        .post(format!("{base}/api/v1/auth/mfa/enroll"))
        .json(&json!({"enrolment_token": stale_token}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = enrol["secret"].as_str().expect("secret").to_string();
    sqlx::query("update mfa_challenges set expires_at = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    for route in ["enroll", "confirm"] {
        let response = client
            .post(format!("{base}/api/v1/auth/mfa/{route}"))
            .json(&json!({"enrolment_token": stale_token, "code": current_code(&secret)}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401, "{route} after expiry");
    }

    // budget: five wrong codes spend a fresh challenge
    let fresh = login_as(&client, &base, "late@example.com", &password).await;
    let token = fresh["enrolment_token"]
        .as_str()
        .expect("token")
        .to_string();
    let enrol: Value = client
        .post(format!("{base}/api/v1/auth/mfa/enroll"))
        .json(&json!({"enrolment_token": token}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = enrol["secret"].as_str().expect("secret").to_string();
    for attempt in 0..5 {
        let wrong = client
            .post(format!("{base}/api/v1/auth/mfa/confirm"))
            .json(&json!({"enrolment_token": token, "code": "000000"}))
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), 400, "attempt {attempt}");
    }
    let correct = client
        .post(format!("{base}/api/v1/auth/mfa/confirm"))
        .json(&json!({"enrolment_token": token, "code": current_code(&secret)}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        correct.status(),
        401,
        "a spent enrolment challenge must not redeem, right code or not"
    );
    let armed: Option<bool> = sqlx::query_scalar(
        "select confirmed_at is not null from user_totp_factors f
         join users u on u.id = f.user_id where u.email = 'late@example.com'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(armed, Some(false), "nothing was armed along the way");
}

/// A grace window announces the requirement without enforcing it: until it
/// passes the unenrolled member signs in with the password and is told by
/// when; after it, the same sign-in is an enrolment (#1852).
#[tokio::test]
async fn a_grace_window_lets_an_unenrolled_member_in_until_it_passes() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    let (_, org_id) = seed_bound_member(&pool, "grace@example.com", &password, "off", "null").await;

    // the window is set through the api, so the round trip is covered too
    let deadline = (chrono::Utc::now() + chrono::Duration::days(7)).to_rfc3339();
    let policy: Value = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .json(&json!({
            "allow_password_login": true,
            "allow_sso": true,
            "mfa_policy": "required_all",
            "mfa_enforce_after": deadline,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(policy["mfa_enforce_after"].is_string(), "{policy}");

    // a client written before the window existed re-sends the policy without
    // it, and must not cancel the date an admin announced
    let resent: Value = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .json(&json!({
            "allow_password_login": true,
            "allow_sso": false,
            "mfa_policy": "required_all",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        resent["mfa_enforce_after"], policy["mfa_enforce_after"],
        "an absent window keeps the stored one: {resent}"
    );

    let signed_in = login_as(&client, &base, "grace@example.com", &password).await;
    let session = signed_in["token"]
        .as_str()
        .expect("inside the window the password is enough")
        .to_string();
    assert_eq!(
        signed_in["mfa_enrol_by"], policy["mfa_enforce_after"],
        "the session says by when: {signed_in}"
    );
    let status: Value = client
        .get(format!("{base}/api/v1/me/mfa"))
        .bearer_auth(&session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["required"], true, "{status}");
    assert_eq!(status["enforce_after"], policy["mfa_enforce_after"]);

    // the window runs out
    sqlx::query("update org_auth_policies set mfa_enforce_after = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    let after = login_as(&client, &base, "grace@example.com", &password).await;
    assert_eq!(after["mfa_enrolment_required"], true, "{after}");
    assert_eq!(
        after["expires_in"], 600,
        "the client times the prompt from this, not from its own clock: {after}"
    );

    // an explicit null is "at once", which an absent key is not
    let at_once: Value = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .json(&json!({
            "allow_password_login": true,
            "allow_sso": true,
            "mfa_policy": "required_all",
            "mfa_enforce_after": null,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(at_once["mfa_enforce_after"].is_null(), "{at_once}");

    // relaxing the policy drops the window with it, rather than leaving it to
    // surprise whoever next turns the requirement on
    let relaxed: Value = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .json(&json!({
            "allow_password_login": true,
            "allow_sso": true,
            "mfa_policy": "optional",
            "mfa_enforce_after": deadline,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(relaxed["mfa_enforce_after"].is_null(), "{relaxed}");
    let plain = login_as(&client, &base, "grace@example.com", &password).await;
    assert!(plain["token"].is_string(), "{plain}");
    assert!(
        plain.get("mfa_enrol_by").is_none(),
        "nothing is owed, so nothing is announced: {plain}"
    );
}

/// The two writes an enrolment makes decide in the statement, not in a read
/// before it (#1852). A confirm arms only the secret its code was checked
/// against, so a second enrolment landing in between is not armed by a code
/// from the first; and a new enrolment never replaces an armed factor, so one
/// racing a confirm cannot disarm what that confirm just armed. Neither race
/// is reachable deterministically through the api, hence the repository.
#[tokio::test]
async fn an_enrolment_never_arms_a_secret_it_did_not_check_or_disarms_one() {
    skip_without_db!();
    use rolter_store::postgres::crypto::Kek;
    use rolter_store::postgres::repo::MfaRepo;
    let db = fresh_db().await;
    let pool = db.pool().clone();
    rolter_store::postgres::run_migrations(&pool).await.unwrap();
    let user_id = seed_local_user(&pool, "race@example.com", &random_password()).await;
    let kek = Kek::from_secret(TEST_KEK);
    let repo = MfaRepo(&pool);

    assert!(repo.begin_enrolment(user_id, &[1; 20], &kek).await.unwrap());
    let checked = repo.open_secret(user_id, &kek).await.unwrap().unwrap();
    // another enrolment replaces the pending secret after the code was checked
    assert!(repo.begin_enrolment(user_id, &[2; 20], &kek).await.unwrap());
    assert!(
        !repo.confirm(user_id, 1, &checked.nonce).await.unwrap(),
        "a code checked against the first secret must not arm the second"
    );
    assert!(!repo.has_armed_factor(user_id).await.unwrap());

    let current = repo.open_secret(user_id, &kek).await.unwrap().unwrap();
    assert!(repo.confirm(user_id, 1, &current.nonce).await.unwrap());
    assert!(
        !repo.begin_enrolment(user_id, &[3; 20], &kek).await.unwrap(),
        "an armed factor is never replaced"
    );
    assert!(repo.has_armed_factor(user_id).await.unwrap());
    let kept = repo.open_secret(user_id, &kek).await.unwrap().unwrap();
    assert_eq!(
        kept.secret,
        vec![2; 20],
        "the armed secret is the one proved"
    );
}

/// A control plane without `ROLTER_KEK` cannot seal a secret, so it cannot
/// enrol anyone. The sign-in keeps the up-front refusal rather than handing out
/// an enrolment challenge that would fail one step later, and the policy
/// endpoint refuses a `required_*` value that would lock out everyone it binds,
/// the admin saving it included (#1852).
#[tokio::test]
async fn without_a_kek_a_required_policy_is_refused_up_front() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_without_kek(pool.clone())
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    // stored behind the api's back: a policy set while the deployment still
    // had a KEK, or before the endpoint refused one without it
    let (user_id, org_id) = seed_bound_member(
        &pool,
        "no-kek@example.com",
        &password,
        "required_all",
        "null",
    )
    .await;

    let refused = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "no-kek@example.com", "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    let body: Value = refused.json().await.unwrap();
    assert_eq!(body["error"]["code"], "mfa_enrolment_required", "{body}");
    let kek: Option<bool> = sqlx::query_scalar(
        "select (detail->>'kek')::boolean from audit_log
         where action = 'auth.mfa_enrolment_required' and actor_user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(&pool)
    .await
    .unwrap()
    .flatten();
    assert_eq!(
        kek,
        Some(false),
        "the refusal says why it is the operator's"
    );
    let challenges: i64 =
        sqlx::query_scalar("select count(*) from mfa_challenges where user_id = $1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        challenges, 0,
        "no enrolment token for a secret nobody can seal"
    );

    for policy in ["required_superadmin", "required_all"] {
        let response = client
            .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
            .json(&json!({
                "allow_password_login": true,
                "allow_sso": true,
                "mfa_policy": policy,
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 409, "{policy} without a KEK");
        let body: Value = response.json().await.unwrap();
        assert!(
            body.to_string().contains("ROLTER_KEK"),
            "the refusal names the remedy: {body}"
        );
    }

    // relaxing stays open, since it is the way out of the lockout above
    let relaxed = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .json(&json!({
            "allow_password_login": true,
            "allow_sso": true,
            "mfa_policy": "optional",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(relaxed.status(), 200);
    let signed_in = login_as(&client, &base, "no-kek@example.com", &password).await;
    assert!(signed_in["token"].is_string(), "{signed_in}");
}

// ---------------------------------------------------------------------------
// people management: change a role, name an invitation's sender, reset a
// password (#2310, #2325, #2240)
// ---------------------------------------------------------------------------

/// Give an existing account a local password. The id comes from `seed_user`
/// rather than from a function that takes a password, so the scanners that
/// follow a password into a request url do not mistake an account id for one.
async fn set_local_password(pool: &sqlx::PgPool, account: uuid::Uuid, password: &str) {
    use argon2::password_hash::PasswordHasher;
    let hash = argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .unwrap()
        .to_string();
    sqlx::query("update users set password_hash = $2 where id = $1")
        .bind(account)
        .bind(&hash)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn an_administrator_resets_a_members_password_with_a_one_time_link() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let (base, org_id) = people_fixture(&pool).await;
    let client = reqwest::Client::new();

    let old_password = random_password();
    let new_password = random_password();
    let admin = seed_user(&pool, "admin@example.com", false).await;
    let member = seed_user(&pool, "member@example.com", false).await;
    set_local_password(&pool, member, &old_password).await;
    seed_membership(&pool, admin, Some(org_id), None, None, "admin").await;
    seed_membership(&pool, member, Some(org_id), None, None, "member").await;
    let admin_session = seed_session(&pool, admin, "reset_admin").await;
    let member_session = seed_session(&pool, member, "reset_member").await;
    let url = format!("{base}/api/v1/orgs/{org_id}/users/{member}/password-reset");

    // a member cannot reset anyone
    let res = client
        .post(&url)
        .bearer_auth(&member_session)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);

    let res = client
        .post(&url)
        .bearer_auth(&admin_session)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let issued: Value = res.json().await.unwrap();
    let token = issued["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("rolter_reset_"), "{issued}");
    assert_eq!(issued["sessions_revoked"], 1, "{issued}");
    assert!(issued["reset_url"].as_str().unwrap().ends_with(&token));
    assert!(
        issued["reset"].get("token_hash").is_none(),
        "the digest never leaves the server: {issued}"
    );

    // the member's live session is gone; the old password still signs in
    let res = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&member_session)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    assert_eq!(
        login_status(&client, &base, "member@example.com", &old_password).await,
        200
    );

    // the audit row names the issuer and never the token
    let detail: Value = sqlx::query_scalar(
        "select detail from audit_log where action = 'user.password_reset' and target_id = $1",
    )
    .bind(member)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(detail["sessions_revoked"], 1, "{detail}");
    assert!(!detail.to_string().contains(&token), "{detail}");

    // a second reset replaces the first: only the newest link works
    let res = client
        .post(&url)
        .bearer_auth(&admin_session)
        .send()
        .await
        .unwrap();
    let second: Value = res.json().await.unwrap();
    let second_token = second["token"].as_str().unwrap().to_string();
    let res = client
        .get(format!("{base}/api/v1/password-reset/{token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    // preview names the address; a too-short password does not spend the link
    let preview: Value = client
        .get(format!("{base}/api/v1/password-reset/{second_token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(preview["email"], "member@example.com");
    let res = client
        .post(format!("{base}/api/v1/password-reset/{second_token}"))
        .json(&json!({"password": "short"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    // an armed factor and a half-finished sign-in: redeeming ends the sign-in
    // and leaves the factor, so the new password alone still opens nothing
    sqlx::query(
        "insert into user_totp_factors (user_id, secret_ciphertext, secret_nonce, confirmed_at)
         values ($1, '\\x00', '\\x00', now())",
    )
    .bind(member)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into mfa_challenges (user_id, token_hash, expires_at)
         values ($1, 'half-finished', now() + interval '5 minutes')",
    )
    .bind(member)
    .execute(&pool)
    .await
    .unwrap();

    let res = client
        .post(format!("{base}/api/v1/password-reset/{second_token}"))
        .json(&json!({"password": new_password}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let done: Value = res.json().await.unwrap();
    assert_eq!(done["sign_in_required"], true, "{done}");
    assert!(done.get("token").is_none(), "redeeming signs nobody in");
    let factors: i64 =
        sqlx::query_scalar("select count(*) from user_totp_factors where user_id = $1")
            .bind(member)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(factors, 1, "a reset never clears the second factor");
    let challenges: i64 =
        sqlx::query_scalar("select count(*) from mfa_challenges where user_id = $1")
            .bind(member)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(challenges, 0, "a sign-in begun with the old password ends");

    // single use
    let res = client
        .post(format!("{base}/api/v1/password-reset/{second_token}"))
        .json(&json!({"password": random_password()}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    assert_eq!(
        login_status(&client, &base, "member@example.com", &old_password).await,
        401
    );
    assert_eq!(
        login_status(&client, &base, "member@example.com", &new_password).await,
        200
    );
}

#[tokio::test]
async fn a_password_reset_respects_who_may_reach_which_account() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let (base, org_id) = people_fixture(&pool).await;
    let client = reqwest::Client::new();
    let other_org: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('Other', 'other') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let team: uuid::Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(org_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    let admin = seed_user(&pool, "admin@example.com", false).await;
    let team_admin = seed_user(&pool, "lead@example.com", false).await;
    let sso_only = seed_user(&pool, "sso@example.com", false).await;
    let shared = seed_user(&pool, "shared@example.com", false).await;
    let root = seed_user(&pool, "root@example.com", true).await;
    let gone = seed_user(&pool, "gone@example.com", false).await;
    for account in [shared, root, gone] {
        set_local_password(&pool, account, &random_password()).await;
    }
    for user in [sso_only, shared, root, gone] {
        seed_membership(&pool, user, Some(org_id), None, None, "member").await;
    }
    seed_membership(&pool, shared, Some(other_org), None, None, "member").await;
    seed_membership(&pool, admin, Some(org_id), None, None, "admin").await;
    seed_membership(&pool, team_admin, None, Some(team), None, "admin").await;
    sqlx::query("update users set deactivated_at = now() where id = $1")
        .bind(gone)
        .execute(&pool)
        .await
        .unwrap();
    let admin_session = seed_session(&pool, admin, "reach_admin").await;
    let team_session = seed_session(&pool, team_admin, "reach_team").await;

    let reset = |token: &str, org: uuid::Uuid, member_id: uuid::Uuid| {
        client
            .post(format!(
                "{base}/api/v1/orgs/{org}/users/{member_id}/password-reset"
            ))
            .bearer_auth(token.to_string())
            .send()
    };

    // a team admin does not manage org members' credentials
    assert_eq!(
        reset(&team_session, org_id, shared).await.unwrap().status(),
        403
    );
    // an org admin: not their own account, not a superadmin, not an account
    // that also belongs to another org, not an sso-only or deactivated one
    for (target, expected) in [
        (admin, 400),
        (root, 403),
        (shared, 403),
        (sso_only, 400),
        (gone, 400),
    ] {
        let res = reset(&admin_session, org_id, target).await.unwrap();
        assert_eq!(res.status(), expected, "target {target}");
    }
    // naming another org in the path does not widen the caller's reach: they
    // hold no admin role there
    assert_eq!(
        reset(&admin_session, other_org, shared)
            .await
            .unwrap()
            .status(),
        403
    );

    // a superadmin reaches the shared account and another superadmin
    assert_eq!(
        reset(admin_token(), org_id, shared).await.unwrap().status(),
        200
    );
    assert_eq!(
        reset(admin_token(), org_id, root).await.unwrap().status(),
        200
    );
    let unknown = reset(admin_token(), org_id, uuid::Uuid::new_v4())
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);
}

/// The roster an admin reads before raising `mfa_policy` (#1590): one row per
/// active member, an armed factor told apart from a pending enrolment, recovery
/// codes counted but never shown, and nothing for a stranger to the org.
#[tokio::test]
async fn the_org_roster_says_who_has_an_armed_second_factor() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let team: uuid::Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(org)
            .fetch_one(&pool)
            .await
            .unwrap();
    let admin = seed_user(&pool, "admin@acme.test", false).await;
    seed_membership(&pool, admin, Some(org), None, None, "admin").await;
    // a member who only holds a team grant is still a member of the org
    let armed = seed_user(&pool, "armed@acme.test", false).await;
    seed_membership(&pool, armed, None, Some(team), None, "member").await;
    seed_membership(&pool, armed, Some(org), None, None, "viewer").await;
    let pending = seed_user(&pool, "pending@acme.test", false).await;
    seed_membership(&pool, pending, Some(org), None, None, "viewer").await;
    let bare = seed_user(&pool, "bare@acme.test", false).await;
    seed_membership(&pool, bare, Some(org), None, None, "viewer").await;
    let gone = seed_user(&pool, "gone@acme.test", false).await;
    seed_membership(&pool, gone, Some(org), None, None, "viewer").await;
    sqlx::query("update users set deactivated_at = now() where id = $1")
        .bind(gone)
        .execute(&pool)
        .await
        .unwrap();
    let stranger = seed_user(&pool, "stranger@elsewhere.test", false).await;

    for (user, confirmed) in [(armed, true), (pending, false)] {
        sqlx::query(
            "insert into user_totp_factors (user_id, secret_ciphertext, secret_nonce, confirmed_at)
             values ($1, '\\x00', '\\x00', case when $2 then now() end)",
        )
        .bind(user)
        .bind(confirmed)
        .execute(&pool)
        .await
        .unwrap();
    }
    for n in 0..3 {
        sqlx::query("insert into user_recovery_codes (user_id, code_hash, used_at) values ($1, $2, case when $3 then now() end)")
            .bind(armed)
            .bind(format!("hash-{n}"))
            .bind(n == 0)
            .execute(&pool)
            .await
            .unwrap();
    }

    let token = seed_session(&pool, admin, "roster").await;
    let response = client
        .get(format!("{base}/api/v1/orgs/{org}/mfa-status"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let rows: Vec<Value> = response.json().await.unwrap();
    let emails: Vec<&str> = rows.iter().map(|r| r["email"].as_str().unwrap()).collect();
    assert_eq!(
        emails,
        [
            "admin@acme.test",
            "armed@acme.test",
            "bare@acme.test",
            "pending@acme.test"
        ],
        "one row per active member, each once, no stranger and no deactivated account"
    );
    let row = |email: &str| rows.iter().find(|r| r["email"] == email).unwrap();
    assert_eq!(row("armed@acme.test")["enabled"], true);
    assert_eq!(row("armed@acme.test")["recovery_codes_remaining"], 2);
    assert_eq!(
        row("pending@acme.test")["enabled"],
        false,
        "a half-finished enrolment is not an armed factor"
    );
    assert_eq!(row("bare@acme.test")["enabled"], false);
    assert_eq!(row("bare@acme.test")["recovery_codes_remaining"], 0);
    let rendered = serde_json::to_string(&rows).unwrap();
    assert!(!rendered.contains("hash-") && !rendered.contains("secret"));

    // below admin there is no roster
    let viewer_token = seed_session(&pool, pending, "roster-viewer").await;
    let refused = client
        .get(format!("{base}/api/v1/orgs/{org}/mfa-status"))
        .bearer_auth(&viewer_token)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    let outsider = seed_session(&pool, stranger, "roster-outsider").await;
    let refused = client
        .get(format!("{base}/api/v1/orgs/{org}/mfa-status"))
        .bearer_auth(&outsider)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
}
