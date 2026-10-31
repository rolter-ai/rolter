//! A small MCP client for the control plane (#1283, #951).
//!
//! The data plane proxies a caller's JSON-RPC to a server; the control plane
//! needs to be a caller itself, to ask a server what tools it has
//! (`tools/list`) and whether it answers at all. This module speaks just
//! enough of the streamable HTTP transport for that: `initialize`,
//! `notifications/initialized` and `tools/list`, with either a JSON or an SSE
//! reply.
//!
//! A server registered with transport `sse` speaks the older HTTP+SSE
//! transport instead (#3299): the client opens a `GET` stream, waits for an
//! `endpoint` event naming where to `POST`, and then reads every reply off that
//! stream, because the `POST`s themselves only answer `202`. The endpoint has to
//! sit on the origin the server was registered with, so a hostile stream cannot
//! point the control plane at an address the egress policy never saw.
//!
//! Every connection is made through [`crate::egress_client::builder`], so the
//! URL is held to the same connect-time egress policy as any other
//! operator-supplied address, and no redirect is followed. Failures are
//! classified into [`FailureKind`] and carry a fixed sentence: the upstream's
//! own text, which can quote a url or a token, never reaches a response.

use std::sync::Arc;
use std::time::Duration;

use rolter_core::EgressPolicy;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// The protocol revision offered in `initialize`. A server answers with the one
/// it will speak, which is recorded as negotiated.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// Most pages of `tools/list` followed before giving up. A server that never
/// stops returning a cursor must not hold the request open.
const MAX_PAGES: usize = 20;

/// Most bytes read from one reply.
const MAX_REPLY_BYTES: usize = 4 << 20;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Why a call to an MCP server did not produce an answer, the states the
/// dashboard distinguishes (#951) plus the policy refusal that precedes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureKind {
    /// the address was refused by the egress policy before any connection
    EgressDenied,
    /// no answer: connection refused, DNS failure, timeout, an unreadable reply
    Unreachable,
    /// the server answered but not as an MCP server does
    HandshakeFailed,
    /// the server answered `401` or `403`
    AuthRejected,
}

impl FailureKind {
    /// Whether a connection was made and something answered. `EgressDenied`
    /// never connected, and `Unreachable` got no answer; the others did.
    pub(crate) fn reached_server(self) -> bool {
        !matches!(self, Self::EgressDenied | Self::Unreachable)
    }

    /// The value stored on the server row and shown by the dashboard.
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::EgressDenied => "egress_denied",
            Self::Unreachable => "unreachable",
            Self::HandshakeFailed => "handshake_failed",
            Self::AuthRejected => "auth_rejected",
        }
    }
}

/// A failed call: a kind and a sentence that is safe to show.
#[derive(Debug, Clone)]
pub(crate) struct Failure {
    pub kind: FailureKind,
    pub message: String,
}

impl Failure {
    fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// How the client authenticates to the server.
#[derive(Clone)]
pub(crate) enum ClientAuth {
    None,
    Bearer(String),
    Header { name: String, value: String },
}

impl std::fmt::Debug for ClientAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => formatter.write_str("None"),
            Self::Bearer(_) => formatter.write_str("Bearer([REDACTED])"),
            Self::Header { name, .. } => write!(formatter, "Header({name}: [REDACTED])"),
        }
    }
}

/// What `initialize` told us about the server.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Handshake {
    pub protocol_version: String,
    pub server_name: Option<String>,
    pub server_version: Option<String>,
}

/// One tool as the server describes it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct DiscoveredTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// The raw JSON-RPC exchange of one call, for the try-it surface.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Exchange {
    pub request: Value,
    pub response: Value,
}

/// The open event stream of a legacy HTTP+SSE server and where to `POST`.
struct Legacy {
    endpoint: String,
    /// `(event name, data)` of each event the stream carried after `endpoint`
    events: mpsc::Receiver<(String, String)>,
    reader: JoinHandle<()>,
}

