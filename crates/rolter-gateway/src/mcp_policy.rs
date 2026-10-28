//! Tool-level access policy for the MCP proxy (#1403).
//!
//! Virtual-key owner, server, OAuth session and scopes decide whether a caller
//! may reach a server at all. This decides which *tools* of that server a call
//! may name: the tool groups an organization enabled are an access boundary,
//! not a description of one.
//!
//! The rule, deny by default where a manifest says so:
//!
//! - a server that at least one enabled tool group names is *governed*, and a
//!   `tools/call` for it is forwarded only if the tool is in the union of the
//!   groups naming that server. Anything else is refused with `403`.
//! - a server no group names is not narrowed by groups.
//! - a server that declares a manifest (discovered or typed) serves only the
//!   tools in it, unless the organization's MCP settings allow unlisted tools
//!   (#1404). A server declaring none has nothing to check against.
//! - a body that cannot be read as JSON-RPC cannot be checked; the
//!   organization's failure policy decides, and the default refuses.
//! - a body that repeats a key in a message or in its `params` is refused
//!   outright (#3298). `serde_json` keeps the *last* of a repeated key, an
//!   upstream parser may keep the first, and the tool the policy judged would
//!   then not be the tool that runs. Unlike an unreadable body, no failure policy
//!   lets it through: nothing legitimate repeats a key.
//!
//! The groups an organization enables apply to every caller in it, unless the
//! caller has an assignment (#3295). A group can be assigned to a virtual key, a
//! project or a team; a caller whose key, project or team has one is held to
//! the groups assigned to those three (their union) instead of to the
//! organization's whole enabled set, so an assignment only ever narrows. A
//! caller with none keeps the organization-wide set, which is how a deployment
//! that never assigns a group behaves exactly as before. On a governed server
//! the assigned caller may call only what its own groups list there, which is
//! nothing when none of them names the server.
//!
//! A `tools/list` reply is narrowed by the same rule ([`list_filter`]), so a
//! client is not shown a tool it would be refused (#3297).
//!
//! The request body is parsed in full whenever the rule applies, with no
//! substring shortcut: JSON lets `tools/call` be written with an escaped slash
//! or a `\u` escape, so a byte search is a bypass here even though it is a fine
//! optimisation for the log.

// spec-ref: json-rpc, mcp-transport
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;

use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use rolter_core::McpServerConfig;

use crate::mcp_list_filter::ListFilter;
use crate::state::Snapshot;

/// Why a `tools/call` was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Denial {
    /// the server is governed by tool groups and none of them lists the tool
    NotInGroup,
    /// the server declares a manifest, the tool is not in it, and the
    /// organization does not allow unlisted tools
    Undeclared,
}

/// A `tools/call` the policy refused, kept whole so the refusal can be logged
/// like any other call.
#[derive(Debug)]
pub(crate) struct Refused {
    /// the tool the caller named; `None` when the call named none, which no
    /// group can allow
    pub tool: Option<String>,
    pub denial: Denial,
    pub request: Map<String, Value>,
}

impl Refused {
    /// What the caller is told. It names the tool the caller sent and the
    /// server it sent it to, and nothing about the groups behind the refusal.
    pub(crate) fn message(&self, server: &str) -> String {
        match (self.denial, self.tool.as_deref()) {
            (Denial::NotInGroup, Some(tool)) => format!(
                "tool '{tool}' is not allowed on MCP server '{server}' by the organization's \
                 tool groups"
            ),
            (Denial::NotInGroup, None) => format!(
                "the request names no tool that the organization's tool groups allow on MCP \
                 server '{server}'"
            ),
            (Denial::Undeclared, tool) => format!(
                "tool '{}' is not declared by MCP server '{server}'; declare it on the server or \
                 allow unlisted tools in the organization's MCP settings",
                tool.unwrap_or_default()
            ),
        }
    }
}

