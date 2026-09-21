//! Dashboard UX event ingestion (#805).
//!
//! Browser spans say how long a screen took and whether it errored. They do not
//! say whether it was *usable*: which screens are slow to become interactive,
//! where people back out, which forms get abandoned, which error states are
//! actually reached. This is the endpoint behind that stream, writing to the
//! `ui_events` ClickHouse table next to `request_logs` — same deployment, same
//! retention, same redaction, and joinable against gateway traffic on
//! `trace_id`.
//!
//! Two properties are worth stating plainly, because they are what make it safe
//! to point a browser at this endpoint:
//!
//! - **Events are structural only.** Every accepted field is a key, an enum, a
//!   duration or an id. There is no field a form value, prompt or free-text
//!   body could travel in, so the schema is the guarantee rather than a policy
//!   applied on write. A validation *rule name* is accepted; the value that
//!   failed it is not.
//! - **Attribution is server-side.** `user_id` is taken from the authenticated
//!   principal and a client-supplied one is ignored, so a caller cannot file
//!   events against somebody else.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::analytics::client_or_503;
use crate::auth::CurrentUser;
use crate::crud::{ApiError, ApiResult};
use crate::ingest_failure::{self, Stream};
use crate::ControlState;

/// The `action` enum in `008_ui_events.sql`, widened by
/// `010_ui_events_struggle_signals.sql`. ClickHouse rejects a value outside
/// its `Enum8`, but failing here gives the caller a 400 naming the field
/// instead of a 500 from the database.
const ACTIONS: &[&str] = &[
    "screen_view",
    "time_to_interactive",
    "navigate",
    "back_out",
    "form_submit",
    "form_abandon",
    "validation_error",
    "empty_state",
    "error_state",
    "save_confirmed",
    // the struggle signals (#1731): what the operator could *not* do. appended
    // rather than sorted in, so this list reads in the same order as the
    // `Enum8` ordinals it mirrors
    "retry_submit",
    "refused_click",
    "abandon_dirty",
];

/// The `outcome` enum in the same migration.
const OUTCOMES: &[&str] = &["ok", "error", "cancelled"];

/// Browsers batch these — a `visibilitychange` beacon flushes everything queued
/// since the last one — so a batch is the normal shape, not an optimization.
/// The cap keeps one call from turning into an unbounded insert.
const MAX_BATCH: usize = 100;

/// `screen`, `target` and `from_screen` are `LowCardinality` columns: they are
/// meant to hold a bounded set of stable keys. This bound is deliberately tight
/// enough that a URL with ids and query parameters in it will not fit, which is
/// the mistake most likely to turn a key column into a free-text one.
const MAX_KEY_LEN: usize = 96;

/// How far ahead of the control plane a browser's own clock may be and still be
/// believed. A skewed clock is common — a laptop resuming from sleep, a VM with
/// no NTP — and a row stamped in the future breaks every window query it lands
/// in, so anything beyond this is treated as unusable rather than trusted.
const MAX_CLOCK_AHEAD: Duration = Duration::minutes(5);

/// And how far behind. Generous, because the whole point of the queue is that a
/// tab can be offline for a while and flush later; beyond a day the event says
/// more about the tab than about the interaction.
const MAX_CLOCK_BEHIND: Duration = Duration::hours(24);

pub(crate) fn router() -> Router<ControlState> {
    Router::new().route("/api/v1/ui-events", post(ingest))
}

#[derive(Debug, Deserialize)]
struct Batch {
    events: Vec<UiEvent>,
}

#[derive(Debug, Deserialize)]
struct UiEvent {
    event_id: String,
    /// when the interaction happened, by the browser's clock (RFC 3339).
    /// absent means "use ingest time", which is what every client sent before
    /// #1224 — the batcher stamps each event as it is queued
    #[serde(default)]
    ts: Option<String>,
    /// stable screen key (a route id), never a URL
    screen: String,
    action: String,
    #[serde(default)]
    outcome: Option<String>,
    /// the name of the form, control or validation rule — never its value
    #[serde(default)]
    target: String,
    #[serde(default)]
    from_screen: String,
    #[serde(default)]
    duration_ms: u64,
    /// links the event to the gateway request it caused; empty when the
    /// interaction issued no request, or when browser tracing is off
    #[serde(default)]
    trace_id: String,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    org_id: String,
    #[serde(default)]
    team_id: String,
    #[serde(default)]
    project_id: String,
    #[serde(default)]
    app_version: String,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(rolter_core::Error::Config(message.into()))
}

