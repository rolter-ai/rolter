//! Restoring a streamed response through the sanitizer (#911).
//!
//! The buffered path restores a whole body with one `POST restore_url`. A
//! stream has no whole body, and a placeholder can straddle any number of SSE
//! frames, so `streaming = "restore"` pipes the stream's text through the
//! sanitizer instead (ADR `2026-10-27-pii-streaming-restoration`, option 4).
//! The sanitizer holds the mapping, the placeholder set and the holdback;
//! rolter keeps what it already owns — SSE framing, dialects, usage.
//!
//! One full-duplex `POST restore_stream_url` per stream carries NDJSON both
//! ways. The first line names the exchange and its token; then each text
//! delta of each frame goes out as `{"seq", "channel", "text"}`, each channel's
//! end as `{"seq", "close"}` and the stream's end as `{"seq", "end"}`. The
//! sanitizer answers every line but the first with exactly one
//! `{"seq", "text", "held"}`, in order, so the gateway re-frames trivially:
//! frame 17 goes out with its text field replaced by reply 17's text and
//! everything else in it as the provider sent it.
//!
//! What this module never does: hold a mapping or a placeholder set, or put a
//! text delta, a reply or the token in a log line, a metric, a span or an
//! error. The text it holds is the provider's own (placeholder-bearing) frame
//! until it is answered, and the restored text on its way to the caller, as
//! the buffered path does with a restored body.
//!
//! Failure is never fatal and never the unsafe way round: a pipe that does not
//! open serves the stream unrestored, and a pipe that fails mid-stream (error,
//! reset, a malformed or out-of-order reply, or no reply within `timeout_ms`)
//! emits the text the sanitizer was holding and the original text of every
//! unanswered delta, then carries on unrestored. Nothing is duplicated or
//! lost, `rolter_pii_restore_errors_total` counts it, and nothing is retried:
//! a new pipe would need holdback state the sanitizer has discarded.

use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_util::Stream;
use rolter_core::pii_sanitizer::{
    PiiSanitizerConfig, RestoreStreamClose, RestoreStreamDelta, RestoreStreamEnd,
    RestoreStreamOpen, RestoreStreamReply, WebhookAuth, WebhookTenant, MAX_STREAM_HELD_CHARS,
};
use serde_json::Value;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use xxhash_rust::xxh3::Xxh3Default;

use crate::egress_client::EgressClient;
use crate::metrics::Metrics;

/// The most deltas and closes the gateway leaves unanswered at once. When the
/// window is full the upstream is not polled, which pushes back on the
/// provider through TCP the way a slow client does, so a slow sanitizer slows
/// the stream rather than growing a buffer.
pub(crate) const MAX_UNANSWERED_LINES: usize = 256;

/// The most text, in bytes, the gateway leaves unanswered at once. A single
/// frame larger than this still goes out when nothing else is unanswered.
pub(crate) const MAX_UNANSWERED_BYTES: usize = 256 * 1024;

/// The longest reply line accepted. A sanitizer that sends more without a
/// newline is not speaking the protocol.
const MAX_REPLY_LINE_BYTES: usize = 4 * 1024 * 1024;

type ByteStream = Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>;
type Opening = Pin<Box<dyn Future<Output = Option<reqwest::Response>> + Send>>;

/// Everything one streamed restore needs, owned, because the response body
/// outlives the handler that built it.
///
/// Deliberately not `Debug`: it carries the restoration token.
pub(crate) struct StreamRestore {
    url: String,
    timeout: Duration,
    auth: Option<WebhookAuth>,
    egress: EgressClient,
    metrics: Arc<Metrics>,
    token: String,
    trace_id: String,
    tenant: WebhookTenant,
}

impl StreamRestore {
    pub(crate) fn new(
        config: &PiiSanitizerConfig,
        egress: &EgressClient,
        metrics: &Arc<Metrics>,
        token: &str,
        trace_id: &str,
        tenant: WebhookTenant,
    ) -> Self {
        Self {
            url: config.restore_stream_url.trim().to_string(),
            timeout: Duration::from_millis(config.timeout_ms.max(1)),
            auth: config.auth.clone(),
            egress: egress.clone(),
            metrics: metrics.clone(),
            token: token.to_string(),
            trace_id: trace_id.to_string(),
            tenant,
        }
    }

    /// The answer to a streamed request is not a stream (an upstream that
    /// ignored `"stream": true`), so there is nothing this can restore. It
    /// goes out with its placeholders, and the counter and the log say so.
    pub(crate) fn not_a_stream(self) {
        self.metrics.pii_restore_errors_total.fetch_add(1, Relaxed);
        tracing::warn!(
            "a streamed request was answered without a stream; its pii placeholders were not \
             restored"
        );
    }

    /// Restore `inner`, an SSE body in the caller's dialect.
    pub(crate) fn wrap(self, inner: ByteStream) -> RestoringStream {
        RestoringStream::new(self, inner)
    }
}

