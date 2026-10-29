import { describe, expect, test } from "bun:test";

import { blobSink, ExportTruncatedError, pump } from "./stream-download";

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

describe("pump", () => {
  test("writes every chunk, reports the bytes and closes the sink", async () => {
    const sink = blobSink();
    const seen: number[] = [];
    await pump(body(["ab", "cde"], "end"), sink, { onProgress: (n) => seen.push(n) });
    expect(seen).toEqual([2, 5]);
    expect(await sink.blob("text/csv").text()).toBe("abcde");
  });

  test("a body that fails part way is a truncated export and the sink is dropped", async () => {
    const sink = blobSink();
    await expect(pump(body(["ab"], "cut"), sink)).rejects.toBeInstanceOf(ExportTruncatedError);
    expect(await sink.blob("text/csv").text()).toBe("");
  });

  test("an abort ends a read that is waiting for a chunk", async () => {
    const sink = blobSink();
    const controller = new AbortController();
    const done = pump(body(["ab"], "hang"), sink, {
      signal: controller.signal,
      onProgress: () => controller.abort(),
    });
    await expect(done).rejects.toMatchObject({ name: "AbortError" });
    expect(await sink.blob("text/csv").text()).toBe("");
  });

  test("a sink that cannot write is reported as it is, not as a short body", async () => {
    const failing = {
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
