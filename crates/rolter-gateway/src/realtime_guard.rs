//! Content policy for `/v1/realtime` sessions (#1880).
//!
//! A realtime session is a stream of small JSON events in both directions, so
//! the request/response guardrail walk of the HTTP pipelines has no moment to
//! run. This module applies the same compiled rules to the text-bearing events
//! instead, and the decision it implements is written down in
//! `docs/dev-docs/architecture/realtime-metering.md`.
//!
//! The policy is resolved once when the session opens and pinned for its
//! lifetime, like the target. A session whose route and snapshot configure no
//! guardrail, webhook or plugin gets no policy at all ([`ContentPolicy::for_session`]
//! returns `None`) and the relay forwards frames untouched.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;

use rolter_core::guardrails::RuleSelection;
use rolter_core::{
    CompiledGuardrails, GuardrailReport, GuardrailWebhookConfig, PluginStage, PluginsConfig,
    ScanOutcome, WebhookStage, WebhookTenant,
};
use serde_json::Value;

use crate::budgets::ScopeIds;
use crate::metrics::Metrics;
use crate::state::{RouteEntry, Snapshot};

/// How much of a delivered text stream is kept to match a pattern split across
/// two deltas. Longer than any built-in entity; a custom pattern that can match
/// more than this straddling a delta boundary is only caught on the completed
/// text, after the deltas have gone out.
const WINDOW_BYTES: usize = 512;

/// Items whose streamed text is tracked at once. A session rarely has more than
/// one or two responses in flight, so the oldest entry is dropped past this.
const MAX_TRACKED: usize = 16;

/// What the relay does with a client event.
pub(crate) enum ClientVerdict {
    /// forward the frame as received
    Pass,
    /// forward this frame instead (a redaction or a transform)
    Replace(String),
    /// do not forward; send this `error` event to the client and keep the
    /// session open
    Reject(String),
}

/// What the relay does with a server event.
pub(crate) struct ServerOutcome {
    pub frame: Frame,
    /// an `error` event for the client, sent before the frame
    pub notice: Option<String>,
    /// an event for the upstream, to stop work the client will never see
    pub cancel: Option<String>,
}

pub(crate) enum Frame {
    Pass,
    Replace(String),
    Drop,
}

impl ServerOutcome {
    const PASS: Self = Self {
        frame: Frame::Pass,
        notice: None,
        cancel: None,
    };
}

/// The rules one session is held to.
pub(crate) struct ContentPolicy {
    guardrails: Arc<CompiledGuardrails>,
    selection: RuleSelection,
    input: bool,
    output: bool,
    webhook: Option<GuardrailWebhookConfig>,
    plugins: Option<Arc<PluginsConfig>>,
    org: String,
    project: Option<String>,
    model: String,
    route: String,
    trace_id: String,
    tenant: WebhookTenant,
    /// responses whose text was withheld; the rest of their text events are
    /// dropped
    blocked: Vec<String>,
    /// the tail of the text delivered so far, per streamed item
    tails: Vec<(String, String)>,
}

impl ContentPolicy {
    /// The policy a session opened now would be held to, or `None` when none
    /// of the snapshot's guardrails, webhook or plugins applies, which keeps
    /// the relay on its unchanged fast path.
    pub(crate) fn for_session(
        snap: &Snapshot,
        entry: &RouteEntry,
        scope: &ScopeIds,
        model: &str,
        trace_id: &str,
    ) -> Option<Self> {
        let input = snap.guardrails.pre_call_active_for(&entry.guardrails);
        let output = snap.guardrails.post_call_active_for(&entry.guardrails);
        let webhook = (snap.guardrail_webhook.enabled
            && snap.guardrail_webhook.stage == WebhookStage::PreCall)
            .then(|| snap.guardrail_webhook.clone());
        let project = (!scope.project.is_empty()).then(|| scope.project.clone());
        let has_plugins = !snap
            .plugins
            .for_stage(PluginStage::PreUpstream, &scope.org, project.as_deref())
            .is_empty();
        if !input && !output && webhook.is_none() && !has_plugins {
            return None;
        }
        Some(Self {
            guardrails: snap.guardrails.clone(),
            selection: entry.guardrails.clone(),
            input,
            output,
            webhook,
            plugins: has_plugins.then(|| snap.plugins.clone()),
            org: scope.org.clone(),
            project,
            model: model.to_string(),
            route: entry.route.model.clone(),
            trace_id: trace_id.to_string(),
            tenant: crate::handlers::plugin_tenant(scope),
            blocked: Vec::new(),
            tails: Vec::new(),
        })
    }

