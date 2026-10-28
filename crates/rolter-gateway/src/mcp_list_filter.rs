//! Filtering of `tools/list` replies to the tools a caller may call (#3297).
//!
//! The tool policy ([`crate::mcp_policy`]) refuses a `tools/call` outside the
//! organization's tool groups or a server's declared manifest. Without this a
//! client would still be *shown* every tool the server has and be refused on
//! the first call. The reply to a `tools/list` request is therefore rewritten to
//! hold only the tools the same rules would let through.
//!
//! Only replies to a `tools/list` request are touched, and only when a rule
//! applies to the server at all; every other request streams through unread.
//! A reply is matched to its request by JSON-RPC id, so a batch holding a
//! `tools/list` beside other calls filters only that answer. Pagination needs
//! nothing special: each page is its own reply and is filtered on its own, with
//! its `nextCursor` left alone (a page can end up holding fewer tools than the
//! server's page size, or none, which the protocol allows).
//!
//! A JSON reply is buffered up to [`MAX_LIST_REPLY`] and an SSE reply is
//! rewritten one event at a time, so a long-lived stream is not held back.

// spec-ref: json-rpc, mcp-transport
use std::collections::{HashSet, VecDeque};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::Stream;
use serde_json::Value;

/// The most of a JSON `tools/list` reply the gateway holds in memory. A reply
/// larger than this is refused rather than forwarded unfiltered.
pub(crate) const MAX_LIST_REPLY: usize = 16 * 1024 * 1024;

/// What a `tools/list` reply is narrowed to.
#[derive(Debug)]
pub(crate) struct ListFilter {
    /// the tools the enabled groups allow, when any names the server
    governed: Option<HashSet<String>>,
    /// the server's declared manifest, when calls must stay inside it
    declared: Option<Vec<String>>,
    /// the ids of the `tools/list` requests in the body; the replies carrying
    /// one of these are filtered
    ids: Vec<Value>,
}

impl ListFilter {
    /// A filter for a request body, or `None` when it holds no `tools/list`
    /// request that expects an answer.
    pub(crate) fn new(
        governed: Option<HashSet<String>>,
        declared: Option<Vec<String>>,
        body: &[u8],
    ) -> Option<Self> {
        let ids = list_ids(body);
        (!ids.is_empty()).then_some(Self {
            governed,
            declared,
            ids,
        })
    }

    /// Whether the policy would let a call for `tool` through.
    fn allows(&self, tool: &str) -> bool {
        crate::mcp_policy::judge(self.governed.as_ref(), self.declared.as_deref(), Some(tool))
            .is_none()
    }

    /// Narrow one reply message in place; whether it was a `tools/list` answer.
    fn apply(&self, message: &mut Value) -> bool {
        let Some(object) = message.as_object_mut() else {
            return false;
        };
        if !object.get("id").is_some_and(|id| self.ids.contains(id)) {
            return false;
        }
        let Some(tools) = object
            .get_mut("result")
            .and_then(|result| result.get_mut("tools"))
            .and_then(Value::as_array_mut)
        else {
            return false;
        };
        // a tool with no readable name cannot be matched to a rule
        tools.retain(|tool| {
            tool.get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| self.allows(name))
        });
        true
    }

    /// Narrow a whole JSON reply, a message or a batch. `None` when the body is
    /// not JSON, which the caller must treat as unfilterable.
    pub(crate) fn filter_json(&self, body: &[u8]) -> Option<Vec<u8>> {
        let mut value: Value = serde_json::from_slice(body).ok()?;
        match &mut value {
            Value::Array(items) => {
                for item in items {
                    self.apply(item);
                }
            }
            other => {
                self.apply(other);
            }
        }
        serde_json::to_vec(&value).ok()
    }

    /// Rewrite one SSE event if it carries a `tools/list` answer; every other
    /// event comes back byte for byte.
    fn filter_event(&self, event: &[u8]) -> Vec<u8> {
        let Ok(text) = std::str::from_utf8(event) else {
            return event.to_vec();
        };
        let mut data = String::new();
        let mut others = Vec::new();
        for line in text.lines() {
            match line.strip_prefix("data:") {
                Some(rest) => {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                }
                None if line.is_empty() => {}
                None => others.push(line),
            }
        }
        let Ok(mut message) = serde_json::from_str::<Value>(&data) else {
            return event.to_vec();
        };
        let touched = match &mut message {
            Value::Array(items) => items.iter_mut().fold(false, |any, m| self.apply(m) | any),
            other => self.apply(other),
        };
        if !touched {
            return event.to_vec();
        }
        let Ok(json) = serde_json::to_string(&message) else {
            return event.to_vec();
        };
        let mut out = String::new();
        for line in others {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("data: ");
        out.push_str(&json);
        out.push_str("\n\n");
        out.into_bytes()
    }
}