/// What the policy decided about one request body.
#[derive(Debug)]
pub(crate) enum Verdict {
    Allow,
    /// at least one call in the body is refused; the body is not forwarded
    Denied(Vec<Refused>),
    /// the policy applies but the body is not a JSON-RPC message it can read
    Unreadable,
    /// a message in the body, or its `params`, repeats a key, so the tool it
    /// names depends on which copy a parser keeps (#3298)
    Ambiguous,
}

impl Verdict {
    /// What the caller is told when the body repeats a key.
    pub(crate) fn ambiguous_message(server: &str) -> String {
        format!(
            "the request to MCP server '{server}' repeats a JSON key, so the tool it names is \
             ambiguous and it is refused"
        )
    }
}

/// Who is calling, as far as group assignments are concerned.
pub(crate) struct Caller<'a> {
    pub org_id: &'a str,
    pub key_id: &'a str,
    pub project_id: &'a str,
    pub team_id: &'a str,
}

impl Caller<'_> {
    /// The enabled groups assigned to the caller's key, project or team, or
    /// `None` when it has no assignment and so keeps the organization-wide set.
    fn assigned_groups<'s>(&self, snapshot: &'s Snapshot) -> Option<Vec<&'s String>> {
        let mut groups = Vec::new();
        for (kind, id) in [
            ("virtual_key", self.key_id),
            ("project", self.project_id),
            ("team", self.team_id),
        ] {
            if id.is_empty() {
                continue;
            }
            let key = (self.org_id.to_string(), kind.to_string(), id.to_string());
            groups.extend(
                snapshot
                    .mcp_group_assignments
                    .get(&key)
                    .into_iter()
                    .flatten(),
            );
        }
        (!groups.is_empty()).then_some(groups)
    }
}

/// What a call to one server is judged against.
struct Rules<'a> {
    /// the tools the caller's tool groups allow, when any group names the
    /// server: the organization's set, or the union of the caller's assigned
    /// groups when it has an assignment
    governed: Option<Cow<'a, HashSet<String>>>,
    /// the server's declared manifest, when calls must stay inside it: it
    /// declares one and the organization either saved settings that do not allow
    /// unlisted tools or never saved any and was created enforcing (#3334). An
    /// organization that existed before manifests were enforced and never saved
    /// its MCP settings keeps that behaviour, so an upgrade does not start
    /// refusing tools a server's manifest happens to leave out
    declared: Option<&'a [String]>,
}

impl<'a> Rules<'a> {
    fn of(snapshot: &'a Snapshot, server: &'a McpServerConfig, caller: &Caller<'_>) -> Self {
        // saved settings decide for themselves; an organization with none
        // follows the marker it was created with (#3334)
        let allow_unlisted = snapshot
            .mcp_org_settings
            .get(&server.org_id)
            .map_or(!server.enforce_manifest_by_default, |settings| {
                settings.allow_unlisted_tools
            });
        Self {
            governed: snapshot.mcp_governed_tools.get(&server.id).map(|org_wide| {
                match caller.assigned_groups(snapshot) {
                    None => Cow::Borrowed(org_wide),
                    Some(groups) => Cow::Owned(
                        groups
                            .into_iter()
                            .filter_map(|group| {
                                snapshot.mcp_group_tools.get(group)?.get(&server.id)
                            })
                            .flatten()
                            .cloned()
                            .collect(),
                    ),
                }
            }),
            declared: (!allow_unlisted && !server.tools.is_empty())
                .then_some(server.tools.as_slice()),
        }
    }

    /// Whether any rule applies. When none does, the body is forwarded without
    /// being parsed.
    fn any(&self) -> bool {
        self.governed.is_some() || self.declared.is_some()
    }

    /// Judge one tool name.
    fn judge(&self, tool: Option<&str>) -> Option<Denial> {
        judge(self.governed.as_deref(), self.declared, tool)
    }
}