/// Where the pipe to the sanitizer is.
enum Pipe {
    /// the request is out and its response head has not arrived
    Opening(Opening),
    /// replies are being read
    Open(ByteStream),
    /// the sanitizer ended its reply; any further line goes unanswered
    Ended,
    /// failed or never opened: everything from here is served unrestored
    Failed,
    /// the stream finished and the pipe was closed cleanly
    Closed,
}

/// One text field of one frame.
struct Slot {
    channel: String,
    path: Vec<Seg>,
    /// the provider's text, kept until it is answered
    original: String,
    /// the text to emit, once the reply (or a failure) has decided it
    out: Option<String>,
    /// the text is JSON source: tool-call arguments, `partial_json`
    json: bool,
    /// a Responses API channel, whose done events repeat the whole text
    responses: bool,
}

#[derive(Clone)]
enum Seg {
    Key(&'static str),
    Index(usize),
}

/// A field of a Responses API event that repeats a channel's whole text.
struct FullText {
    channel: String,
    path: Vec<Seg>,
}

/// One SSE event whose text the gateway may rewrite.
struct Event {
    /// the event's lines without their terminators; `None` marks where the
    /// data line goes
    lines: Vec<Option<Bytes>>,
    crlf: bool,
    json: Value,
    slots: Vec<Slot>,
    full_text: Vec<FullText>,
}

struct Frame {
    raw: Bytes,
    event: Option<Event>,
    /// lines sent for this frame that are not answered yet
    pending: usize,
    /// slots of this frame whose channel's held text has not been released
    held: usize,
}

impl Frame {
    fn ready(&self) -> bool {
        self.pending == 0 && self.held == 0
    }
}

/// The holdback state of one channel, as far as the gateway can see it: how
/// much the sanitizer says it holds and which original text that is.
#[derive(Default)]
struct Channel {
    /// the last `held` characters of the channel's input
    tail: String,
    /// the slot the held text belongs to, so a close can append to it and a
    /// failure can emit it
    held_slot: Option<(u64, usize)>,
    closed: bool,
    /// digest of the channel's whole original text, so a Responses API event
    /// that repeats it can be recognised without keeping the text
    original: Xxh3Default,
    /// what was delivered on the channel; kept only for a Responses API
    /// channel, whose done events repeat the whole text
    delivered: Option<String>,
}

enum LineKind {
    Delta { frame: u64, slot: usize },
    Close { frame: u64, channel: String },
}

struct Line {
    seq: u64,
    kind: LineKind,
    bytes: usize,
}

/// Why a pipe failed, for the one log line it leaves. Never content.
#[derive(Clone, Copy)]
enum Failure {
    Open,
    Transport,
    Protocol,
    Timeout,
}

impl Failure {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "the restore stream did not open",
            Self::Transport => "the restore stream broke",
            Self::Protocol => "the restore stream sent a reply out of contract",
            Self::Timeout => "the restore stream did not answer within timeout_ms",
        }
    }
}

/// An SSE body whose text is restored through the sanitizer on its way out.
pub(crate) struct RestoringStream {
    inner: ByteStream,
    /// the upstream returned its last item
    upstream_ended: bool,
    /// everything the upstream sent is queued as frames
    inner_done: bool,
    inner_error: Option<reqwest::Error>,
    /// bytes of an event that has not ended yet
    partial: Vec<u8>,
    frames: VecDeque<Frame>,
    /// id of the frame at the front of `frames`
    front: u64,
    outstanding: VecDeque<Line>,
    unanswered_bytes: usize,
    next_seq: u64,
    channels: BTreeMap<String, Channel>,
    pipe: Pipe,
    tx: Option<UnboundedSender<std::io::Result<Bytes>>>,
    replies: Vec<u8>,
    deadline: Option<Pin<Box<tokio::time::Sleep>>>,
    timeout: Duration,
    metrics: Arc<Metrics>,
    failed: bool,
    finished: bool,
}