impl Drop for Legacy {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// A connected, initialized session with one server.
pub(crate) struct McpClient {
    http: reqwest::Client,
    url: String,
    auth: ClientAuth,
    legacy: Option<Legacy>,
    session_id: Option<String>,
    protocol_version: String,
    next_id: u64,
    pub handshake: Handshake,
}

impl McpClient {
    /// Check the address against the egress policy, connect, and run the
    /// `initialize` handshake. `transport` is the server's registered transport:
    /// `sse` opens the legacy event stream first, anything else is streamable
    /// HTTP.
    pub(crate) async fn connect(
        egress: &Arc<EgressPolicy>,
        url: &str,
        auth: ClientAuth,
        transport: &str,
    ) -> Result<Self, Failure> {
        egress.check_url(url, "MCP server URL").map_err(|_| {
            Failure::new(
                FailureKind::EgressDenied,
                "the server address is refused by the egress policy",
            )
        })?;
        let http = crate::egress_client::builder(egress)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|_| {
                Failure::new(
                    FailureKind::Unreachable,
                    "the HTTP client could not be built",
                )
            })?;
        let mut client = Self {
            http,
            url: url.to_string(),
            auth,
            legacy: None,
            session_id: None,
            protocol_version: PROTOCOL_VERSION.to_string(),
            next_id: 1,
            handshake: Handshake::default(),
        };
        if transport == "sse" {
            client.open_legacy_stream().await?;
        }
        let result = client
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "rolter", "version": env!("CARGO_PKG_VERSION")},
                }),
            )
            .await?
            .response;
        let result = result.get("result").ok_or_else(|| {
            Failure::new(
                FailureKind::HandshakeFailed,
                "the server answered initialize with an error",
            )
        })?;
        let negotiated = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Failure::new(
                    FailureKind::HandshakeFailed,
                    "the server's initialize reply names no protocol version",
                )
            })?;
        client.protocol_version = negotiated.to_string();
        client.handshake = Handshake {
            protocol_version: negotiated.to_string(),
            server_name: text(result.pointer("/serverInfo/name")),
            server_version: text(result.pointer("/serverInfo/version")),
        };
        // the spec's second handshake step; a server that does not accept the
        // notification has still answered initialize, so this is best effort
        let _ = client.notify("notifications/initialized").await;
        Ok(client)
    }

    /// Every tool the server lists, following pagination.
    pub(crate) async fn list_tools(&mut self) -> Result<Vec<DiscoveredTool>, Failure> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = match &cursor {
                Some(cursor) => json!({"cursor": cursor}),
                None => json!({}),
            };
            let reply = self.request("tools/list", params).await?.response;
            let result = reply.get("result").ok_or_else(|| {
                Failure::new(
                    FailureKind::HandshakeFailed,
                    "the server answered tools/list with an error",
                )
            })?;
            let page = result
                .get("tools")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    Failure::new(
                        FailureKind::HandshakeFailed,
                        "the server's tools/list reply carries no tools array",
                    )
                })?;
            tools.extend(page.iter().filter_map(parse_tool));
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|next| !next.is_empty())
                .map(str::to_string);
            if cursor.is_none() {
                return Ok(tools);
            }
        }
        Ok(tools)
    }

    /// Run one tool and return the raw exchange. Whether the tool itself
    /// reported an error is the caller's to read from the reply: the call
    /// reached the server and got an answer, which is not a transport failure.
    pub(crate) async fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> Result<Exchange, Failure> {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))
            .await
    }

    async fn notify(&mut self, method: &str) -> Result<(), Failure> {
        let body = json!({"jsonrpc": "2.0", "method": method});
        self.post(&body).await.map(|_| ())
    }

    /// Send a JSON-RPC request and read its reply.
    pub(crate) async fn request(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Exchange, Failure> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let response = self.post(&request).await?;
        let reply = response.ok_or_else(|| {
            Failure::new(FailureKind::HandshakeFailed, "the server sent no reply")
        })?;
        Ok(Exchange {
            request,
            response: reply,
        })
    }

    /// Open the legacy event stream and wait for the `endpoint` event.
    async fn open_legacy_stream(&mut self) -> Result<(), Failure> {
        let base = reqwest::Url::parse(&self.url).map_err(|_| {
            Failure::new(
                FailureKind::HandshakeFailed,
                "the server address is invalid",
            )
        })?;
        let request = self
            .http
            .get(&self.url)
            .header("accept", "text/event-stream");
        let mut response = tokio::time::timeout(REQUEST_TIMEOUT, self.authorize(request).send())
            .await
            .map_err(|_| {
                Failure::new(
                    FailureKind::Unreachable,
                    "the server did not answer in time",
                )
            })?
            .map_err(|error| send_failure(&error))?;
        check_status(response.status())?;
        let is_stream = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
        if !is_stream {
            return Err(Failure::new(
                FailureKind::HandshakeFailed,
                "the server did not open an event stream",
            ));
        }
        let (tx, mut events) = mpsc::channel::<(String, String)>(64);
        let reader = tokio::spawn(async move {
            let mut buffer: Vec<u8> = Vec::new();
            while let Ok(Some(chunk)) = response.chunk().await {
                buffer.extend_from_slice(&chunk);
                if buffer.len() > MAX_REPLY_BYTES {
                    return;
                }
                while let Some(event) = take_event(&mut buffer) {
                    if tx.send(event).await.is_err() {
                        return;
                    }
                }
            }
        });
        let mut legacy = Legacy {
            endpoint: String::new(),
            events: mpsc::channel(1).1,
            reader,
        };
        let endpoint = tokio::time::timeout(REQUEST_TIMEOUT, async {
            while let Some((name, data)) = events.recv().await {
                if name == "endpoint" {
                    return Some(data);
                }
            }
            None
        })
        .await
        .map_err(|_| {
            Failure::new(
                FailureKind::HandshakeFailed,
                "the server sent no endpoint event",
            )
        })?
        .ok_or_else(|| {
            Failure::new(
                FailureKind::HandshakeFailed,
                "the event stream closed before an endpoint event",
            )
        })?;
        let resolved = base
            .join(endpoint.trim())
            .ok()
            .filter(|resolved| resolved.origin() == base.origin())
            .ok_or_else(|| {
                Failure::new(
                    FailureKind::HandshakeFailed,
                    "the server's endpoint event names an address that is not on its own origin",
                )
            })?;
        legacy.endpoint = resolved.into();
        legacy.events = events;
        self.legacy = Some(legacy);
        Ok(())
    }

    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth {
            ClientAuth::None => request,
            ClientAuth::Bearer(token) => request.bearer_auth(token),
            ClientAuth::Header { name, value } => request.header(name, value),
        }
    }

    /// One message over the legacy transport: `POST` it to the endpoint, then
    /// read its reply off the event stream.
    async fn post_legacy(&mut self, body: &Value) -> Result<Option<Value>, Failure> {
        let Some(endpoint) = self.legacy.as_ref().map(|legacy| legacy.endpoint.clone()) else {
            return Err(Failure::new(
                FailureKind::HandshakeFailed,
                "the event stream is not open",
            ));
        };
        let request = self
            .http
            .post(&endpoint)
            .timeout(REQUEST_TIMEOUT)
            .header("content-type", "application/json")
            .body(body.to_string());
        let response = self
            .authorize(request)
            .send()
            .await
            .map_err(|error| send_failure(&error))?;
        check_status(response.status())?;
        if body.get("id").is_none() {
            return Ok(None);
        }
        let id = body.get("id").cloned().unwrap_or(Value::Null);
        let Some(legacy) = self.legacy.as_mut() else {
            return Ok(None);
        };
        let wait = async {
            while let Some((name, data)) = legacy.events.recv().await {
                if name != "message" {
                    continue;
                }
                if let Ok(message) = serde_json::from_str::<Value>(&data) {
                    if message.get("id") == Some(&id) {
                        return Some(message);
                    }
                }
            }
            None
        };
        match tokio::time::timeout(REQUEST_TIMEOUT, wait).await {
            Ok(Some(reply)) => Ok(Some(reply)),
            Ok(None) => Err(Failure::new(
                FailureKind::Unreachable,
                "the event stream closed before the reply arrived",
            )),
            Err(_) => Err(Failure::new(
                FailureKind::Unreachable,
                "the server did not answer in time",
            )),
        }
    }

    async fn post(&mut self, body: &Value) -> Result<Option<Value>, Failure> {
        if self.legacy.is_some() {
            return self.post_legacy(body).await;
        }
        let wants_reply = body.get("id").is_some();
        let mut request = self
            .http
            .post(&self.url)
            .timeout(REQUEST_TIMEOUT)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", &self.protocol_version)
            .body(body.to_string());
        if let Some(session) = &self.session_id {
            request = request.header("mcp-session-id", session);
        }
        let mut response = self
            .authorize(request)
            .send()
            .await
            .map_err(|error| send_failure(&error))?;
        check_status(response.status())?;
        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
        {
            self.session_id = Some(session.to_string());
        }
        if !wants_reply {
            return Ok(None);
        }
        let id = body.get("id").cloned().unwrap_or(Value::Null);
        let is_sse = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
        let mut buffer: Vec<u8> = Vec::new();
        loop {
            let chunk = response.chunk().await.map_err(|_| {
                Failure::new(
                    FailureKind::Unreachable,
                    "the connection dropped while the reply was read",
                )
            })?;
            let Some(chunk) = chunk else { break };
            buffer.extend_from_slice(&chunk);
            if buffer.len() > MAX_REPLY_BYTES {
                return Err(Failure::new(
                    FailureKind::HandshakeFailed,
                    "the server's reply is larger than the limit",
                ));
            }
            // an SSE stream can stay open after its reply, so stop as soon as
            // the reply to this request has arrived
            if is_sse {
                if let Some(found) = find_sse_reply(&buffer, &id) {
                    return Ok(Some(found));
                }
            }
        }
        let parsed = if is_sse {
            find_sse_reply(&buffer, &id)
        } else {
            serde_json::from_slice::<Value>(&buffer).ok()
        };
        parsed.map(Some).ok_or_else(|| {
            Failure::new(
                FailureKind::HandshakeFailed,
                "the server's reply is not a JSON-RPC message",
            )
        })
    }
}

