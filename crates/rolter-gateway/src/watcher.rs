//! Reload-free config watcher.
//!
//! Polls the control plane's `GET /internal/snapshot?version=N` on an
//! interval and hot-swaps the gateway's routing [`Snapshot`] via
//! [`AppState::reload`] whenever a newer config version is published. The
//! control plane replies `304 Not Modified` when the gateway is already
//! current, so steady-state polling is cheap.
//!
//! Polling is the reliable baseline transport. When a redis url is
//! configured, a subscriber on [`rolter_core::CONFIG_CHANNEL`] additionally
//! wakes the loop the moment the control plane publishes a version bump, so
//! changes propagate immediately instead of waiting out the poll interval.
//! Redis being down never breaks config propagation — the subscriber
//! reconnects with backoff while interval polling keeps working.
//!
//! Which of the two is in effect is logged at startup. Without a redis url the
//! gateway is polling-only, and a write can take up to the poll interval to
//! take effect: a virtual key minted in the dashboard is rejected until then,
//! which is indistinguishable from a wrong key (#933). That is a supported way
//! to run, but it is stated rather than left to be inferred from a key that
//! "does not work yet".
//!
//! Every poll states the snapshot formats this build reads (`formats=N`), and
//! every reply's `format` is read before its config is decoded, so a control
//! plane serving a shape this gateway does not know is a refusal with a code,
//! one `error` line, the `rolter_snapshot_format_refused` gauge and, before a
//! first snapshot, a not-ready `/readyz` rather than a serde error or a config
//! applied in part (ADR-0038).

use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use tokio::sync::Notify;

use rolter_core::snapshot_format::{FormatRange, FORMATS_PARAM, LEGACY_FORMAT};
use rolter_core::GatewayConfig;

use crate::state::AppState;

/// What a snapshot reply says about its own format, read over the same bytes
/// before the config is decoded so a mismatch is reported as one rather than as
/// a serde error. Absent from every control plane that predates negotiation,
/// which serves format 1.
#[derive(Deserialize)]
struct FormatHead {
    #[serde(default)]
    format: Option<u32>,
}

impl FormatHead {
    fn format(&self) -> u32 {
        self.format.unwrap_or(LEGACY_FORMAT)
    }
}

/// A `{"format": F, "version": N, "config": {...}}` document from the snapshot
/// endpoint.
#[derive(Deserialize)]
struct SnapshotResponse {
    version: u64,
    config: GatewayConfig,
    /// entries the control plane dropped because they were unservable on their
    /// own, one line each saying which and why (#926). Absent when everything
    /// was served, and absent entirely from an older control plane, so it
    /// defaults rather than failing the decode
    #[serde(default)]
    problems: Vec<String>,
}

/// Ask the control plane where the fleet logs, before the log writer is built.
///
/// The ClickHouse writer is spawned once, from the bootstrap config, at
/// startup — the hot-swappable [`Snapshot`] carries routing, not background
/// tasks. So a destination that only arrives on the first *poll* arrives too
/// late to open a sink. This is the one read that happens early enough to
/// matter (#929).
///
/// Best-effort by construction: a control plane that is not up yet, is slow, or
/// answers with something unparsable yields `None` and the gateway starts
/// exactly as it did before. Startup must not depend on the control plane
/// being reachable, so every failure here is a debug line, not an error.
pub async fn fetch_log_destination(
    snapshot_url: &str,
    admin_token: Option<&str>,
    timeout: Duration,
) -> Option<String> {
    let client = Client::builder().timeout(timeout).build().ok()?;
    // version=0 asks for the current snapshot rather than a 304
    let readable = FormatRange::readable().to_string();
    let mut request = client
        .get(snapshot_url)
        .query(&[("version", "0"), (FORMATS_PARAM, readable.as_str())]);
    if let Some(token) = admin_token {
        request = request.bearer_auth(token);
    }
    let resp = match request.send().await {
        Ok(resp) if resp.status().is_success() => resp,
        Ok(resp) => {
            tracing::debug!(status = %resp.status(), "no log destination from the control plane");
            return None;
        }
        Err(err) => {
            tracing::debug!(error = %err, "no log destination from the control plane");
            return None;
        }
    };
    let bytes = match resp.bytes().await {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::debug!(error = %err, "no log destination from the control plane");
            return None;
        }
    };
    // a format this build does not read is the poll loop's to report; here it
    // only means there is no destination to take from it
    let format = serde_json::from_slice::<FormatHead>(&bytes).map(|head| head.format());
    if !format
        .as_ref()
        .is_ok_and(|format| FormatRange::readable().contains(*format))
    {
        tracing::debug!(
            format = ?format.ok(),
            "snapshot in a format this gateway does not read; no log destination taken from it"
        );
        return None;
    }
    let body: SnapshotResponse = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(err) => {
            tracing::debug!(error = %err, "unparsable snapshot while reading log destination");
            return None;
        }
    };
    body.config
        .logging
        .clickhouse_url
        .filter(|url| !url.trim().is_empty())
}

/// Spawn the background watcher. `snapshot_url` is the control plane's
/// snapshot endpoint (e.g. `http://control:4001/internal/snapshot`); `period`
/// is the poll interval; `redis_url`, when set, adds an instant pub/sub
/// wake-up on top of the interval. Returns immediately; the tasks run until
/// the process exits.
pub fn spawn(
    state: AppState,
    snapshot_url: String,
    period: Duration,
    redis_url: Option<String>,
    admin_token: Option<String>,
) {
    let wakeup = Arc::new(Notify::new());
    // which transport is in effect decides whether a config change lands on the
    // next request or up to `period` later, and that difference is what an
    // operator experiences as a brand-new virtual key being rejected as if it
    // were a wrong one (#933). Polling-only is a supported deployment, but it
    // has to be stated rather than inferred from a key that "does not work yet"
    match redis_url {
        Some(url) => {
            tracing::info!(
                poll_interval_secs = period.as_secs(),
                "config propagation: redis pub/sub + interval polling (changes apply immediately)"
            );
            tokio::spawn(subscribe(url, Arc::clone(&wakeup)));
        }
        None => tracing::warn!(
            poll_interval_secs = period.as_secs(),
            "config propagation: interval polling only — no redis url configured, so a new \
             virtual key, provider or route can take up to the poll interval to take effect \
             and will be rejected until then"
        ),
    }
    tokio::spawn(async move {
        // a dedicated short-timeout client so a hung control plane can't wedge
        // the watcher loop
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());
        run(
            &client,
            &state,
            &snapshot_url,
            admin_token.as_deref(),
            period,
            &wakeup,
        )
        .await;
    });
}