/// A bounded, control-character-free key. Empty is allowed for the optional
/// columns; the caller decides which fields are required.
fn validate_key(value: &str, name: &str, required: bool) -> Result<(), String> {
    if value.is_empty() {
        return if required {
            Err(format!("{name} is required"))
        } else {
            Ok(())
        };
    }
    if value.len() > MAX_KEY_LEN {
        return Err(format!("{name} must be at most {MAX_KEY_LEN} characters"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{name} must not contain control characters"));
    }
    Ok(())
}

/// The instant to record, in the RFC 3339 form ClickHouse's `best_effort`
/// parser reads into `DateTime64(3)`.
///
/// The browser's clock is not the server's, so a supplied instant is believed
/// only inside a window around ingest time: further ahead than
/// `MAX_CLOCK_AHEAD` or further behind than `MAX_CLOCK_BEHIND` and the ingest
/// instant is recorded instead. Clamping to the boundary was the alternative
/// and is worse — it would manufacture a cluster of rows exactly five minutes
/// out, which reads as real traffic rather than as a bad clock (#1224).
fn event_ts(supplied: Option<&str>, now: DateTime<Utc>) -> Result<String, String> {
    let ts = match supplied.filter(|value| !value.is_empty()) {
        Some(value) => {
            let parsed = DateTime::parse_from_rfc3339(value)
                .map_err(|_| "ts must be an RFC 3339 timestamp".to_string())?
                .with_timezone(&Utc);
            if parsed > now + MAX_CLOCK_AHEAD || parsed < now - MAX_CLOCK_BEHIND {
                now
            } else {
                parsed
            }
        }
        None => now,
    };
    Ok(ts.to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn validate(event: &UiEvent) -> Result<(), String> {
    validate_key(&event.event_id, "event_id", true)?;
    validate_key(&event.screen, "screen", true)?;
    validate_key(&event.target, "target", false)?;
    validate_key(&event.from_screen, "from_screen", false)?;
    validate_key(&event.session_id, "session_id", false)?;
    validate_key(&event.trace_id, "trace_id", false)?;
    validate_key(&event.app_version, "app_version", false)?;
    validate_key(&event.org_id, "org_id", false)?;
    validate_key(&event.team_id, "team_id", false)?;
    validate_key(&event.project_id, "project_id", false)?;
    if !ACTIONS.contains(&event.action.as_str()) {
        return Err(format!("action must be one of {ACTIONS:?}"));
    }
    if let Some(outcome) = &event.outcome {
        if !OUTCOMES.contains(&outcome.as_str()) {
            return Err(format!("outcome must be one of {OUTCOMES:?}"));
        }
    }
    if event.duration_ms > u32::MAX.into() {
        return Err("duration_ms exceeds UInt32 range".to_string());
    }
    // parsed here rather than at insert time so a malformed instant is a 400
    // naming the field, like every other bad value in the batch
    event_ts(event.ts.as_deref(), Utc::now())?;
    Ok(())
}

/// Build the ClickHouse row. `user_id` is threaded in from the authenticated
/// principal rather than read off the event, so attribution cannot be forged.
fn row(event: &UiEvent, user_id: &str, now: DateTime<Utc>) -> Value {
    json!({
        // when the interaction happened, not when the batch arrived: the
        // dashboard queues events and flushes on a timer or at unload, so
        // ingest time collapsed a whole session onto one instant and put the
        // event minutes away from the request it joins on `trace_id` (#1224)
        "ts": event_ts(event.ts.as_deref(), now).unwrap_or_else(|_| {
            now.to_rfc3339_opts(SecondsFormat::Millis, true)
        }),
        "event_id": event.event_id,
        "trace_id": event.trace_id,
        "session_id": event.session_id,
        "org_id": event.org_id,
        "team_id": event.team_id,
        "project_id": event.project_id,
        "user_id": user_id,
        "screen": event.screen,
        "action": event.action,
        "target": event.target,
        // the column is not nullable; 'ok' is the neutral value for the
        // actions that carry no outcome of their own, such as screen_view
        "outcome": event.outcome.as_deref().unwrap_or("ok"),
        "duration_ms": event.duration_ms as u32,
        "from_screen": event.from_screen,
        "app_version": event.app_version,
    })
}

async fn ingest(
    caller: CurrentUser,
    State(state): State<ControlState>,
    Json(batch): Json<Batch>,
) -> ApiResult<StatusCode> {
    // holding a `CurrentUser` is the whole check, as in `me.rs`: this is the
    // dashboard reporting on its own user's session, not an operator surface.
    // There is deliberately no capability row — the RBAC model holds that a
    // viewer writes nothing, and a viewer who could not file their own screen
    // views would simply be missing from every funnel

    // deployment-level opt-out, checked before validation so a deployment that
    // wants none of this is not also a deployment that argues about payloads
    if !state.store.load().await?.logging.ui_events {
        return Ok(StatusCode::ACCEPTED);
    }

    if batch.events.is_empty() {
        return Ok(StatusCode::ACCEPTED);
    }
    if batch.events.len() > MAX_BATCH {
        return Err(invalid(format!(
            "batch holds {} events; the limit is {MAX_BATCH}",
            batch.events.len()
        )));
    }
    for event in &batch.events {
        validate(event).map_err(invalid)?;
    }

    let user_id = caller.user.id.to_string();
    // one `now` for the batch: the events that fall back to ingest time should
    // agree with each other rather than drift across the loop
    let now = Utc::now();
    let rows: Vec<Value> = batch.events.iter().map(|e| row(e, &user_id, now)).collect();

    let ch = client_or_503(&state).map_err(|_| {
        ingest_failure::unconfigured(
            &state.metrics,
            Stream::UiEvents,
            "UX event ingestion requires CLICKHOUSE_URL",
        )
    })?;
    ch.insert_ui_events(&rows)
        .await
        .map_err(|err| ingest_failure::insert_failed(&state.metrics, Stream::UiEvents, &err))?;
    Ok(StatusCode::ACCEPTED)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> UiEvent {
        UiEvent {
            event_id: "01J8Z0".to_string(),
            ts: None,
            screen: "models".to_string(),
            action: "screen_view".to_string(),
            outcome: None,
            target: String::new(),
            from_screen: String::new(),
            duration_ms: 0,
            trace_id: String::new(),
            session_id: String::new(),
            org_id: String::new(),
            team_id: String::new(),
            project_id: String::new(),
            app_version: String::new(),
        }
    }

    #[test]
    fn two_events_queued_apart_survive_one_batch_with_distinct_ts() {
        // the bug: the row was stamped at ingest, so a batch flushed once every
        // few seconds put every event in it at the same instant (#1224)
        let mut first = event();
        first.ts = Some("2026-09-18T10:00:00.250Z".to_string());
        let mut second = event();
        second.event_id = "01J8Z1".to_string();
        second.ts = Some("2026-09-18T10:00:04.750Z".to_string());

        // pinned beside the fixtures: against the wall clock both fall outside
        // MAX_CLOCK_BEHIND a day after they were written and get replaced
        let now = DateTime::parse_from_rfc3339("2026-09-18T10:00:05Z")
            .unwrap()
            .with_timezone(&Utc);
        let rows = [row(&first, "user-1", now), row(&second, "user-1", now)];
        assert_eq!(rows[0]["ts"], "2026-09-18T10:00:00.250Z");
        assert_eq!(rows[1]["ts"], "2026-09-18T10:00:04.750Z");
    }

    #[test]
    fn an_absent_ts_falls_back_to_ingest_time() {
        let now = DateTime::parse_from_rfc3339("2026-09-18T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            row(&event(), "user-1", now)["ts"],
            "2026-09-18T10:00:00.000Z"
        );
        // an empty string is what a client that stringifies an unset field
        // sends, and means the same thing
        let mut e = event();
        e.ts = Some(String::new());
        assert_eq!(row(&e, "user-1", now)["ts"], "2026-09-18T10:00:00.000Z");
    }

    #[test]
    fn an_offset_ts_is_normalised_to_utc() {
        let now = Utc::now();
        let mut e = event();
        e.ts = Some(
            (now - Duration::minutes(1))
                .with_timezone(&chrono::FixedOffset::east_opt(2 * 3600).unwrap())
                .to_rfc3339(),
        );
        let ts = row(&e, "user-1", now)["ts"].as_str().unwrap().to_string();
        assert!(ts.ends_with('Z'), "{ts} kept its offset");
    }

    #[test]
    fn a_skewed_client_clock_is_replaced_by_ingest_time() {
        let now = DateTime::parse_from_rfc3339("2026-09-18T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        for supplied in [
            // a laptop whose clock is a year fast would otherwise sit at the
            // top of every "recent events" query forever
            "2027-09-18T10:00:00Z",
            "2020-01-01T00:00:00Z",
        ] {
            assert_eq!(
                event_ts(Some(supplied), now).unwrap(),
                "2026-09-18T10:00:00.000Z",
                "{supplied} was trusted"
            );
        }
        // just inside the window is still believed
        assert_eq!(
            event_ts(Some("2026-09-18T10:04:00Z"), now).unwrap(),
            "2026-09-18T10:04:00.000Z"
        );
    }

    #[test]
    fn rejects_a_ts_that_is_not_rfc_3339() {
        let mut e = event();
        e.ts = Some("18/09/2026 10:00".to_string());
        let err = validate(&e).unwrap_err();
        assert!(err.contains("ts"), "{err}");
    }

    #[test]
    fn accepts_a_minimal_event() {
        assert!(validate(&event()).is_ok());
    }

    #[test]
    fn every_migration_action_is_accepted() {
        // drift here would 500 on the ClickHouse Enum8 instead of 400ing
        for action in ACTIONS {
            let mut e = event();
            e.action = (*action).to_string();
            assert!(validate(&e).is_ok(), "{action} rejected");
        }
    }

    #[test]
    fn the_action_list_is_the_enum8_in_ordinal_order() {
        // an `Enum8` ordinal *is* the stored value, so the migrations may only
        // append. this pins the order rather than the membership: a value moved
        // up the list here is a value that would have to be renumbered there,
        // which rewrites the meaning of every row already written (#1731)
        assert_eq!(
            ACTIONS,
            &[
                "screen_view",
                "time_to_interactive",
                "navigate",
                "back_out",
                "form_submit",
                "form_abandon",
                "validation_error",
                "empty_state",
                "error_state",
                "save_confirmed",
                "retry_submit",
                "refused_click",
                "abandon_dirty",
            ]
        );
    }

    #[test]
    fn a_struggle_signal_round_trips_with_its_control_and_capability() {
        // refused_click is the one with a shape of its own: the target is the
        // control key joined to the capability that refused it, and nothing
        // else. no label, no message, no identity beyond the ids every event
        // already carries
        let mut e = event();
        "refused_click".clone_into(&mut e.action);
        "providers-new:provider:create".clone_into(&mut e.target);
        e.outcome = Some("error".to_string());
        assert!(validate(&e).is_ok());

        let built = row(&e, "user-1", Utc::now());
        assert_eq!(built["action"], json!("refused_click"));
        assert_eq!(built["target"], json!("providers-new:provider:create"));
        assert_eq!(built["outcome"], json!("error"));

        for action in ["retry_submit", "abandon_dirty"] {
            let mut e = event();
            action.clone_into(&mut e.action);
            "provider-create".clone_into(&mut e.target);
            e.duration_ms = 4_200;
            assert!(validate(&e).is_ok(), "{action} rejected");
            let built = row(&e, "user-1", Utc::now());
            assert_eq!(built["action"], json!(action));
            assert_eq!(built["duration_ms"], json!(4_200));
        }
    }

    #[test]
    fn rejects_an_unknown_action_and_outcome() {
        let mut e = event();
        "sudo_make_me_a_sandwich".clone_into(&mut e.action);
        assert!(validate(&e).is_err());

        let mut e = event();
        e.outcome = Some("maybe".to_string());
        assert!(validate(&e).is_err());
    }

    #[test]
    fn requires_the_identifying_fields() {
        let mut e = event();
        e.event_id = String::new();
        assert!(validate(&e).is_err());

        let mut e = event();
        e.screen = String::new();
        assert!(validate(&e).is_err());
    }

    #[test]
    fn a_url_shaped_screen_key_does_not_fit() {
        // the failure mode this bound exists for: a real URL carries ids and
        // query parameters, which would make a LowCardinality column unbounded
        let mut e = event();
        e.screen = format!(
            "https://rolter.example/models/{}?tab=targets",
            "x".repeat(80)
        );
        assert!(validate(&e).is_err());
    }

    #[test]
    fn rejects_control_characters_in_a_key() {
        let mut e = event();
        "models\nrows".clone_into(&mut e.target);
        assert!(validate(&e).is_err());
    }

    #[test]
    fn duration_must_fit_the_uint32_column() {
        let mut e = event();
        e.duration_ms = u64::from(u32::MAX) + 1;
        assert!(validate(&e).is_err());
    }

    #[test]
    fn the_row_takes_user_id_from_the_principal_not_the_event() {
        let built = row(&event(), "11111111-2222-3333-4444-555555555555", Utc::now());
        assert_eq!(
            built["user_id"],
            json!("11111111-2222-3333-4444-555555555555")
        );
        // and an action with no outcome of its own still fills the column
        assert_eq!(built["outcome"], json!("ok"));
    }

    #[test]
    fn the_row_carries_every_column_the_table_declares() {
        let built = row(&event(), "u", Utc::now());
        for column in [
            "ts",
            "event_id",
            "trace_id",
            "session_id",
            "org_id",
            "team_id",
            "project_id",
            "user_id",
            "screen",
            "action",
            "target",
            "outcome",
            "duration_ms",
            "from_screen",
            "app_version",
        ] {
            assert!(built.get(column).is_some(), "row is missing {column}");
        }
    }
}
