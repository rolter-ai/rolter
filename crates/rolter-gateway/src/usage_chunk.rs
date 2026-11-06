//! The usage chunk a stream carries only because the gateway asked for it
//! (#3667).
//!
//! An OpenAI-style upstream reports token usage on a streamed chat completion
//! only when the request sets `stream_options.include_usage`, and many callers
//! do not. Their requests were logged with no tokens and no cost, and budgets
//! could not count them. The gateway now sets the option for them, reads the
//! usage chunk the upstream then sends (a final event with an empty `choices`
//! and a `usage`), and takes that event out of the stream the caller reads, so
//! a caller that did not ask for it sees the stream it always saw.

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use futures_util::Stream;

/// `inner` without its usage-only events. Every other byte passes through in
/// order; an event is held only until its blank line arrives.
pub(crate) struct StripUsageChunk<S> {
    inner: S,
    buf: BytesMut,
    ended: bool,
}

impl<S> StripUsageChunk<S> {
    pub(crate) fn new(inner: S) -> Self {
        Self {
            inner,
            buf: BytesMut::new(),
            ended: false,
        }
    }
}

/// Where the first complete event in `buf` ends, its separator included.
fn event_end(buf: &[u8]) -> Option<usize> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|i| i + 2);
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Whether `event` is the usage chunk: one `data:` payload whose `choices` is
/// empty and that carries a `usage` object.
pub(crate) fn is_usage_only(event: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(event) else {
        return false;
    };
    let data: String = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() || data == "[DONE]" {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) else {
        return false;
    };
    value
        .get("choices")
        .and_then(serde_json::Value::as_array)
        .is_some_and(Vec::is_empty)
        && value.get("usage").is_some_and(serde_json::Value::is_object)
}

impl<S, E> Stream for StripUsageChunk<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    type Item = Result<Bytes, E>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        loop {
            // hand on every complete event held, skipping the usage chunk
            let mut out = BytesMut::new();
            while let Some(end) = event_end(&this.buf) {
                let event = this.buf.split_to(end);
                if !is_usage_only(&event) {
                    out.extend_from_slice(&event);
                }
            }
            if !out.is_empty() {
                return Poll::Ready(Some(Ok(out.freeze())));
            }
            if this.ended {
                // whatever trails the last separator is not an event to judge
                if this.buf.is_empty() {
                    return Poll::Ready(None);
                }
                return Poll::Ready(Some(Ok(this.buf.split().freeze())));
            }
            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => this.buf.extend_from_slice(&bytes),
                Poll::Ready(Some(Err(err))) => return Poll::Ready(Some(Err(err))),
                Poll::Ready(None) => this.ended = true,
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    const CONTENT: &str = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\n";
    const USAGE: &str =
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1,\"total_tokens\":4}}\n\n";
    const DONE: &str = "data: [DONE]\n\n";

    async fn strip(chunks: Vec<&'static str>) -> String {
        let inner = futures_util::stream::iter(
            chunks
                .into_iter()
                .map(|c| Ok::<_, std::io::Error>(Bytes::from_static(c.as_bytes()))),
        );
        let out: Vec<Bytes> = StripUsageChunk::new(inner)
            .map(|r| r.unwrap())
            .collect()
            .await;
        String::from_utf8(out.concat()).unwrap()
    }

    #[tokio::test]
    async fn the_usage_chunk_is_taken_out_and_the_rest_kept_in_order() {
        let out = strip(vec![CONTENT, USAGE, DONE]).await;
        assert_eq!(out, format!("{CONTENT}{DONE}"));
    }

    #[tokio::test]
    async fn an_event_split_across_chunks_is_judged_whole() {
        let (head, tail) = USAGE.split_at(20);
        let out = strip(vec![CONTENT, head, tail, DONE]).await;
        assert_eq!(out, format!("{CONTENT}{DONE}"));
    }

    #[tokio::test]
    async fn a_content_chunk_that_also_carries_usage_is_kept() {
        let both =
            "data: {\"choices\":[{\"index\":0,\"delta\":{}}],\"usage\":{\"total_tokens\":4}}\n\n";
        let out = strip(vec![both, DONE]).await;
        assert_eq!(out, format!("{both}{DONE}"));
    }

    #[tokio::test]
    async fn a_trailing_fragment_with_no_separator_is_passed_on() {
        let out = strip(vec![CONTENT, "data: {\"partial\""]).await;
        assert_eq!(out, format!("{CONTENT}data: {{\"partial\""));
    }

    #[test]
    fn crlf_events_are_split_too() {
        assert_eq!(event_end(b"data: x\r\n\r\nmore"), Some(11));
        assert!(is_usage_only(USAGE.as_bytes()));
        assert!(!is_usage_only(DONE.as_bytes()));
    }
}
