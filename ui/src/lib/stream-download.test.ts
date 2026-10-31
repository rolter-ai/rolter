import { describe, expect, it } from "bun:test";

import { ExportTruncatedError, pump, type DownloadSink } from "./stream-download";

const bytes = (text: string) => new TextEncoder().encode(text);

function body(chunks: string[], then: "end" | "cut" | "hang") {
  let sent = 0;
  return new ReadableStream<Uint8Array>({
    pull(controller) {
      if (sent < chunks.length) controller.enqueue(bytes(chunks[sent++]!));
      else if (then === "end") controller.close();
      else if (then === "cut") controller.error(new TypeError("network error"));
      else return new Promise<void>(() => undefined);
    },
  });
}

/** an in-memory file, to see what `pump` wrote and whether it was dropped */
function memorySink() {
  const state = { text: "", closed: false, aborted: false };
  const decoder = new TextDecoder();
  const sink: DownloadSink = {
    write: async (chunk) => {
      state.text += decoder.decode(chunk, { stream: true });
    },
    close: async () => {
      state.closed = true;
    },
    abort: async () => {
      state.aborted = true;
      state.text = "";
    },
  };
  return { sink, state };
}

describe("pump", () => {
  it("writes every chunk, reports the bytes and closes the sink", async () => {
    const { sink, state } = memorySink();
    const seen: number[] = [];
    await pump(body(["ab", "cde"], "end"), sink, { onProgress: (n) => seen.push(n) });
    expect(seen).toEqual([2, 5]);
    expect(state).toEqual({ text: "abcde", closed: true, aborted: false });
  });

  it("a body that fails part way is a truncated export and the sink is dropped", async () => {
    const { sink, state } = memorySink();
    await expect(pump(body(["ab"], "cut"), sink)).rejects.toBeInstanceOf(ExportTruncatedError);
    expect(state).toEqual({ text: "", closed: false, aborted: true });
  });

  it("an abort ends a read that is waiting for a chunk", async () => {
    const { sink, state } = memorySink();
    const controller = new AbortController();
    const done = pump(body(["ab"], "hang"), sink, {
      signal: controller.signal,
      onProgress: () => controller.abort(),
    });
    await expect(done).rejects.toMatchObject({ name: "AbortError" });
    expect(state.aborted).toBe(true);
  });

  it("a sink that cannot write is reported as it is, not as a short body", async () => {
    const failing: DownloadSink = {
      write: async () => {
        throw new DOMException("disk full", "QuotaExceededError");
      },
      close: async () => undefined,
      abort: async () => undefined,
    };
    await expect(pump(body(["ab"], "end"), failing)).rejects.toMatchObject({
      name: "QuotaExceededError",
    });
  });
});