/// The poll loop, factored out so tests can drive a single tick. Polls on
/// the interval and immediately whenever the redis subscriber signals.
async fn run(
    client: &Client,
    state: &AppState,
    snapshot_url: &str,
    admin_token: Option<&str>,
    period: Duration,
    wakeup: &Notify,
) {
    let mut ticker = tokio::time::interval(period);
    // the initial config was applied at startup; treat it as version 0 so the
    // first successful poll always applies the authoritative version
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = wakeup.notified() => {}
        }
        if let Err(err) = poll_once(client, state, snapshot_url, admin_token).await {
            state
                .metrics
                .config_reload_failures_total
                .fetch_add(1, Relaxed);
            tracing::warn!(error = %rolter_core::redact::redact_urls_in_text(&err.to_string()), "config snapshot poll failed");
        }
    }
}

/// Subscribe to [`rolter_core::CONFIG_CHANNEL`] and signal `wakeup` on every
/// message. Reconnects with a fixed backoff forever; interval polling covers
/// propagation while redis is unavailable.
async fn subscribe(redis_url: String, wakeup: Arc<Notify>) {
    const BACKOFF: Duration = Duration::from_secs(5);
    loop {
        let session = async {
            let client = redis::Client::open(redis_url.as_str())?;
            let mut pubsub = client.get_async_pubsub().await?;
            pubsub.subscribe(rolter_core::CONFIG_CHANNEL).await?;
            tracing::info!(
                channel = rolter_core::CONFIG_CHANNEL,
                "config pub/sub connected"
            );
            let mut stream = pubsub.on_message();
            while stream.next().await.is_some() {
                wakeup.notify_one();
            }
            Ok::<_, redis::RedisError>(())
        };
        if let Err(err) = session.await {
            tracing::warn!(error = %err, "config pub/sub disconnected; retrying");
        }
        tokio::time::sleep(BACKOFF).await;
    }
}

/// Header names this node identifies itself with on each poll, so the control
/// plane can keep a cluster inventory without a second channel (#543).
const NODE_ID_HEADER: &str = "x-rolter-node-id";
const NODE_ROLE_HEADER: &str = "x-rolter-node-role";
const NODE_BUILD_HEADER: &str = "x-rolter-node-build";
/// Response header carrying the operator-requested state for this node.
const NODE_STATE_HEADER: &str = "x-rolter-node-state";

/// Stable identity for this gateway process, resolved by `rolter_core` so the
/// cluster inventory, the adaptive-routing telemetry and `service.instance.id`
/// all name this node the same way. An unidentified node polls exactly as
/// before and stays out of the inventory rather than churning it with a
/// per-restart id.
pub(crate) fn node_id() -> Option<String> {
    rolter_core::node_identity::node_id()
}

/// Say once, at boot, which node this is — or that it is nobody.
///
/// Both the cluster inventory and the adaptive-routing scoreboard are keyed on
/// the node header, and the control plane cannot record a report that does not
/// carry one. Before #1644 that produced two permanently empty dashboard
/// screens and not one line of log on either plane, so the absence is now
/// stated at the only moment an operator can act on it.
pub(crate) fn log_node_identity() {
    match rolter_core::node_identity::node_identity() {
        Some(identity) => tracing::info!(
            node_id = %identity.id,
            source = identity.source.as_str(),
            "node identity resolved"
        ),
        None => tracing::warn!(
            "no node identity could be resolved (ROLTER_NODE_ID, HOSTNAME and the \
             hostname syscall all came up empty); this gateway will stay out of the \
             cluster inventory and report no adaptive-routing telemetry until \
             ROLTER_NODE_ID is set"
        ),
    }
}

/// Fetch the snapshot once and apply it if newer. Returns `Ok(Some(version))`
/// when a reload happened, `Ok(None)` on `304`/no-change, `Err` on transport
/// or decode failure, or on a snapshot format this gateway does not read.
async fn poll_once(
    client: &Client,
    state: &AppState,
    snapshot_url: &str,
    admin_token: Option<&str>,
) -> anyhow::Result<Option<u64>> {
    poll_once_reading(
        client,
        state,
        snapshot_url,
        admin_token,
        FormatRange::readable(),
    )
    .await
}

/// [`poll_once`] for a gateway that reads `readable` rather than this build's
/// own format, so tests can stand in for a gateway of a later release.
async fn poll_once_reading(
    client: &Client,
    state: &AppState,
    snapshot_url: &str,
    admin_token: Option<&str>,
    readable: FormatRange,
) -> anyhow::Result<Option<u64>> {
    let current = state.metrics.config_version.load(Relaxed);
    let mut request = client.get(snapshot_url).query(&[
        ("version", current.to_string()),
        (FORMATS_PARAM, readable.to_string()),
    ]);
    if let Some(token) = admin_token {
        request = request.bearer_auth(token);
    }
    if let Some(id) = node_id() {
        request = request
            .header(NODE_ID_HEADER, id)
            .header(NODE_ROLE_HEADER, "gateway")
            .header(NODE_BUILD_HEADER, env!("CARGO_PKG_VERSION"));
    }
    let resp = request.send().await?;

    // the control plane answers every poll with the node's requested state, so
    // a drain lands even when the config itself has not changed
    apply_node_state(state, resp.headers());
    warn_format_deprecation(state, resp.headers(), readable);

    if resp.status() == StatusCode::NOT_MODIFIED {
        // a 304 answers for the format this gateway stated
        state.clear_snapshot_format_refusal();
        return Ok(None);
    }
    if !resp.status().is_success() {
        let status = resp.status();
        let body = read_refusal(resp).await;
        // a 406 is a format refusal only when the control plane says so: one
        // from a proxy or a misrouted request is an ordinary failed poll
        if status == StatusCode::NOT_ACCEPTABLE {
            if let Some(refusal) = FormatRefusal::from_body(&body, readable) {
                enter_format_refusal(state, &refusal, current);
            }
        }
        anyhow::bail!(
            "snapshot endpoint returned {status}{}",
            refusal_detail(&body)
        );
    }

    let bytes = resp.bytes().await?;
    let format = serde_json::from_slice::<FormatHead>(&bytes)?.format();
    if !readable.contains(format) {
        // either the control plane predates negotiation and ignored the
        // statement, or something served what was not asked for. Neither is
        // applied: a shape this build does not know is the partial
        // application negotiation exists to rule out
        let refusal = FormatRefusal {
            code: FORMAT_UNSUPPORTED,
            served: Some(FormatRange::single(format)),
            readable,
        };
        enter_format_refusal(state, &refusal, current);
        anyhow::bail!(
            "the control plane served snapshot format {format}; this gateway reads {}",
            rolter_core::snapshot_format::describe(readable)
        );
    }
    state.clear_snapshot_format_refusal();

    let mut body: SnapshotResponse = serde_json::from_slice(&bytes)?;
    // guard against a stale/racy response older than what we already run
    if body.version <= current && current != 0 {
        return Ok(None);
    }
    // a value from a newer release decoded as `Unknown` instead of failing the
    // whole snapshot; read it fail-closed for its entity alone, so the rest of
    // the config, revocations included, still applies (#3347)
    let unknown = body.config.fail_closed_unknown_report();
    if !unknown.lines.is_empty() {
        tracing::warn!(
            version = body.version,
            count = unknown.lines.len(),
            problems = ?unknown.lines,
            "this snapshot carries values this gateway does not know; each is read \
             fail-closed (the entity is left out, or takes its most restrictive \
             known value). Upgrade this gateway to serve them as configured"
        );
    }
    // never apply a broken snapshot; keep serving the last good config
    if let Err(problems) = body.config.validate_snapshot() {
        anyhow::bail!("snapshot v{} failed validation: {problems:?}", body.version);
    }
    // model files are read here, off the async workers, so the reload below
    // only installs what is already in memory (#3329). the reads are bounded
    // by MODEL_READ_DEADLINE, so a hung mount cannot stall this loop and the
    // revocations it carries (#3512)
    state
        .local_classifiers
        .preload(&body.config.semantic_guards, &body.config.guardrail_webhook)
        .await;
    state.reload(&body.config, body.version);
    // one gauge per kind for the snapshot now serving, back to 0 for a kind
    // this one no longer carries, so a mixed-release fleet can be alerted on
    // (#3424)
    state.metrics.set_snapshot_unknown_values(&unknown.kinds);
    state.record_snapshot_format(format);
    tracing::info!(
        version = body.version,
        format,
        "applied new config snapshot"
    );
    // logged here rather than on every poll: this line is only reached when the
    // version actually moved, so a fleet polling every 5s does not repeat the
    // same complaint 17k times a day (#926)
    if !body.problems.is_empty() {
        tracing::warn!(
            version = body.version,
            count = body.problems.len(),
            problems = ?body.problems,
            "control plane omitted unservable entries from this config; \
             requests to them will error"
        );
    }
    Ok(Some(body.version))
}