impl RestoringStream {
    fn new(restore: StreamRestore, inner: ByteStream) -> Self {
        let StreamRestore {
            url,
            timeout,
            auth,
            egress,
            metrics,
            token,
            trace_id,
            tenant,
        } = restore;
        let (tx, mut rx) = unbounded_channel::<std::io::Result<Bytes>>();
        let open = RestoreStreamOpen {
            trace_id: &trace_id,
            tenant,
            restoration_token: &token,
        };
        // the token leaves in this line and is not kept anywhere else
        let opened = serde_json::to_vec(&open).ok().and_then(|mut line| {
            line.push(b'\n');
            tx.send(Ok(Bytes::from(line))).ok()
        });
        drop(token);
        let request = opened.and_then(|()| egress.post(&url)).map(|req| {
            let body = futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx));
            let req = req
                .header("X-Rolter-Trace-Id", trace_id.as_str())
                .header(reqwest::header::CONTENT_TYPE, "application/x-ndjson")
                .body(reqwest::Body::wrap_stream(body));
            crate::pii_sanitizer::authorize(req, auth.as_ref())
        });
        let mut stream = Self {
            inner,
            upstream_ended: false,
            inner_done: false,
            inner_error: None,
            partial: Vec::new(),
            frames: VecDeque::new(),
            front: 0,
            outstanding: VecDeque::new(),
            unanswered_bytes: 0,
            next_seq: 0,
            channels: BTreeMap::new(),
            pipe: Pipe::Failed,
            tx: Some(tx),
            replies: Vec::new(),
            deadline: None,
            timeout,
            metrics,
            failed: false,
            finished: false,
        };
        match request {
            Some(req) => {
                // no timeout of its own: a sanitizer may send its head with
                // its first reply, and the first delta can be a long prefill
                // away. the per-line deadline, armed by the first delta sent,
                // covers the head too
                stream.pipe = Pipe::Opening(Box::pin(async move {
                    match req.send().await {
                        Ok(response) if response.status().is_success() => Some(response),
                        _ => None,
                    }
                }));
            }
            None => stream.fail(Failure::Open),
        }
        stream
    }

    fn frame_mut(&mut self, id: u64) -> Option<&mut Frame> {
        let index = usize::try_from(id.checked_sub(self.front)?).ok()?;
        self.frames.get_mut(index)
    }

    fn slot_mut(&mut self, frame: u64, slot: usize) -> Option<&mut Slot> {
        self.frame_mut(frame)?.event.as_mut()?.slots.get_mut(slot)
    }

    fn window_has_room(&self) -> bool {
        self.outstanding.is_empty()
            || (self.outstanding.len() < MAX_UNANSWERED_LINES
                && self.unanswered_bytes < MAX_UNANSWERED_BYTES)
    }

    /// Give up on the pipe and serve everything from here unrestored: the held
    /// text and every unanswered delta go out as the provider sent them.
    fn fail(&mut self, why: Failure) {
        if self.failed {
            return;
        }
        self.failed = true;
        self.metrics.pii_restore_errors_total.fetch_add(1, Relaxed);
        tracing::warn!(
            reason = why.as_str(),
            "pii stream restore failed; serving the rest of the stream unrestored"
        );
        // an error, not a clean end, so the sanitizer sees a reset rather
        // than a stream that simply stopped
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Err(std::io::Error::other("restore stream abandoned")));
        }
        self.pipe = Pipe::Failed;
        self.deadline = None;
        self.replies = Vec::new();
        let held: Vec<(String, (u64, usize))> = self
            .channels
            .iter_mut()
            .filter_map(|(name, channel)| channel.held_slot.take().map(|slot| (name.clone(), slot)))
            .collect();
        for (name, (frame, slot)) in held {
            let tail = self
                .channels
                .get_mut(&name)
                .map(|channel| std::mem::take(&mut channel.tail))
                .unwrap_or_default();
            if let Some(out) = self.slot_mut(frame, slot).and_then(|s| s.out.as_mut()) {
                out.push_str(&tail);
            }
            if let Some(frame) = self.frame_mut(frame) {
                frame.held = frame.held.saturating_sub(1);
            }
        }
        while let Some(line) = self.outstanding.pop_front() {
            match line.kind {
                LineKind::Delta { frame, slot } => {
                    if let Some(slot) = self.slot_mut(frame, slot) {
                        slot.out = Some(std::mem::take(&mut slot.original));
                    }
                    if let Some(frame) = self.frame_mut(frame) {
                        frame.pending = frame.pending.saturating_sub(1);
                    }
                }
                LineKind::Close { frame, .. } => {
                    if let Some(frame) = self.frame_mut(frame) {
                        frame.pending = frame.pending.saturating_sub(1);
                    }
                }
            }
        }
        self.unanswered_bytes = 0;
    }

    fn send(&mut self, line: Vec<u8>) -> bool {
        if matches!(self.pipe, Pipe::Ended) {
            self.fail(Failure::Protocol);
            return false;
        }
        let Some(tx) = &self.tx else {
            return false;
        };
        let mut line = line;
        line.push(b'\n');
        if tx.send(Ok(Bytes::from(line))).is_err() {
            // the body was dropped: the request failed and the reply stream
            // will say so, but there is nothing to wait for
            self.fail(Failure::Transport);
            return false;
        }
        if self.deadline.is_none() {
            self.deadline = Some(Box::pin(tokio::time::sleep(self.timeout)));
        }
        true
    }

    /// Queue `frame` and send its deltas and closes, or resolve them at once
    /// when the pipe has failed.
    fn push_frame(&mut self, mut frame: Frame, closes: &[Option<String>]) {
        let id = self.front + self.frames.len() as u64;
        let mut lines = Vec::new();
        if let Some(event) = frame.event.as_mut() {
            for (index, slot) in event.slots.iter_mut().enumerate() {
                let channel = self.channels.entry(slot.channel.clone()).or_default();
                channel.original.update(slot.original.as_bytes());
                if slot.responses && channel.delivered.is_none() {
                    channel.delivered = Some(String::new());
                }
                // a delta after its channel's close starts the channel again
                channel.closed = false;
                if self.failed {
                    slot.out = Some(std::mem::take(&mut slot.original));
                    continue;
                }
                let seq = self.next_seq;
                let line = serde_json::to_vec(&RestoreStreamDelta {
                    seq,
                    channel: &slot.channel,
                    text: &slot.original,
                    format: slot.json.then_some("json"),
                })
                .unwrap_or_default();
                let bytes = slot.original.len();
                self.outstanding.push_back(Line {
                    seq,
                    kind: LineKind::Delta {
                        frame: id,
                        slot: index,
                    },
                    bytes,
                });
                self.unanswered_bytes += bytes;
                frame.pending += 1;
                self.next_seq += 1;
                lines.push(line);
            }
        }
        // closes go after the frame's own deltas, so a frame that carries
        // both its last text and its finish reason is answered in order
        if !self.failed {
            let names: Vec<String> = self
                .channels
                .iter()
                .filter(|(name, channel)| {
                    !channel.closed
                        && closes.iter().any(|prefix| match prefix {
                            None => true,
                            Some(prefix) => {
                                name.as_str() == prefix
                                    || name
                                        .strip_prefix(prefix.as_str())
                                        .is_some_and(|rest| rest.starts_with('/'))
                            }
                        })
                })
                .map(|(name, _)| name.clone())
                .collect();
            for name in names {
                if let Some(channel) = self.channels.get_mut(&name) {
                    channel.closed = true;
                }
                let seq = self.next_seq;
                let line = serde_json::to_vec(&RestoreStreamClose { seq, close: &name })
                    .unwrap_or_default();
                self.outstanding.push_back(Line {
                    seq,
                    kind: LineKind::Close {
                        frame: id,
                        channel: name,
                    },
                    bytes: 0,
                });
                frame.pending += 1;
                self.next_seq += 1;
                lines.push(line);
            }
        }
        self.frames.push_back(frame);
        for line in lines {
            if !self.send(line) {
                break;
            }
        }
    }

    /// Take one reply line off the pipe and apply it.
    fn on_reply(&mut self, line: &[u8]) -> Result<(), Failure> {
        let reply: RestoreStreamReply =
            serde_json::from_slice(line).map_err(|_| Failure::Protocol)?;
        let Some(front) = self.outstanding.front() else {
            return Err(Failure::Protocol);
        };
        if front.seq != reply.seq {
            return Err(Failure::Protocol);
        }
        match &front.kind {
            LineKind::Delta { frame, slot } => {
                let (frame, slot) = (*frame, *slot);
                let Some(entry) = self.slot_mut(frame, slot) else {
                    return Err(Failure::Protocol);
                };
                let name = entry.channel.clone();
                let input_chars = entry.original.chars().count();
                let tail_chars = self
                    .channels
                    .get(&name)
                    .map_or(0, |channel| channel.tail.chars().count());
                // the held text is a suffix of everything the channel was
                // sent, so the sanitizer cannot hold more than that. checked
                // before anything moves, so a failure still has the tail
                if reply.held > input_chars + tail_chars || reply.held > MAX_STREAM_HELD_CHARS {
                    return Err(Failure::Protocol);
                }
                let input = self
                    .slot_mut(frame, slot)
                    .map(|entry| std::mem::take(&mut entry.original))
                    .unwrap_or_default();
                let channel = self.channels.entry(name).or_default();
                let mut seen = std::mem::take(&mut channel.tail);
                seen.push_str(&input);
                let keep_from = seen
                    .char_indices()
                    .nth(input_chars + tail_chars - reply.held)
                    .map_or(seen.len(), |(at, _)| at);
                channel.tail = seen.split_off(keep_from);
                let released = channel.held_slot.take();
                let held = reply.held > 0;
                if held {
                    channel.held_slot = Some((frame, slot));
                }
                if let Some((held_frame, _)) = released {
                    if let Some(held_frame) = self.frame_mut(held_frame) {
                        held_frame.held = held_frame.held.saturating_sub(1);
                    }
                }
                if let Some(entry) = self.slot_mut(frame, slot) {
                    entry.out = Some(reply.text);
                }
                if let Some(frame) = self.frame_mut(frame) {
                    frame.pending = frame.pending.saturating_sub(1);
                    if held {
                        frame.held += 1;
                    }
                }
            }
            LineKind::Close { frame, channel } => {
                let (frame, name) = (*frame, channel.clone());
                if reply.held != 0 {
                    return Err(Failure::Protocol);
                }
                let channel = self.channels.entry(name).or_default();
                let released = channel.held_slot.take();
                channel.tail.clear();
                match released {
                    Some((held_frame, held_slot)) => {
                        if let Some(out) = self
                            .slot_mut(held_frame, held_slot)
                            .and_then(|s| s.out.as_mut())
                        {
                            out.push_str(&reply.text);
                        }
                        if let Some(held_frame) = self.frame_mut(held_frame) {
                            held_frame.held = held_frame.held.saturating_sub(1);
                        }
                    }
                    // text on a close with nothing held has nowhere to go:
                    // the sanitizer said it held nothing
                    None if !reply.text.is_empty() => return Err(Failure::Protocol),
                    None => {}
                }
                if let Some(frame) = self.frame_mut(frame) {
                    frame.pending = frame.pending.saturating_sub(1);
                }
            }
        }
        if let Some(line) = self.outstanding.pop_front() {
            self.unanswered_bytes = self.unanswered_bytes.saturating_sub(line.bytes);
        }
        // the next unanswered line gets a fresh `timeout_ms`
        if self.outstanding.is_empty() {
            self.deadline = None;
        } else if let Some(deadline) = self.deadline.as_mut() {
            deadline
                .as_mut()
                .reset(tokio::time::Instant::now() + self.timeout);
        }
        Ok(())
    }

    /// Poll the pipe: its head while opening, then its replies. Returns true
    /// when anything changed.
    fn poll_pipe(&mut self, cx: &mut Context<'_>) -> bool {
        let mut progressed = false;
        if let Pipe::Opening(open) = &mut self.pipe {
            match open.as_mut().poll(cx) {
                Poll::Ready(Some(response)) => {
                    self.pipe = Pipe::Open(Box::pin(response.bytes_stream()));
                    progressed = true;
                }
                Poll::Ready(None) => {
                    self.fail(Failure::Open);
                    return true;
                }
                Poll::Pending => return false,
            }
        }
        loop {
            let Pipe::Open(replies) = &mut self.pipe else {
                return progressed;
            };
            match replies.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    progressed = true;
                    self.replies.extend_from_slice(&chunk);
                    while let Some(end) = self.replies.iter().position(|b| *b == b'\n') {
                        let line: Vec<u8> = self.replies.drain(..=end).collect();
                        let line = line.trim_ascii();
                        if line.is_empty() {
                            continue;
                        }
                        if let Err(why) = self.on_reply(line) {
                            self.fail(why);
                            return true;
                        }
                    }
                    if self.replies.len() > MAX_REPLY_LINE_BYTES {
                        self.fail(Failure::Protocol);
                        return true;
                    }
                }
                Poll::Ready(Some(Err(_))) => {
                    self.fail(Failure::Transport);
                    return true;
                }
                Poll::Ready(None) => {
                    if self.outstanding.is_empty() {
                        self.pipe = Pipe::Ended;
                    } else {
                        self.fail(Failure::Transport);
                    }
                    return true;
                }
                Poll::Pending => return progressed,
            }
        }
    }

    /// Split what has arrived into SSE events and queue them while the window
    /// has room. The rest stays in `partial` until replies free some, so one
    /// large chunk — a buffered cache miss, a cached replay — is held to the
    /// window like a live stream. Returns true when anything was queued.
    fn take_events(&mut self) -> bool {
        let mut queued = false;
        while self.window_has_room() {
            let Some(end) = event_end(&self.partial) else {
                break;
            };
            let raw: Vec<u8> = self.partial.drain(..end).collect();
            self.queue_event(Bytes::from(raw));
            queued = true;
        }
        queued
    }

    fn queue_event(&mut self, raw: Bytes) {
        let (event, closes) = parse_event(&raw);
        self.push_frame(
            Frame {
                raw,
                event,
                pending: 0,
                held: 0,
            },
            &closes,
        );
    }

    /// The upstream ended: queue what is left of it and close every channel
    /// still open, so the sanitizer flushes what it holds.
    fn end_of_upstream(&mut self) {
        let rest = Bytes::from(std::mem::take(&mut self.partial));
        if rest.is_empty() {
            self.push_frame(
                Frame {
                    raw: rest,
                    event: None,
                    pending: 0,
                    held: 0,
                },
                &[None],
            );
        } else {
            let (event, mut closes) = parse_event(&rest);
            closes.push(None);
            self.push_frame(
                Frame {
                    raw: rest,
                    event,
                    pending: 0,
                    held: 0,
                },
                &closes,
            );
        }
    }

    /// The front frame, rendered, if it is ready to go.
    fn pop_ready(&mut self) -> Option<Bytes> {
        loop {
            if !self.frames.front()?.ready() {
                return None;
            }
            let frame = self.frames.pop_front()?;
            self.front += 1;
            let bytes = self.render(frame);
            if !bytes.is_empty() {
                return Some(bytes);
            }
        }
    }

    fn render(&mut self, frame: Frame) -> Bytes {
        let Some(mut event) = frame.event else {
            return frame.raw;
        };
        let mut changed = false;
        for slot in event.slots.drain(..) {
            let out = slot.out.unwrap_or(slot.original);
            if let Some(delivered) = self
                .channels
                .get_mut(&slot.channel)
                .and_then(|channel| channel.delivered.as_mut())
            {
                delivered.push_str(&out);
            }
            if let Some(field) = pointer_mut(&mut event.json, &slot.path) {
                if field.as_str() != Some(out.as_str()) {
                    *field = Value::String(out);
                    changed = true;
                }
            }
        }
        for full in &event.full_text {
            let Some(channel) = self.channels.get(&full.channel) else {
                continue;
            };
            let Some(delivered) = &channel.delivered else {
                continue;
            };
            let Some(field) = pointer_mut(&mut event.json, &full.path) else {
                continue;
            };
            let Some(text) = field.as_str() else {
                continue;
            };
            // only the field that repeats exactly what the channel streamed
            if text != delivered.as_str()
                && xxhash_rust::xxh3::xxh3_64(text.as_bytes()) == channel.original.digest()
            {
                *field = Value::String(delivered.clone());
                changed = true;
            }
        }
        if !changed {
            return frame.raw;
        }
        let Ok(data) = serde_json::to_vec(&event.json) else {
            return frame.raw;
        };
        let newline: &[u8] = if event.crlf { b"\r\n" } else { b"\n" };
        let mut out = Vec::with_capacity(frame.raw.len() + data.len());
        for line in &event.lines {
            match line {
                Some(line) => out.extend_from_slice(line),
                None => {
                    out.extend_from_slice(b"data: ");
                    out.extend_from_slice(&data);
                }
            }
            out.extend_from_slice(newline);
        }
        out.extend_from_slice(newline);
        Bytes::from(out)
    }

    /// Close the pipe after the last frame went out.
    fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.deadline = None;
        if self.failed {
            return;
        }
        let seq = self.next_seq;
        if let Ok(line) = serde_json::to_vec(&RestoreStreamEnd { seq, end: true }) {
            let mut line = line;
            line.push(b'\n');
            if let Some(tx) = &self.tx {
                let _ = tx.send(Ok(Bytes::from(line)));
            }
        }
        // a clean end of the request body; the reply to `end` is not awaited
        self.tx = None;
        self.pipe = Pipe::Closed;
        self.metrics.pii_restorations_total.fetch_add(1, Relaxed);
    }
}

