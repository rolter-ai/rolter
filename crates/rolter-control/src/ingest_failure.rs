//! What the telemetry ingest endpoints do when a write cannot be persisted
//! (#1747).
//!
//! `POST /api/v1/ui-events` and `POST /api/v1/mcp-logs` answer callers that
//! swallow every failure on purpose — the dashboard's `ux.ts` drops a batch the
//! store refused and keeps going, because a save button that breaks when
//! analytics 500s is worse than no analytics. Before this module the server was
//! just as quiet: a failed insert became an `ApiError` and nothing else, so a
//! deployment whose `ui_events` table was missing lost its whole UX stream with
//! no signal anywhere. Every failure now does three things:
//!
//! - adds one to `rolter_control_ingest_failures`, so the loss is alertable;
//! - emits a `tracing::warn!` — at most once per [`WARN_INTERVAL`] per stream,
//!   since the dashboard flushes every few seconds from every open tab and a
//!   warn per batch would bury the log it is meant to be found in. The warn
//!   carries how many failures it stands for, so the rate is not lost;
//! - answers the caller with a generic 500. The store's own error text quotes
//!   the insert URL and ClickHouse's internals, which are the operator's
//!   business and not a browser's; they go to the log instead.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use rolter_core::telemetry::ControlHistograms;

use crate::crud::ApiError;

/// How often one stream may warn while it keeps failing. Long enough that a
/// dead store costs one log line a minute rather than one per flush, short
/// enough that an operator tailing the log after an alert sees it promptly.
pub(crate) const WARN_INTERVAL: Duration = Duration::from_secs(60);

/// What the caller is told when the store refused or never received the write.
/// Deliberately says nothing about which store, where it lives or what it said.
pub(crate) const INSERT_FAILED: &str =
    "the event store did not accept the write; the control-plane log has the detail";

/// The telemetry streams the control plane ingests. The label is a metric
/// attribute and a log field, so it is a closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stream {
    UiEvents,
    McpLogs,
}

impl Stream {
    fn label(self) -> &'static str {
        match self {
            Self::UiEvents => "ui_events",
            Self::McpLogs => "mcp_logs",
        }
    }

    /// One gate per stream, so a UX stream failing on every flush cannot
    /// spend the interval an MCP failure would have warned in.
    fn gate(self) -> &'static WarnGate {
        static UI_EVENTS: WarnGate = WarnGate::new();
        static MCP_LOGS: WarnGate = WarnGate::new();
        match self {
            Self::UiEvents => &UI_EVENTS,
            Self::McpLogs => &MCP_LOGS,
        }
    }
}

/// Why a write was lost; a metric label, so a closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    /// the store was reached and refused, or could not be reached
    Insert,
    /// the control plane has no column store configured at all
    Unconfigured,
}

impl Reason {
    fn label(self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Unconfigured => "unconfigured",
        }
    }
}

/// Admits one warn per interval and counts what it held back.
///
/// Lock-free: this sits on a request path that every open dashboard tab hits,
/// and a failing store is exactly when those requests pile up.
pub(crate) struct WarnGate {
    /// milliseconds on [`now_ms`]'s clock at the last admitted warn; zero
    /// means none yet, which is why that clock never reads zero
    last_warn_ms: AtomicU64,
    /// failures since the last admitted warn that were not themselves logged
    suppressed: AtomicU64,
}

impl WarnGate {
    pub(crate) const fn new() -> Self {
        Self {
            last_warn_ms: AtomicU64::new(0),
            suppressed: AtomicU64::new(0),
        }
    }

    /// `Some(n)` when a warn is due now, `n` being how many failures were held
    /// back since the previous one; `None` when this failure is to be held.
    fn admit(&self, now_ms: u64, interval: Duration) -> Option<u64> {
        let interval_ms = u64::try_from(interval.as_millis()).unwrap_or(u64::MAX);
        let last = self.last_warn_ms.load(Ordering::Acquire);
        let due = last == 0 || now_ms.saturating_sub(last) >= interval_ms;
        // the exchange settles a race between two failing requests: exactly
        // one of them wins the interval and the other is counted as held
        if due
            && self
                .last_warn_ms
                .compare_exchange(last, now_ms, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            return Some(self.suppressed.swap(0, Ordering::AcqRel));
        }
        self.suppressed.fetch_add(1, Ordering::AcqRel);
        None
    }
}