/// Classify a failed send. The error text quotes the url, which can carry a
/// token, so only the class is kept.
fn send_failure(error: &reqwest::Error) -> Failure {
    if refused_by_egress_policy(error) {
        return Failure::new(
            FailureKind::EgressDenied,
            "the server address is refused by the egress policy",
        );
    }
    let message = if error.is_timeout() {
        "the server did not answer in time"
    } else {
        "the server could not be reached"
    };
    Failure::new(FailureKind::Unreachable, message)
}

fn check_status(status: reqwest::StatusCode) -> Result<(), Failure> {
    if matches!(status.as_u16(), 401 | 403) {
        return Err(Failure::new(
            FailureKind::AuthRejected,
            format!(
                "the server rejected the credential with HTTP {}",
                status.as_u16()
            ),
        ));
    }
    if !status.is_success() {
        return Err(Failure::new(
            FailureKind::HandshakeFailed,
            format!("the server answered HTTP {}", status.as_u16()),
        ));
    }
    Ok(())
}

/// The first complete SSE event in `buffer` as `(event name, data)`, removed
/// from it. The name defaults to `message`.
fn take_event(buffer: &mut Vec<u8>) -> Option<(String, String)> {
    let end = [&b"\n\n"[..], &b"\r\n\r\n"[..]]
        .iter()
        .filter_map(|terminator| {
            buffer
                .windows(terminator.len())
                .position(|window| window == *terminator)
                .map(|at| at + terminator.len())
        })
        .min()?;
    let raw: Vec<u8> = buffer.drain(..end).collect();
    let text = String::from_utf8_lossy(&raw);
    let mut name = String::from("message");
    let mut data = Vec::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            name = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.strip_prefix(' ').unwrap_or(value).to_string());
        }
    }
    Some((name, data.join("\n")))
}