impl Stream for RestoringStream {
    type Item = reqwest::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        loop {
            if let Some(bytes) = this.pop_ready() {
                return Poll::Ready(Some(Ok(bytes)));
            }
            if this.inner_done && this.frames.is_empty() {
                this.finish();
                return Poll::Ready(this.inner_error.take().map(Err));
            }
            let mut progressed = this.poll_pipe(cx);
            if let Some(deadline) = this.deadline.as_mut() {
                if deadline.as_mut().poll(cx).is_ready() {
                    this.fail(Failure::Timeout);
                    progressed = true;
                }
            }
            // events already read but held back by the window go first
            progressed |= this.take_events();
            let backlog = event_end(&this.partial).is_some();
            if this.upstream_ended && !this.inner_done && !backlog && this.window_has_room() {
                this.inner_done = true;
                this.end_of_upstream();
                progressed = true;
            }
            if !this.upstream_ended && !backlog && this.window_has_room() {
                match this.inner.as_mut().poll_next(cx) {
                    Poll::Ready(Some(Ok(chunk))) => {
                        this.partial.extend_from_slice(&chunk);
                        progressed = true;
                    }
                    Poll::Ready(Some(Err(err))) => {
                        // the upstream broke: deliver what is queued, then the
                        // error, as an unrestored stream would
                        this.inner_error = Some(err);
                        this.upstream_ended = true;
                        progressed = true;
                    }
                    Poll::Ready(None) => {
                        this.upstream_ended = true;
                        progressed = true;
                    }
                    Poll::Pending => {}
                }
            }
            if !progressed {
                return Poll::Pending;
            }
        }
    }
}