/// Monotonic milliseconds since the first failure this process saw, offset by
/// one so that [`WarnGate`] can keep zero for "never warned".
fn now_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    let elapsed = START.get_or_init(Instant::now).elapsed().as_millis();
    u64::try_from(elapsed).unwrap_or(u64::MAX - 1) + 1
}

/// A write the store refused or never received. `err` is logged, not returned.
pub(crate) fn insert_failed(
    metrics: &ControlHistograms,
    stream: Stream,
    err: &anyhow::Error,
) -> ApiError {
    report(
        metrics,
        stream.gate(),
        now_ms(),
        stream,
        Reason::Insert,
        &format!("{err:#}"),
    );
    ApiError::Core(rolter_core::Error::Store(INSERT_FAILED.to_string()))
}

/// A write with nowhere to go, because no column store is configured.
///
/// `message` is what the caller is told. Unlike an insert failure it names a
/// setting rather than anything the store said, so it stays actionable.
pub(crate) fn unconfigured(
    metrics: &ControlHistograms,
    stream: Stream,
    message: &'static str,
) -> ApiError {
    report(
        metrics,
        stream.gate(),
        now_ms(),
        stream,
        Reason::Unconfigured,
        message,
    );
    ApiError::Core(rolter_core::Error::Store(message.to_string()))
}

/// Count the failure and, when the gate admits it, log it.
fn report(
    metrics: &ControlHistograms,
    gate: &WarnGate,
    now_ms: u64,
    stream: Stream,
    reason: Reason,
    detail: &str,
) {
    metrics.record_ingest_failure(stream.label(), reason.label());
    if let Some(suppressed) = gate.admit(now_ms, WARN_INTERVAL) {
        tracing::warn!(
            stream = stream.label(),
            reason = reason.label(),
            suppressed,
            error = %redact_userinfo(detail),
            "telemetry ingest failed; the events were dropped (further failures on this \
             stream are summarised once a minute)"
        );
    }
}