    /// Apply the input stage to one client text frame.
    pub(crate) async fn client_event(&mut self, metrics: &Metrics, frame: &str) -> ClientVerdict {
        // audio arrives in frames of tens of kilobytes. skipped only when the
        // frame names its type once, so a second `type` key cannot disguise a
        // text event as audio
        if crate::realtime_metering::leading_type(frame) == Some("input_audio_buffer.append")
            && frame.matches("\"type\"").count() == 1
        {
            return ClientVerdict::Pass;
        }
        let Ok(mut event) = serde_json::from_str::<Value>(frame) else {
            // the upstream will refuse a frame that is not JSON too
            return ClientVerdict::Pass;
        };
        let Some(kind) = event.get("type").and_then(Value::as_str).map(str::to_owned) else {
            return ClientVerdict::Pass;
        };
        if !matches!(
            kind.as_str(),
            "conversation.item.create" | "session.update" | "response.create"
        ) {
            return ClientVerdict::Pass;
        }
        let event_id = event
            .get("event_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let mut changed = false;

        if self.input {
            let mut scan = InputScan {
                policy: &*self,
                budget: self.guardrails.scan_budget(),
                report: GuardrailReport::default(),
                blocked: None,
            };
            for_each_client_text(&mut event, &kind, &mut |slot, is_system| {
                scan.slot(slot, is_system)
            });
            let InputScan {
                report, blocked, ..
            } = scan;
            if let Some(rule) = blocked {
                metrics.guardrail_blocks_total.fetch_add(1, Relaxed);
                return ClientVerdict::Reject(rejection(
                    "guardrail_blocked",
                    format!("event blocked by guardrail '{rule}'"),
                    event_id,
                ));
            }
            if report.redactions > 0 {
                changed = true;
                metrics
                    .guardrail_redactions_total
                    .fetch_add(report.redactions as u64, Relaxed);
            }
        }

        if let Some(webhook) = &self.webhook {
            match crate::guardrail_webhook::consult_pre_call(
                webhook,
                metrics,
                &self.model,
                &self.route,
                &self.trace_id,
                self.tenant.clone(),
                &event,
            )
            .await
            {
                crate::guardrail_webhook::WebhookOutcome::Allow => {}
                crate::guardrail_webhook::WebhookOutcome::Block(reason) => {
                    return ClientVerdict::Reject(rejection(
                        "guardrail_blocked",
                        reason.unwrap_or_else(|| "event blocked by guardrail service".into()),
                        event_id,
                    ));
                }
                crate::guardrail_webhook::WebhookOutcome::Transform(content) => {
                    match transformed(&kind, content) {
                        Some(content) => {
                            event = content;
                            changed = true;
                        }
                        None => return malformed_transform(event_id),
                    }
                }
            }
        }

        if let Some(plugins) = &self.plugins {
            let list =
                plugins.for_stage(PluginStage::PreUpstream, &self.org, self.project.as_deref());
            match crate::plugin_dispatch::dispatch(
                &list,
                PluginStage::PreUpstream,
                metrics,
                &self.model,
                &self.route,
                &self.trace_id,
                &self.tenant,
                &event,
            )
            .await
            {
                crate::plugin_dispatch::DispatchOutcome::Allow(content) => {
                    if content != event {
                        match transformed(&kind, content) {
                            Some(content) => {
                                event = content;
                                changed = true;
                            }
                            None => return malformed_transform(event_id),
                        }
                    }
                }
                crate::plugin_dispatch::DispatchOutcome::Block(reason) => {
                    return ClientVerdict::Reject(rejection(
                        "plugin_blocked",
                        reason.unwrap_or_else(|| "event blocked by plugin".into()),
                        event_id,
                    ));
                }
            }
        }

        if !changed {
            return ClientVerdict::Pass;
        }
        match serde_json::to_string(&event) {
            Ok(text) => ClientVerdict::Replace(text),
            // forwarding the original would skip the rewrite
            Err(_) => malformed_transform(event_id),
        }
    }

    /// Apply the output stage to one server text frame.
    pub(crate) fn server_event(&mut self, metrics: &Metrics, frame: &str) -> ServerOutcome {
        if !self.output {
            return ServerOutcome::PASS;
        }
        // the type is trusted here, the upstream being the one writing it, so
        // audio deltas are skipped without parsing them
        if let Some(kind) = crate::realtime_metering::leading_type(frame) {
            if !is_text_event(kind) {
                return ServerOutcome::PASS;
            }
        }
        let Ok(mut event) = serde_json::from_str::<Value>(frame) else {
            return ServerOutcome::PASS;
        };
        let Some(kind) = event.get("type").and_then(Value::as_str).map(str::to_owned) else {
            return ServerOutcome::PASS;
        };
        if !is_text_event(&kind) {
            return ServerOutcome::PASS;
        }
        if is_delta(&kind) {
            self.delta(metrics, &mut event)
        } else {
            self.completed(metrics, &kind, &mut event)
        }
    }

    fn delta(&mut self, metrics: &Metrics, event: &mut Value) -> ServerOutcome {
        let response_id = event
            .get("response_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if self.blocked.contains(&response_id) {
            return drop_frame();
        }
        let item_id = event
            .get("item_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let Some(delta) = event.get("delta").and_then(Value::as_str) else {
            return ServerOutcome::PASS;
        };
        let mut report = GuardrailReport::default();
        let mut budget = self.guardrails.scan_budget();
        let mut redacted = None;
        match self
            .guardrails
            .scan_output(delta, &mut budget, &mut report, &self.selection)
        {
            ScanOutcome::Unchanged => {}
            ScanOutcome::Redacted(text) => redacted = Some(text),
            ScanOutcome::Blocked(rule) => {
                return self.withhold(metrics, response_id, rule);
            }
        }
        let delivered = redacted.as_deref().unwrap_or(delta);

        // a pattern split over two deltas matches neither alone
        let position = self.tails.iter().position(|(id, _)| *id == item_id);
        let tail = position.map(|i| self.tails[i].1.as_str()).unwrap_or("");
        let mut window = String::with_capacity(tail.len() + delivered.len());
        window.push_str(tail);
        window.push_str(delivered);
        if !tail.is_empty() {
            let mut budget = self.guardrails.scan_budget();
            let mut straddle = GuardrailReport::default();
            if let ScanOutcome::Blocked(rule) =
                self.guardrails
                    .scan_output(&window, &mut budget, &mut straddle, &self.selection)
            {
                return self.withhold(metrics, response_id, rule);
            }
        }
        let keep_from = window.len().saturating_sub(WINDOW_BYTES);
        let keep_from = (keep_from..=window.len())
            .find(|i| window.is_char_boundary(*i))
            .unwrap_or(window.len());
        let kept = window[keep_from..].to_owned();
        match position {
            Some(i) => self.tails[i].1 = kept,
            None => {
                if self.tails.len() >= MAX_TRACKED {
                    self.tails.remove(0);
                }
                self.tails.push((item_id, kept));
            }
        }

        match redacted {
            None => ServerOutcome::PASS,
            Some(text) => {
                metrics
                    .guardrail_output_redactions_total
                    .fetch_add(report.redactions as u64, Relaxed);
                event["delta"] = Value::String(text);
                replace(event)
            }
        }
    }

    /// Withhold the rest of a response after a delta matched a blocking rule:
    /// the client is told, and the upstream stops generating what will not be
    /// delivered.
    fn withhold(&mut self, metrics: &Metrics, response_id: String, rule: String) -> ServerOutcome {
        metrics.guardrail_output_blocks_total.fetch_add(1, Relaxed);
        let cancel = (!response_id.is_empty()).then(|| {
            serde_json::json!({"type": "response.cancel", "response_id": response_id}).to_string()
        });
        if self.blocked.len() >= MAX_TRACKED {
            self.blocked.remove(0);
        }
        self.blocked.push(response_id);
        ServerOutcome {
            frame: Frame::Drop,
            notice: Some(rejection(
                "guardrail_blocked",
                format!("response blocked by guardrail '{rule}'"),
                None,
            )),
            cancel,
        }
    }

    /// A `.done` event or `response.done`: the whole text of an item, checked
    /// once more because a delta check can miss what only the whole shows.
    fn completed(&mut self, metrics: &Metrics, kind: &str, event: &mut Value) -> ServerOutcome {
        let response_id = event
            .get("response_id")
            .or_else(|| event.pointer("/response/id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if let Some(item_id) = event.get("item_id").and_then(Value::as_str) {
            self.tails.retain(|(id, _)| id != item_id);
        }
        if kind == FUNCTION_ARGS_DONE && self.blocked.contains(&response_id) {
            // the response is already cancelled and its client told
            return drop_frame();
        }
        let already_told = self.blocked.contains(&response_id);
        if already_told && kind == "response.done" {
            self.blocked.retain(|id| *id != response_id);
        }

        let mut report = GuardrailReport::default();
        let mut blocked_by = None;
        let mut changed = false;
        let mut budget = self.guardrails.scan_budget();
        let guardrails = &self.guardrails;
        let selection = &self.selection;
        for_each_server_text(event, kind, &mut |slot| {
            let Some(text) = slot.as_str() else { return };
            if already_told {
                *slot = Value::String(String::new());
                changed = true;
                return;
            }
            match guardrails.scan_output(text, &mut budget, &mut report, selection) {
                ScanOutcome::Unchanged => {}
                ScanOutcome::Redacted(text) => {
                    *slot = Value::String(text);
                    changed = true;
                }
                ScanOutcome::Blocked(rule) => {
                    // the event still goes out so the client's turn ends, minus
                    // the text
                    *slot = Value::String(String::new());
                    changed = true;
                    blocked_by.get_or_insert(rule);
                }
            }
        });
        if report.redactions > 0 {
            metrics
                .guardrail_output_redactions_total
                .fetch_add(report.redactions as u64, Relaxed);
        }
        if kind == FUNCTION_ARGS_DONE {
            if let Some(rule) = blocked_by {
                // blanking would hand the client a call with no arguments to
                // run, so the event is withheld and the response stopped
                return self.withhold(metrics, response_id, rule);
            }
        }
        let notice = blocked_by.map(|rule| {
            metrics.guardrail_output_blocks_total.fetch_add(1, Relaxed);
            rejection(
                "guardrail_blocked",
                format!("response blocked by guardrail '{rule}'"),
                None,
            )
        });
        if !changed {
            return ServerOutcome::PASS;
        }
        let mut outcome = replace(event);
        outcome.notice = notice;
        outcome
    }
}

/// The input stage's walk over one event.
struct InputScan<'a> {
    policy: &'a ContentPolicy,
    budget: usize,
    report: GuardrailReport,
    blocked: Option<String>,
}

impl InputScan<'_> {
    fn slot(&mut self, slot: &mut Value, is_system: bool) {
        if self.blocked.is_some() {
            return;
        }
        let Some(text) = slot.as_str() else { return };
        match self.policy.guardrails.scan_segment_with(
            text,
            is_system,
            &mut self.budget,
            &mut self.report,
            &self.policy.selection,
        ) {
            ScanOutcome::Unchanged => {}
            ScanOutcome::Redacted(text) => *slot = Value::String(text),
            ScanOutcome::Blocked(rule) => self.blocked = Some(rule),
        }
    }
}

