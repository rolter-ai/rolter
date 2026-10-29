//! Request- and response-path wiring for the built-in guardrails (ROL-261).
//!
//! Walks the text-bearing fields of an OpenAI- or Anthropic-shaped body and
//! applies the compiled rules for one stage from the snapshot: the request body
//! before proxying (`pre_call`), or a buffered response body before delivering
//! it (`post_call`, #591). Redactions are written back into the JSON in place; a
//! blocking match returns the offending rule name so the handler can reject with
//! an OpenAI-compatible error. The walk only ever passes bounded text slices to
//! the engine, and the returned report carries rule names and counts only —
//! never the matched values.

use rolter_core::guardrails::RuleSelection;
use rolter_core::{CompiledGuardrails, GuardStage, GuardrailReport, ScanOutcome};
use serde_json::Value;

/// One stage's walk over one body: the engine, the route's rule selection, and
/// the mutable scan state that has to be threaded through every nested field.
struct Scan<'a> {
    guardrails: &'a CompiledGuardrails,
    selection: &'a RuleSelection,
    stage: GuardStage,
    budget: usize,
    report: GuardrailReport,
}

impl<'a> Scan<'a> {
    fn new(
        guardrails: &'a CompiledGuardrails,
        selection: &'a RuleSelection,
        stage: GuardStage,
    ) -> Self {
        Self {
            guardrails,
            selection,
            stage,
            budget: guardrails.scan_budget(),
            report: GuardrailReport::default(),
        }
    }

    /// Scan a single JSON string in place, applying a redaction or surfacing a
    /// block.
    fn string(&mut self, value: &mut Value, is_system: bool) -> Result<(), String> {
        let Some(text) = value.as_str() else {
            return Ok(());
        };
        let outcome = match self.stage {
            GuardStage::PreCall => self.guardrails.scan_segment_with(
                text,
                is_system,
                &mut self.budget,
                &mut self.report,
                self.selection,
            ),
            GuardStage::PostCall => self.guardrails.scan_output(
                text,
                &mut self.budget,
                &mut self.report,
                self.selection,
            ),
        };
        match outcome {
            ScanOutcome::Unchanged => Ok(()),
            ScanOutcome::Redacted(redacted) => {
                *value = Value::String(redacted);
                Ok(())
            }
            ScanOutcome::Blocked(rule) => Err(rule),
        }
    }

