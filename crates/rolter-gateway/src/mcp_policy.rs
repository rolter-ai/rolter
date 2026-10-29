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
//! A `tools/list` reply is narrowed by the same rule ([`ListFilter`]), so a
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

use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
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
    /// the body repeats a key, so the tool it names depends on which copy a
    /// parser keeps (#3298)
    Ambiguous,
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
            (Denial::Ambiguous, _) => ambiguous_message(server),
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
    /// names depends on which copy a parser keeps (#3298). Holds the body's
    /// `tools/call` requests so they can be logged like any refusal
    Ambiguous(Vec<Refused>),
}

/// What the caller is told when the body repeats a key.
pub(crate) fn ambiguous_message(server: &str) -> String {
    format!(
        "the request to MCP server '{server}' repeats a JSON key, so the tool it names is \
         ambiguous and it is refused"
    )
}

/// Who is calling, as far as group assignments are concerned.
pub(crate) struct Caller<'a> {
    pub org_id: &'a str,
    pub key_id: &'a str,
    pub project_id: &'a str,
    pub team_id: &'a str,
}

impl Caller<'_> {
    /// The groups assigned to the caller's key, project or team, or `None` when
    /// it has no assignment and so keeps the organization-wide set. A disabled
    /// group is among them, with no tools: its callers stay narrowed.
    fn assigned_groups<'s>(&self, snapshot: &'s Snapshot) -> Option<Vec<&'s String>> {
        // nothing is assigned in most deployments, and this runs per request
        if snapshot.mcp_group_assignments.is_empty() {
            return None;
        }
        let mut groups = Vec::new();
        for (kind, id) in [
            ("virtual_key", self.key_id),
            ("project", self.project_id),
            ("team", self.team_id),
        ] {
            if id.is_empty() {
                continue;
            }
            for (assigned_kind, org_id, group) in
                snapshot.mcp_group_assignments.get(id).into_iter().flatten()
            {
                if assigned_kind == kind && org_id == self.org_id {
                    groups.push(group);
                }
            }
        }
        (!groups.is_empty()).then_some(groups)
    }
}

/// What a call to one server is judged against.
pub(crate) struct Rules<'a> {
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
    pub(crate) fn of(
        snapshot: &'a Snapshot,
        server: &'a McpServerConfig,
        caller: &Caller<'_>,
    ) -> Self {
        // saved settings decide for themselves; an organization with none
        // follows the marker it was created with (#3334)
        let allow_unlisted = snapshot
            .mcp_org_settings
            .get(&server.org_id)
            .map_or(!server.enforce_manifest_by_default, |settings| {
                settings.allow_unlisted_tools
            });
        Self {
            governed: Self::governed(snapshot, server, caller),
            declared: (!allow_unlisted && !server.tools.is_empty())
                .then_some(server.tools.as_slice()),
        }
    }

    /// The tools the caller's groups allow on `server`, or `None` when no group
    /// governs it for this caller. The organization's enabled groups govern it
    /// for everyone; a caller with an assignment is held to the union of its
    /// assigned groups, and also governed by a *disabled* assigned group that
    /// names the server, with none of that group's tools.
    ///
    /// A key of the snapshot with an assignment finds that union already merged
    /// (`Snapshot::mcp_caller_tools`, #3435): per request that is two lookups
    /// and a borrow, where [`Self::governed_merged`] builds a `Vec` of the
    /// caller's groups and clones the union of their tool names. The caller's
    /// project and team are the key's own, taken from the same snapshot, which
    /// is what lets the key id stand for all three subjects. Any other caller
    /// takes the merge.
    fn governed<'s>(
        snapshot: &'s Snapshot,
        server: &McpServerConfig,
        caller: &Caller<'_>,
    ) -> Option<Cow<'s, HashSet<String>>> {
        match snapshot.mcp_caller_tools.get(caller.key_id) {
            Some(merged) => merged.governed.get(&server.id).map(Cow::Borrowed),
            None => Self::governed_merged(snapshot, server, caller),
        }
    }

    /// [`Self::governed`] worked out from the assignments for this one caller:
    /// what a caller the snapshot built nothing for gets, and the reference the
    /// precomputed union is held to.
    fn governed_merged<'s>(
        snapshot: &'s Snapshot,
        server: &McpServerConfig,
        caller: &Caller<'_>,
    ) -> Option<Cow<'s, HashSet<String>>> {
        let org_wide = snapshot.mcp_governed_tools.get(&server.id);
        let Some(groups) = caller.assigned_groups(snapshot) else {
            return org_wide.map(Cow::Borrowed);
        };
        let names_server = org_wide.is_some()
            || groups.iter().any(|group| {
                snapshot
                    .mcp_group_servers
                    .get(*group)
                    .is_some_and(|servers| servers.contains(&server.id))
            });
        names_server.then(|| {
            Cow::Owned(
                groups
                    .into_iter()
                    .filter_map(|group| snapshot.mcp_group_tools.get(group)?.get(&server.id))
                    .flatten()
                    .cloned()
                    .collect(),
            )
        })
    }

    /// Whether any rule applies. When none does, the body is forwarded without
    /// being parsed.
    pub(crate) fn any(&self) -> bool {
        self.governed.is_some() || self.declared.is_some()
    }

    /// Judge one tool name.
    fn judge(&self, tool: Option<&str>) -> Option<Denial> {
        judge(self.governed.as_deref(), self.declared, tool)
    }

    /// The filter for a reply that lists this server's tools. Owns its copy of
    /// the rules, so it is only built once something needs it.
    fn list_filter(&self) -> ListFilter {
        ListFilter::new(
            self.governed.as_deref().cloned(),
            self.declared.map(<[String]>::to_vec),
        )
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

/// The key a Go-style decoder would match `key` to: ASCII case folded, plus the
/// two non-ASCII characters (U+017F, U+212A) that fold to `s` and `k`.
/// `encoding/json` matches struct fields this way and keeps the last copy, so
/// `"NAME"` is `"name"` to an upstream written in Go (#3298).
pub(crate) fn fold_key(key: &str) -> String {
    key.chars()
        .map(|c| match c {
            '\u{17f}' => 's',
            '\u{212a}' => 'k',
            c => c.to_ascii_lowercase(),
        })
        .collect()
}

/// The value of the key a decoder would take for `name`, whatever its case.
fn get_folded<'a>(object: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    match object.get(name) {
        Some(value) => Some(value),
        None => object
            .iter()
            .find(|(key, _)| key.len() >= name.len() && fold_key(key) == name)
            .map(|(_, value)| value),
    }
}