fn replace(event: &Value) -> ServerOutcome {
    match serde_json::to_string(event) {
        Ok(text) => ServerOutcome {
            frame: Frame::Replace(text),
            notice: None,
            cancel: None,
        },
        // delivering the original would deliver the text this meant to mask
        Err(_) => drop_frame(),
    }
}

fn drop_frame() -> ServerOutcome {
    ServerOutcome {
        frame: Frame::Drop,
        notice: None,
        cancel: None,
    }
}

/// A transform may rewrite an event's content but not turn it into another
/// kind of event, which would carry text past the checks that ran on the
/// original.
fn transformed(kind: &str, content: Value) -> Option<Value> {
    (content.get("type").and_then(Value::as_str) == Some(kind)).then_some(content)
}

fn malformed_transform(event_id: Option<String>) -> ClientVerdict {
    ClientVerdict::Reject(rejection(
        "guardrail_blocked",
        "event blocked: a guardrail service returned an event of another type".into(),
        event_id,
    ))
}

/// The Realtime `error` event for a refused event.
fn rejection(code: &'static str, message: String, event_id: Option<String>) -> String {
    let mut event =
        crate::realtime::error_event(axum::http::StatusCode::BAD_REQUEST, code, &message);
    if let Some(id) = event_id {
        // how the Realtime API ties an error to the client event behind it
        if let Ok(mut parsed) = serde_json::from_str::<Value>(&event) {
            parsed["error"]["event_id"] = Value::String(id);
            event = parsed.to_string();
        }
    }
    event
}