/// Most of a refusal's body the gateway reads (#2947). The control plane's
/// explanation is a handful of short lines; the cap keeps a misbehaving or
/// unrelated endpoint from making every poll buffer an arbitrary body.
const REFUSAL_BODY_LIMIT: usize = 8 * 1024;

/// A bounded prefix of a non-success answer, as JSON, or `Null` when it is not.
async fn read_refusal(mut resp: reqwest::Response) -> serde_json::Value {
    let mut body: Vec<u8> = Vec::new();
    while body.len() < REFUSAL_BODY_LIMIT {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let room = REFUSAL_BODY_LIMIT - body.len();
                body.extend_from_slice(&chunk[..chunk.len().min(room)]);
            }
            _ => break,
        }
    }
    serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)
}

/// What a non-success snapshot answer says about itself, formatted to follow
/// "snapshot endpoint returned <status>", or empty when it says nothing.
///
/// The control plane puts `error.message` and, when it knows which entry or
/// setting held the snapshot back, `error.problems` in the body. Without them
/// the log line reads like a bare 500 and an operator tailing a gateway cannot
/// tell why a change never took effect. Only the log reads this: nothing from a
/// refused response is ever applied.
fn refusal_detail(value: &serde_json::Value) -> String {
    let error = &value["error"];
    let mut detail = String::new();
    if let Some(message) = error["message"].as_str() {
        detail.push_str(": ");
        detail.push_str(message);
    }
    if let Some(problems) = error["problems"].as_array() {
        let lines: Vec<&str> = problems.iter().filter_map(|p| p.as_str()).collect();
        if !lines.is_empty() {
            detail.push_str(" (");
            detail.push_str(&lines.join("; "));
            detail.push(')');
        }
    }
    detail
}

/// The code of a control plane that serves no format this gateway reads.
const FORMAT_UNSUPPORTED: &str = "snapshot_format_unsupported";
/// The code of one whose config the format this gateway reads cannot express.
const FORMAT_INEXPRESSIBLE: &str = "snapshot_format_inexpressible";

/// A snapshot this gateway would not apply for its format.
struct FormatRefusal {
    code: &'static str,
    /// what the control plane serves, when it said
    served: Option<FormatRange>,
    readable: FormatRange,
}

impl FormatRefusal {
    /// Read a `406` body, or `None` when it carries neither format code and
    /// so is not the control plane's refusal.
    fn from_body(body: &serde_json::Value, readable: FormatRange) -> Option<Self> {
        let error = &body["error"];
        let code = match error["code"].as_str() {
            Some(FORMAT_INEXPRESSIBLE) => FORMAT_INEXPRESSIBLE,
            Some(FORMAT_UNSUPPORTED) => FORMAT_UNSUPPORTED,
            _ => return None,
        };
        let bound = |key: &str| {
            error["served"][key]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
        };
        let served = match (bound("min"), bound("max")) {
            (Some(min), Some(max)) if min <= max => Some(FormatRange { min, max }),
            _ => None,
        };
        Some(Self {
            code,
            served,
            readable,
        })
    }

    /// The one-line account an operator acts on.
    fn sentence(&self, version: u64) -> String {
        use rolter_core::snapshot_format::describe;
        let reads = describe(self.readable);
        let what = match (self.code, self.served) {
            (FORMAT_INEXPRESSIBLE, _) => format!(
                "the control plane's config cannot be expressed in the snapshot {reads} this \
                 gateway reads"
            ),
            (_, Some(served)) => format!(
                "the control plane serves {} and this gateway reads {reads}",
                describe(served)
            ),
            (_, None) => format!("the control plane serves no snapshot {reads}"),
        };
        let keeping = if version == 0 {
            "Serving the bootstrap config, and not ready until a snapshot is applied".to_string()
        } else {
            format!("Keeping the config applied at version {version}")
        };
        let action = match (self.code, self.served) {
            (FORMAT_INEXPRESSIBLE, _) => {
                "Upgrade this gateway, or stop using the setting the control plane names"
            }
            (_, Some(served)) if self.readable.min > served.max => {
                "Upgrade the control plane first, or roll this gateway back"
            }
            _ => "Upgrade this gateway, or roll the control plane back",
        };
        format!(
            "snapshot format refused: {what} ({}). {keeping}. {action}.",
            self.code
        )
    }
}

