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

use crate::mcp_policy::fold_key;

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
    /// Keys are read the way a Go decoder reads them, whatever their case.
    fn apply(&self, message: &mut Value) -> bool {
        let Some(object) = message.as_object_mut() else {
            return false;
        };
        let mut found = false;
        for (key, result) in object.iter_mut() {
            if fold_key(key) != "result" {
                continue;
            }
            let Some(result) = result.as_object_mut() else {
                continue;
            };
            for (key, tools) in result.iter_mut() {
                if fold_key(key) != "tools" {
                    continue;
                }
                if let Some(tools) = tools.as_array_mut() {
                    // a tool with no readable name, or with two keys that name it
                    // once case is folded, cannot be matched to a rule
                    tools.retain(|tool| self.allows_entry(tool));
                    found = true;
                }
            }
        }
        found
    }

    fn allows_entry(&self, tool: &Value) -> bool {
        let Some(tool) = tool.as_object() else {
            return false;
        };
        let mut names = tool.iter().filter(|(key, _)| fold_key(key) == "name");
        match (names.next(), names.next()) {
            (Some((_, name)), None) => name.as_str().is_some_and(|name| self.allows(name)),
            _ => false,
        }
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
    /// back byte for byte. An event that is not an `endpoint` announcement and
    /// whose data cannot be read as JSON cannot be shown to hold no tool list, so
    /// it is `Err` and the stream is cut, as a JSON reply that does not parse is
    /// refused. Lines end with `\r\n`, `\n` or `\r`, bytes are read lossily and a
    /// leading BOM is dropped, because that is how a client reads them.
    fn filter_event(&self, event: &[u8]) -> Result<Vec<u8>, Unfilterable> {
        let text = String::from_utf8_lossy(event);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let mut name = "message";
        let mut data = String::new();
        let mut has_data = false;
        let mut others = Vec::new();
        for line in sse_lines(text) {
            if let Some(rest) = line.strip_prefix("data:") {
                if has_data {
                    data.push('\n');
                }
                has_data = true;
                data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            } else if let Some(rest) = line.strip_prefix("event:") {
                name = rest.trim();
                others.push(line);
            } else if !line.is_empty() {
                others.push(line);
            }
        }
        if name == "endpoint" || data.trim().is_empty() {
            return Ok(event.to_vec());
        }
        let Ok(mut message) = serde_json::from_str::<Value>(&data) else {
            return Err(Unfilterable);
        };
        if !self.apply_all(&mut message) {
            return Ok(event.to_vec());
        }
        let json = serde_json::to_string(&message).map_err(|_| Unfilterable)?;
        let mut out = String::new();
        for line in others {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("data: ");
        out.push_str(&json);
        out.push_str("\n\n");
        Ok(out.into_bytes())
    }
}

/// An event the filter cannot show holds no tool list.
#[derive(Debug)]
struct Unfilterable;

/// The lines of an SSE event, ending with `\r\n`, `\n` or `\r`.
fn sse_lines(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                lines.push(&text[start..i]);
                i += 1;
                start = i;
            }
            b'\r' => {
                lines.push(&text[start..i]);
                i += if bytes.get(i + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < bytes.len() {
        lines.push(&text[start..]);
    }
    lines
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
                        match this.filter.filter_event(&event) {
                            Ok(rewritten) => this.ready.push_back(Bytes::from(rewritten)),
                            Err(Unfilterable) => {
                                this.finished = true;
                                this.pending.clear();
                                this.ready.clear();
                                return Poll::Ready(Some(Err(io::Error::other(
                                    "an event in the MCP stream could not be read, so it \
                                     cannot be filtered",
                                ))));
                            }
                        }
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
                        match self.filter.filter_event(&rest) {
                            Ok(rewritten) => self.ready.push_back(Bytes::from(rewritten)),
                            Err(Unfilterable) => {
                                return Poll::Ready(Some(Err(io::Error::other(
                                    "an event in the MCP stream could not be read, so it \
                                     cannot be filtered",
                                ))));
                            }
                        }
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

    async fn outcome(chunks: Vec<Vec<u8>>) -> (String, bool) {
        let stream = futures_util::stream::iter(
            chunks
                .into_iter()
                .map(|chunk| Ok::<_, std::io::Error>(Bytes::from(chunk))),
        );
        let out: Vec<_> = FilteredSse::new(stream, Arc::new(filter(&["a"])))
            .collect()
            .await;
        let cut = out.iter().any(Result::is_err);
        let text = out
            .into_iter()
            .filter_map(Result::ok)
            .map(|chunk| String::from_utf8_lossy(&chunk).into_owned())
            .collect();
        (text, cut)
    }

    /// the four ways an event used to reach the client unfiltered because the
    /// filter could not read it though the client could
    #[tokio::test]
    async fn an_event_the_filter_reads_differently_from_a_client_is_narrowed_or_cut() {
        let answer = reply(7, &["a", "b"], None);
        // (a) a bare CR ends lines, and `lines()` does not split on it
        let (text, cut) = outcome(vec![
            format!("event: message\rdata: {answer}\r\r").into_bytes()
        ])
        .await;
        assert!(!cut && !text.contains("\"b\""), "{text}");
        // (b) a byte that is not UTF-8 in a comment line
        let mut bytes = b": \xff\ndata: ".to_vec();
        bytes.extend_from_slice(answer.as_bytes());
        bytes.extend_from_slice(b"\n\n");
        let (text, cut) = outcome(vec![bytes]).await;
        assert!(!cut && !text.contains("\"b\""), "{text}");
        // (d) a leading byte order mark
        let (text, cut) = outcome(vec![format!("\u{feff}data: {answer}\n\n").into_bytes()]).await;
        assert!(!cut && !text.contains("\"b\""), "{text}");
        // (c) data that is not JSON the filter reads: a lone surrogate escape, or
        // nesting past the parser's limit, is cut rather than forwarded
        let lone = format!("data: {}\n\n", answer.replace("\"a\"", "\"\\ud800\""));
        let deep = format!("data: {}1{}\n\n", "[".repeat(200), "]".repeat(200));
        for bad in [lone, deep] {
            let (text, cut) = outcome(vec![bad.into_bytes()]).await;
            assert!(cut && text.is_empty(), "{text}");
        }
        // an `endpoint` announcement and an empty keep-alive still pass
        let (text, cut) = outcome(vec![
            b"event: endpoint\ndata: /messages?s=1\n\n: ping\n\n".to_vec()
        ])
        .await;
        assert!(!cut && text.contains("/messages?s=1"));
    }

    /// the reply side reads keys the way the request side does
    #[test]
    fn a_reply_is_read_with_keys_folded_like_a_go_decoder_reads_them() {
        let filter = filter(&["ok"]);
        let body = r#"{"jsonrpc":"2.0","id":1,"Result":{"Tools":[
            {"name":"ok"},{"name":"ok","NAME":"drop"},{"NAME":"ok"},{"name":"drop"}]}}"#;
        let out: Value =
            serde_json::from_slice(&filter.filter_json(body.as_bytes()).unwrap()).unwrap();
        let kept = out["Result"]["Tools"].as_array().unwrap();
        // the entry with two name keys is dropped, not guessed at
        assert_eq!(kept.len(), 2, "{out}");
        assert!(kept
            .iter()
            .all(|tool| tool.get("name") != Some(&Value::from("drop"))));
    }
}