/// [`get_folded`], moving the value out.
fn take_folded(object: &mut Map<String, Value>, name: &str) -> Option<Value> {
    let key = if object.contains_key(name) {
        name.to_string()
    } else {
        object
            .keys()
            .find(|key| key.len() >= name.len() && fold_key(key) == name)?
            .clone()
    };
    object.remove(&key)
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
    /// anything deeper, which is the tool's own data
    Plain,
}

/// What [`Parse`] read: the value, whether a message or `params` below it
/// repeats a key, and for a message whether *any* copy of its `method` is
/// `tools/call` (a repeated `method` keeps only the last copy in the value).
struct Out {
    value: Value,
    repeated: bool,
    call: bool,
    /// at the body level, `call` of each message in order
    calls: Vec<bool>,
}

impl Out {
    fn plain(value: Value) -> Self {
        Self {
            value,
            repeated: false,
            call: false,
            calls: Vec::new(),
        }
    }
}

/// Parses a body into a [`Value`] and, in the same pass, notes whether a
/// message or the `params` of one repeats a key (compared as a Go decoder folds
/// them). Keys deeper than `params` (tool `arguments`) are the tool's own data
/// and are not the policy's to judge.
struct Parse(Level);

impl<'de> DeserializeSeed<'de> for Parse {
    type Value = Out;

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Out, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Parse {
    type Value = Out;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Out, A::Error> {
        let checked = !matches!(self.0, Level::Plain);
        let mut seen = HashSet::new();
        let mut repeated = false;
        let mut call = false;
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let mut child = Level::Plain;
            let mut is_method = false;
            if checked {
                let folded = fold_key(&key);
                if matches!(self.0, Level::Body | Level::Message) {
                    if folded == "params" {
                        child = Level::Params;
                    }
                    is_method = folded == "method";
                }
                repeated |= !seen.insert(folded);
            }
            let below = map.next_value_seed(Parse(child))?;
            repeated |= below.repeated;
            if is_method {
                call |= below.value.as_str() == Some("tools/call");
            }
            object.insert(key, below.value);
        }
        let calls = if matches!(self.0, Level::Body) {
            vec![call]
        } else {
            Vec::new()
        };
        Ok(Out {
            value: Value::Object(object),
            repeated,
            call,
            calls,
        })
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Out, A::Error> {
        let level = match self.0 {
            Level::Body => Level::Message,
            _ => Level::Plain,
        };
        let mut items = Vec::new();
        let mut calls = Vec::new();
        let mut repeated = false;
        while let Some(item) = seq.next_element_seed(Parse(level))? {
            repeated |= item.repeated;
            calls.push(item.call);
            items.push(item.value);
        }
        let calls = if matches!(self.0, Level::Body) {
            calls
        } else {
            Vec::new()
        };
        Ok(Out {
            value: Value::Array(items),
            repeated,
            call: false,
            calls,
        })
    }

    fn visit_bool<E>(self, v: bool) -> Result<Out, E> {
        Ok(Out::plain(Value::Bool(v)))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Out, E> {
        Ok(Out::plain(Value::from(v)))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Out, E> {
        Ok(Out::plain(Value::from(v)))
    }
    fn visit_f64<E>(self, v: f64) -> Result<Out, E> {
        Ok(Out::plain(Value::from(v)))
    }
    fn visit_str<E>(self, v: &str) -> Result<Out, E> {
        Ok(Out::plain(Value::String(v.to_string())))
    }
    fn visit_string<E>(self, v: String) -> Result<Out, E> {
        Ok(Out::plain(Value::String(v)))
    }
    fn visit_unit<E>(self) -> Result<Out, E> {
        Ok(Out::plain(Value::Null))
    }
}

/// A `tools/call` request: the tool it names, if any, and the canonical request.
type ToolCall = (Option<String>, Map<String, Value>);

/// The `tools/call` requests the log records from one body, and how many it
/// left out for want of room.
#[derive(Default)]
pub(crate) struct Calls {
    pub kept: Vec<Map<String, Value>>,
    pub dropped: usize,
}

/// A request body read once: its messages, and whether any of them repeats a
/// key. `None` when it is not JSON at all.
pub(crate) struct Body {
    messages: Vec<Value>,
    /// whether any copy of each message's `method` is `tools/call`
    called: Vec<bool>,
    repeated: bool,
}

impl Body {
    fn parse(bytes: &[u8]) -> Option<Self> {
        let mut parser = serde_json::Deserializer::from_slice(bytes);
        let out = Parse(Level::Body).deserialize(&mut parser).ok()?;
        parser.end().ok()?;
        let messages = match out.value {
            Value::Array(items) => items,
            other => vec![other],
        };
        Some(Self {
            messages,
            called: out.calls,
            repeated: out.repeated,
        })
    }

    /// Whether `bytes` can hold a `tools/call`: it spells the method out, or it
    /// holds a `\u` or `\/` escape that could spell it. Most MCP traffic is neither, and is
    /// not worth a parse when only the log wants it.
    fn may_call(bytes: &[u8]) -> bool {
        const NEEDLE: &[u8] = b"tools/call";
        // only `\u` and `\/` can spell a character of it
        bytes.windows(NEEDLE.len()).any(|w| w == NEEDLE)
            || bytes
                .windows(2)
                .any(|w| w[0] == b'\\' && matches!(w[1], b'u' | b'/'))
    }

    /// The `tools/call` requests of a body the policy allowed, consumed: a single
    /// request, every call in a batch, and a notification (no `id`, so no reply
    /// to follow). Each is the canonical request [`Self::tool_calls`] builds, so a
    /// key written in another case is read as a decoder would read it, and it is
    /// moved out of the parsed body rather than copied, so a batch costs no more
    /// than the parse that already happened (#3466). A call that names no tool is
    /// left out: the log has no tool to put in its row.
    pub(crate) fn into_calls(self, arguments: bool, limit: usize) -> Calls {
        let mut calls = Calls::default();
        for mut message in self.messages {
            if Self::method(&message) != Some("tools/call") {
                continue;
            }
            // a body is bounded only by the request size limit, so the rows
            // made from it are bounded here and the rest are counted
            if calls.kept.len() >= limit {
                calls.dropped += 1;
                continue;
            }
            let Some(request) = message.as_object_mut() else {
                continue;
            };
            // `null` is no id a reply could be matched to
            let id = take_folded(request, "id").filter(|id| !id.is_null());
            let Some(Value::Object(mut params)) = take_folded(request, "params") else {
                continue;
            };
            let Some(name) = take_folded(&mut params, "name") else {
                continue;
            };
            let mut kept = Map::new();
            kept.insert("name".into(), name);
            // the arguments are only taken when the log will keep them
            if arguments {
                if let Some(arguments) = take_folded(&mut params, "arguments") {
                    kept.insert("arguments".into(), arguments);
                }
            }
            let mut canonical = Map::new();
            if let Some(id) = id {
                canonical.insert("id".into(), id);
            }
            canonical.insert("params".into(), Value::Object(kept));
            calls.kept.push(canonical);
        }
        calls
    }

    /// The method a message names, read the way a decoder would.
    fn method(message: &Value) -> Option<&str> {
        get_folded(message.as_object()?, "method")?.as_str()
    }

    fn holds_method(&self, wanted: &str) -> bool {
        self.messages
            .iter()
            .any(|message| Self::method(message) == Some(wanted))
    }

    /// The `tools/call` requests, as the canonical request the log expects
    /// (`id`, `params.name`, `params.arguments`), whatever case the caller wrote
    /// the keys in. A batch may hold anything; only an object can be a call, and
    /// an entry that is not one is the upstream's to reject. When `any_copy` is
    /// set a message counts if *any* copy of its `method` is `tools/call`, which
    /// is how a body that repeats a key is logged; a call that names no tool is
    /// logged with an empty one.
    ///
    /// `refused` is asked the tool a call names before anything of it is copied,
    /// so a call the policy allows costs no clone (the arguments of a request
    /// can be large and most calls are allowed). At most `limit` are built; the
    /// number of the rest is the second field.
    fn tool_calls(
        &self,
        any_copy: bool,
        limit: usize,
        refused: impl Fn(Option<&str>) -> bool,
    ) -> (Vec<ToolCall>, usize) {
        let mut calls = Vec::new();
        let mut dropped = 0;
        for (i, message) in self.messages.iter().enumerate() {
            if Self::method(message) != Some("tools/call")
                && !(any_copy && self.called.get(i).copied().unwrap_or(false))
            {
                continue;
            }
            let Some(request) = message.as_object() else {
                continue;
            };
            let params = get_folded(request, "params").and_then(Value::as_object);
            let name = params.and_then(|params| get_folded(params, "name"));
            let tool = name.and_then(Value::as_str);
            if !refused(tool) {
                continue;
            }
            if calls.len() >= limit {
                dropped += 1;
                continue;
            }
            let mut canonical = Map::new();
            if let Some(id) = get_folded(request, "id") {
                canonical.insert("id".into(), id.clone());
            }
            let mut kept = Map::new();
            if let Some(name) = name {
                kept.insert("name".into(), name.clone());
            }
            if any_copy && tool.is_none() {
                kept.insert("name".into(), Value::String(String::new()));
            }
            if let Some(arguments) = params.and_then(|params| get_folded(params, "arguments")) {
                kept.insert("arguments".into(), arguments.clone());
            }
            if params.is_some() || any_copy {
                canonical.insert("params".into(), Value::Object(kept));
            }
            calls.push((tool.map(str::to_string), canonical));
        }
        (calls, dropped)
    }
}

/// The UTF-8 byte-order mark.
const BOM: &[u8] = b"\xef\xbb\xbf";

/// `body` without a leading UTF-8 byte-order mark, which RFC 8259 lets a
/// parser ignore (#3527). The relay strips it before it judges a body for a
/// governed server and forwards the stripped bytes, so the upstream receives
/// exactly what was judged. Any other encoding is left as it is, and refused
/// when it does not parse.
pub(crate) fn without_bom(body: bytes::Bytes) -> bytes::Bytes {
    if body.starts_with(BOM) {
        body.slice(BOM.len()..)
    } else {
        body
    }
}

/// A request body as far as it was read, once, for the policy and the MCP log
/// alike (#3399): both take their keys from the same case-folded lookup, so a
/// call the policy allows is a call the log records.
pub(crate) enum Read {
    /// nothing needed the body, so it was not parsed
    Skipped,
    /// something needed it and it is not JSON
    Unreadable,
    Parsed(Body),
}

impl Read {
    /// Read `bytes` when a rule applies (`governed`) or when `logging` wants to
    /// know whether it is a `tools/call`; otherwise leave it alone.
    pub(crate) fn of(bytes: &[u8], governed: bool, logging: bool) -> Self {
        let wanted = !bytes.is_empty() && (governed || (logging && Body::may_call(bytes)));
        if !wanted {
            return Self::Skipped;
        }
        Body::parse(bytes).map_or(Self::Unreadable, Self::Parsed)
    }

    /// The allowed `tools/call` requests the log records, consuming the read.
    /// `arguments` says whether the log keeps them. Called only once the policy
    /// has allowed the body (or an earlier check has refused the request, which
    /// the log records for every call the body holds).
    pub(crate) fn into_calls(self, arguments: bool, limit: usize) -> Calls {
        match self {
            Self::Parsed(body) => body.into_calls(arguments, limit),
            _ => Calls::default(),
        }
    }
}

/// What the policy made of one request body.
pub(crate) struct Decision {
    pub verdict: Verdict,
    /// set when the body is allowed and asks for a tool list, so the reply can
    /// be narrowed (#3297)
    pub list_filter: Option<ListFilter>,
    /// refused calls beyond what a request may log, so they are not rows
    pub unlogged: usize,
}

impl Decision {
    fn of(verdict: Verdict) -> Self {
        Self {
            verdict,
            list_filter: None,
            unlogged: 0,
        }
    }
}

/// The filter for a stream the caller opens with `GET`, which can carry
/// `tools/list` replies to requests posted elsewhere, or `None` when no rule
/// applies to the server (#3297).
pub(crate) fn stream_filter(rules: &Rules<'_>) -> Option<ListFilter> {
    rules.any().then(|| rules.list_filter())
}

/// Apply the policy to a request body bound for `server`, read once by the
/// caller as `read`.
pub(crate) fn evaluate_read(rules: &Rules<'_>, read: &Read) -> Decision {
    if !rules.any() {
        return Decision::of(Verdict::Allow);
    }
    let parsed = match read {
        Read::Skipped => return Decision::of(Verdict::Allow),
        Read::Parsed(parsed) => parsed,
        // the relay forwards only a body it has judged (#3527): one it cannot
        // read is refused whatever the organization's deprecated `fail_open`
        // says, since an upstream with a more lenient parser (Python's
        // `json.loads` takes `NaN`, and on bytes UTF-16) could run a call the
        // policy never saw
        Read::Unreadable => return Decision::of(Verdict::Unreadable),
    };
    if parsed.repeated {
        let (calls, unlogged) =
            parsed.tool_calls(true, crate::mcp_log::MAX_CALLS_PER_REQUEST, |_| true);
        return Decision {
            unlogged,
            ..Decision::of(Verdict::Ambiguous(
                calls
                    .into_iter()
                    .map(|(tool, request)| Refused {
                        tool,
                        denial: Denial::Ambiguous,
                        request,
                    })
                    .collect(),
            ))
        };
    }
    let (calls, unlogged) =
        parsed.tool_calls(false, crate::mcp_log::MAX_CALLS_PER_REQUEST, |tool| {
            rules.judge(tool).is_some()
        });
    let refused: Vec<Refused> = calls
        .into_iter()
        .filter_map(|(tool, request)| {
            let denial = rules.judge(tool.as_deref())?;
            Some(Refused {
                tool,
                denial,
                request,
            })
        })
        .collect();
    if !refused.is_empty() {
        return Decision {
            unlogged,
            ..Decision::of(Verdict::Denied(refused))
        };
    }
    // a reply is narrowed whatever id it carries: ids are compared as JSON
    // values (`1` is not `1.0`) and a decoder may read `ID` as `id`, so
    // matching replies to requests is a way to be bypassed
    let list_filter = parsed
        .holds_method("tools/list")
        .then(|| rules.list_filter());
    Decision {
        verdict: Verdict::Allow,
        list_filter,
        unlogged: 0,
    }
}

/// Read and judge a body in one step, as the proxy does across its own.
#[cfg(test)]
pub(crate) fn evaluate(
    snapshot: &Snapshot,
    server: &McpServerConfig,
    caller: &Caller<'_>,
    body: &[u8],
) -> Decision {
    let rules = Rules::of(snapshot, server, caller);
    let read = Read::of(body, rules.any(), false);
    evaluate_read(&rules, &read)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolter_core::{
        GatewayConfig, McpOrgSettingsConfig, McpToolGroupConfig, McpToolGroupSubjectConfig,
        McpToolRefConfig, VirtualKeyRecord,
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
        super::evaluate(snapshot, server, &caller(), body).verdict
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
        // the default caller and one nobody is assigned to, as keys of the
        // snapshot, so their groups are the ones merged when it is built
        config.db_virtual_keys.extend(
            [
                ("key-1", "project-1", "team-1"),
                ("key-2", "project-2", "team-2"),
            ]
            .map(|(id, project, team)| VirtualKeyRecord {
                id: id.into(),
                org_id: "org".into(),
                project_id: project.into(),
                team_id: team.into(),
                key_hash: format!("hash-{id}"),
                user_id: "user-1".into(),
                ..Default::default()
            }),
        );
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
                    Verdict::Ambiguous(_)
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
            Verdict::Ambiguous(_)
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
                    super::evaluate(snapshot, server, caller, call(tool).as_bytes()).verdict,
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

    /// #3435: the union merged when the snapshot is built is the one a request
    /// would have merged for itself, for a key with groups on every subject, an
    /// overlap, a disabled group, a server only a disabled group names, and for
    /// callers with none.
    #[test]
    fn the_precomputed_union_equals_the_merged_one() {
        let mut off = assigned(
            named(group("org", "elsewhere", &["z"]), "off"),
            "team",
            "team-2",
        );
        off.disabled = true;
        let (snapshot, server) = world(vec![
            named(group("org", "srv", &["a", "b", "c"]), "all"),
            assigned(
                named(group("org", "srv", &["a", "b"]), "g1"),
                "team",
                "team-1",
            ),
            assigned(
                named(group("org", "srv", &["b", "c"]), "g2"),
                "project",
                "project-1",
            ),
            assigned(
                named(group("org", "srv", &["a"]), "g3"),
                "virtual_key",
                "key-1",
            ),
            assigned(
                named(group("org", "elsewhere", &["z"]), "g4"),
                "virtual_key",
                "key-1",
            ),
            off,
            // another organization's assignment matches no caller
            assigned(
                named(group("other", "srv", &["c"]), "foreign"),
                "virtual_key",
                "key-1",
            ),
        ]);
        assert!(snapshot.mcp_caller_tools.contains_key("key-1"));
        assert!(snapshot.mcp_caller_tools.contains_key("key-2"));
        let elsewhere = snapshot
            .mcp_servers
            .values()
            .find(|candidate| candidate.id == "elsewhere")
            .cloned()
            .unwrap();
        for caller in [
            caller(),
            Caller {
                key_id: "key-2",
                project_id: "project-2",
                team_id: "team-2",
                ..caller()
            },
            Caller {
                key_id: "unknown",
                ..caller()
            },
        ] {
            for server in [&server, &elsewhere] {
                let precomputed = Rules::governed(&snapshot, server, &caller);
                let merged = Rules::governed_merged(&snapshot, server, &caller);
                assert_eq!(
                    precomputed.as_deref(),
                    merged.as_deref(),
                    "{} on {}",
                    caller.key_id,
                    server.id
                );
            }
        }
        // the key with groups on all three subjects: a, b and c, each once, and
        // nothing is cloned to read it
        let governed = Rules::governed(&snapshot, &server, &caller()).unwrap();
        assert!(matches!(governed, Cow::Borrowed(_)));
        assert_eq!(governed.len(), 3);
        // key-2's only assignment is a disabled group naming another server, so
        // here it is the organization's set, and there it is narrowed to none
        assert_eq!(
            allowed_for(&snapshot, &server, &caller()),
            [true, true, true]
        );
        let other = Caller {
            key_id: "key-2",
            project_id: "project-2",
            team_id: "team-2",
            ..caller()
        };
        assert_eq!(
            Rules::governed(&snapshot, &elsewhere, &other)
                .unwrap()
                .len(),
            0
        );
    }

    /// a governed caller's body is always read, whatever the log wants: the
    /// policy cannot judge what it did not parse
    #[test]
    fn a_governed_body_is_always_read_and_an_ungoverned_one_only_for_the_log() {
        let plain: &[u8] = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let call: &[u8] =
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"a"}}"#;
        let escaped = br#"{"method":"tools\u002fcall","id":1,"params":{"name":"a"}}"#;
        for (logging, body) in [(true, plain), (false, plain), (true, call), (false, call)] {
            assert!(matches!(Read::of(body, true, logging), Read::Parsed(_)));
        }
        assert!(matches!(Read::of(plain, false, true), Read::Skipped));
        assert!(matches!(Read::of(call, false, false), Read::Skipped));
        assert!(matches!(Read::of(call, false, true), Read::Parsed(_)));
        assert!(matches!(Read::of(escaped, false, true), Read::Parsed(_)));
        // an escape that cannot spell the method is not worth a parse
        let newline = br#"{"method":"ping","id":1,"params":{"q":"a\nb \"c\""}}"#;
        assert!(matches!(Read::of(newline, false, true), Read::Skipped));
        assert!(matches!(
            Read::of(b"not json", true, false),
            Read::Unreadable
        ));
    }

    /// a key's entry holds the servers of its own organization only (#3435)
    #[test]
    fn a_precomputed_entry_holds_only_its_own_organizations_servers() {
        let mut config = GatewayConfig::default();
        for (id, org) in [("srv-a", "org-a"), ("srv-b", "org-b")] {
            config.mcp_servers.push(McpServerConfig {
                id: id.into(),
                org_id: org.into(),
                slug: id.into(),
                ..Default::default()
            });
        }
        config.mcp_tool_groups = vec![
            assigned(group("org-a", "srv-a", &["a"]), "team", "team-a"),
            // org-b governs its own server for everyone
            group("org-b", "srv-b", &["b"]),
        ];
        config.mcp_tool_groups[0].id = "g-a".into();
        config.mcp_tool_groups[1].id = "g-b".into();
        config.db_virtual_keys.push(VirtualKeyRecord {
            id: "key-a".into(),
            org_id: "org-a".into(),
            project_id: "project-a".into(),
            team_id: "team-a".into(),
            key_hash: "hash-a".into(),
            user_id: "user-1".into(),
            ..Default::default()
        });
        let snapshot = Snapshot::build(&config, &crate::load::LoadTracker::default());
        let entry = &snapshot.mcp_caller_tools["key-a"];
        let mut servers: Vec<&String> = entry.governed.keys().collect();
        servers.sort();
        assert_eq!(servers, ["srv-a"], "org-b's server is not this key's");
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
        let filter = evaluate(&snapshot, &server, &caller(), request)
            .list_filter
            .unwrap();
        let reply = br#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"a"},{"name":"b"}]}}"#;
        let filtered = String::from_utf8(filter.filter_json(reply).unwrap()).unwrap();
        assert!(
            filtered.contains("\"a\"") && !filtered.contains("\"b\""),
            "{filtered}"
        );
    }

    /// #3298: a Go decoder matches keys case-insensitively and keeps the last
    /// copy, so a repeat that only differs in case is a repeat
    #[test]
    fn a_repeat_that_differs_only_in_case_is_refused() {
        let (snapshot, server) = snapshot(vec![group("org", "srv", &["get_issue"])]);
        for body in [
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_issue","NAME":"drop"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"Name":"drop","name":"get_issue"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","Method":"tools/call","params":{"name":"drop"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"drop"},"Params":{"name":"get_issue"}}"#,
            // U+017F and U+212A fold to `s` and `k` in a Go decoder
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"param\u{17f}\":{\"name\":\"get_issue\"},\"params\":{\"name\":\"drop\"}}",
        ] {
            assert!(
                matches!(enforce(&snapshot, &server, body.as_bytes()), Verdict::Ambiguous(_)),
                "{body}"
            );
        }
    }

    /// a lone `Method` or `NAME` is read by such a decoder as `method` or
    /// `name`, so the policy has to read it the same way
    #[test]
    fn keys_in_another_case_are_judged_like_the_lowercase_ones() {
        let (snapshot, server) = snapshot(vec![group("org", "srv", &["get_issue"])]);
        for body in [
            r#"{"jsonrpc":"2.0","id":1,"Method":"tools/call","params":{"name":"drop"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","PARAMS":{"Name":"drop"}}"#,
        ] {
            assert_eq!(
                denied_tools(enforce(&snapshot, &server, body.as_bytes())),
                [Some("drop".to_string())],
                "{body}"
            );
        }
        let allowed =
            r#"{"jsonrpc":"2.0","id":1,"Method":"tools/call","params":{"NAME":"get_issue"}}"#;
        assert!(matches!(
            enforce(&snapshot, &server, allowed.as_bytes()),
            Verdict::Allow
        ));
    }

    /// #3297: whatever the request's id, the reply to a tool list is narrowed
    #[test]
    fn a_tools_list_request_makes_a_filter_whatever_its_id() {
        let (snapshot, server) = snapshot(vec![group("org", "srv", &["get_issue"])]);
        for body in [
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":1.0,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":null,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":1,"Method":"tools/list"}"#,
            r#"[{"jsonrpc":"2.0","id":2,"method":"ping"},{"jsonrpc":"2.0","id":1,"method":"tools/list"}]"#,
        ] {
            let decision = evaluate(&snapshot, &server, &caller(), body.as_bytes());
            assert!(matches!(decision.verdict, Verdict::Allow), "{body}");
            assert!(decision.list_filter.is_some(), "{body}");
        }
        for body in [
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_issue"}}"#,
        ] {
            let decision = evaluate(&snapshot, &server, &caller(), body.as_bytes());
            assert!(decision.list_filter.is_none(), "{body}");
        }
    }

    #[test]
    fn a_stream_is_filtered_only_where_a_rule_applies() {
        let (snapshot, server) = snapshot(vec![group("org", "srv", &["get_issue"])]);
        assert!(stream_filter(&Rules::of(&snapshot, &server, &caller())).is_some());
        let (snapshot, server) = world(vec![]);
        assert!(stream_filter(&Rules::of(&snapshot, &server, &caller())).is_none());
    }

    /// a group disabled while callers are assigned to it narrows them to
    /// nothing; it does not hand them the organization-wide set back
    #[test]
    fn a_caller_assigned_to_a_disabled_group_gets_none_of_its_tools() {
        let mut off = assigned(named(group("org", "srv", &["a"]), "off"), "team", "team-1");
        off.disabled = true;
        let (snapshot, server) = world(vec![named(group("org", "srv", &["a", "b"]), "all"), off]);
        assert_eq!(
            allowed_for(&snapshot, &server, &caller()),
            [false, false, false]
        );
    }

    /// the disabled group was the only one naming the server: its callers are
    /// still held to none of its tools, while everyone else is not governed
    #[test]
    fn a_disabled_group_that_is_the_only_one_still_governs_its_callers() {
        let mut off = assigned(named(group("org", "srv", &["a"]), "off"), "team", "team-1");
        off.disabled = true;
        let (snapshot, server) = world(vec![off]);
        assert_eq!(
            allowed_for(&snapshot, &server, &caller()),
            [false, false, false]
        );
        let other = Caller {
            key_id: "key-2",
            project_id: "project-2",
            team_id: "team-2",
            ..caller()
        };
        assert_eq!(allowed_for(&snapshot, &server, &other), [true, true, true]);
    }

    /// #3381: a refused repeat is logged whichever copy of `method` is the call
    #[test]
    fn a_repeat_is_logged_whichever_copy_of_method_is_tools_call() {
        let (snapshot, server) = snapshot(vec![group("org", "srv", &["get_issue"])]);
        for body in [
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","method":"tools/list","params":{"name":"drop"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","method":"tools/call","params":{"name":"drop"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","method":"ping"}"#,
        ] {
            let Verdict::Ambiguous(refused) = enforce(&snapshot, &server, body.as_bytes()) else {
                panic!("{body}");
            };
            assert_eq!(refused.len(), 1, "{body}");
            let name = refused[0].request["params"]["name"].as_str();
            assert!(name.is_some(), "a row needs a name, empty at worst: {body}");
        }
        // a repeat in a message that is no call logs nothing
        let body = r#"{"jsonrpc":"2.0","id":1,"id":2,"method":"ping"}"#;
        let Verdict::Ambiguous(refused) = enforce(&snapshot, &server, body.as_bytes()) else {
            panic!();
        };
        assert!(refused.is_empty());
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
            let (mut snapshot_config, server) = (
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

    /// #3527: `fail_open` no longer decides anything. An unreadable body is
    /// refused in an organization that saved it, and a readable refused call
    /// stays refused.
    #[test]
    fn an_unreadable_body_is_refused_whatever_fail_open_says() {
        let garbage = b"{ not json";
        for fail_open in [false, true] {
            let (snapshot, server) = with_manifest(&["a"], Some(settings(false, fail_open)));
            assert!(
                matches!(enforce(&snapshot, &server, garbage), Verdict::Unreadable),
                "{fail_open}"
            );
            assert!(
                matches!(
                    enforce(&snapshot, &server, call("b").as_bytes()),
                    Verdict::Denied(_)
                ),
                "{fail_open}"
            );
        }
    }

    /// #3527: each body a lenient upstream parser might read and `serde_json`
    /// does not is a parser differential, and is refused rather than forwarded.
    #[test]
    fn a_body_only_a_lenient_parser_reads_is_unreadable() {
        let (snapshot, server) = with_manifest(&["a"], Some(settings(false, true)));
        let text = call("b");
        let utf16le: Vec<u8> = "\u{feff}"
            .chars()
            .chain(text.chars())
            .collect::<String>()
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let utf16be: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
        // a BOM is unreadable here: the relay strips it before this layer
        // sees the body (`without_bom`), so a mark that reaches the policy is
        // one the relay did not take off and must not be judged around
        let mut bom = BOM.to_vec();
        bom.extend_from_slice(text.as_bytes());
        let nan = text.replace(r#""arguments":{}"#, r#""arguments":{"x":NaN}"#);
        assert_ne!(nan, text);
        let infinity = text.replace(r#""arguments":{}"#, r#""arguments":{"x":Infinity}"#);
        let trailing = text.replace(r#""arguments":{}"#, r#""arguments":{},"#);
        let comment = format!("/* hi */{text}");
        let deep = format!("{}{}", "[".repeat(1000), "]".repeat(1000));
        for body in [
            utf16le,
            utf16be,
            bom,
            nan.into_bytes(),
            infinity.into_bytes(),
            trailing.into_bytes(),
            comment.into_bytes(),
            deep.into_bytes(),
        ] {
            assert!(
                matches!(enforce(&snapshot, &server, &body), Verdict::Unreadable),
                "{}",
                String::from_utf8_lossy(&body)
            );
        }
    }

    #[test]
    fn only_a_leading_utf8_bom_is_stripped() {
        let text = bytes::Bytes::from_static(b"{\"a\":1}");
        let mut bom = BOM.to_vec();
        bom.extend_from_slice(&text);
        assert_eq!(without_bom(bytes::Bytes::from(bom)), text);
        assert_eq!(without_bom(text.clone()), text);
        // a second mark is the body's own, and still fails to parse
        let mut twice = BOM.repeat(2);
        twice.extend_from_slice(&text);
        assert_eq!(&without_bom(bytes::Bytes::from(twice))[..3], BOM);
        assert_eq!(without_bom(bytes::Bytes::new()), bytes::Bytes::new());
    }
}