impl Drop for RestoringStream {
    // a client that hangs up drops the body mid-stream. an error on the request
    // body resets the pipe, so the sanitizer can discard the exchange's state,
    // and dropping `inner` drops the upstream request
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Err(std::io::Error::other("client disconnected")));
        }
    }
}

/// Where the first complete event in `buf` ends (after its blank line).
fn event_end(buf: &[u8]) -> Option<usize> {
    let mut at = 0;
    while let Some(nl) = buf[at..].iter().position(|b| *b == b'\n') {
        let next = at + nl + 1;
        match buf.get(next..) {
            Some([b'\n', ..]) => return Some(next + 1),
            Some([b'\r', b'\n', ..]) => return Some(next + 2),
            _ => at = next,
        }
    }
    None
}

fn pointer_mut<'v>(value: &'v mut Value, path: &[Seg]) -> Option<&'v mut Value> {
    path.iter().try_fold(value, |value, seg| match seg {
        Seg::Key(key) => value.get_mut(*key),
        Seg::Index(index) => value.get_mut(*index),
    })
}

fn index_of(value: &Value, key: &str, position: usize) -> u64 {
    value
        .get(key)
        .and_then(Value::as_u64)
        .unwrap_or(position as u64)
}

/// Parse one SSE event: its text slots, the channels it closes (`None` closes
/// every channel) and, for the Responses API, the fields that repeat a whole
/// channel. An event with none of those is forwarded as it came.
fn parse_event(raw: &Bytes) -> (Option<Event>, Vec<Option<String>>) {
    let crlf = raw.windows(2).any(|w| w == b"\r\n");
    let mut lines = Vec::new();
    let mut data: Vec<u8> = Vec::new();
    let mut data_lines = 0;
    for line in raw.split(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        if let Some(value) = line.strip_prefix(b"data:") {
            let value = value.strip_prefix(b" ").unwrap_or(value);
            if data_lines > 0 {
                data.push(b'\n');
            } else {
                lines.push(None);
            }
            data.extend_from_slice(value);
            data_lines += 1;
        } else {
            lines.push(Some(raw.slice_ref(line)));
        }
    }
    if data_lines == 0 {
        return (None, Vec::new());
    }
    let Ok(json) = serde_json::from_slice::<Value>(&data) else {
        return (None, Vec::new());
    };
    let mut slots = Vec::new();
    let mut closes = Vec::new();
    let mut full_text = Vec::new();
    if let Some(choices) = json.get("choices").and_then(Value::as_array) {
        openai_chat(choices, &mut slots, &mut closes);
    } else if let Some(kind) = json.get("type").and_then(Value::as_str) {
        if kind.starts_with("response.") {
            responses(kind, &json, &mut slots, &mut closes, &mut full_text);
        } else {
            anthropic(kind, &json, &mut slots, &mut closes);
        }
    }
    if slots.is_empty() && closes.is_empty() && full_text.is_empty() {
        return (None, Vec::new());
    }
    (
        Some(Event {
            lines,
            crlf,
            json,
            slots,
            full_text,
        }),
        closes,
    )
}