/// Whether the connect-time resolver refused the address, which it reports as
/// the cause of a connect error. A name that passed the saved-URL check can
/// still resolve to a denied address (or be rebound to one later).
fn refused_by_egress_policy(error: &reqwest::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(cause) = source {
        if cause.to_string().contains("the egress policy denies") {
            return true;
        }
        source = cause.source();
    }
    false
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(|text| text.chars().take(200).collect())
}

fn parse_tool(value: &Value) -> Option<DiscoveredTool> {
    let name = value.get("name")?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    Some(DiscoveredTool {
        name: name.to_string(),
        description: value
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        input_schema: value
            .get("inputSchema")
            .cloned()
            .unwrap_or_else(|| json!({"type": "object"})),
    })
}

/// The JSON-RPC message in an SSE body whose `id` is `id`, once its event has
/// been fully received (terminated by a blank line).
fn find_sse_reply(buffer: &[u8], id: &Value) -> Option<Value> {
    let text = String::from_utf8_lossy(buffer).replace("\r\n", "\n");
    for event in text.split("\n\n") {
        let data: String = event
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim_start)
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            continue;
        }
        if let Ok(message) = serde_json::from_str::<Value>(&data) {
            if message.get("id") == Some(id) {
                return Some(message);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::http::{header, HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::Router;
    use parking_lot::Mutex;

    /// a request the mock saw: its method and the `Authorization` it carried
    type Seen = (String, Option<String>);

    #[derive(Clone, Default)]
    struct Server {
        seen: Arc<Mutex<Vec<Seen>>>,
        sse: bool,
        status: Option<u16>,
        tools: Vec<Value>,
        pages: bool,
    }

    async fn handle(
        State(server): State<Server>,
        headers: HeaderMap,
        body: String,
    ) -> axum::response::Response {
        let message: Value = serde_json::from_str(&body).unwrap();
        let method = message["method"].as_str().unwrap().to_string();
        let auth = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        server.seen.lock().push((method.clone(), auth));
        if let Some(status) = server.status {
            return StatusCode::from_u16(status).unwrap().into_response();
        }
        let id = message.get("id").cloned();
        let result = match method.as_str() {
            "initialize" => json!({
                "protocolVersion": "2025-03-26",
                "serverInfo": {"name": "mock", "version": "1.2.3"},
            }),
            "tools/list" if server.pages => {
                if message["params"].get("cursor").is_some() {
                    json!({"tools": [server.tools[1].clone()]})
                } else {
                    json!({"tools": [server.tools[0].clone()], "nextCursor": "page-2"})
                }
            }
            "tools/list" => json!({"tools": server.tools}),
            "tools/call" => json!({"content": [{"type": "text", "text": "ok"}], "isError": false}),
            _ => return StatusCode::ACCEPTED.into_response(),
        };
        let reply = json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string();
        let mut response = if server.sse {
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                format!("event: message\ndata: {reply}\n\n"),
            )
                .into_response()
        } else {
            ([(header::CONTENT_TYPE, "application/json")], reply).into_response()
        };
        response
            .headers_mut()
            .insert("mcp-session-id", "session-1".parse().unwrap());
        response
    }

    async fn serve(server: Server) -> String {
        let app = Router::new().route("/mcp", post(handle)).with_state(server);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/mcp")
    }

    fn policy() -> Arc<EgressPolicy> {
        Arc::new(EgressPolicy::default())
    }

    fn tool(name: &str) -> Value {
        json!({"name": name, "description": format!("{name} tool"), "inputSchema": {"type": "object"}})
    }

    #[tokio::test]
    async fn lists_tools_with_their_schemas_over_json() {
        let server = Server {
            tools: vec![tool("a"), tool("b")],
            ..Default::default()
        };
        let seen = server.seen.clone();
        let url = serve(server).await;
        let mut client = McpClient::connect(
            &policy(),
            &url,
            ClientAuth::Bearer("tok".into()),
            "streamable_http",
        )
        .await
        .unwrap();
        assert_eq!(client.handshake.protocol_version, "2025-03-26");
        assert_eq!(client.handshake.server_name.as_deref(), Some("mock"));
        let tools = client.list_tools().await.unwrap();
        assert_eq!(
            tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(tools[0].description, "a tool");
        let methods: Vec<_> = seen.lock().iter().map(|(m, _)| m.clone()).collect();
        assert_eq!(
            methods,
            ["initialize", "notifications/initialized", "tools/list"]
        );
        assert!(seen
            .lock()
            .iter()
            .all(|(_, auth)| auth.as_deref() == Some("Bearer tok")));
    }

    #[tokio::test]
    async fn reads_an_sse_reply_and_follows_pagination() {
        let url = serve(Server {
            sse: true,
            pages: true,
            tools: vec![tool("a"), tool("b")],
            ..Default::default()
        })
        .await;
        let mut client = McpClient::connect(&policy(), &url, ClientAuth::None, "streamable_http")
            .await
            .unwrap();
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 2);
    }

    #[tokio::test]
    async fn an_unauthorized_server_is_auth_rejected() {
        let url = serve(Server {
            status: Some(401),
            ..Default::default()
        })
        .await;
        let failure = McpClient::connect(&policy(), &url, ClientAuth::None, "streamable_http")
            .await
            .err()
            .unwrap();
        assert_eq!(failure.kind, FailureKind::AuthRejected);
    }

    #[tokio::test]
    async fn a_non_mcp_server_is_a_handshake_failure() {
        let url = serve(Server {
            status: Some(404),
            ..Default::default()
        })
        .await;
        let failure = McpClient::connect(&policy(), &url, ClientAuth::None, "streamable_http")
            .await
            .err()
            .unwrap();
        assert_eq!(failure.kind, FailureKind::HandshakeFailed);
    }

    #[tokio::test]
    async fn a_closed_port_is_unreachable_and_never_echoes_the_url() {
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let failure = McpClient::connect(
            &policy(),
            &format!("http://u:secret-token@{closed}/mcp?key=secret-token"),
            ClientAuth::None,
            "streamable_http",
        )
        .await
        .err()
        .unwrap();
        assert_eq!(failure.kind, FailureKind::Unreachable);
        assert!(!failure.message.contains("secret-token"));
    }

    #[tokio::test]
    async fn the_egress_policy_refuses_before_any_connection() {
        let listener = crate::egress_client::testing::Counter::start().await;
        let denied = crate::egress_client::testing::deny_loopback();
        let failure = McpClient::connect(
            &denied,
            &listener.url("/mcp"),
            ClientAuth::None,
            "streamable_http",
        )
        .await
        .err()
        .unwrap();
        assert_eq!(failure.kind, FailureKind::EgressDenied);
        assert_eq!(listener.accepted(), 0);
    }

    #[tokio::test]
    async fn calls_a_tool_and_returns_the_raw_exchange() {
        let url = serve(Server {
            tools: vec![tool("a")],
            ..Default::default()
        })
        .await;
        let mut client = McpClient::connect(&policy(), &url, ClientAuth::None, "streamable_http")
            .await
            .unwrap();
        let exchange = client.call_tool("a", json!({"x": 1})).await.unwrap();
        assert_eq!(exchange.request["params"]["name"], "a");
        assert_eq!(exchange.request["params"]["arguments"]["x"], 1);
        assert_eq!(exchange.response["result"]["content"][0]["text"], "ok");
    }

    /// A stub of the legacy HTTP+SSE transport: `GET /sse` opens the stream and
    /// names `/messages`, `POST /messages` answers `202` and the reply goes out
    /// on the stream.
    #[derive(Clone)]
    struct Legacy {
        replies: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<String>>>>,
        send: tokio::sync::mpsc::UnboundedSender<String>,
        seen: Arc<Mutex<Vec<Seen>>>,
        endpoint: String,
        tools: Vec<Value>,
    }

    async fn legacy_stream(State(legacy): State<Legacy>) -> axum::response::Response {
        let Some(receiver) = legacy.replies.lock().take() else {
            return StatusCode::CONFLICT.into_response();
        };
        let first = format!("event: endpoint\ndata: {}\n\n", legacy.endpoint);
        let events = futures_util::stream::unfold(Some((receiver, Some(first))), |state| async {
            let (mut receiver, first) = state?;
            if let Some(first) = first {
                return Some((Ok::<_, std::io::Error>(first), Some((receiver, None))));
            }
            let reply = receiver.recv().await?;
            Some((
                Ok(format!("event: message\ndata: {reply}\n\n")),
                Some((receiver, None)),
            ))
        });
        (
            [(header::CONTENT_TYPE, "text/event-stream")],
            axum::body::Body::from_stream(events),
        )
            .into_response()
    }

    async fn legacy_post(
        State(legacy): State<Legacy>,
        headers: HeaderMap,
        body: String,
    ) -> StatusCode {
        let message: Value = serde_json::from_str(&body).unwrap();
        let method = message["method"].as_str().unwrap().to_string();
        let auth = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        legacy.seen.lock().push((method.clone(), auth));
        let result = match method.as_str() {
            "initialize" => json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "legacy", "version": "0.1"},
            }),
            "tools/list" => json!({"tools": legacy.tools}),
            _ => return StatusCode::ACCEPTED,
        };
        let reply = json!({"jsonrpc": "2.0", "id": message["id"], "result": result});
        let _ = legacy.send.send(reply.to_string());
        StatusCode::ACCEPTED
    }

    async fn serve_legacy(endpoint: &str, tools: Vec<Value>) -> (String, Arc<Mutex<Vec<Seen>>>) {
        let (send, receiver) = tokio::sync::mpsc::unbounded_channel();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let legacy = Legacy {
            replies: Arc::new(Mutex::new(Some(receiver))),
            send,
            seen: seen.clone(),
            endpoint: endpoint.to_string(),
            tools,
        };
        let app = Router::new()
            .route("/sse", axum::routing::get(legacy_stream))
            .route("/messages", post(legacy_post))
            .with_state(legacy);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/sse"), seen)
    }

    #[tokio::test]
    async fn a_legacy_sse_server_is_discovered_over_its_event_stream() {
        let (url, seen) = serve_legacy("/messages?session=abc", vec![tool("a"), tool("b")]).await;
        let mut client =
            McpClient::connect(&policy(), &url, ClientAuth::Bearer("tok".into()), "sse")
                .await
                .unwrap();
        assert_eq!(client.handshake.protocol_version, "2024-11-05");
        assert_eq!(client.handshake.server_name.as_deref(), Some("legacy"));
        let tools = client.list_tools().await.unwrap();
        assert_eq!(
            tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        let methods: Vec<_> = seen.lock().iter().map(|(m, _)| m.clone()).collect();
        assert_eq!(
            methods,
            ["initialize", "notifications/initialized", "tools/list"]
        );
        assert!(seen
            .lock()
            .iter()
            .all(|(_, auth)| auth.as_deref() == Some("Bearer tok")));
    }

    #[tokio::test]
    async fn the_same_server_asked_the_streamable_way_fails_its_handshake() {
        let (url, _) = serve_legacy("/messages", vec![]).await;
        // the legacy stub has no POST on the registered address
        let failure = McpClient::connect(&policy(), &url, ClientAuth::None, "streamable_http")
            .await
            .err()
            .unwrap();
        assert_eq!(failure.kind, FailureKind::HandshakeFailed);
    }

    #[tokio::test]
    async fn an_endpoint_on_another_origin_is_refused() {
        let (url, seen) = serve_legacy("http://127.0.0.1:9/messages", vec![]).await;
        let failure = McpClient::connect(&policy(), &url, ClientAuth::None, "sse")
            .await
            .err()
            .unwrap();
        assert_eq!(failure.kind, FailureKind::HandshakeFailed);
        assert!(failure.message.contains("own origin"));
        assert!(seen.lock().is_empty(), "nothing was sent to it");
    }

    #[test]
    fn only_a_refused_or_silent_server_never_answered() {
        assert!(!FailureKind::EgressDenied.reached_server());
        assert!(!FailureKind::Unreachable.reached_server());
        assert!(FailureKind::AuthRejected.reached_server());
        assert!(FailureKind::HandshakeFailed.reached_server());
    }

    #[test]
    fn auth_debug_never_prints_the_secret() {
        let shown = format!(
            "{:?} {:?}",
            ClientAuth::Bearer("hunter2".into()),
            ClientAuth::Header {
                name: "x-api-key".into(),
                value: "hunter2".into()
            }
        );
        assert!(!shown.contains("hunter2"));
    }
}