/// The model's completed tool call arguments, a JSON document in a string.
const FUNCTION_ARGS_DONE: &str = "response.function_call_arguments.done";

fn is_delta(kind: &str) -> bool {
    matches!(
        kind,
        "response.output_text.delta"
            | "response.text.delta"
            | "response.audio_transcript.delta"
            | "response.output_audio_transcript.delta"
    )
}

fn is_text_event(kind: &str) -> bool {
    is_delta(kind)
        || matches!(
            kind,
            "response.output_text.done"
                | "response.text.done"
                | "response.audio_transcript.done"
                | "response.output_audio_transcript.done"
                | "response.content_part.done"
                | "response.output_item.done"
                | "response.done"
                | FUNCTION_ARGS_DONE
        )
}

/// Visit the `text` and `transcript` strings of one content part.
fn part_texts(part: &mut Value, visit: &mut dyn FnMut(&mut Value)) {
    for key in ["text", "transcript"] {
        if let Some(slot) = part.get_mut(key).filter(|slot| slot.is_string()) {
            visit(slot);
        }
    }
}

fn content_texts(holder: &mut Value, visit: &mut dyn FnMut(&mut Value)) {
    if let Some(parts) = holder.get_mut("content").and_then(Value::as_array_mut) {
        for part in parts {
            part_texts(part, visit);
        }
    }
}