/// Judge one tool name against a server's groups and manifest. A `tools/call`
/// and the entries of a `tools/list` reply are judged by this one rule, so what
/// a client is shown is what it may call.
pub(crate) fn judge(
    governed: Option<&HashSet<String>>,
    declared: Option<&[String]>,
    tool: Option<&str>,
) -> Option<Denial> {
    if let Some(allowed) = governed {
        if !tool.is_some_and(|tool| allowed.contains(tool)) {
            return Some(Denial::NotInGroup);
        }
    }
    if let Some(declared) = declared {
        if !tool.is_some_and(|tool| declared.iter().any(|name| name == tool)) {
            return Some(Denial::Undeclared);
        }
    }
    None
}

/// The filter a `tools/list` reply from `server` must pass through, or `None`
/// when the request holds no `tools/list` or no rule applies to the server and
/// the reply can be forwarded untouched (#3297).
pub(crate) fn list_filter(
    snapshot: &Snapshot,
    server: &McpServerConfig,
    caller: &Caller<'_>,
    body: &[u8],
) -> Option<ListFilter> {
    let rules = Rules::of(snapshot, server, caller);
    if !rules.any() || body.is_empty() {
        return None;
    }
    ListFilter::new(
        rules.governed.map(Cow::into_owned),
        rules.declared.map(<[String]>::to_vec),
        body,
    )
}

/// The `tools/call` requests in a JSON-RPC body, which is one message or a
/// batch of them. `None` when the body is not JSON-RPC at all.
fn tool_calls(body: &[u8]) -> Option<Vec<Map<String, Value>>> {
    let messages = match serde_json::from_slice::<Value>(body).ok()? {
        Value::Array(items) => items,
        other => vec![other],
    };
    let mut calls = Vec::new();
    for message in messages {
        // a batch may hold anything; only an object can be a call, and an
        // entry that is not one is the upstream's to reject
        let Value::Object(request) = message else {
            continue;
        };
        if request.get("method").and_then(Value::as_str) == Some("tools/call") {
            calls.push(request);
        }
    }
    Some(calls)
}

/// Where in a JSON-RPC body a repeated key is looked for.
#[derive(Clone, Copy)]
enum Level {
    /// the body: a message, or a batch of them
    Body,
    /// one message
    Message,
    /// the `params` of a message
    Params,
}

/// Walks a body and reports whether a message, or the `params` of one, repeats
/// a key. Keys deeper than `params` (tool `arguments`) are the tool's own data
/// and are not the policy's to judge.
struct RepeatedKey(Level);