fn slot(channel: String, path: Vec<Seg>, text: &str, json: bool, responses: bool) -> Slot {
    Slot {
        channel,
        path,
        original: text.to_string(),
        out: None,
        json,
        responses,
    }
}

/// OpenAI chat completions (and legacy completions) chunks: one channel per
/// choice and text field, one per tool call's arguments.
fn openai_chat(choices: &[Value], slots: &mut Vec<Slot>, closes: &mut Vec<Option<String>>) {
    for (position, choice) in choices.iter().enumerate() {
        let index = index_of(choice, "index", position);
        let base = vec![Seg::Key("choices"), Seg::Index(position)];
        if let Some(delta) = choice.get("delta") {
            for field in ["content", "reasoning_content", "reasoning", "refusal"] {
                if let Some(text) = delta.get(field).and_then(Value::as_str) {
                    if !text.is_empty() {
                        let mut path = base.clone();
                        path.extend([Seg::Key("delta"), Seg::Key(field)]);
                        slots.push(slot(format!("{index}/{field}"), path, text, false, false));
                    }
                }
            }
            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for (call_position, call) in calls.iter().enumerate() {
                    let call_index = index_of(call, "index", call_position);
                    let Some(text) = call
                        .get("function")
                        .and_then(|f| f.get("arguments"))
                        .and_then(Value::as_str)
                    else {
                        continue;
                    };
                    if text.is_empty() {
                        continue;
                    }
                    let mut path = base.clone();
                    path.extend([
                        Seg::Key("delta"),
                        Seg::Key("tool_calls"),
                        Seg::Index(call_position),
                        Seg::Key("function"),
                        Seg::Key("arguments"),
                    ]);
                    slots.push(slot(
                        format!("{index}/tool_calls/{call_index}"),
                        path,
                        text,
                        true,
                        false,
                    ));
                }
            }
        }
        if let Some(text) = choice.get("text").and_then(Value::as_str) {
            if !text.is_empty() {
                let mut path = base.clone();
                path.push(Seg::Key("text"));
                slots.push(slot(format!("{index}/text"), path, text, false, false));
            }
        }
        // some providers send `""` rather than `null` on every chunk
        if choice
            .get("finish_reason")
            .is_some_and(|reason| !reason.is_null() && reason.as_str() != Some(""))
        {
            closes.push(Some(index.to_string()));
        }
    }
}

