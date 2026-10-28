//! Filtering of `tools/list` replies to the tools a caller may call (#3297).
//!
//! The tool policy ([`crate::mcp_policy`]) refuses a `tools/call` outside the
//! organization's tool groups or a server's declared manifest. Without this a
//! client would still be *shown* every tool the server has and be refused on
//! the first call. A reply that lists tools is therefore rewritten to hold only
//! the tools the same rules would let through.
//!
//! A reply is recognised by its shape, a `result.tools` array, and never by its
//! JSON-RPC id: ids compare as JSON values (`1` is not `1.0`), a request may
//! carry none or a `null`, and a decoder may read `ID` as `id`, so correlating
//! replies with requests is a way to be bypassed. Whenever a request asked for a
//! tool list, or the caller opened a stream that could carry the reply to one,
//! every message with that shape is narrowed.
//!
//! That covers the three places a reply can come from: the JSON or SSE body of
//! the `POST` that asked, and the `GET` stream of a legacy `sse` server, which
//! answers the `POST` with `202` and delivers the reply on the stream (or a
//! replayed streamable-HTTP stream). Pagination needs nothing special: each page
//! is its own reply and is filtered on its own, with its `nextCursor` left alone
//! (a page can end up holding fewer tools than the server's page size, or none,
//! which the protocol allows).
//!
//! A JSON reply is buffered up to [`MAX_LIST_REPLY`] and an SSE reply is
//! rewritten one event at a time, so a long-lived stream is not held back.

// spec-ref: json-rpc, mcp-transport
use std::collections::{HashSet, VecDeque};
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::Stream;
use serde_json::Value;

/// The most of a JSON `tools/list` reply the gateway holds in memory. A reply
/// larger than this is refused rather than forwarded unfiltered.
pub(crate) const MAX_LIST_REPLY: usize = 16 * 1024 * 1024;

/// The most of one SSE event the filter holds while waiting for its end. A
/// stream with a longer one is cut rather than passed on unfiltered.
const MAX_EVENT: usize = 16 * 1024 * 1024;

/// What a reply that lists tools is narrowed to.
#[derive(Debug)]
pub(crate) struct ListFilter {
    /// the tools the enabled groups allow, when any names the server
    governed: Option<HashSet<String>>,
    /// the server's declared manifest, when calls must stay inside it
    declared: Option<Vec<String>>,
}

impl ListFilter {
    pub(crate) fn new(governed: Option<HashSet<String>>, declared: Option<Vec<String>>) -> Self {
        Self { governed, declared }
    }

    /// Whether the policy would let a call for `tool` through.
    fn allows(&self, tool: &str) -> bool {
        crate::mcp_policy::judge(self.governed.as_ref(), self.declared.as_deref(), Some(tool))
            .is_none()
    }