/// Enter the refused state: the gauge goes to 1 and, on the transition only,
/// one `error` line says what happened and what to do. The per-poll `warn` of
/// the failed poll still follows on every poll.
fn enter_format_refusal(state: &AppState, refusal: &FormatRefusal, version: u64) {
    if state.refuse_snapshot_format() {
        tracing::error!(
            code = refusal.code,
            served = refusal.served.map(|r| r.to_string()).unwrap_or_default(),
            readable = %refusal.readable,
            version,
            "{}",
            refusal.sentence(version)
        );
    }
}

/// Warn once per process when the control plane marks the format it serves
/// this gateway deprecated, naming the date after which it stops. Returns
/// whether it warned.
fn warn_format_deprecation(
    state: &AppState,
    headers: &reqwest::header::HeaderMap,
    readable: FormatRange,
) -> bool {
    if headers.get("deprecation").is_none() {
        return false;
    }
    if state
        .metrics
        .snapshot_format_deprecation_warned
        .swap(true, Relaxed)
    {
        return false;
    }
    let sunset = headers
        .get("sunset")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("an unannounced date");
    tracing::warn!(
        sunset,
        readable = %readable,
        "the control plane marks the snapshot {} this gateway reads deprecated; it may stop \
         serving it after {sunset}. Upgrade this gateway before then",
        rolter_core::snapshot_format::describe(readable)
    );
    true
}