/// What the model emitted in an output item: message text, or for a function
/// call its arguments, which stand where a message's text does. Only the
/// server leg reads arguments; a client's own call items are left alone.
fn output_item_texts(item: &mut Value, visit: &mut dyn FnMut(&mut Value)) {
    content_texts(item, visit);
    if item.get("type").and_then(Value::as_str) == Some("function_call") {
        if let Some(slot) = item.get_mut("arguments").filter(|slot| slot.is_string()) {
            visit(slot);
        }
    }
}

fn for_each_server_text(event: &mut Value, kind: &str, visit: &mut dyn FnMut(&mut Value)) {
    match kind {
        "response.output_text.done"
        | "response.text.done"
        | "response.audio_transcript.done"
        | "response.output_audio_transcript.done" => part_texts(event, visit),
        FUNCTION_ARGS_DONE => {
            if let Some(slot) = event.get_mut("arguments").filter(|slot| slot.is_string()) {
                visit(slot);
            }
        }
        "response.content_part.done" => {
            if let Some(part) = event.get_mut("part") {
                part_texts(part, visit);
            }
        }
        "response.output_item.done" => {
            if let Some(item) = event.get_mut("item") {
                output_item_texts(item, visit);
            }
        }
        "response.done" => {
            if let Some(items) = event
                .pointer_mut("/response/output")
                .and_then(Value::as_array_mut)
            {
                for item in items {
                    output_item_texts(item, visit);
                }
            }
        }
        _ => {}
    }
}

/// An item a client creates: message text, an audio part's transcript, or the
/// output of a function call it ran.
fn item_texts(item: &mut Value, visit: &mut dyn FnMut(&mut Value, bool)) {
    let mut plain = |slot: &mut Value| visit(slot, false);
    content_texts(item, &mut plain);
    if let Some(output) = item.get_mut("output").filter(|slot| slot.is_string()) {
        visit(output, false);
    }
}

/// `is_system` marks operator-authored instructions, which the rules leave
/// alone unless they opt in, as they do for a chat system message.
fn for_each_client_text(event: &mut Value, kind: &str, visit: &mut dyn FnMut(&mut Value, bool)) {
    match kind {
        "conversation.item.create" => {
            if let Some(item) = event.get_mut("item") {
                item_texts(item, visit);
            }
        }
        "session.update" => {
            if let Some(slot) = event
                .pointer_mut("/session/instructions")
                .filter(|slot| slot.is_string())
            {
                visit(slot, true);
            }
        }
        "response.create" => {
            if let Some(slot) = event
                .pointer_mut("/response/instructions")
                .filter(|slot| slot.is_string())
            {
                visit(slot, true);
            }
            if let Some(items) = event
                .pointer_mut("/response/input")
                .and_then(Value::as_array_mut)
            {
                for item in items {
                    item_texts(item, visit);
                }
            }
        }
        _ => {}
    }
}