    /// Narrow one message in place if it carries a tool list; whether it did.
    fn apply(&self, message: &mut Value) -> bool {
        let Some(tools) = message
            .as_object_mut()
            .and_then(|object| object.get_mut("result"))
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

    fn apply_all(&self, value: &mut Value) -> bool {
        match value {
            Value::Array(items) => items.iter_mut().fold(false, |any, m| self.apply(m) | any),
            other => self.apply(other),
        }
    }

    /// Narrow a whole JSON reply, a message or a batch. `None` when the body is
    /// not JSON, which the caller must treat as unfilterable.
    pub(crate) fn filter_json(&self, body: &[u8]) -> Option<Vec<u8>> {
        let mut value: Value = serde_json::from_slice(body).ok()?;
        self.apply_all(&mut value);
        serde_json::to_vec(&value).ok()
    }

    /// Rewrite one SSE event if it carries a tool list; every other event comes
    /// back byte for byte.
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
        if !self.apply_all(&mut message) {
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

/// Finds the end of SSE events in a growing buffer without rescanning it. A
/// line ends with `\r\n`, `\n` or `\r`, and an event ends at the first empty
/// line after at least one line of content.
#[derive(Default)]
struct EventScanner {
    /// how far the buffer has been scanned
    pos: usize,
    /// where the current line began
    line_start: usize,
    /// whether the event so far holds a line
    has_content: bool,
}

impl EventScanner {
    /// The end of the first complete event in `buf`, terminator included, or
    /// `None` when more bytes are needed. The scanner is reset by the caller
    /// after it removes the event.
    fn end(&mut self, buf: &[u8]) -> Option<usize> {
        while self.pos < buf.len() {
            let terminator = match buf[self.pos] {
                b'\n' => 1,
                b'\r' if self.pos + 1 < buf.len() => {
                    if buf[self.pos + 1] == b'\n' {
                        2
                    } else {
                        1
                    }
                }
                // a trailing `\r` may be the first half of `\r\n`
                b'\r' => return None,
                _ => {
                    self.pos += 1;
                    continue;
                }
            };
            let empty = self.pos == self.line_start;
            self.pos += terminator;
            self.line_start = self.pos;
            if empty && self.has_content {
                return Some(self.pos);
            }
            self.has_content |= !empty;
        }
        None
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// An SSE response body with its tool lists narrowed, one event at a time.
pub(crate) struct FilteredSse<S> {
    inner: S,
    filter: Arc<ListFilter>,
    pending: Vec<u8>,
    scanner: EventScanner,
    ready: VecDeque<Bytes>,
    finished: bool,
}

impl<S> FilteredSse<S> {
    pub(crate) fn new(inner: S, filter: Arc<ListFilter>) -> Self {
        Self {
            inner,
            filter,
            pending: Vec::new(),
            scanner: EventScanner::default(),
            ready: VecDeque::new(),
            finished: false,
        }
    }
}

impl<S, E> Stream for FilteredSse<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Item = Result<Bytes, io::Error>;

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
                Poll::Ready(Some(Err(error))) => {
                    self.finished = true;
                    return Poll::Ready(Some(Err(io::Error::other(error))));
                }
                Poll::Ready(Some(Ok(chunk))) => {
                    self.pending.extend_from_slice(&chunk);
                    loop {
                        let this = &mut *self;
                        let Some(end) = this.scanner.end(&this.pending) else {
                            break;
                        };
                        let event: Vec<u8> = this.pending.drain(..end).collect();
                        this.scanner.reset();
                        let rewritten = this.filter.filter_event(&event);
                        this.ready.push_back(Bytes::from(rewritten));
                    }
                    if self.pending.len() > MAX_EVENT {
                        // refuse the stream rather than pass an event on
                        // unfiltered
                        self.finished = true;
                        self.pending.clear();
                        return Poll::Ready(Some(Err(io::Error::other(
                            "an event in the MCP stream is larger than the filter holds",
                        ))));
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
        )
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

    /// a reply is narrowed whatever id it carries: `1` is not `1.0` as JSON, a
    /// request may have sent `null` or none, and `ID` is `id` to a Go decoder
    #[test]
    fn a_reply_is_narrowed_whatever_its_id() {
        let filter = filter(&["a"]);
        for id in ["1", "1.0", "1e0", "null", "\"one\"", "999"] {
            let body = format!(
                r#"{{"jsonrpc":"2.0","id":{id},"result":{{"tools":[{{"name":"a"}},{{"name":"b"}}]}}}}"#
            );
            assert_eq!(
                names(&filter.filter_json(body.as_bytes()).unwrap()),
                ["a"],
                "{id}"
            );
        }
        let no_id = r#"{"jsonrpc":"2.0","result":{"tools":[{"name":"a"},{"name":"b"}]}}"#;
        assert_eq!(names(&filter.filter_json(no_id.as_bytes()).unwrap()), ["a"]);
        let upper = r#"{"jsonrpc":"2.0","ID":1,"result":{"tools":[{"name":"a"},{"name":"b"}]}}"#;
        assert_eq!(names(&filter.filter_json(upper.as_bytes()).unwrap()), ["a"]);
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

    #[test]
    fn events_end_at_any_empty_line_and_resume_where_they_stopped() {
        for (terminator, ends) in [
            ("\n\n", true),
            ("\r\n\r\n", true),
            ("\r\r", true),
            ("\n\r\n", true),
            ("\r\n\n", true),
            ("\n", false),
        ] {
            let mut scanner = EventScanner::default();
            // a trailing `\r` could still become `\r\n`, so a byte follows
            let buf = format!("data: x{terminator}d").into_bytes();
            assert_eq!(scanner.end(&buf).is_some(), ends, "{terminator:?}");
        }
        // a chunk boundary inside `\r\n` or between the two line ends
        let mut scanner = EventScanner::default();
        let mut buf = b"data: x\r".to_vec();
        assert_eq!(scanner.end(&buf), None);
        buf.extend_from_slice(b"\n\r");
        assert_eq!(scanner.end(&buf), None);
        buf.extend_from_slice(b"\n");
        assert_eq!(scanner.end(&buf), Some(buf.len()));
    }

    #[tokio::test]
    async fn an_sse_answer_ended_by_a_bare_cr_is_still_narrowed() {
        let answer = reply(7, &["a", "b"], None);
        let event: &'static str = Box::leak(format!("data: {answer}\r\r").into_boxed_str());
        let out = collect(filter(&["a"]), vec![event]).await;
        let json = out.trim().trim_start_matches("data: ");
        assert_eq!(names(json.as_bytes()), ["a"]);
    }

    #[tokio::test]
    async fn an_event_larger_than_the_cap_cuts_the_stream_instead_of_passing_it_on() {
        let huge: &'static str =
            Box::leak(format!("data: {}", "x".repeat(MAX_EVENT + 1)).into_boxed_str());
        let stream = futures_util::stream::iter([Ok::<_, std::io::Error>(Bytes::from(huge))]);
        let out: Vec<_> = FilteredSse::new(stream, Arc::new(filter(&["a"])))
            .collect()
            .await;
        assert_eq!(out.len(), 1);
        assert!(out[0].is_err(), "the oversized event is not forwarded");
    }
}