impl<'de> DeserializeSeed<'de> for RepeatedKey {
    type Value = bool;

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<bool, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for RepeatedKey {
    type Value = bool;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<bool, A::Error> {
        let mut seen = HashSet::new();
        let mut repeated = false;
        while let Some(key) = map.next_key::<String>()? {
            let child = match (self.0, key.as_str()) {
                (Level::Body | Level::Message, "params") => Some(Level::Params),
                _ => None,
            };
            repeated |= !seen.insert(key);
            match child {
                Some(level) => repeated |= map.next_value_seed(RepeatedKey(level))?,
                None => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(repeated)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<bool, A::Error> {
        let level = match self.0 {
            Level::Body => Level::Message,
            // an array anywhere else is data
            _ => {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                return Ok(false);
            }
        };
        let mut repeated = false;
        while let Some(found) = seq.next_element_seed(RepeatedKey(level))? {
            repeated |= found;
        }
        Ok(repeated)
    }

    fn visit_bool<E>(self, _: bool) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_i64<E>(self, _: i64) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_u64<E>(self, _: u64) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_f64<E>(self, _: f64) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_str<E>(self, _: &str) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_unit<E>(self) -> Result<bool, E> {
        Ok(false)
    }
}

/// Whether a message of `body`, or the `params` of one, repeats a key. A body
/// that does not parse has none to report: the caller has already refused it.
fn repeats_a_key(body: &[u8]) -> bool {
    let mut parser = serde_json::Deserializer::from_slice(body);
    RepeatedKey(Level::Body)
        .deserialize(&mut parser)
        .unwrap_or(false)
}

/// Apply the policy to a request body bound for `server`.
pub(crate) fn enforce(
    snapshot: &Snapshot,
    server: &McpServerConfig,
    caller: &Caller<'_>,
    body: &[u8],
) -> Verdict {
    let rules = Rules::of(snapshot, server, caller);
    if !rules.any() || body.is_empty() {
        return Verdict::Allow;
    }
    let Some(calls) = tool_calls(body) else {
        // what to do with a request that cannot be inspected is the
        // organization's failure policy; refusing is the default
        let fail_open = snapshot
            .mcp_org_settings
            .get(&server.org_id)
            .is_some_and(|settings| settings.fail_open);
        return if fail_open {
            Verdict::Allow
        } else {
            Verdict::Unreadable
        };
    };
    if repeats_a_key(body) {
        return Verdict::Ambiguous;
    }
    let refused: Vec<Refused> = calls
        .into_iter()
        .filter_map(|request| {
            let tool = request
                .get("params")
                .and_then(|params| params.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let denial = rules.judge(tool.as_deref())?;
            Some(Refused {
                tool,
                denial,
                request,
            })
        })
        .collect();
    if refused.is_empty() {
        Verdict::Allow
    } else {
        Verdict::Denied(refused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolter_core::{
        GatewayConfig, McpOrgSettingsConfig, McpToolGroupConfig, McpToolGroupSubjectConfig,
        McpToolRefConfig,
    };

    fn caller() -> Caller<'static> {
        Caller {
            org_id: "org",
            key_id: "key-1",
            project_id: "project-1",
            team_id: "team-1",
        }
    }

    /// the policy for the default caller, which no group is assigned to
    fn enforce(snapshot: &Snapshot, server: &McpServerConfig, body: &[u8]) -> Verdict {
        super::enforce(snapshot, server, &caller(), body)
    }

    fn snapshot(groups: Vec<McpToolGroupConfig>) -> (Snapshot, McpServerConfig) {
        let server = McpServerConfig {
            id: "srv".into(),
            org_id: "org".into(),
            slug: "github".into(),
            ..Default::default()
        };
        let mut config = GatewayConfig::default();
        config.mcp_servers.push(server.clone());
        config.mcp_tool_groups = groups;
        (
            Snapshot::build(&config, &crate::load::LoadTracker::default()),
            server,
        )
    }

    fn group(org: &str, server: &str, tools: &[&str]) -> McpToolGroupConfig {
        McpToolGroupConfig {
            id: format!("g-{}", tools.join("-")),
            org_id: org.into(),
            slug: "g".into(),
            tools: tools
                .iter()
                .map(|tool| McpToolRefConfig {
                    server_id: server.into(),
                    tool: (*tool).into(),
                })
                .collect(),
            ..Default::default()
        }
    }

    /// like [`snapshot`], with a second server of the organization for groups
    /// that name something else
    fn world(groups: Vec<McpToolGroupConfig>) -> (Snapshot, McpServerConfig) {
        let server = McpServerConfig {
            id: "srv".into(),
            org_id: "org".into(),
            slug: "github".into(),
            ..Default::default()
        };
        let elsewhere = McpServerConfig {
            id: "elsewhere".into(),
            org_id: "org".into(),
            slug: "other".into(),
            ..Default::default()
        };
        let mut config = GatewayConfig::default();
        config.mcp_servers.extend([server.clone(), elsewhere]);
        config.mcp_tool_groups = groups;
        (
            Snapshot::build(&config, &crate::load::LoadTracker::default()),
            server,
        )
    }

    fn assigned(mut group: McpToolGroupConfig, kind: &str, id: &str) -> McpToolGroupConfig {
        group.assignments.push(McpToolGroupSubjectConfig {
            kind: kind.into(),
            id: id.into(),
        });
        group
    }

    fn call(name: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{name}","arguments":{{}}}}}}"#
        )
    }

    fn denied_tools(verdict: Verdict) -> Vec<Option<String>> {
        match verdict {
            Verdict::Denied(refused) => refused.into_iter().map(|r| r.tool).collect(),
            other => panic!("expected a denial, got {other:?}"),
        }
    }

    #[test]
    fn a_server_no_group_names_is_not_governed() {
        let (snapshot, server) = snapshot(vec![]);
        assert!(matches!(
            enforce(&snapshot, &server, call("anything").as_bytes()),
            Verdict::Allow
        ));
        // not even parsed: garbage passes through to the upstream
        assert!(matches!(
            enforce(&snapshot, &server, b"not json"),
            Verdict::Allow
        ));
    }

    #[test]
    fn a_governed_server_serves_only_the_tools_its_groups_list() {
        let (snapshot, server) = snapshot(vec![
            group("org", "srv", &["get_issue"]),
            group("org", "srv", &["list_issues"]),
        ]);
        for allowed in ["get_issue", "list_issues"] {
            assert!(matches!(
                enforce(&snapshot, &server, call(allowed).as_bytes()),
                Verdict::Allow
            ));
        }
        assert_eq!(
            denied_tools(enforce(&snapshot, &server, call("delete_repo").as_bytes())),
            [Some("delete_repo".to_string())]
        );
    }

    #[test]
    fn requests_that_are_not_tool_calls_pass_a_governed_server() {
        let (snapshot, server) = snapshot(vec![group("org", "srv", &["get_issue"])]);
        for body in [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        ] {
            assert!(matches!(
                enforce(&snapshot, &server, body.as_bytes()),
                Verdict::Allow
            ));
        }
    }

    #[test]
    fn a_batch_is_refused_as_a_whole_naming_every_refused_call() {
        let (snapshot, server) = snapshot(vec![group("org", "srv", &["get_issue"])]);
        let body = format!("[{},{},{}]", call("get_issue"), call("drop"), call("wipe"));
        assert_eq!(
            denied_tools(enforce(&snapshot, &server, body.as_bytes())),
            [Some("drop".to_string()), Some("wipe".to_string())]
        );
    }

    #[test]
    fn an_escaped_method_name_does_not_get_around_the_policy() {
        let (snapshot, server) = snapshot(vec![group("org", "srv", &["get_issue"])]);
        // JSON decodes `\/` to `/` and `c` to `c`, so a byte search for
        // `tools/call` finds neither
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools\/call","params":{"name":"drop"}}"#;
        assert_eq!(
            denied_tools(enforce(&snapshot, &server, body.as_bytes())),
            [Some("drop".to_string())]
        );
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"drop"}}"#;
        assert_eq!(
            denied_tools(enforce(&snapshot, &server, body.as_bytes())),
            [Some("drop".to_string())]
        );
    }

    #[test]
    fn a_call_that_names_no_tool_is_refused() {
        let (snapshot, server) = snapshot(vec![group("org", "srv", &["get_issue"])]);
        for body in [
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":7}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call"}"#,
        ] {
            assert_eq!(
                denied_tools(enforce(&snapshot, &server, body.as_bytes())),
                [None],
                "{body}"
            );
        }
    }

    #[test]
    fn a_tool_call_sent_as_a_notification_is_still_refused() {
        let (snapshot, server) = snapshot(vec![group("org", "srv", &["get_issue"])]);
        let body = r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"drop"}}"#;
        assert_eq!(
            denied_tools(enforce(&snapshot, &server, body.as_bytes())),
            [Some("drop".to_string())]
        );
    }