/// The ids of the `tools/list` requests in a body that expect an answer.
fn list_ids(body: &[u8]) -> Vec<Value> {
    let Ok(parsed) = serde_json::from_slice::<Value>(body) else {
        return Vec::new();
    };
    let messages = match parsed {
        Value::Array(items) => items,
        other => vec![other],
    };
    messages
        .into_iter()
        .filter(|message| message.get("method").and_then(Value::as_str) == Some("tools/list"))
        .filter_map(|message| message.get("id").cloned())
        .filter(|id| !id.is_null())
        .collect()
}

/// The first complete SSE event in `buf`, terminator included.
fn take_event(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    let end = [&b"\n\n"[..], &b"\r\n\r\n"[..]]
        .iter()
        .filter_map(|terminator| {
            buf.windows(terminator.len())
                .position(|window| window == *terminator)
                .map(|at| at + terminator.len())
        })
        .min()?;
    Some(buf.drain(..end).collect())
}

/// An SSE response body with its `tools/list` answers narrowed, one event at a
/// time.
pub(crate) struct FilteredSse<S> {
    inner: S,
    filter: Arc<ListFilter>,
    pending: Vec<u8>,
    ready: VecDeque<Bytes>,
    finished: bool,
}

impl<S> FilteredSse<S> {
    pub(crate) fn new(inner: S, filter: Arc<ListFilter>) -> Self {
        Self {
            inner,
            filter,
            pending: Vec::new(),
            ready: VecDeque::new(),
            finished: false,
        }
    }
}