/// Mask the `user:password@` part of any URL in `text`.
///
/// The store's error quotes the URL it posted to, and `CLICKHOUSE_URL` may
/// carry its credentials inline; the log is the right place for the URL but
/// not for the password in it.
fn redact_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(scheme_end) = rest.find("://") {
        let authority_start = scheme_end + 3;
        out.push_str(&rest[..authority_start]);
        rest = &rest[authority_start..];
        let authority_end = rest
            .find(|c: char| c == '/' || c == '?' || c == '#' || c.is_whitespace() || c == ')')
            .unwrap_or(rest.len());
        if let Some(at) = rest[..authority_end].rfind('@') {
            out.push_str("***");
            rest = &rest[at..];
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::response::IntoResponse;
    use parking_lot::Mutex;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    const MINUTE_MS: u64 = 60_000;

    #[test]
    fn the_first_failure_warns_and_the_rest_of_the_interval_is_held() {
        let gate = WarnGate::new();
        assert_eq!(gate.admit(1, WARN_INTERVAL), Some(0));
        for t in [2, 500, MINUTE_MS - 1, MINUTE_MS] {
            assert_eq!(gate.admit(t, WARN_INTERVAL), None, "warned again at {t}ms");
        }
        // one interval after the warn, the next failure warns and says how
        // many it stands for
        assert_eq!(gate.admit(MINUTE_MS + 1, WARN_INTERVAL), Some(4));
        assert_eq!(gate.admit(MINUTE_MS + 2, WARN_INTERVAL), None);
        assert_eq!(gate.admit(3 * MINUTE_MS, WARN_INTERVAL), Some(1));
    }

    /// Records the warn events emitted while it is the default subscriber.
    #[derive(Clone, Default)]
    struct Warns(Arc<Mutex<Vec<String>>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Warns {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            struct Fields(String);
            impl tracing::field::Visit for Fields {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    self.0.push_str(&format!("{}={value:?} ", field.name()));
                }
            }
            if *event.metadata().level() == tracing::Level::WARN {
                let mut fields = Fields(String::new());
                event.record(&mut fields);
                self.0.lock().push(fields.0);
            }
        }
    }

    #[test]
    fn a_failing_flush_loop_logs_once_per_interval_but_counts_every_batch() {
        let warns = Warns::default();
        let subscriber = tracing_subscriber::registry().with(warns.clone());
        let gate = WarnGate::new();
        let metrics = ControlHistograms::default();

        tracing::subscriber::with_default(subscriber, || {
            // a dashboard flushing every 5s for three minutes into a dead store
            for tick in 0..36u64 {
                report(
                    &metrics,
                    &gate,
                    1 + tick * 5_000,
                    Stream::UiEvents,
                    Reason::Insert,
                    "clickhouse UX event insert failed (404 Not Found): no table",
                );
            }
        });

        let seen = warns.0.lock().clone();
        assert_eq!(seen.len(), 3, "one warn per minute, got {seen:?}");
        assert!(seen[0].contains("stream=\"ui_events\""), "{}", seen[0]);
        assert!(seen[0].contains("reason=\"insert\""), "{}", seen[0]);
        assert!(seen[0].contains("suppressed=0"), "{}", seen[0]);
        assert!(
            seen[0].contains("no table"),
            "the log keeps the detail: {}",
            seen[0]
        );
        // the later warns account for the batches they stood in for
        assert!(seen[1].contains("suppressed=11"), "{}", seen[1]);
    }

    #[test]
    fn the_log_keeps_the_url_but_not_its_password() {
        let warns = Warns::default();
        let subscriber = tracing_subscriber::registry().with(warns.clone());
        tracing::subscriber::with_default(subscriber, || {
            report(
                &ControlHistograms::default(),
                &WarnGate::new(),
                1,
                Stream::McpLogs,
                Reason::Insert,
                "error sending request for url (http://rolter:s3cret@ch:8123/?query=INSERT)",
            );
        });
        let seen = warns.0.lock().clone();
        assert_eq!(seen.len(), 1);
        assert!(!seen[0].contains("s3cret"), "{}", seen[0]);
        assert!(seen[0].contains("http://***@ch:8123/"), "{}", seen[0]);
    }

    #[test]
    fn userinfo_redaction_leaves_everything_else_alone() {
        assert_eq!(redact_userinfo("no url here"), "no url here");
        assert_eq!(
            redact_userinfo("post http://ch:8123/?q=a@b failed"),
            "post http://ch:8123/?q=a@b failed"
        );
        assert_eq!(
            redact_userinfo("a https://u:p@h/x and http://v@k"),
            "a https://***@h/x and http://***@k"
        );
    }

    async fn body_of(err: ApiError) -> (u16, String) {
        let response = err.into_response();
        let status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn the_500_body_quotes_nothing_the_store_said() {
        let upstream = anyhow::anyhow!(
            "clickhouse UX event insert failed (404 Not Found): Code: 60. DB::Exception: \
             Table default.ui_events does not exist. (UNKNOWN_TABLE) \
             url http://rolter:s3cret@clickhouse.internal:8123/?query=INSERT%20INTO%20ui_events"
        );
        let (status, body) = body_of(insert_failed(
            &ControlHistograms::default(),
            Stream::UiEvents,
            &upstream,
        ))
        .await;
        // still a 500: `ux.ts` reads that as transient and keeps sending, which
        // is the behaviour a store that comes back wants
        assert_eq!(status, 500);
        for leak in [
            "clickhouse",
            "ClickHouse",
            "8123",
            "ui_events",
            "UNKNOWN_TABLE",
            "INSERT",
            "http",
            "s3cret",
            "404",
        ] {
            assert!(!body.contains(leak), "the body leaks {leak:?}: {body}");
        }
        assert!(body.contains(INSERT_FAILED), "{body}");
    }

    #[tokio::test]
    async fn an_unconfigured_store_still_names_the_setting_to_fix() {
        let (status, body) = body_of(unconfigured(
            &ControlHistograms::default(),
            Stream::UiEvents,
            "UX event ingestion requires CLICKHOUSE_URL",
        ))
        .await;
        assert_eq!(status, 500);
        assert!(body.contains("requires CLICKHOUSE_URL"), "{body}");
    }
}
