//! Reverse proxy for `/gw/*` → the rolter-gateway data plane.
//!
//! The dashboard is served by the control plane, but the Playground calls the
//! gateway's OpenAI-compatible surface (`/v1/chat/completions`, `/v1/embeddings`,
//! `/v1/images/generations`, `/v1/audio/*`, `/v1/realtime`). Browsers can't reach
//! the gateway cross-origin (no CORS layer there), so the control plane forwards
//! `/gw/*` to it — HTTP (including SSE streaming) and the realtime WebSocket.
//!
//! No admin-token gate: the gateway authenticates every call with a virtual key,
//! so `/gw` exposes nothing the gateway doesn't already expose itself.

use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message as UpstreamMessage},
};

use super::ControlState;

// generous ceiling so audio uploads (/v1/audio/transcriptions) pass through,
// while still bounding memory per request
const MAX_BODY: usize = 32 * 1024 * 1024;

// hop-by-hop headers must not be forwarded end-to-end (RFC 7230 §6.1); host and
// content-length are recomputed by the client/server on each leg.
const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
];

pub(crate) fn router() -> Router<ControlState> {
    Router::new().route("/gw/{*path}", any(proxy))
}

// a valid `Upgrade: websocket` request yields a `WebSocketUpgrade` (extracted
// from the request parts); everything else is a plain HTTP proxy. Both share
// the `/gw/{*path}` route.
async fn proxy(State(state): State<ControlState>, req: Request) -> Response {
    let (mut parts, body) = req.into_parts();
    let path = parts
        .uri
        .path()
        .strip_prefix("/gw")
        .unwrap_or_default()
        .to_string();
    let query = parts
        .uri
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();

    let ws = WebSocketUpgrade::from_request_parts(&mut parts, &state)
        .await
        .ok();
    if let Some(ws) = ws {
        return proxy_ws(state, ws, &parts.headers, &path, &query);
    }
    proxy_http(state, parts.method, parts.headers, body, &path, &query).await
}

async fn proxy_http(
    state: ControlState,
    method: Method,
    mut headers: HeaderMap,
    body: Body,
    path: &str,
    query: &str,
) -> Response {
    let url = format!("{}{path}{query}", state.gateway_url);

    let bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(bytes) => bytes,
        Err(_) => return gw_error(StatusCode::PAYLOAD_TOO_LARGE, "request body too large"),
    };

    strip_hop_by_hop(&mut headers);
    headers.remove(header::HOST);
    headers.remove(header::CONTENT_LENGTH);

    let upstream = state
        .http
        .request(method, &url)
        .headers(headers)
        .body(bytes)
        .send()
        .await;

    let resp = match upstream {
        Ok(resp) => resp,
        Err(err) => {
            return gw_error(
                StatusCode::BAD_GATEWAY,
                &format!("gateway unreachable: {err}"),
            )
        }
    };

    let status = resp.status();
    let mut out_headers = resp.headers().clone();
    strip_hop_by_hop(&mut out_headers);
    // body is re-streamed, so let axum frame it rather than trusting the
    // upstream length; content-type (e.g. text/event-stream) is preserved
    out_headers.remove(header::CONTENT_LENGTH);

    let stream = resp.bytes_stream();
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    *response.headers_mut() = out_headers;
    response
}

fn proxy_ws(
    state: ControlState,
    ws: WebSocketUpgrade,
    headers: &HeaderMap,
    path: &str,
    query: &str,
) -> Response {
    let url = ws_url(&state.gateway_url, path, query);
    let request = match url.into_client_request() {
        Ok(mut request) => {
            // browsers can't set WS headers, so the virtual key usually rides in
            // the query string; forward an Authorization header too when present
            if let Some(value) = headers.get(header::AUTHORIZATION) {
                request
                    .headers_mut()
                    .insert(header::AUTHORIZATION, value.clone());
            }
            request
        }
        Err(err) => {
            return gw_error(
                StatusCode::BAD_GATEWAY,
                &format!("invalid realtime url: {err}"),
            )
        }
    };
    ws.on_upgrade(move |socket| relay(socket, request))
        .into_response()
}

// bridge the client socket to the gateway socket until either side closes
async fn relay(
    client: WebSocket,
    request: tokio_tungstenite::tungstenite::handshake::client::Request,
) {
    let upstream = match connect_async(request).await {
        Ok((upstream, _)) => upstream,
        Err(_) => {
            // best-effort: drop the client socket, the gateway couldn't be reached
            let _ = client;
            return;
        }
    };
    let (mut client_tx, mut client_rx) = client.split();
    let (mut up_tx, mut up_rx) = upstream.split();

    loop {
        tokio::select! {
            message = client_rx.next() => match message {
                Some(Ok(message)) => {
                    let close = matches!(message, Message::Close(_));
                    if up_tx.send(to_upstream(message)).await.is_err() { break; }
                    if close { break; }
                }
                Some(Err(_)) | None => break,
            },
            message = up_rx.next() => match message {
                Some(Ok(message)) => {
                    let close = matches!(message, UpstreamMessage::Close(_));
                    if client_tx.send(to_client(message)).await.is_err() { break; }
                    if close { break; }
                }
                Some(Err(_)) | None => break,
            },
        }
    }
}

// http(s) gateway base → ws(s) realtime url
fn ws_url(gateway_url: &str, path: &str, query: &str) -> String {
    let base = gateway_url.trim_end_matches('/');
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{ws_base}{path}{query}")
}

fn strip_hop_by_hop(headers: &mut HeaderMap) {
    for name in HOP_BY_HOP {
        headers.remove(name);
    }
}

fn gw_error(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(json!({"error": {"message": message}}))).into_response()
}

fn to_upstream(message: Message) -> UpstreamMessage {
    match message {
        Message::Text(text) => UpstreamMessage::Text(text.to_string().into()),
        Message::Binary(bytes) => UpstreamMessage::Binary(bytes),
        Message::Ping(bytes) => UpstreamMessage::Ping(bytes),
        Message::Pong(bytes) => UpstreamMessage::Pong(bytes),
        Message::Close(_) => UpstreamMessage::Close(None),
    }
}

fn to_client(message: UpstreamMessage) -> Message {
    match message {
        UpstreamMessage::Text(text) => Message::Text(text.to_string().into()),
        UpstreamMessage::Binary(bytes) => Message::Binary(bytes),
        UpstreamMessage::Ping(bytes) => Message::Ping(bytes),
        UpstreamMessage::Pong(bytes) => Message::Pong(bytes),
        UpstreamMessage::Close(_) => Message::Close(None),
        UpstreamMessage::Frame(_) => Message::Close(None),
    }
}

#[cfg(test)]
mod tests {
    use super::ws_url;

    #[test]
    fn builds_ws_url_from_http_base() {
        assert_eq!(
            ws_url(
                "http://localhost:4000",
                "/v1/realtime",
                "?model=gpt-realtime"
            ),
            "ws://localhost:4000/v1/realtime?model=gpt-realtime"
        );
        assert_eq!(
            ws_url("https://gw.example.com/", "/v1/realtime", ""),
            "wss://gw.example.com/v1/realtime"
        );
    }
}
