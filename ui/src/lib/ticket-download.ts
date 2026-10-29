// Handing an export to the browser's download manager (#3504). Where the File
// System Access API is missing the page cannot write a file as it arrives, and
// reading the body into the tab is what a million-row export must not do. A
// link the download manager follows cannot carry the bearer header, so the
// control plane mints a single-use ticket for the export and the link carries
// that. The page never sees the response, so it learns how the download went by
// polling the ticket's status. See docs/dev-docs/adr/2026-10-29-export-download-tickets.md.

import i18n from "@/lib/i18n";
import {
  ApiError,
  exportTicketStatus,
  mintExportTicket,
  type ExportRequest,
  type ExportTicketStatus,
} from "@/lib/api";
import { ExportTruncatedError } from "@/lib/stream-download";

/** how often the status is read while a download runs */
export const POLL_EVERY_MS = 1000;

/** failed status reads in a row before the page gives up on following */
export const POLL_FAILURES_TOLERATED = 3;

/** how long the status may stay the same before the page stops following */
export const STALL_AFTER_MS = 5 * 60 * 1000;

/**
 * The mint of an older control plane refused the export as one it has no
 * ticket for: one from before the audit log's exports could be downloaded this
 * way (#3545). Its answer is a `400` naming the paths it does take, which a
 * screen reads as "this export is not there" during a rolling upgrade, as it
 * does a `404` from one with no tickets at all.
 */
export function isUnknownExport(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    error.status === 400 &&
    error.message.startsWith("path must be one of")
  );
}

/** the download was answered with an error rather than a file, other than a 429 */
export class ExportRefusedError extends Error {
  readonly status: number;
  constructor(status: number) {
    super(`the export download was refused with ${status}`);
    this.name = "ExportRefusedError";
    this.status = status;
  }
}

/**
 * The body was cut short while the browser's download manager was receiving
 * it. Unlike the stream-to-file path, the page cannot discard the partial
 * file: the browser marks the download failed, and the message says so.
 */
export class DownloadTruncatedError extends ExportTruncatedError {
  constructor() {
    super();
    this.name = "DownloadTruncatedError";
  }
}

/**
 * The browser's download stopped before its end, most likely because the
 * person cancelled it in the download list: not a failure to report, only a
 * file that is not whole.
 */
export class DownloadStoppedError extends Error {
  constructor() {
    super("the browser stopped the export download");
    this.name = "DownloadStoppedError";
  }
}

/** the status stopped changing for {@link STALL_AFTER_MS}: the page stops following */
export class ExportStalledError extends Error {
  constructor() {
    super("the export download stopped reporting progress");
    this.name = "ExportStalledError";
  }
}

/** the browser never followed the link before the ticket lapsed */
export class ExportNotStartedError extends Error {
  constructor() {
    super("the browser did not start the export download");
    this.name = "ExportNotStartedError";
  }
}

/**
 * Follow `url` with a hidden same-origin `<a download>`, so the browser's
 * download manager receives the file and the page holds none of it. No
 * referrer is sent: the link carries a secret.
 */
export function followDownload(url: string): void {
  const link = document.createElement("a");
  link.href = url;
  link.download = "";
  link.rel = "noreferrer";
  link.style.display = "none";
  document.body.append(link);
  try {
    link.click();
  } finally {
    link.remove();
  }
}

export interface TicketDownloadOptions {
  signal?: AbortSignal;
  /** rows written so far, each time the status reports more */
  onProgress?: (rows: number) => void;
  /** test seams: how to follow the link, how long to wait between reads and before giving up */
  follow?: (url: string) => void;
  pollEveryMs?: number;
  stallAfterMs?: number;
}

function wait(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) return reject(signal.reason);
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    const onAbort = () => {
      clearTimeout(timer);
      reject(signal?.reason);
    };
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

/** the error a terminal status that is not `complete` stands for */
function failure(status: ExportTicketStatus): Error {
  switch (status.state) {
    case "truncated":
      return status.reason === "client_closed"
        ? new DownloadStoppedError()
        : new DownloadTruncatedError();
    case "refused":
      // the same error a 429 from the bearer route throws, so the button's
      // translated "wait and retry" message applies unchanged
      return status.status === 429
        ? new ApiError(i18n.t("analytics.export.busy"), 429, undefined, status.retry_after)
        : new ExportRefusedError(status.status);
    default:
      return new ExportNotStartedError();
  }
}

/**
 * Mint a ticket for `request`, start the download, and follow it until it
 * ends. Resolves with the rows written once the file is complete. Rejects with
 * the mint's own `ApiError` (a `429` included), {@link ExportTruncatedError}
 * for a body cut short, an `ApiError` 429 for a gate that filled before the
 * download began, {@link ExportRefusedError} for any other refusal, and
 * {@link ExportNotStartedError} when the link was never followed,
 * {@link DownloadStoppedError} when the browser stopped reading, and
 * {@link ExportStalledError} when the status has not changed for
 * {@link STALL_AFTER_MS}. A status read that fails is tried again, up to
 * {@link POLL_FAILURES_TOLERATED} in a row. Aborting `signal` stops watching;
 * the browser's download, which the page cannot reach, carries on.
 */
export async function downloadWithTicket(
  request: ExportRequest,
  options: TicketDownloadOptions = {},
): Promise<number> {
  const {
    signal,
    onProgress,
    follow = followDownload,
    pollEveryMs = POLL_EVERY_MS,
    stallAfterMs = STALL_AFTER_MS,
  } = options;
  const ticket = await mintExportTicket(request);
  signal?.throwIfAborted();
  follow(ticket.download_url);
  let shown = -1;
  let failures = 0;
  let last = "";
  let changedAt = Date.now();
  for (;;) {
    await wait(pollEveryMs, signal);
    let status: ExportTicketStatus;
    try {
      status = await exportTicketStatus(ticket.id);
      failures = 0;
    } catch (error) {
      signal?.throwIfAborted();
      // a blip in the control plane is not the end of a download the browser
      // is still receiving; a run of failures is
      failures += 1;
      if (failures >= POLL_FAILURES_TOLERATED) throw error;
      continue;
    }
    const seen = JSON.stringify([status.state, "rows" in status ? status.rows : null]);
    if (seen !== last) {
      last = seen;
      changedAt = Date.now();
    } else if (Date.now() - changedAt >= stallAfterMs) {
      throw new ExportStalledError();
    }
    if (status.state === "streaming" || status.state === "complete") {
      if (status.rows !== shown) {
        shown = status.rows;
        onProgress?.(status.rows);
      }
    }
    if (status.state === "complete") return status.rows;
    if (status.done) throw failure(status);
  }
}