/// Apply the operator-requested node state carried on the snapshot response.
/// A response without the header leaves the current state alone, so an
/// unmanaged or unidentified node never flips itself out of service.
fn apply_node_state(state: &AppState, headers: &reqwest::header::HeaderMap) {
    let Some(requested) = headers.get(NODE_STATE_HEADER).and_then(|v| v.to_str().ok()) else {
        return;
    };
    let draining = requested.eq_ignore_ascii_case("draining");
    let previous = state.draining.swap(draining, Relaxed);
    if previous != draining {
        tracing::info!(draining, "control plane changed this node's serving state");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn applies_newer_version_and_skips_same() {
        // spin a tiny control-plane stub that serves an incrementing version
        use axum::extract::Query;
        use axum::routing::get;
        use axum::{Json, Router};
        use std::collections::HashMap;

        async fn snapshot(Query(q): Query<HashMap<String, String>>) -> axum::response::Response {
            use axum::response::IntoResponse;
            let seen: u64 = q.get("version").and_then(|v| v.parse().ok()).unwrap_or(0);
            // authoritative version is 5; reply 304 once the caller has it
            if seen >= 5 {
                return axum::http::StatusCode::NOT_MODIFIED.into_response();
            }
            Json(serde_json::json!({
                "version": 5,
                "config": {"server": {"host": "0.0.0.0", "port": 4000}}
            }))
            .into_response()
        }

        let app = Router::new().route("/internal/snapshot", get(snapshot));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = AppState::new(&GatewayConfig::default());
        let client = Client::new();
        let url = format!("http://{addr}/internal/snapshot");

        // first poll applies version 5
        let applied = poll_once(&client, &state, &url, None).await.unwrap();
        assert_eq!(applied, Some(5));
        assert_eq!(state.metrics.config_version.load(Relaxed), 5);
        assert_eq!(state.metrics.config_reloads_total.load(Relaxed), 1);

        // second poll is a no-op (control replies 304)
        let applied = poll_once(&client, &state, &url, None).await.unwrap();
        assert_eq!(applied, None);
        assert_eq!(state.metrics.config_reloads_total.load(Relaxed), 1);
    }

    /// A model read that never returns must not hold the snapshot loop: the
    /// snapshot applies at the read deadline, with the local provider on its
    /// failure mode (#3512).
    #[tokio::test]
    async fn a_model_read_that_never_returns_does_not_stall_the_snapshot() {
        use axum::routing::get;
        use axum::{Json, Router};

        use crate::local_classifier::tests::{fixture_sha256, local_provider, HungReader};

        let config = GatewayConfig {
            semantic_guards: rolter_core::SemanticGuardsConfig {
                providers: vec![local_provider(&fixture_sha256(), 2_000)],
                ..Default::default()
            },
            ..GatewayConfig::default()
        };
        let body = serde_json::json!({
            "version": 7,
            "config": serde_json::to_value(&config).unwrap(),
        });
        let app = Router::new().route(
            "/internal/snapshot",
            get(move || {
                let body = body.clone();
                async move { Json(body) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = AppState::new(&GatewayConfig::default());
        let reader = HungReader::default();
        reader.install(&state.local_classifiers, Duration::from_millis(200));
        let url = format!("http://{addr}/internal/snapshot");
        let applied = tokio::time::timeout(
            Duration::from_secs(10),
            poll_once(&Client::new(), &state, &url, None),
        )
        .await
        .expect("the poll is not held by the read")
        .unwrap();
        assert_eq!(applied, Some(7));
        assert_eq!(state.metrics.config_version.load(Relaxed), 7);
        assert_eq!(reader.reads.load(std::sync::atomic::Ordering::SeqCst), 1);
        // let the stuck read finish rather than outlive the test
        reader.open();
    }

    #[tokio::test]
    async fn drain_state_from_the_snapshot_response_flips_readiness() {
        use reqwest::header::{HeaderMap, HeaderValue};

        let state = AppState::new(&GatewayConfig::default());
        assert!(!state.draining.load(Relaxed));

        // a response with no header leaves the node serving
        apply_node_state(&state, &HeaderMap::new());
        assert!(!state.draining.load(Relaxed));

        let mut draining = HeaderMap::new();
        draining.insert(NODE_STATE_HEADER, HeaderValue::from_static("draining"));
        apply_node_state(&state, &draining);
        assert!(state.draining.load(Relaxed));

        // an unchanged poll must not silently return the node to service
        apply_node_state(&state, &HeaderMap::new());
        assert!(state.draining.load(Relaxed));

        let mut active = HeaderMap::new();
        active.insert(NODE_STATE_HEADER, HeaderValue::from_static("active"));
        apply_node_state(&state, &active);
        assert!(!state.draining.load(Relaxed));
    }

    #[tokio::test]
    async fn rejects_invalid_snapshot_and_keeps_old_config() {
        use axum::routing::get;
        use axum::{Json, Router};

        // route targets a provider that doesn't exist -> must be rejected
        async fn snapshot() -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "version": 9,
                "config": {
                    "routes": [{"model": "broken", "targets": [{"provider": "ghost"}]}]
                }
            }))
        }

        let app = Router::new().route("/internal/snapshot", get(snapshot));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = AppState::new(&GatewayConfig::default());
        let client = Client::new();
        let url = format!("http://{addr}/internal/snapshot");

        let res = poll_once(&client, &state, &url, None).await;
        assert!(res.is_err(), "invalid snapshot must be an error");
        // old config stays: no reload recorded, version unchanged
        assert_eq!(state.metrics.config_reloads_total.load(Relaxed), 0);
        assert_eq!(state.metrics.config_version.load(Relaxed), 0);
    }

    /// #2943: the control plane answers `500` rather than publish a snapshot
    /// without a gate it could not read (`security_settings.required_headers`).
    /// The gateway must go on enforcing the gate it already holds, not fall
    /// back to a config without it: that is the whole of what makes refusing to
    /// publish the safe answer.
    #[tokio::test]
    async fn a_refused_snapshot_leaves_the_gate_the_gateway_already_holds() {
        use axum::routing::get;
        use axum::Router;
        use std::sync::atomic::AtomicBool;

        let refusing = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&refusing);
        let app = Router::new().route(
            "/internal/snapshot",
            get(move || {
                let flag = Arc::clone(&flag);
                async move {
                    use axum::response::IntoResponse;
                    if flag.load(Relaxed) {
                        return (
                            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                            axum::Json(serde_json::json!({
                                "error": {
                                    "message": "failed to load config snapshot",
                                    "problems": ["required_headers cannot be read"]
                                }
                            })),
                        )
                            .into_response();
                    }
                    axum::Json(serde_json::json!({
                        "version": 5,
                        "config": {"security": {"required_headers": {"x-mesh-id": "edge"}}}
                    }))
                    .into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = AppState::new(&GatewayConfig::default());
        let client = Client::new();
        let url = format!("http://{addr}/internal/snapshot");
        assert_eq!(
            poll_once(&client, &state, &url, None).await.unwrap(),
            Some(5)
        );
        let held = |state: &AppState| {
            state
                .snapshot
                .load()
                .security
                .required_headers
                .get("x-mesh-id")
                .cloned()
        };
        assert_eq!(held(&state).as_deref(), Some("edge"));

        // a newer version exists, and the control plane will not build it
        refusing.store(true, Relaxed);
        state.metrics.config_version.store(4, Relaxed);
        let err = poll_once(&client, &state, &url, None).await.unwrap_err();
        assert!(err.to_string().contains("500"), "{err}");
        // the log line says which setting held the snapshot back (#2947)
        assert!(
            err.to_string().contains("failed to load config snapshot")
                && err.to_string().contains("required_headers cannot be read"),
            "{err}"
        );
        assert_eq!(
            held(&state).as_deref(),
            Some("edge"),
            "the gate was dropped"
        );
        assert_eq!(state.metrics.config_reloads_total.load(Relaxed), 1);
    }

    /// #2947: only a bounded prefix of a refusal is read, a body that is not
    /// the control plane's shape adds nothing, and nothing in it is applied.
    #[tokio::test]
    async fn a_refusal_body_is_read_only_for_its_explanation() {
        use axum::routing::get;
        use axum::Router;

        async fn huge() -> axum::response::Response {
            use axum::response::IntoResponse;
            (
                axum::http::StatusCode::BAD_GATEWAY,
                "x".repeat(10 * REFUSAL_BODY_LIMIT),
            )
                .into_response()
        }
        async fn config_shaped() -> axum::response::Response {
            use axum::response::IntoResponse;
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({
                    "version": 9,
                    "config": {"security": {"required_headers": {"x-mesh-id": "edge"}}}
                })),
            )
                .into_response()
        }
        let app = Router::new()
            .route("/huge", get(huge))
            .route("/shaped", get(config_shaped));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = AppState::new(&GatewayConfig::default());
        let client = Client::new();
        for path in ["huge", "shaped"] {
            let url = format!("http://{addr}/{path}");
            let err = poll_once(&client, &state, &url, None).await.unwrap_err();
            let message = err.to_string();
            assert!(
                message.starts_with("snapshot endpoint returned "),
                "{message}"
            );
            assert!(message.len() < 200, "{message}");
        }
        assert_eq!(state.metrics.config_reloads_total.load(Relaxed), 0);
        assert!(state.snapshot.load().security.required_headers.is_empty());
    }

    /// #926: a snapshot that carries `problems` is a *partial* config, not a
    /// broken one. It must apply — the alternative is the gateway refusing the
    /// same fleet config the control plane just decided to serve.
    #[tokio::test]
    async fn a_snapshot_carrying_problems_still_applies() {
        use axum::routing::get;
        use axum::{Json, Router};

        async fn snapshot() -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "version": 12,
                "config": {
                    "providers": [{
                        "name": "openai",
                        "kind": "openai",
                        "api_base": "https://api.openai.com/v1",
                    }],
                    "routes": [{"model": "gpt-4o", "targets": [{"provider": "openai"}]}],
                },
                "problems": [
                    "provider 'openrouter-edge' omitted from the snapshot: openrouter \
                     provider 'openrouter-edge' api_base must be https://openrouter.ai/api/v1"
                ],
            }))
        }

        let app = Router::new().route("/internal/snapshot", get(snapshot));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = AppState::new(&GatewayConfig::default());
        let client = Client::new();
        let url = format!("http://{addr}/internal/snapshot");

        let applied = poll_once(&client, &state, &url, None).await.unwrap();
        assert_eq!(applied, Some(12));
        assert_eq!(state.metrics.config_version.load(Relaxed), 12);

        // and a second poll at the same version is a no-op, which is what keeps
        // the warning to once per change rather than once per poll
        assert_eq!(poll_once(&client, &state, &url, None).await.unwrap(), None);
    }

    /// A control plane that has not been upgraded yet sends no `problems` key
    /// at all; the field must default rather than fail the decode and take
    /// config propagation down during a rollout.
    #[tokio::test]
    async fn a_snapshot_without_problems_still_decodes() {
        use axum::routing::get;
        use axum::{Json, Router};

        async fn snapshot() -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "version": 3,
                "config": {
                    "providers": [{
                        "name": "openai",
                        "kind": "openai",
                        "api_base": "https://api.openai.com/v1",
                    }],
                    "routes": [{"model": "gpt-4o", "targets": [{"provider": "openai"}]}],
                },
            }))
        }

        let app = Router::new().route("/internal/snapshot", get(snapshot));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = AppState::new(&GatewayConfig::default());
        let client = Client::new();
        let url = format!("http://{addr}/internal/snapshot");
        assert_eq!(
            poll_once(&client, &state, &url, None).await.unwrap(),
            Some(3)
        );
    }

    /// The control plane serves two orgs' `openai` provider and `gpt-4o` route
    /// (#1857): the poll decodes, validates and builds them as two rows each,
    /// keyed apart, with each route on its own org's provider.
    #[tokio::test]
    async fn two_orgs_namesakes_are_applied_as_two_rows_each() {
        let row = |org: &str, api_base: &str| {
            serde_json::json!({
                "provider": {"name": "openai", "slug": "openai", "kind": "openai_compatible",
                             "api_base": api_base, "tenancy": {"org_id": org}},
                "route": {"model": "gpt-4o", "targets": [{"provider": "openai"}],
                          "tenancy": {"org_id": org, "project_id": format!("{org}-app")}},
            })
        };
        let (a, b) = (
            row("org-a", "https://a.example"),
            row("org-b", "https://b.example"),
        );
        let stub = stub(StubAnswer::Ok(serde_json::json!({
            "format": 1,
            "version": 9,
            "config": {
                "providers": [a["provider"], b["provider"]],
                "routes": [a["route"], b["route"]]
            }
        })))
        .await;
        let state = AppState::new(&GatewayConfig::default());
        assert_eq!(
            poll_once(&Client::new(), &state, &stub.url, None)
                .await
                .unwrap(),
            Some(9)
        );
        let held = state.snapshot.load();
        let mut providers: Vec<&str> = held.providers.keys().map(String::as_str).collect();
        providers.sort_unstable();
        assert_eq!(providers, ["openai@org-a", "openai@org-b"]);
        for (org, base) in [
            ("org-a", "https://a.example"),
            ("org-b", "https://b.example"),
        ] {
            let entry = held.routes.get(org, "gpt-4o").unwrap();
            assert_eq!(*entry.namespace, format!("gpt-4o@{org}"));
            let provider = &held.providers[&entry.route.targets[0].provider];
            assert_eq!(provider.api_base, base);
            assert_eq!(held.provider_label(&provider.name), ("openai", org));
        }
        assert!(held.routes.get("org-c", "gpt-4o").is_none());
    }

    /// A newer control plane names a provider kind, a strategy and a budget
    /// period this gateway does not know (#3347). The poll still applies: the
    /// provider and the route that needed it are left out, the strategy's
    /// route refuses, and the rest, keys above all, is served as configured.
    #[tokio::test]
    async fn a_value_this_build_does_not_know_fails_closed_for_its_entity_only() {
        let stub = stub(StubAnswer::Ok(serde_json::json!({
            "format": 1,
            "version": 7,
            "config": {
                "providers": [
                    {"name": "known", "kind": "openai", "api_base": "https://a.example"},
                    {"name": "future", "kind": "quantum", "api_base": "https://q.example"}
                ],
                "routes": [
                    {"model": "kept", "targets": [{"provider": "known"}]},
                    {"model": "orphaned", "targets": [{"provider": "future"}]},
                    {"model": "guessing", "strategy": "mind_reading",
                     "targets": [{"provider": "known"}]}
                ],
                "budgets": [{"scope": "org", "id": "org-1", "limit_usd": 1,
                             "period": "fortnightly"}],
                "security": {"required_headers": {"x-mesh-id": "edge"}}
            }
        })))
        .await;
        let state = AppState::new(&GatewayConfig::default());
        let client = Client::new();
        assert_eq!(
            poll_once(&client, &state, &stub.url, None).await.unwrap(),
            Some(7)
        );
        let held = state.snapshot.load();
        let mut providers: Vec<&str> = held.providers.keys().map(String::as_str).collect();
        providers.sort_unstable();
        assert_eq!(providers, ["known"]);
        let routes: Vec<&str> = held.routes.iter().map(|(_, model, _)| model).collect();
        assert_eq!(routes, ["kept"]);
        assert_eq!(held.budgets[0].period, rolter_core::BudgetPeriod::Unknown);
        assert!(holds_gate(&state));

        // the gauge names each kind once, and never the value (#3424)
        let metrics = state.metrics.render();
        for series in [
            "rolter_snapshot_unknown_values{kind=\"provider_kind\"} 1",
            "rolter_snapshot_unknown_values{kind=\"route_strategy\"} 1",
            "rolter_snapshot_unknown_values{kind=\"budget_period\"} 1",
        ] {
            assert!(metrics.contains(series), "{series} missing:\n{metrics}");
        }
        for value in ["quantum", "mind_reading", "fortnightly"] {
            assert!(!metrics.contains(value), "{value} leaked into the metrics");
        }

        // a later snapshot with nothing unknown drops every kind back to 0
        *stub.answer.lock() = StubAnswer::Ok(serde_json::json!({
            "format": 1,
            "version": 8,
            "config": {"security": {"required_headers": {"x-mesh-id": "edge"}}}
        }));
        assert_eq!(
            poll_once(&client, &state, &stub.url, None).await.unwrap(),
            Some(8)
        );
        let metrics = state.metrics.render();
        for kind in ["provider_kind", "route_strategy", "budget_period"] {
            let series = format!("rolter_snapshot_unknown_values{{kind=\"{kind}\"}} 0");
            assert!(metrics.contains(&series), "{series} missing:\n{metrics}");
        }
    }

    /// A stub standing in for a control plane of another build. It records
    /// the last `formats` it was sent, and answers with whatever `answer`
    /// holds: a JSON body for a `200`, or a status with a body.
    struct Stub {
        url: String,
        seen_formats: Arc<parking_lot::Mutex<Option<String>>>,
        answer: Arc<parking_lot::Mutex<StubAnswer>>,
    }

    #[derive(Clone)]
    enum StubAnswer {
        Ok(serde_json::Value),
        Refuse(u16, serde_json::Value),
        Deprecated(serde_json::Value),
    }

    async fn stub(answer: StubAnswer) -> Stub {
        use axum::extract::Query;
        use axum::response::IntoResponse;
        use axum::routing::get;
        use axum::Router;
        use std::collections::HashMap;

        let seen_formats = Arc::new(parking_lot::Mutex::new(None));
        let answer = Arc::new(parking_lot::Mutex::new(answer));
        let (seen, current) = (Arc::clone(&seen_formats), Arc::clone(&answer));
        let app = Router::new().route(
            "/internal/snapshot",
            get(move |Query(q): Query<HashMap<String, String>>| {
                let (seen, current) = (Arc::clone(&seen), Arc::clone(&current));
                async move {
                    *seen.lock() = q.get("formats").cloned();
                    let answer = current.lock().clone();
                    match answer {
                        StubAnswer::Ok(body) => axum::Json(body).into_response(),
                        StubAnswer::Refuse(status, body) => (
                            axum::http::StatusCode::from_u16(status).unwrap(),
                            axum::Json(body),
                        )
                            .into_response(),
                        StubAnswer::Deprecated(body) => (
                            [
                                ("deprecation", "@1799971200"),
                                ("sunset", "Tue, 20 Apr 2027 00:00:00 GMT"),
                            ],
                            axum::Json(body),
                        )
                            .into_response(),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Stub {
            url: format!("http://{addr}/internal/snapshot"),
            seen_formats,
            answer,
        }
    }

    /// The envelope every control plane built before negotiation serves: no
    /// `format`, and the `formats` statement ignored.
    fn legacy_envelope(version: u64) -> serde_json::Value {
        serde_json::json!({
            "version": version,
            "config": {"security": {"required_headers": {"x-mesh-id": "edge"}}}
        })
    }

    fn unsupported_406() -> serde_json::Value {
        serde_json::json!({"error": {
            "code": "snapshot_format_unsupported",
            "message": "the control plane serves snapshot formats 2-3; the gateway reads 1",
            "problems": ["upgrade the gateway, or roll the control plane back"],
            "served": {"min": 2, "max": 3},
            "requested": {"min": 1, "max": 1},
        }})
    }

    async fn ready(state: &AppState) -> (u16, String) {
        use axum::response::IntoResponse;
        let response = crate::handlers::readyz(axum::extract::State(state.clone()))
            .await
            .into_response();
        let status = response.status().as_u16();
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    fn holds_gate(state: &AppState) -> bool {
        state
            .snapshot
            .load()
            .security
            .required_headers
            .contains_key("x-mesh-id")
    }

    /// Newer gateway, control plane from before negotiation, gateway reads 1:
    /// the absent `format` is 1, and the reply is applied.
    #[tokio::test]
    async fn a_pre_negotiation_control_plane_is_read_as_format_one() {
        let stub = stub(StubAnswer::Ok(legacy_envelope(5))).await;
        let state = AppState::new(&GatewayConfig::default());
        let client = Client::new();
        assert_eq!(
            poll_once(&client, &state, &stub.url, None).await.unwrap(),
            Some(5)
        );
        assert_eq!(
            stub.seen_formats.lock().as_deref(),
            Some(
                rolter_core::snapshot_format::SNAPSHOT_FORMAT
                    .to_string()
                    .as_str()
            ),
            "the poll did not state its format"
        );
        assert!(holds_gate(&state));
        assert_eq!(state.metrics.snapshot_format.load(Relaxed), 1);
        assert_eq!(state.metrics.snapshot_format_refused.load(Relaxed), 0);
        assert_eq!(ready(&state).await, (200, "ok".to_string()));
    }

    /// Newer gateway, control plane from before negotiation, gateway reads 2:
    /// the absent `format` is 1, so the reply is refused, nothing is applied,
    /// and a gateway that was never served is not ready.
    #[tokio::test]
    async fn a_gateway_that_reads_a_later_format_refuses_the_legacy_envelope() {
        let stub = stub(StubAnswer::Ok(legacy_envelope(5))).await;
        let state = AppState::new(&GatewayConfig::default());
        let client = Client::new();
        let err = poll_once_reading(&client, &state, &stub.url, None, FormatRange::single(2))
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("served snapshot format 1; this gateway reads format 2"),
            "{err}"
        );
        assert_eq!(stub.seen_formats.lock().as_deref(), Some("2"));
        assert!(!holds_gate(&state), "a refused snapshot was applied");
        assert_eq!(state.metrics.config_reloads_total.load(Relaxed), 0);
        assert_eq!(state.metrics.snapshot_format_refused.load(Relaxed), 1);
        assert_eq!(state.metrics.snapshot_format.load(Relaxed), 0);
        assert_eq!(
            ready(&state).await,
            (503, "snapshot_format_unsupported".to_string())
        );
    }

    /// A control plane answering `406`: the config is unchanged and the gauge
    /// set, and `/readyz` is `503` only for a gateway that has never applied a
    /// snapshot. A compatible answer clears the gauge again.
    #[tokio::test]
    async fn a_406_keeps_the_config_and_is_unready_only_before_a_first_snapshot() {
        let client = Client::new();

        // a gateway that never received a snapshot
        let stub = stub(StubAnswer::Refuse(406, unsupported_406())).await;
        let fresh = AppState::new(&GatewayConfig::default());
        let err = poll_once(&client, &fresh, &stub.url, None)
            .await
            .unwrap_err();
        // the per-poll line carries the control plane's explanation
        assert!(err.to_string().contains("406"), "{err}");
        assert!(
            err.to_string()
                .contains("upgrade the gateway, or roll the control plane back"),
            "{err}"
        );
        assert_eq!(fresh.metrics.snapshot_format_refused.load(Relaxed), 1);
        assert_eq!(
            ready(&fresh).await,
            (503, "snapshot_format_unsupported".to_string())
        );

        // a running gateway: served once, then refused
        *stub.answer.lock() = StubAnswer::Ok(legacy_envelope(5));
        let running = AppState::new(&GatewayConfig::default());
        assert_eq!(
            poll_once(&client, &running, &stub.url, None).await.unwrap(),
            Some(5)
        );
        *stub.answer.lock() = StubAnswer::Refuse(406, unsupported_406());
        running.metrics.config_version.store(4, Relaxed);
        assert!(poll_once(&client, &running, &stub.url, None).await.is_err());
        assert!(holds_gate(&running), "the last config was dropped");
        assert_eq!(running.metrics.config_reloads_total.load(Relaxed), 1);
        assert_eq!(running.metrics.snapshot_format_refused.load(Relaxed), 1);
        assert_eq!(ready(&running).await, (200, "ok".to_string()));

        // and the fresh gateway, once served, is ready with the gauge cleared
        *stub.answer.lock() = StubAnswer::Ok(legacy_envelope(6));
        assert_eq!(
            poll_once(&client, &fresh, &stub.url, None).await.unwrap(),
            Some(6)
        );
        assert_eq!(fresh.metrics.snapshot_format_refused.load(Relaxed), 0);
        assert_eq!(ready(&fresh).await, (200, "ok".to_string()));
    }

    /// A `406` that does not carry a snapshot-format code (a proxy, a
    /// misrouted request) is an ordinary failed poll, not a format refusal.
    #[tokio::test]
    async fn a_406_without_a_format_code_is_an_ordinary_failure() {
        let client = Client::new();
        for body in [
            serde_json::Value::Null,
            serde_json::json!({"error": {"code": "something_else", "message": "nope"}}),
        ] {
            let stub = stub(StubAnswer::Refuse(406, body.clone())).await;
            let state = AppState::new(&GatewayConfig::default());
            let err = poll_once(&client, &state, &stub.url, None)
                .await
                .unwrap_err();
            assert!(err.to_string().contains("406"), "{err}");
            assert_eq!(
                state.metrics.snapshot_format_refused.load(Relaxed),
                0,
                "{body}"
            );
            assert_eq!(ready(&state).await, (200, "ok".to_string()));
        }
        assert!(
            FormatRefusal::from_body(&serde_json::Value::Null, FormatRange::single(1)).is_none()
        );
    }

    /// The `error` line is said on the transition into the refused state, not
    /// on every poll, and says what to do.
    #[test]
    fn the_refusal_is_reported_once_per_transition() {
        let state = AppState::new(&GatewayConfig::default());
        assert!(state.refuse_snapshot_format());
        assert!(!state.refuse_snapshot_format());
        state.clear_snapshot_format_refusal();
        assert!(state.refuse_snapshot_format());

        let refusal = FormatRefusal::from_body(&unsupported_406(), FormatRange::single(1)).unwrap();
        assert_eq!(refusal.code, FORMAT_UNSUPPORTED);
        assert_eq!(refusal.served, Some(FormatRange { min: 2, max: 3 }));
        assert_eq!(
            refusal.sentence(41),
            "snapshot format refused: the control plane serves formats 2-3 and this gateway \
             reads format 1 (snapshot_format_unsupported). Keeping the config applied at \
             version 41. Upgrade this gateway, or roll the control plane back."
        );
        assert!(refusal
            .sentence(0)
            .contains("not ready until a snapshot is applied"));

        let newer = FormatRefusal {
            code: FORMAT_UNSUPPORTED,
            served: Some(FormatRange::single(1)),
            readable: FormatRange::single(2),
        };
        assert!(newer
            .sentence(3)
            .contains("Upgrade the control plane first"));
        let inexpressible = FormatRefusal::from_body(
            &serde_json::json!({"error": {"code": "snapshot_format_inexpressible"}}),
            FormatRange::single(1),
        )
        .unwrap();
        assert_eq!(inexpressible.code, FORMAT_INEXPRESSIBLE);
        assert!(inexpressible.sentence(3).contains("cannot be expressed"));
    }

    /// A deprecated format is served with `Deprecation` and `Sunset`, and the
    /// gateway warns once across several polls.
    #[tokio::test]
    async fn a_deprecated_format_is_warned_about_once() {
        let mut body = legacy_envelope(5);
        body["format"] = serde_json::json!(1);
        let stub = stub(StubAnswer::Deprecated(body)).await;
        let state = AppState::new(&GatewayConfig::default());
        let client = Client::new();
        for _ in 0..3 {
            poll_once(&client, &state, &stub.url, None).await.unwrap();
        }
        assert!(state
            .metrics
            .snapshot_format_deprecation_warned
            .load(Relaxed));
        assert_eq!(state.metrics.snapshot_format.load(Relaxed), 1);

        // the once is per process, not per poll
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("deprecation", "@1799971200".parse().unwrap());
        headers.insert("sunset", "Tue, 20 Apr 2027 00:00:00 GMT".parse().unwrap());
        let fresh = AppState::new(&GatewayConfig::default());
        let readable = FormatRange::single(1);
        assert!(warn_format_deprecation(&fresh, &headers, readable));
        assert!(!warn_format_deprecation(&fresh, &headers, readable));
        assert!(!warn_format_deprecation(
            &AppState::new(&GatewayConfig::default()),
            &reqwest::header::HeaderMap::new(),
            readable
        ));
    }

    /// The early log-destination read states its format too, and takes
    /// nothing from a snapshot in a format it does not read.
    #[tokio::test]
    async fn the_log_destination_is_not_taken_from_an_unreadable_format() {
        let mut body = serde_json::json!({
            "version": 7,
            "config": {"logging": {"clickhouse_url": "http://clickhouse:8123"}}
        });
        let stub = stub(StubAnswer::Ok(body.clone())).await;
        let found = fetch_log_destination(&stub.url, None, Duration::from_secs(2)).await;
        assert_eq!(found.as_deref(), Some("http://clickhouse:8123"));
        assert!(stub.seen_formats.lock().is_some());

        body["format"] = serde_json::json!(rolter_core::snapshot_format::SNAPSHOT_FORMAT + 1);
        *stub.answer.lock() = StubAnswer::Ok(body);
        assert_eq!(
            fetch_log_destination(&stub.url, None, Duration::from_secs(2)).await,
            None
        );
    }

    #[tokio::test]
    async fn counts_failure_on_unreachable_control() {
        let state = AppState::new(&GatewayConfig::default());
        let client = Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap();
        // nothing is listening here
        let err = poll_once(
            &client,
            &state,
            "http://127.0.0.1:1/internal/snapshot",
            None,
        )
        .await;
        assert!(err.is_err());
    }

    /// #929: the log writer is spawned once at startup from the bootstrap
    /// config, so a destination that only arrives on the first poll arrives too
    /// late to open a sink. This read is the one that happens early enough.
    #[tokio::test]
    async fn the_control_planes_log_destination_is_readable_before_startup() {
        use axum::routing::get;
        use axum::{Json, Router};

        async fn snapshot() -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "version": 7,
                "config": {
                    "server": {"host": "0.0.0.0", "port": 4000},
                    "logging": {"clickhouse_url": "http://clickhouse:8123"}
                }
            }))
        }

        let app = Router::new().route("/internal/snapshot", get(snapshot));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let url = format!("http://{addr}/internal/snapshot");
        let found = fetch_log_destination(&url, None, Duration::from_secs(2)).await;
        assert_eq!(found.as_deref(), Some("http://clickhouse:8123"));
    }

    #[tokio::test]
    async fn a_control_plane_that_names_no_destination_yields_none() {
        use axum::routing::get;
        use axum::{Json, Router};

        // both shapes a real control plane produces: the field absent, and the
        // field present but empty. Neither may open a sink pointed at nothing
        async fn absent() -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "version": 1,
                "config": {"server": {"host": "0.0.0.0", "port": 4000}}
            }))
        }
        async fn blank() -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "version": 1,
                "config": {
                    "server": {"host": "0.0.0.0", "port": 4000},
                    "logging": {"clickhouse_url": "   "}
                }
            }))
        }

        let app = Router::new()
            .route("/absent", get(absent))
            .route("/blank", get(blank));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        for path in ["absent", "blank"] {
            let url = format!("http://{addr}/{path}");
            assert_eq!(
                fetch_log_destination(&url, None, Duration::from_secs(2)).await,
                None,
                "{path}"
            );
        }
    }

    /// startup must not depend on the control plane being up: an unreachable
    /// one costs one short timeout and the gateway starts as it always did
    #[tokio::test]
    async fn an_unreachable_control_plane_costs_a_timeout_and_nothing_else() {
        let found = fetch_log_destination(
            "http://127.0.0.1:1/internal/snapshot",
            None,
            Duration::from_millis(200),
        )
        .await;
        assert_eq!(found, None);
    }
}