    /// Scan a `content`/`prompt`/`system` value that may be a string, an array of
    /// strings, or an array of typed parts (`{ "type": "text", "text": "…" }`).
    fn content(&mut self, value: &mut Value, is_system: bool) -> Result<(), String> {
        match value {
            Value::String(_) => self.string(value, is_system),
            Value::Array(parts) => {
                for part in parts {
                    match part {
                        Value::String(_) => self.string(part, is_system)?,
                        Value::Object(_) => {
                            if let Some(text) = part.get_mut("text").filter(|t| t.is_string()) {
                                self.string(text, is_system)?;
                            }
                        }
                        _ => {}
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Scan every message in a `messages` array, treating `system`/`developer`
    /// roles as operator-authored system content.
    fn messages(&mut self, body: &mut Value) -> Result<(), String> {
        let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
            return Ok(());
        };
        for message in messages {
            let is_system = matches!(
                message.get("role").and_then(Value::as_str),
                Some("system") | Some("developer")
            );
            if let Some(content) = message.get_mut("content") {
                self.content(content, is_system)?;
            }
        }
        Ok(())
    }

    /// Scan a named top-level field when the body has it.
    fn field(&mut self, body: &mut Value, name: &str, is_system: bool) -> Result<(), String> {
        match body.get_mut(name) {
            Some(value) => self.content(value, is_system),
            None => Ok(()),
        }
    }

    /// Scan `choices[].message.content`, the legacy `choices[].text`, and the
    /// arguments of any tool call the model emitted.
    fn choices(&mut self, body: &mut Value) -> Result<(), String> {
        let Some(choices) = body.get_mut("choices").and_then(Value::as_array_mut) else {
            return Ok(());
        };
        for choice in choices {
            if let Some(content) = choice.pointer_mut("/message/content") {
                self.content(content, false)?;
            }
            if let Some(text) = choice.get_mut("text").filter(|t| t.is_string()) {
                self.string(text, false)?;
            }
            if let Some(calls) = choice
                .pointer_mut("/message/tool_calls")
                .and_then(Value::as_array_mut)
            {
                for call in calls {
                    if let Some(arguments) = call.pointer_mut("/function/arguments") {
                        self.encoded_arguments(arguments)?;
                    }
                }
            }
            // the pre-tool_calls spelling, still emitted by some providers
            if let Some(arguments) = choice.pointer_mut("/message/function_call/arguments") {
                self.encoded_arguments(arguments)?;
            }
        }
        Ok(())
    }

    /// Scan every string nested anywhere in a value, keys excluded.
    ///
    /// Used for tool-call payloads, whose shape is the tool's own schema rather
    /// than anything the API defines, so there is no field list to walk.
    fn nested(&mut self, value: &mut Value) -> Result<(), String> {
        match value {
            Value::String(_) => self.string(value, false),
            Value::Array(items) => {
                for item in items {
                    self.nested(item)?;
                }
                Ok(())
            }
            Value::Object(map) => {
                for (_, item) in map.iter_mut() {
                    self.nested(item)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Scan tool arguments that arrive as a JSON document inside a JSON string.
    ///
    /// Parsed and re-encoded rather than scanned as raw text, so a replacement
    /// token containing a quote or a backslash cannot turn valid arguments into
    /// a string the client fails to parse. Arguments that are not valid JSON —
    /// a model mid-stream can emit those — are scanned as plain text instead.
    fn encoded_arguments(&mut self, value: &mut Value) -> Result<(), String> {
        let Some(raw) = value.as_str() else {
            // some providers send arguments as an object rather than a string
            return self.nested(value);
        };
        let Ok(mut parsed) = serde_json::from_str::<Value>(raw) else {
            return self.string(value, false);
        };
        let before = self.report.redactions;
        self.nested(&mut parsed)?;
        if self.report.redactions > before {
            if let Ok(encoded) = serde_json::to_string(&parsed) {
                *value = Value::String(encoded);
            }
        }
        Ok(())
    }
}

/// A pre-call block: the rule that refused the request, and what the rules
/// that ran before it had done to the same request.
///
/// The report matters because an earlier rule may already have redacted the
/// content, and the request log should still say so (#3038).
#[derive(Debug)]
pub struct InputBlocked {
    pub rule: String,
    pub report: GuardrailReport,
}

/// Apply pre-call guardrails to a parsed request body for the given API path.
///
/// Returns `Err` when a rule blocks the request, carrying the rule name and the
/// report gathered up to that point. On `Ok`, the body may have been mutated in
/// place (redactions); `report.redactions > 0` signals the caller should
/// re-serialize before forwarding.
pub fn apply_input(
    guardrails: &CompiledGuardrails,
    selection: &RuleSelection,
    path: &str,
    body: &mut Value,
) -> Result<GuardrailReport, Box<InputBlocked>> {
    let mut scan = Scan::new(guardrails, selection, GuardStage::PreCall);

    let walked = (|| -> Result<(), String> {
        match path {
            // OpenAI chat + Responses share the `messages` shape; Responses also
            // accepts a bare `input` string/array
            "/v1/chat/completions" | "/v1/responses" => {
                scan.messages(body)?;
                scan.field(body, "input", false)?;
            }
            // legacy text completions carry a `prompt` string or array of strings
            "/v1/completions" => scan.field(body, "prompt", false)?,
            // Anthropic Messages: a top-level `system` plus user/assistant `messages`
            "/v1/messages" => {
                scan.field(body, "system", true)?;
                scan.messages(body)?;
            }
            // System One decisions: the `state` is the caller's content and may
            // be a string, an object or an array, and the questions' text
            // (instructions and criteria) can carry content of its own, so every
            // string in both is scanned. images and videos are not text and are
            // left alone (#3150)
            rolter_core::upstream::SYSTEM_ONE_PATH => {
                if let Some(state) = body.get_mut("state") {
                    scan.nested(state)?;
                }
                if let Some(questions) = body.get_mut("questions").and_then(Value::as_object_mut) {
                    for question in questions.values_mut() {
                        for field in ["instructions", "criteria"] {
                            if let Some(text) = question.get_mut(field) {
                                scan.nested(text)?;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    })();

    match walked {
        Ok(()) => Ok(scan.report),
        Err(rule) => Err(Box::new(InputBlocked {
            rule,
            report: scan.report,
        })),
    }
}

/// A post-call block: the rule that withheld the response, and what the rules
/// that ran before it had done to the same body, so the row records those too
/// (#3403).
#[derive(Debug)]
pub struct OutputBlocked {
    pub rule: String,
    pub report: GuardrailReport,
}

/// Apply post-call guardrails to a buffered, non-streaming response body.
///
/// Same contract as [`apply_input`], one stage later: `Err` means the
/// completion must not be delivered and carries the blocking rule with the
/// report gathered up to it, and a mutated `body` with
/// `report.redactions > 0` must be re-serialized before it goes to the client.
///
/// Assistant text and tool-call payloads are both walked. Tool arguments are a
/// place model output routinely lands — an address the model passes to a
/// `send_email` tool has left the completion just as surely as one it printed —
/// so they are parsed, masked and re-encoded rather than skipped.
pub fn apply_output(
    guardrails: &CompiledGuardrails,
    selection: &RuleSelection,
    path: &str,
    body: &mut Value,
) -> Result<GuardrailReport, Box<OutputBlocked>> {
    let mut scan = Scan::new(guardrails, selection, GuardStage::PostCall);
    match scan_output(&mut scan, path, body) {
        Ok(()) => Ok(scan.report),
        Err(rule) => Err(Box::new(OutputBlocked {
            rule,
            report: scan.report,
        })),
    }
}

/// Walk the fields of a response of `path` that [`apply_output`] scans.
fn scan_output(scan: &mut Scan<'_>, path: &str, body: &mut Value) -> Result<(), String> {
    match path {
        // OpenAI chat + legacy completions: one message/text per choice
        "/v1/chat/completions" | "/v1/completions" => scan.choices(body)?,
        // Anthropic Messages replies with a top-level `content` parts array,
        // where a `tool_use` part carries its arguments as an `input` object
        "/v1/messages" => {
            scan.field(body, "content", false)?;
            if let Some(parts) = body.get_mut("content").and_then(Value::as_array_mut) {
                for part in parts {
                    if part.get("type").and_then(Value::as_str) == Some("tool_use") {
                        if let Some(input) = part.get_mut("input") {
                            scan.nested(input)?;
                        }
                    }
                }
            }
        }
        // Responses: typed `output[].content[].text`, function-call arguments on
        // the output item itself, plus the flattened `output_text` convenience
        // field when the provider sends it
        "/v1/responses" => {
            if let Some(output) = body.get_mut("output").and_then(Value::as_array_mut) {
                for item in output {
                    if let Some(content) = item.get_mut("content") {
                        scan.content(content, false)?;
                    }
                    if let Some(arguments) = item.get_mut("arguments") {
                        scan.encoded_arguments(arguments)?;
                    }
                }
            }
            scan.field(body, "output_text", false)?;
        }
        // a transcription or translation: the whole `text` and the timed
        // `segments[].text` of `verbose_json`. a plain-text or subtitle answer
        // reaches here as `{"text": …}` (#3331)
        "/v1/audio/transcriptions" | "/v1/audio/translations" => {
            scan.field(body, "text", false)?;
            if let Some(segments) = body.get_mut("segments").and_then(Value::as_array_mut) {
                for segment in segments {
                    scan.field(segment, "text", false)?;
                }
            }
            // `words[].word` and `logprobs[].token` split the text one token
            // at a time, so a card or phone number spread over several of
            // them matches no rule there. once anything was redacted they are
            // dropped whole rather than delivered with the redacted value
            // still in pieces
            if scan.report.redactions > 0 {
                if let Some(object) = body.as_object_mut() {
                    object.remove("words");
                    object.remove("logprobs");
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// The output stage resolved on the request path, carried to whichever delivery
/// path ends up serving the response.
///
/// Constructing one is the decision that the response has to be buffered, so
/// [`resolve`](Self::resolve) returns `None` whenever no post-call rule applies —
/// a route without them keeps streaming its non-SSE JSON straight through.
pub struct OutputGuard<'a> {
    guardrails: &'a CompiledGuardrails,
    selection: &'a RuleSelection,
    metrics: &'a crate::metrics::Metrics,
    path: &'a str,
}

/// What the output stage decided about a buffered body.
pub enum OutputDecision {
    /// deliver the bytes as they are
    Unchanged,
    /// deliver these bytes instead
    Masked(bytes::Bytes),
    /// withhold the response; carries the rule name, never the matched text
    Blocked(String),
}

impl<'a> OutputGuard<'a> {
    pub fn resolve(
        guardrails: &'a CompiledGuardrails,
        selection: &'a RuleSelection,
        metrics: &'a crate::metrics::Metrics,
        path: &'a str,
    ) -> Option<Self> {
        guardrails.post_call_active_for(selection).then_some(Self {
            guardrails,
            selection,
            metrics,
            path,
        })
    }

    /// Scan a buffered response body.
    ///
    /// A body that is not JSON is delivered untouched: the stage masks fields of
    /// a known response shape, and there is nothing to walk in an opaque payload.
    /// Callers apply this to successful responses only, so this is not the path
    /// an upstream error body takes.
    ///
    /// The `redacted:<rule>` and `annotated:<rule>` outcomes are appended to
    /// `events` for the log row (#3330), on a withheld body too: there the
    /// rules that ran before the blocking one are kept (#3403), and the block
    /// itself is recorded through the withheld reason.
    pub fn apply(&self, bytes: &bytes::Bytes, events: &mut Vec<String>) -> OutputDecision {
        // response-side scanning is its own attributable stage: masking a large
        // body costs real time, and it lands after the upstream leg (#805)
        let _stage = crate::trace::stage_span!("guardrails.post");
        let Ok(mut parsed) = serde_json::from_slice::<Value>(bytes) else {
            return OutputDecision::Unchanged;
        };
        let report = apply_output(self.guardrails, self.selection, self.path, &mut parsed);
        match &report {
            Ok(report) => events.extend(report.events()),
            // the withheld reason records the block itself, so only the
            // earlier rules' outcomes come from the report
            Err(blocked) => events.extend(
                blocked
                    .report
                    .events()
                    .into_iter()
                    .filter(|event| !event.starts_with("blocked:")),
            ),
        }
        match report {
            Ok(report) if report.redactions > 0 => {
                self.metrics.guardrail_output_redactions_total.fetch_add(
                    report.redactions as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                match serde_json::to_vec(&parsed) {
                    Ok(masked) => OutputDecision::Masked(bytes::Bytes::from(masked)),
                    // re-serializing a value that just came out of serde_json
                    // cannot realistically fail, but delivering the unmasked
                    // original is not an acceptable fallback
                    Err(_) => OutputDecision::Blocked("guardrail_serialization".to_string()),
                }
            }
            Ok(_) => OutputDecision::Unchanged,
            Err(blocked) => {
                self.metrics
                    .guardrail_output_blocks_total
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                OutputDecision::Blocked(blocked.rule)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolter_core::{BuiltinRule, GuardAction, GuardrailRule, GuardrailsConfig};
    use serde_json::json;

    fn engine(rules: Vec<GuardrailRule>) -> CompiledGuardrails {
        CompiledGuardrails::from_config(&GuardrailsConfig {
            enabled: true,
            max_scan_bytes: None,
            streaming_post_call: Default::default(),
            rules,
        })
    }

    fn block(name: &str, builtin: BuiltinRule) -> GuardrailRule {
        GuardrailRule {
            action: GuardAction::Block,
            ..redact(name, builtin)
        }
    }

    fn selection(engine: &CompiledGuardrails, disable: &[&str], enable: &[&str]) -> RuleSelection {
        engine.resolve_selection(&rolter_core::guardrails::RouteGuardrails {
            disable: disable.iter().map(|s| s.to_string()).collect(),
            enable: enable.iter().map(|s| s.to_string()).collect(),
        })
    }

    #[test]
    fn a_decision_state_and_its_questions_are_scanned() {
        let g = engine(vec![redact("email", BuiltinRule::Email)]);
        let mut body = json!({
            "model": "jev-latest",
            "state": {"ticket": {"from": "a@b.com", "text": "refund please"}},
            "questions": {
                "route": {
                    "type": "choice",
                    "instructions": "Should c@d.com handle this?",
                    "criteria": {"billing": "write to e@f.com", "bug": "software"}
                }
            }
        });
        let report =
            apply_input(&g, &RuleSelection::default(), "/v1/systemone", &mut body).unwrap();
        let text = body.to_string();
        for address in ["a@b.com", "c@d.com", "e@f.com"] {
            assert!(!text.contains(address), "{address} survived: {text}");
        }
        assert_eq!(report.redactions, 3);
        // the shape the upstream reads is untouched: ids, keys and types stay
        assert_eq!(body["questions"]["route"]["type"], "choice");
        assert!(body["questions"]["route"]["criteria"]["billing"].is_string());
        assert_eq!(body["state"]["ticket"]["text"], "refund please");
    }

    #[test]
    fn a_decision_state_can_be_blocked() {
        let g = engine(vec![block("email", BuiltinRule::Email)]);
        let mut body = json!({"model": "m", "state": "mail a@b.com", "questions": {}});
        let blocked = apply_input(&g, &RuleSelection::default(), "/v1/systemone", &mut body);
        assert_eq!(blocked.map_err(|b| b.rule).unwrap_err(), "email");
    }

    #[test]
    fn a_route_override_can_disable_a_redaction_rule() {
        let g = engine(vec![redact("email", BuiltinRule::Email)]);
        let body = || json!({"messages": [{"role": "user", "content": "mail me at a@b.com"}]});

        // global behaviour: redacted
        let mut on = body();
        apply_input(
            &g,
            &RuleSelection::default(),
            "/v1/chat/completions",
            &mut on,
        )
        .unwrap();
        assert!(!on.to_string().contains("a@b.com"), "{on}");

        // the route opts out: the same body passes through untouched
        let mut off = body();
        let report = apply_input(
            &g,
            &selection(&g, &["email"], &[]),
            "/v1/chat/completions",
            &mut off,
        )
        .unwrap();
        assert!(off.to_string().contains("a@b.com"), "{off}");
        assert_eq!(report.redactions, 0);
    }

    #[test]
    fn a_route_override_can_disable_a_blocking_rule() {
        let g = engine(vec![block("card", BuiltinRule::PaymentCard)]);
        let body = || json!({"messages": [{"role": "user", "content": "4111 1111 1111 1111"}]});

        let mut blocked = body();
        assert_eq!(
            apply_input(
                &g,
                &RuleSelection::default(),
                "/v1/chat/completions",
                &mut blocked
            )
            .unwrap_err()
            .rule,
            "card"
        );

        let mut allowed = body();
        assert!(apply_input(
            &g,
            &selection(&g, &["card"], &[]),
            "/v1/chat/completions",
            &mut allowed
        )
        .is_ok());
    }

    #[test]
    fn enable_wins_over_disable_for_the_same_rule() {
        // naming a rule in both is an explicit opt-in, not ambiguous
        let g = engine(vec![redact("email", BuiltinRule::Email)]);
        let mut body = json!({"messages": [{"role": "user", "content": "a@b.com"}]});
        apply_input(
            &g,
            &selection(&g, &["email"], &["email"]),
            "/v1/chat/completions",
            &mut body,
        )
        .unwrap();
        assert!(!body.to_string().contains("a@b.com"), "{body}");
    }

    #[test]
    fn disabling_one_rule_leaves_the_others_running() {
        let g = engine(vec![
            redact("email", BuiltinRule::Email),
            block("card", BuiltinRule::PaymentCard),
        ]);
        let mut body =
            json!({"messages": [{"role": "user", "content": "a@b.com and 4111 1111 1111 1111"}]});
        assert_eq!(
            apply_input(
                &g,
                &selection(&g, &["email"], &[]),
                "/v1/chat/completions",
                &mut body
            )
            .unwrap_err()
            .rule,
            "card",
            "disabling email must not disable card"
        );
    }

    #[test]
    fn a_route_disabling_every_rule_skips_the_scan_entirely() {
        // not just "matches nothing" — the handler must be able to skip the
        // JSON walk, so the engine has to report the route as inactive
        let g = engine(vec![redact("email", BuiltinRule::Email)]);
        assert!(g.pre_call_active_for(&RuleSelection::default()));
        assert!(!g.pre_call_active_for(&selection(&g, &["email"], &[])));
    }

    fn redact(name: &str, builtin: BuiltinRule) -> GuardrailRule {
        GuardrailRule {
            name: name.to_string(),
            builtin: Some(builtin),
            pattern: None,
            stage: GuardStage::PreCall,
            action: GuardAction::Redact,
            replacement: None,
            include_system: false,
        }
    }

    /// The same rule, moved to the output stage.
    fn out(mut rule: GuardrailRule) -> GuardrailRule {
        rule.stage = GuardStage::PostCall;
        rule
    }

    #[test]
    fn openai_chat_string_content_redacted() {
        let g = engine(vec![redact("email", BuiltinRule::Email)]);
        let mut body = json!({
            "model": "gpt-4",
            "messages": [{"role": "user", "content": "reach me at joe@acme.io"}]
        });
        let report = apply_input(
            &g,
            &RuleSelection::default(),
            "/v1/chat/completions",
            &mut body,
        )
        .unwrap();
        assert_eq!(report.redactions, 1);
        assert_eq!(
            body["messages"][0]["content"],
            json!("reach me at [REDACTED:EMAIL]")
        );
    }

    #[test]
    fn openai_chat_parts_array_redacted() {
        let g = engine(vec![redact("email", BuiltinRule::Email)]);
        let mut body = json!({
            "model": "gpt-4",
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "ping a@b.com"},
                {"type": "image_url", "image_url": {"url": "http://x"}}
            ]}]
        });
        apply_input(
            &g,
            &RuleSelection::default(),
            "/v1/chat/completions",
            &mut body,
        )
        .unwrap();
        assert_eq!(
            body["messages"][0]["content"][0]["text"],
            json!("ping [REDACTED:EMAIL]")
        );
    }

    #[test]
    fn system_message_excluded_by_default() {
        let g = engine(vec![redact("email", BuiltinRule::Email)]);
        let mut body = json!({
            "model": "gpt-4",
            "messages": [
                {"role": "system", "content": "admin ops@corp.com"},
                {"role": "user", "content": "hi user@corp.com"}
            ]
        });
        apply_input(
            &g,
            &RuleSelection::default(),
            "/v1/chat/completions",
            &mut body,
        )
        .unwrap();
        assert_eq!(body["messages"][0]["content"], json!("admin ops@corp.com"));
        assert_eq!(body["messages"][1]["content"], json!("hi [REDACTED:EMAIL]"));
    }

    #[test]
    fn block_returns_rule_name() {
        let mut rule = redact("card", BuiltinRule::PaymentCard);
        rule.action = GuardAction::Block;
        let g = engine(vec![rule]);
        let mut body = json!({
            "model": "gpt-4",
            "messages": [{"role": "user", "content": "card 4111 1111 1111 1111"}]
        });
        let err = apply_input(
            &g,
            &RuleSelection::default(),
            "/v1/chat/completions",
            &mut body,
        )
        .unwrap_err();
        assert_eq!(err.rule, "card");
    }

    #[test]
    fn anthropic_system_and_messages() {
        let mut rule = redact("email", BuiltinRule::Email);
        rule.include_system = true;
        let g = engine(vec![rule]);
        let mut body = json!({
            "model": "claude-3",
            "system": "route to ops@corp.com",
            "messages": [{"role": "user", "content": "mail me me@x.com"}]
        });
        apply_input(&g, &RuleSelection::default(), "/v1/messages", &mut body).unwrap();
        assert_eq!(body["system"], json!("route to [REDACTED:EMAIL]"));
        assert_eq!(
            body["messages"][0]["content"],
            json!("mail me [REDACTED:EMAIL]")
        );
    }

    #[test]
    fn completions_prompt_redacted() {
        let g = engine(vec![redact("email", BuiltinRule::Email)]);
        let mut body = json!({"model": "gpt-4", "prompt": "write to z@z.io"});
        apply_input(&g, &RuleSelection::default(), "/v1/completions", &mut body).unwrap();
        assert_eq!(body["prompt"], json!("write to [REDACTED:EMAIL]"));
    }

    #[test]
    fn openai_chat_completion_content_masked() {
        let g = engine(vec![out(redact("email", BuiltinRule::Email))]);
        let mut body = json!({
            "id": "chatcmpl-1",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "write to ops@corp.com"},
                "finish_reason": "stop"
            }]
        });
        let report = apply_output(
            &g,
            &RuleSelection::default(),
            "/v1/chat/completions",
            &mut body,
        )
        .unwrap();
        assert_eq!(report.redactions, 1);
        assert_eq!(
            body["choices"][0]["message"]["content"],
            json!("write to [REDACTED:EMAIL]")
        );
    }

    #[test]
    fn a_transcript_is_masked_in_its_text_and_segments_and_loses_its_words() {
        let g = engine(vec![out(redact("email", BuiltinRule::Email))]);
        for path in ["/v1/audio/transcriptions", "/v1/audio/translations"] {
            let mut body = json!({
                "task": "transcribe",
                "duration": 3.5,
                "text": "write to ops@corp.com",
                "segments": [{"id": 0, "start": 0.0, "end": 3.5, "text": "write to ops@corp.com"}],
                "words": [{"word": "ops@corp.com", "start": 2.0, "end": 3.5}]
            });
            let report = apply_output(&g, &RuleSelection::default(), path, &mut body).unwrap();
            assert_eq!(report.redactions, 2, "{path}");
            let text = body.to_string();
            assert!(!text.contains("ops@corp.com"), "{path}: {text}");
            assert!(body.get("words").is_none(), "{path}: {text}");
            // what billing reads is left alone
            assert_eq!(body["duration"], json!(3.5));
        }
    }

    /// A card number spoken as several words is one token per digit group in
    /// `words[]` and `logprobs[]`, where no rule can see it whole, so both go
    /// once the text was redacted (#3331).
    #[test]
    fn a_multi_token_card_number_does_not_survive_in_words_or_logprobs() {
        let g = engine(vec![out(redact("card", BuiltinRule::PaymentCard))]);
        let groups = ["4111", "1111", "1111", "1111"];
        let mut body = json!({
            "text": format!("my card is {}", groups.join(" ")),
            "words": groups.iter().map(|g| json!({"word": g})).collect::<Vec<_>>(),
            "logprobs": groups.iter().map(|g| json!({"token": g, "logprob": -0.1})).collect::<Vec<_>>()
        });
        let report = apply_output(
            &g,
            &RuleSelection::default(),
            "/v1/audio/transcriptions",
            &mut body,
        )
        .unwrap();
        assert_eq!(report.redactions, 1);
        let text = body.to_string();
        assert!(!text.contains("4111"), "{text}");
        assert!(body.get("words").is_none() && body.get("logprobs").is_none());
    }

    /// Nothing redacted, nothing dropped: the timings stay for the caller.
    #[test]
    fn a_clean_transcript_keeps_its_words() {
        let g = engine(vec![out(redact("email", BuiltinRule::Email))]);
        let mut body = json!({"text": "hello", "words": [{"word": "hello"}]});
        apply_output(
            &g,
            &RuleSelection::default(),
            "/v1/audio/transcriptions",
            &mut body,
        )
        .unwrap();
        assert_eq!(body["words"], json!([{"word": "hello"}]));
    }

    #[test]
    fn a_blocking_output_rule_withholds_a_transcript() {
        let g = engine(vec![out(block("card", BuiltinRule::PaymentCard))]);
        let mut body = json!({"text": "my card is 4111 1111 1111 1111"});
        assert_eq!(
            apply_output(
                &g,
                &RuleSelection::default(),
                "/v1/audio/transcriptions",
                &mut body
            )
            .map_err(|blocked| blocked.rule),
            Err("card".to_string())
        );
    }

    /// A block keeps what the rules before it did, as the request stage does
    /// (#3403).
    #[test]
    fn a_blocking_output_rule_keeps_the_earlier_rules_report() {
        let g = engine(vec![
            out(redact("email", BuiltinRule::Email)),
            out(block("card", BuiltinRule::PaymentCard)),
        ]);
        let mut body = json!({
            "choices": [{"message": {
                "role": "assistant",
                "content": "mail a@b.com, card 4111 1111 1111 1111"
            }}]
        });
        let blocked = apply_output(
            &g,
            &RuleSelection::default(),
            "/v1/chat/completions",
            &mut body,
        )
        .unwrap_err();
        assert_eq!(blocked.rule, "card");
        assert_eq!(blocked.report.redacted_by, vec!["email".to_string()]);
    }

    #[test]
    fn a_blocking_output_rule_names_the_rule() {
        let g = engine(vec![out(block("card", BuiltinRule::PaymentCard))]);
        let mut body = json!({
            "choices": [{"message": {"role": "assistant", "content": "4111 1111 1111 1111"}}]
        });
        assert_eq!(
            apply_output(
                &g,
                &RuleSelection::default(),
                "/v1/chat/completions",
                &mut body
            )
            .map_err(|blocked| blocked.rule),
            Err("card".to_string())
        );
    }

    #[test]
    fn anthropic_completion_parts_masked() {
        let g = engine(vec![out(redact("email", BuiltinRule::Email))]);
        let mut body = json!({
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "mail ops@corp.com"},
                {"type": "tool_use", "name": "lookup", "input": {"q": "who@corp.com"}}
            ]
        });
        apply_output(&g, &RuleSelection::default(), "/v1/messages", &mut body).unwrap();
        assert_eq!(body["content"][0]["text"], json!("mail [REDACTED:EMAIL]"));
        // a `tool_use` payload is model output too: an address handed to a tool
        // has left the completion just as surely as one the model printed
        assert_eq!(body["content"][1]["input"]["q"], json!("[REDACTED:EMAIL]"));
    }

    #[test]
    fn openai_tool_call_arguments_are_masked_and_stay_parseable() {
        let g = engine(vec![out(redact("email", BuiltinRule::Email))]);
        let mut body = json!({
            "choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "function": {
                    "name": "send_email",
                    "arguments": "{\"to\":\"ops@corp.com\",\"retries\":2}"
                }
            }]}}]
        });
        apply_output(
            &g,
            &RuleSelection::default(),
            "/v1/chat/completions",
            &mut body,
        )
        .unwrap();

        // arguments stay a JSON string the client can parse, with non-string
        // values untouched
        let arguments = body["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .expect("arguments must remain a string");
        let parsed: Value = serde_json::from_str(arguments).expect("must still parse as JSON");
        assert_eq!(parsed["to"], json!("[REDACTED:EMAIL]"));
        assert_eq!(parsed["retries"], json!(2));
    }

    #[test]
    fn a_blocking_rule_fires_on_tool_arguments() {
        let g = engine(vec![out(block("card", BuiltinRule::PaymentCard))]);
        let mut body = json!({
            "choices": [{"message": {"tool_calls": [{
                "function": {"name": "pay", "arguments": "{\"card\":\"4111 1111 1111 1111\"}"}
            }]}}]
        });
        assert_eq!(
            apply_output(
                &g,
                &RuleSelection::default(),
                "/v1/chat/completions",
                &mut body
            )
            .map_err(|blocked| blocked.rule),
            Err("card".to_string())
        );
    }

    #[test]
    fn unparsable_tool_arguments_are_scanned_as_text() {
        // a truncated or malformed argument blob still gets masked; it just does
        // not get re-encoded as JSON, because it never was JSON
        let g = engine(vec![out(redact("email", BuiltinRule::Email))]);
        let mut body = json!({
            "choices": [{"message": {"tool_calls": [{
                "function": {"name": "send", "arguments": "{\"to\": \"ops@corp.com\""}
            }]}}]
        });
        apply_output(
            &g,
            &RuleSelection::default(),
            "/v1/chat/completions",
            &mut body,
        )
        .unwrap();
        let arguments = body["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap();
        assert!(!arguments.contains("ops@corp.com"), "{arguments}");
    }

    #[test]
    fn responses_function_call_arguments_are_masked() {
        let g = engine(vec![out(redact("email", BuiltinRule::Email))]);
        let mut body = json!({
            "output": [{
                "type": "function_call",
                "name": "send_email",
                "arguments": "{\"to\":\"ops@corp.com\"}"
            }]
        });
        apply_output(&g, &RuleSelection::default(), "/v1/responses", &mut body).unwrap();
        let arguments = body["output"][0]["arguments"].as_str().unwrap();
        assert!(!arguments.contains("ops@corp.com"), "{arguments}");
    }

    #[test]
    fn responses_output_and_output_text_masked() {
        let g = engine(vec![out(redact("email", BuiltinRule::Email))]);
        let mut body = json!({
            "output": [{"type": "message", "content": [{"type": "output_text", "text": "a@b.com"}]}],
            "output_text": "a@b.com"
        });
        apply_output(&g, &RuleSelection::default(), "/v1/responses", &mut body).unwrap();
        assert_eq!(
            body["output"][0]["content"][0]["text"],
            json!("[REDACTED:EMAIL]")
        );
        assert_eq!(body["output_text"], json!("[REDACTED:EMAIL]"));
    }

    #[test]
    fn the_stages_do_not_bleed_into_each_other() {
        // an input rule must not fire on a response body, and vice versa: the
        // stage is the whole point of having two of them
        let g = engine(vec![redact("email", BuiltinRule::Email)]);
        let mut response =
            json!({"choices": [{"message": {"role": "assistant", "content": "a@b.com"}}]});
        apply_output(
            &g,
            &RuleSelection::default(),
            "/v1/chat/completions",
            &mut response,
        )
        .unwrap();
        assert_eq!(
            response["choices"][0]["message"]["content"],
            json!("a@b.com")
        );
        assert!(!g.post_call_active_for(&RuleSelection::default()));

        let out_only = engine(vec![out(redact("email", BuiltinRule::Email))]);
        let mut request = json!({"messages": [{"role": "user", "content": "a@b.com"}]});
        apply_input(
            &out_only,
            &RuleSelection::default(),
            "/v1/chat/completions",
            &mut request,
        )
        .unwrap();
        assert_eq!(request["messages"][0]["content"], json!("a@b.com"));
        assert!(!out_only.pre_call_active_for(&RuleSelection::default()));
    }

    #[test]
    fn a_route_override_reaches_the_output_stage_too() {
        let g = engine(vec![out(redact("email", BuiltinRule::Email))]);
        let mut body = json!({"choices": [{"message": {"content": "a@b.com"}}]});
        apply_output(
            &g,
            &selection(&g, &["email"], &[]),
            "/v1/chat/completions",
            &mut body,
        )
        .unwrap();
        assert_eq!(body["choices"][0]["message"]["content"], json!("a@b.com"));
        assert!(!g.post_call_active_for(&selection(&g, &["email"], &[])));
    }
}