    #[test]
    fn a_repeated_key_in_a_message_is_refused_whichever_copy_wins() {
        let (snapshot, server) = snapshot(vec![group("org", "srv", &["get_issue"])]);
        // serde_json keeps the last copy, so each of these reads as an allowed
        // call (or not a call at all) here while a first-wins upstream sees a
        // call for `drop`
        for body in [
            // `name`
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"drop","name":"get_issue"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_issue","name":"drop"}}"#,
            // `params`
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"drop"},"params":{"name":"get_issue"}}"#,
            // `method`
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","method":"tools/list","params":{"name":"drop"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","method":"tools/call","params":{"name":"get_issue"}}"#,
            // a repeat inside a batch
            r#"[{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_issue"}},{"id":1,"id":2,"method":"tools/call","params":{"name":"get_issue"}}]"#,
        ] {
            assert!(
                matches!(
                    enforce(&snapshot, &server, body.as_bytes()),
                    Verdict::Ambiguous
                ),
                "{body}"
            );
        }
    }

    #[test]
    fn no_failure_policy_lets_a_repeated_key_through() {
        let (open, server) = with_manifest(&["a"], Some(settings(false, true)));
        let body =
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"a","name":"b"}}"#;
        assert!(matches!(
            enforce(&open, &server, body.as_bytes()),
            Verdict::Ambiguous
        ));
    }

    #[test]
    fn a_repeated_key_inside_tool_arguments_is_the_tools_own_business() {
        let (snapshot, server) = snapshot(vec![group("org", "srv", &["get_issue"])]);
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_issue","arguments":{"q":1,"q":2}}}"#;
        assert!(matches!(
            enforce(&snapshot, &server, body.as_bytes()),
            Verdict::Allow
        ));
    }

    fn named(mut group: McpToolGroupConfig, id: &str) -> McpToolGroupConfig {
        group.id = id.into();
        group
    }

    fn allowed_for(
        snapshot: &Snapshot,
        server: &McpServerConfig,
        caller: &Caller<'_>,
    ) -> Vec<bool> {
        ["a", "b", "c"]
            .iter()
            .map(|tool| {
                matches!(
                    super::enforce(snapshot, server, caller, call(tool).as_bytes()),
                    Verdict::Allow
                )
            })
            .collect()
    }

    /// #3295: a group assigned to a key, a project or a team narrows the callers
    /// it names; everyone else keeps the organization-wide groups.
    #[test]
    fn an_assignment_narrows_its_subject_and_leaves_everyone_else_alone() {
        for (kind, id) in [
            ("virtual_key", "key-1"),
            ("project", "project-1"),
            ("team", "team-1"),
        ] {
            let (snapshot, server) = world(vec![
                // the organization-wide set is a, b and c
                named(group("org", "srv", &["a", "b", "c"]), "all"),
                assigned(named(group("org", "srv", &["a"]), "narrow"), kind, id),
            ]);
            assert_eq!(
                allowed_for(&snapshot, &server, &caller()),
                [true, false, false],
                "{kind}"
            );
            let other = Caller {
                key_id: "key-2",
                project_id: "project-2",
                team_id: "team-2",
                ..caller()
            };
            assert_eq!(
                allowed_for(&snapshot, &server, &other),
                [true, true, true],
                "{kind}: an unassigned caller keeps the organization's groups"
            );
        }
    }

    #[test]
    fn several_assignments_union_and_a_server_none_of_them_names_is_closed() {
        let (snapshot, server) = world(vec![
            named(group("org", "srv", &["a", "b", "c"]), "all"),
            assigned(named(group("org", "srv", &["a"]), "g1"), "team", "team-1"),
            assigned(
                named(group("org", "srv", &["b"]), "g2"),
                "project",
                "project-1",
            ),
            assigned(
                named(group("org", "elsewhere", &["z"]), "g3"),
                "virtual_key",
                "key-1",
            ),
        ]);
        assert_eq!(
            allowed_for(&snapshot, &server, &caller()),
            [true, true, false]
        );
        // the caller's only assignment names another server: this one governed
        // by the organization's groups, and nothing the caller holds names it
        let (snapshot, server) = world(vec![
            named(group("org", "srv", &["a"]), "all"),
            assigned(
                named(group("org", "elsewhere", &["z"]), "g3"),
                "virtual_key",
                "key-1",
            ),
        ]);
        assert_eq!(
            allowed_for(&snapshot, &server, &caller()),
            [false, false, false]
        );
    }

    #[test]
    fn an_assignment_in_another_organization_matches_no_caller() {
        let (snapshot, server) = world(vec![
            named(group("org", "srv", &["a", "b", "c"]), "all"),
            assigned(
                named(group("other-org", "srv", &["a"]), "foreign"),
                "virtual_key",
                "key-1",
            ),
        ]);
        assert_eq!(
            allowed_for(&snapshot, &server, &caller()),
            [true, true, true]
        );
    }

    #[test]
    fn a_tools_list_is_narrowed_to_the_assigned_groups() {
        let (snapshot, server) = world(vec![
            named(group("org", "srv", &["a", "b"]), "all"),
            assigned(
                named(group("org", "srv", &["a"]), "narrow"),
                "team",
                "team-1",
            ),
        ]);
        let request = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let filter = list_filter(&snapshot, &server, &caller(), request).unwrap();
        let reply = br#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"a"},{"name":"b"}]}}"#;
        let filtered = String::from_utf8(filter.filter_json(reply).unwrap()).unwrap();
        assert!(
            filtered.contains("\"a\"") && !filtered.contains("\"b\""),
            "{filtered}"
        );
    }

    #[test]
    fn a_body_that_is_not_json_cannot_be_checked() {
        let (snapshot, server) = snapshot(vec![group("org", "srv", &["get_issue"])]);
        assert!(matches!(
            enforce(&snapshot, &server, b"{\"method\": tools/call"),
            Verdict::Unreadable
        ));
        // no body is nothing to refuse
        assert!(matches!(enforce(&snapshot, &server, b""), Verdict::Allow));
    }

    #[test]
    fn a_group_never_governs_another_organizations_server() {
        let (snapshot, server) = snapshot(vec![group("other-org", "srv", &["get_issue"])]);
        assert!(matches!(
            enforce(&snapshot, &server, call("delete_repo").as_bytes()),
            Verdict::Allow
        ));
    }

    fn with_manifest(
        tools: &[&str],
        settings: Option<McpOrgSettingsConfig>,
    ) -> (Snapshot, McpServerConfig) {
        let server = McpServerConfig {
            id: "srv".into(),
            org_id: "org".into(),
            slug: "github".into(),
            tools: tools.iter().map(|t| (*t).to_string()).collect(),
            ..Default::default()
        };
        let mut config = GatewayConfig::default();
        config.mcp_servers.push(server.clone());
        config.mcp_settings.extend(settings);
        (
            Snapshot::build(&config, &crate::load::LoadTracker::default()),
            server,
        )
    }

    fn settings(allow_unlisted_tools: bool, fail_open: bool) -> McpOrgSettingsConfig {
        McpOrgSettingsConfig {
            org_id: "org".into(),
            connect_timeout_ms: 5000,
            request_timeout_ms: 30000,
            max_retries: 1,
            fail_open,
            allow_unlisted_tools,
        }
    }

    #[test]
    fn saved_settings_that_refuse_unlisted_tools_enforce_the_manifest() {
        let (snapshot, server) = with_manifest(&["get_issue"], Some(settings(false, false)));
        assert!(matches!(
            enforce(&snapshot, &server, call("get_issue").as_bytes()),
            Verdict::Allow
        ));
        match enforce(&snapshot, &server, call("delete_repo").as_bytes()) {
            Verdict::Denied(refused) => {
                assert_eq!(refused[0].denial, Denial::Undeclared);
                assert!(refused[0].message("github").contains("MCP settings"));
            }
            other => panic!("expected a denial, got {other:?}"),
        }
    }

    #[test]
    fn an_org_that_never_saved_its_settings_keeps_unlisted_tools() {
        // the behaviour before manifests were enforced, kept so an upgrade does
        // not start refusing a tool the manifest leaves out
        let (snapshot, server) = with_manifest(&["get_issue"], None);
        assert!(matches!(
            enforce(&snapshot, &server, call("delete_repo").as_bytes()),
            Verdict::Allow
        ));
    }

    /// #3334: an organization created enforcing refuses a tool outside the
    /// manifest without having saved anything, and saving settings that allow
    /// unlisted tools is how it opts out.
    #[test]
    fn a_new_organization_enforces_its_manifest_until_it_opts_out() {
        let enforcing = |settings: Option<McpOrgSettingsConfig>| {
            let (mut snapshot_config, mut server) = (
                GatewayConfig::default(),
                McpServerConfig {
                    id: "srv".into(),
                    org_id: "org".into(),
                    slug: "github".into(),
                    tools: vec!["get_issue".into()],
                    enforce_manifest_by_default: true,
                    ..Default::default()
                },
            );
            snapshot_config.mcp_servers.push(server.clone());
            snapshot_config.mcp_settings.extend(settings);
            (
                Snapshot::build(&snapshot_config, &crate::load::LoadTracker::default()),
                server,
            )
        };
        let (snapshot, server) = enforcing(None);
        assert!(matches!(
            enforce(&snapshot, &server, call("get_issue").as_bytes()),
            Verdict::Allow
        ));
        assert!(matches!(
            enforce(&snapshot, &server, call("delete_repo").as_bytes()),
            Verdict::Denied(_)
        ));
        let (snapshot, server) = enforcing(Some(settings(true, false)));
        assert!(matches!(
            enforce(&snapshot, &server, call("delete_repo").as_bytes()),
            Verdict::Allow
        ));
    }

    #[test]
    fn allowing_unlisted_tools_lifts_the_manifest_check() {
        let (snapshot, server) = with_manifest(&["get_issue"], Some(settings(true, false)));
        assert!(matches!(
            enforce(&snapshot, &server, call("delete_repo").as_bytes()),
            Verdict::Allow
        ));
    }

    #[test]
    fn a_server_declaring_no_manifest_has_nothing_to_check_against() {
        let (snapshot, server) = with_manifest(&[], None);
        assert!(matches!(
            enforce(&snapshot, &server, call("anything").as_bytes()),
            Verdict::Allow
        ));
    }

    #[test]
    fn groups_and_the_manifest_both_have_to_agree() {
        // `stale` is in the group but no longer in the manifest, which the
        // org's saved settings enforce
        let server = McpServerConfig {
            id: "srv".into(),
            org_id: "org".into(),
            slug: "github".into(),
            tools: vec!["a".into()],
            ..Default::default()
        };
        let mut config = GatewayConfig::default();
        config.mcp_servers.push(server.clone());
        config.mcp_settings.push(settings(false, false));
        config.mcp_tool_groups = vec![group("org", "srv", &["a", "stale"])];
        let snapshot = Snapshot::build(&config, &crate::load::LoadTracker::default());
        assert!(matches!(
            enforce(&snapshot, &server, call("a").as_bytes()),
            Verdict::Allow
        ));
        assert!(matches!(
            enforce(&snapshot, &server, call("stale").as_bytes()),
            Verdict::Denied(_)
        ));
    }

    #[test]
    fn the_failure_policy_decides_what_an_unreadable_body_gets() {
        let garbage = b"{ not json";
        let (closed, server) = with_manifest(&["a"], Some(settings(false, false)));
        assert!(matches!(
            enforce(&closed, &server, garbage),
            Verdict::Unreadable
        ));
        let (open, server) = with_manifest(&["a"], Some(settings(false, true)));
        assert!(matches!(enforce(&open, &server, garbage), Verdict::Allow));
        // fail open never lets a readable refused call through
        assert!(matches!(
            enforce(&open, &server, call("b").as_bytes()),
            Verdict::Denied(_)
        ));
    }
}
