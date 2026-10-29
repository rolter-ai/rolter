import { afterEach, beforeEach, describe, expect, it } from "bun:test";

import { ApiError } from "./api";
import { ExportTruncatedError } from "./stream-download";
import {
  DownloadStoppedError,
  DownloadTruncatedError,
  downloadWithTicket,
  ExportNotStartedError,
  ExportRefusedError,
  ExportStalledError,
} from "./ticket-download";

const originalFetch = globalThis.fetch;

const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...headers },
  });

const TICKET = {
  id: "7d1c0a52-1f0e-4a39-9a43-0f8e5b9f1a10",
  download_url: "/api/v1/analytics/exports/download/rolter_dl_abc",
  status_url: "/api/v1/analytics/exports/tickets/7d1c0a52-1f0e-4a39-9a43-0f8e5b9f1a10",
  expires_at: "2026-10-29T12:01:00Z",
};

const status = (state: Record<string, unknown>, done = false) => ({
  id: TICKET.id,
  export: "invocations",
  expires_at: TICKET.expires_at,
  done,
  ...state,
});

/**
 * A control plane that mints `TICKET` (or answers `mint`) and then answers the
 * status reads with `reads` in turn, recording every request.
 */
function controlPlane(reads: unknown[], mint: () => Response = () => json(TICKET)) {
  const calls: { method: string; url: string; body?: string }[] = [];
  let read = 0;
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    calls.push({ method: init?.method ?? "GET", url, body: init?.body as string | undefined });
    if (url === "/api/v1/analytics/exports/tickets") return mint();
    if (url.startsWith("/api/v1/analytics/exports/tickets/")) {
      const answer = reads[Math.min(read, reads.length - 1)];
      read += 1;
      // a read the control plane failed to answer
      if (answer === "fail") return json({ error: { message: "unavailable" } }, 503);
      return json(answer);
    }
    return json({ error: { message: "unexpected" } }, 404);
  }) as typeof fetch;
  return calls;
}

const REQUEST = { path: "/api/v1/analytics/invocations/export", query: "status=error" };

describe("downloadWithTicket", () => {
  let followed: string[];
  beforeEach(() => {
    followed = [];
  });
  afterEach(() => {
    globalThis.fetch = originalFetch;
  });
  const options = (extra: Partial<Parameters<typeof downloadWithTicket>[1]> = {}) => ({
    follow: (url: string) => followed.push(url),
    pollEveryMs: 0,
    ...extra,
  });

  it("mints for the export, follows the link once and reports rows until complete", async () => {
    const calls = controlPlane([
      status({ state: "pending" }),
      status({ state: "streaming", rows: 1000 }),
      status({ state: "streaming", rows: 1000 }),
      status({ state: "streaming", rows: 2000 }),
      status({ state: "complete", rows: 2500 }, true),
    ]);
    const seen: number[] = [];
    const rows = await downloadWithTicket(REQUEST, options({ onProgress: (n) => seen.push(n) }));
    expect(rows).toBe(2500);
    // a repeated count is not announced twice
    expect(seen).toEqual([1000, 2000, 2500]);
    expect(followed).toEqual([TICKET.download_url]);
    const mint = calls[0]!;
    expect(mint.method).toBe("POST");
    expect(JSON.parse(mint.body!)).toEqual(REQUEST);
    // the download link itself is never fetched by the page
    expect(calls.some((c) => c.url.includes("/exports/download/"))).toBe(false);
  });

  it("is the mint's own 429 when the gate is full, and follows nothing", async () => {
    controlPlane([], () => json({ error: { message: "busy" } }, 429, { "retry-after": "15" }));
    const error = await downloadWithTicket(REQUEST, options()).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).status).toBe(429);
    expect((error as ApiError).retryAfterSeconds).toBe(15);
    expect(followed).toEqual([]);
  });

  it("turns a 429 at download time into the same error, with its wait", async () => {
    controlPlane([status({ state: "refused", status: 429, retry_after: 15 }, true)]);
    const error = await downloadWithTicket(REQUEST, options()).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).status).toBe(429);
    expect((error as ApiError).retryAfterSeconds).toBe(15);
  });

  it("ends a truncated export in the truncated error", async () => {
    controlPlane([
      status({ state: "streaming", rows: 1500 }),
      status({ state: "truncated", rows: 1500, reason: "row_limit" }, true),
    ]);
    const error = await downloadWithTicket(REQUEST, options()).catch((e: unknown) => e);
    // the truncated error the button already words, as the download's own kind
    expect(error).toBeInstanceOf(ExportTruncatedError);
    expect(error).toBeInstanceOf(DownloadTruncatedError);
  });

  it("names any other refusal and a link that was never followed", async () => {
    controlPlane([status({ state: "refused", status: 401 }, true)]);
    const refused = await downloadWithTicket(REQUEST, options()).catch((e: unknown) => e);
    expect(refused).toBeInstanceOf(ExportRefusedError);
    expect((refused as ExportRefusedError).status).toBe(401);
    controlPlane([status({ state: "pending" }), status({ state: "expired" }, true)]);
    await expect(downloadWithTicket(REQUEST, options())).rejects.toBeInstanceOf(
      ExportNotStartedError,
    );
  });

  it("stops polling when aborted", async () => {
    const calls = controlPlane([status({ state: "streaming", rows: 1 })]);
    const abort = new AbortController();
    const running = downloadWithTicket(
      REQUEST,
      options({ signal: abort.signal, pollEveryMs: 5, onProgress: () => abort.abort() }),
    );
    await expect(running).rejects.toBeDefined();
    const reads = calls.filter((c) => c.url.startsWith("/api/v1/analytics/exports/tickets/"));
    expect(reads).toHaveLength(1);
  });

  it("rides out a short run of failed status reads", async () => {
    controlPlane(["fail", "fail", status({ state: "complete", rows: 4 }, true)]);
    await expect(downloadWithTicket(REQUEST, options())).resolves.toBe(4);
  });

  it("gives up after three failed status reads in a row", async () => {
    controlPlane(["fail", "fail", "fail", status({ state: "complete", rows: 4 }, true)]);
    const error = await downloadWithTicket(REQUEST, options()).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).status).toBe(503);
  });

  it("stops following a download whose status no longer changes", async () => {
    controlPlane([status({ state: "streaming", rows: 10 })]);
    await expect(
      downloadWithTicket(REQUEST, options({ pollEveryMs: 1, stallAfterMs: 20 })),
    ).rejects.toBeInstanceOf(ExportStalledError);
  });

  it("reads a download the browser stopped as stopped, not failed", async () => {
    controlPlane([status({ state: "truncated", rows: 3, reason: "client_closed" }, true)]);
    const error = await downloadWithTicket(REQUEST, options()).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(DownloadStoppedError);
    expect(error).not.toBeInstanceOf(ExportTruncatedError);
  });
});