/// Anthropic messages events: one channel per content block.
fn anthropic(kind: &str, json: &Value, slots: &mut Vec<Slot>, closes: &mut Vec<Option<String>>) {
    let index = index_of(json, "index", 0);
    match kind {
        "content_block_delta" => {
            let Some(delta) = json.get("delta") else {
                return;
            };
            let (field, is_json) = match delta.get("type").and_then(Value::as_str) {
                Some("text_delta") => ("text", false),
                Some("thinking_delta") => ("thinking", false),
                Some("input_json_delta") => ("partial_json", true),
                _ => return,
            };
            if let Some(text) = delta.get(field).and_then(Value::as_str) {
                if !text.is_empty() {
                    slots.push(slot(
                        format!("{index}/{field}"),
                        vec![Seg::Key("delta"), Seg::Key(field)],
                        text,
                        is_json,
                        false,
                    ));
                }
            }
        }
        "content_block_stop" => closes.push(Some(index.to_string())),
        "message_stop" => closes.push(None),
        _ => {}
    }
}

/// Responses API events: a channel per output item and content part (or
/// summary part, or a function call's arguments). Done events and the final
/// response repeat each channel's whole text, so those fields are restored to
/// what the channel delivered.
fn responses(
    kind: &str,
    json: &Value,
    slots: &mut Vec<Slot>,
    closes: &mut Vec<Option<String>>,
    full_text: &mut Vec<FullText>,
) {
    let output = index_of(json, "output_index", 0);
    let content = index_of(json, "content_index", 0);
    let summary = index_of(json, "summary_index", 0);
    let part = format!("{output}/{content}");
    let summary_part = format!("{output}/summary/{summary}");
    let arguments = format!("{output}/arguments");
    let mut delta = |channel: String, is_json: bool| {
        if let Some(text) = json.get("delta").and_then(Value::as_str) {
            if !text.is_empty() {
                slots.push(slot(channel, vec![Seg::Key("delta")], text, is_json, true));
            }
        }
    };
    match kind {
        "response.output_text.delta"
        | "response.reasoning_text.delta"
        | "response.refusal.delta" => delta(part, false),
        "response.reasoning_summary_text.delta" => delta(summary_part, false),
        "response.function_call_arguments.delta" => delta(arguments, true),
        "response.output_text.done" | "response.reasoning_text.done" => {
            full_text.push(FullText {
                channel: part.clone(),
                path: vec![Seg::Key("text")],
            });
            closes.push(Some(part));
        }
        "response.refusal.done" => {
            full_text.push(FullText {
                channel: part.clone(),
                path: vec![Seg::Key("refusal")],
            });
            closes.push(Some(part));
        }
        "response.reasoning_summary_text.done" => {
            full_text.push(FullText {
                channel: summary_part.clone(),
                path: vec![Seg::Key("text")],
            });
            closes.push(Some(summary_part));
        }
        "response.function_call_arguments.done" => {
            full_text.push(FullText {
                channel: arguments.clone(),
                path: vec![Seg::Key("arguments")],
            });
            closes.push(Some(arguments));
        }
        "response.content_part.done" => {
            for field in ["text", "refusal"] {
                full_text.push(FullText {
                    channel: part.clone(),
                    path: vec![Seg::Key("part"), Seg::Key(field)],
                });
            }
        }
        "response.reasoning_summary_part.done" => full_text.push(FullText {
            channel: summary_part,
            path: vec![Seg::Key("part"), Seg::Key("text")],
        }),
        "response.output_item.done" => {
            if let Some(item) = json.get("item") {
                item_full_text(output, item, vec![Seg::Key("item")], full_text);
            }
            closes.push(Some(output.to_string()));
        }
        "response.completed" | "response.incomplete" | "response.failed" => {
            if let Some(items) = json
                .get("response")
                .and_then(|r| r.get("output"))
                .and_then(Value::as_array)
            {
                for (position, item) in items.iter().enumerate() {
                    item_full_text(
                        position as u64,
                        item,
                        vec![
                            Seg::Key("response"),
                            Seg::Key("output"),
                            Seg::Index(position),
                        ],
                        full_text,
                    );
                }
            }
            closes.push(None);
        }
        _ => {}
    }
}

/// The fields of one output item that repeat a channel's whole text.
fn item_full_text(output: u64, item: &Value, base: Vec<Seg>, full_text: &mut Vec<FullText>) {
    for (list, channel_of) in [("content", None), ("summary", Some("summary"))] {
        let Some(parts) = item.get(list).and_then(Value::as_array) else {
            continue;
        };
        for position in 0..parts.len() {
            let channel = match channel_of {
                None => format!("{output}/{position}"),
                Some(kind) => format!("{output}/{kind}/{position}"),
            };
            for field in ["text", "refusal"] {
                let mut path = base.clone();
                path.extend([Seg::Key(list), Seg::Index(position), Seg::Key(field)]);
                full_text.push(FullText {
                    channel: channel.clone(),
                    path,
                });
            }
        }
    }
    let mut path = base;
    path.push(Seg::Key("arguments"));
    full_text.push(FullText {
        channel: format!("{output}/arguments"),
        path,
    });
}
