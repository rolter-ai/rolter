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
//!
//! The groups an organization enables apply to every caller in it: there is no
//! per-key or per-project assignment of a group yet, so "the caller's effective
//! tool groups" is the organization's enabled set.
//!
//! The request body is parsed in full whenever the rule applies, with no
//! substring shortcut: JSON lets `tools/call` be written with an escaped slash
//! or a `\u` escape, so a byte search is a bypass here even though it is a fine
//! optimisation for the log.

use std::collections::HashSet;

use serde_json::{Map, Value};

use rolter_core::McpServerConfig;

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
}

/// What a call to one server is judged against.
struct Rules<'a> {
    /// the tools the enabled tool groups allow, when any names the server
    governed: Option<&'a HashSet<String>>,
    /// the server's declared manifest, when calls must stay inside it: it
    /// declares one and the organization does not allow unlisted tools
    declared: Option<&'a [String]>,
}

impl<'a> Rules<'a> {
    fn of(snapshot: &'a Snapshot, server: &'a McpServerConfig) -> Self {
        let allow_unlisted = snapshot
            .mcp_org_settings
            .get(&server.org_id)
            .is_some_and(|settings| settings.allow_unlisted_tools);
        Self {
            governed: snapshot.mcp_governed_tools.get(&server.id),
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
        if let Some(allowed) = self.governed {
            if !tool.is_some_and(|tool| allowed.contains(tool)) {
                return Some(Denial::NotInGroup);
            }
        }
        if let Some(declared) = self.declared {
            if !tool.is_some_and(|tool| declared.iter().any(|name| name == tool)) {
                return Some(Denial::Undeclared);
            }
        }
        None
    }
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

/// Apply the policy to a request body bound for `server`.
pub(crate) fn enforce(snapshot: &Snapshot, server: &McpServerConfig, body: &[u8]) -> Verdict {
    let rules = Rules::of(snapshot, server);
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
    use rolter_core::{GatewayConfig, McpOrgSettingsConfig, McpToolGroupConfig, McpToolRefConfig};

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
        }
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
    fn a_declared_manifest_refuses_an_undeclared_tool_by_default() {
        let (snapshot, server) = with_manifest(&["get_issue"], None);
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
        // the same with an explicit settings row that keeps the default
        let (snapshot, server) = with_manifest(&["get_issue"], Some(settings(false, false)));
        assert!(matches!(
            enforce(&snapshot, &server, call("delete_repo").as_bytes()),
            Verdict::Denied(_)
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
        let (mut snapshot, mut server) = snapshot(vec![group("org", "srv", &["a", "stale"])]);
        server.tools = vec!["a".into()];
        // `stale` is in the group but no longer in the manifest
        let mut config = GatewayConfig::default();
        config.mcp_servers.push(server.clone());
        config.mcp_tool_groups = vec![group("org", "srv", &["a", "stale"])];
        snapshot = Snapshot::build(&config, &crate::load::LoadTracker::default());
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