impl<S, E> Stream for FilteredSse<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    type Item = Result<Bytes, E>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(chunk) = self.ready.pop_front() {
                return Poll::Ready(Some(Ok(chunk)));
            }
            if self.finished {
                return Poll::Ready(None);
            }
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Some(Err(error))),
                Poll::Ready(Some(Ok(chunk))) => {
                    self.pending.extend_from_slice(&chunk);
                    while let Some(event) = take_event(&mut self.pending) {
                        let rewritten = self.filter.filter_event(&event);
                        self.ready.push_back(Bytes::from(rewritten));
                    }
                }
                Poll::Ready(None) => {
                    self.finished = true;
                    // an event the stream ended in the middle of
                    if !self.pending.is_empty() {
                        let rest = std::mem::take(&mut self.pending);
                        let rewritten = self.filter.filter_event(&rest);
                        self.ready.push_back(Bytes::from(rewritten));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    fn filter(allowed: &[&str]) -> ListFilter {
        ListFilter::new(
            Some(allowed.iter().map(|tool| (*tool).to_string()).collect()),
            None,
            br#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#,
        )
        .expect("a tools/list request")
    }

    fn reply(id: u64, names: &[&str], cursor: Option<&str>) -> String {
        let tools: Vec<Value> = names
            .iter()
            .map(|name| serde_json::json!({"name": name, "inputSchema": {}}))
            .collect();
        let mut result = serde_json::json!({ "tools": tools });
        if let Some(cursor) = cursor {
            result["nextCursor"] = cursor.into();
        }
        serde_json::json!({"jsonrpc":"2.0","id":id,"result":result}).to_string()
    }

    fn names(body: &[u8]) -> Vec<String> {
        let value: Value = serde_json::from_slice(body).unwrap();
        value["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn only_a_tools_list_request_with_an_id_makes_a_filter() {
        for body in [
            &br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}}"#[..],
            br#"{"jsonrpc":"2.0","method":"tools/list"}"#,
            br#"not json"#,
        ] {
            assert!(ListFilter::new(None, None, body).is_none());
        }
        assert!(ListFilter::new(
            None,
            None,
            br#"[{"jsonrpc":"2.0","id":"a","method":"tools/list"}]"#
        )
        .is_some());
    }

    #[test]
    fn a_json_reply_keeps_only_the_allowed_tools_and_its_cursor() {
        let filtered = filter(&["get_issue"])
            .filter_json(reply(7, &["get_issue", "drop"], Some("page-2")).as_bytes())
            .unwrap();
        assert_eq!(names(&filtered), ["get_issue"]);
        let value: Value = serde_json::from_slice(&filtered).unwrap();
        assert_eq!(value["result"]["nextCursor"], "page-2");
    }

    #[test]
    fn each_page_is_filtered_on_its_own() {
        let filter = filter(&["a", "c"]);
        let first = filter
            .filter_json(reply(7, &["a", "b"], Some("2")).as_bytes())
            .unwrap();
        let second = filter
            .filter_json(reply(7, &["c", "d"], None).as_bytes())
            .unwrap();
        assert_eq!(
            (names(&first), names(&second)),
            (vec!["a".to_string()], vec!["c".to_string()])
        );
    }

    #[test]
    fn a_reply_that_answers_another_request_is_left_alone() {
        let filter = filter(&["a"]);
        let other = reply(8, &["a", "b"], None);
        assert_eq!(
            names(&filter.filter_json(other.as_bytes()).unwrap()),
            ["a", "b"]
        );
    }

    #[test]
    fn a_batch_reply_filters_only_the_tools_list_answer() {
        let filter = filter(&["a"]);
        let batch = format!(
            r#"[{},{{"jsonrpc":"2.0","id":9,"result":{{"content":[]}}}}]"#,
            reply(7, &["a", "b"], None)
        );
        let out: Value =
            serde_json::from_slice(&filter.filter_json(batch.as_bytes()).unwrap()).unwrap();
        assert_eq!(out[0]["result"]["tools"].as_array().unwrap().len(), 1);
        assert_eq!(out[1]["result"]["content"], serde_json::json!([]));
    }

    #[test]
    fn an_error_reply_and_a_non_json_body_are_not_rewritten() {
        let filter = filter(&["a"]);
        let error = r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32601,"message":"no"}}"#;
        let out: Value =
            serde_json::from_slice(&filter.filter_json(error.as_bytes()).unwrap()).unwrap();
        assert_eq!(out["error"]["code"], -32601);
        assert!(filter.filter_json(b"<html>").is_none());
    }

    async fn collect(filter: ListFilter, chunks: Vec<&'static str>) -> String {
        let stream = futures_util::stream::iter(
            chunks
                .into_iter()
                .map(|chunk| Ok::<_, std::io::Error>(Bytes::from(chunk))),
        );
        let out: Vec<_> = FilteredSse::new(stream, Arc::new(filter)).collect().await;
        out.into_iter()
            .map(|chunk| String::from_utf8(chunk.unwrap().to_vec()).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn an_sse_answer_is_rewritten_and_other_events_pass_byte_for_byte() {
        let answer = reply(7, &["a", "b"], None);
        let event = format!("event: message\nid: 4\ndata: {answer}\n\n");
        let chunks: Vec<&'static str> = vec![
            ": keep-alive\n\n",
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n",
            Box::leak(event.into_boxed_str()),
        ];
        let out = collect(filter(&["a"]), chunks).await;
        assert!(out.starts_with(
            ": keep-alive\n\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n"
        ));
        let tail = out.split("event: message\n").nth(1).unwrap();
        assert!(tail.starts_with("id: 4\ndata: "));
        let json = tail.trim_end().split("data: ").nth(1).unwrap();
        assert_eq!(names(json.as_bytes()), ["a"]);
    }

    #[tokio::test]
    async fn an_sse_event_split_across_chunks_and_crlf_framing_are_handled() {
        let answer = reply(7, &["a", "b"], None);
        let (head, tail) = answer.split_at(20);
        let first: &'static str = Box::leak(format!("data: {head}").into_boxed_str());
        let second: &'static str = Box::leak(format!("{tail}\r\n\r\n").into_boxed_str());
        let out = collect(filter(&["a"]), vec![first, second]).await;
        assert_eq!(
            names(out.trim().trim_start_matches("data: ").as_bytes()),
            ["a"]
        );
    }
}
