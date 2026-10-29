// Saving a streamed file without holding it (#3434). The request-log export can
// run to a million rows, so where the browser lets a page write straight to a
// file it chose (the File System Access API) the body is copied there a chunk
// at a time. Elsewhere it is handed to the browser's own Blob, which keeps the
// body outside the JavaScript heap: collecting chunks into an array here would
// hold the export twice over.

/** where a streamed body goes */
export interface DownloadSink {
  write(chunk: Uint8Array): Promise<void>;
  /** the body ended whole */
  close(): Promise<void>;
  /** the body did not end whole: drop what was written so far */
  abort(): Promise<void>;
}

/** the body stopped before its end: a row or time cap, or a dropped connection */
export class ExportTruncatedError extends Error {
  constructor() {
    super("the export ended before its last row");
    this.name = "ExportTruncatedError";
  }
}

// the slice of the File System Access API used here; lib.dom does not carry it
interface WritableFile {
  write(chunk: Uint8Array): Promise<void>;
  close(): Promise<void>;
  abort(reason?: unknown): Promise<void>;
}
interface SavedFileHandle {
  createWritable(): Promise<WritableFile>;
  getFile?(): Promise<{ size: number }>;
  /** Chromium only */
  remove?(): Promise<void>;
}
interface SavePicker {
  (options: {
    suggestedName?: string;
    types?: { description?: string; accept: Record<string, string[]> }[];
  }): Promise<SavedFileHandle>;
}

function picker(): SavePicker | undefined {
  const fn = (window as { showSaveFilePicker?: SavePicker }).showSaveFilePicker;
  return typeof fn === "function" ? fn.bind(window) : undefined;
}

/** this browser can write a download to a file as it arrives */
export function diskSaveSupported(): boolean {
  return picker() !== undefined;
}

/**
 * Ask where to save, and open that file for writing. Resolves to `null` when
 * the person closed the dialog. It has to run straight from the click: the
 * dialog needs the click's user activation, which a download that has already
 * waited on the network may no longer have.
 */
export async function chooseDiskSink(suggestedName: string): Promise<DownloadSink | null> {
  const pick = picker();
  if (!pick) return null;
  let writable: WritableFile;
  let handle: SavedFileHandle;
  let empty = false;
  try {
    handle = await pick({
      suggestedName,
      types: [{ description: "CSV", accept: { "text/csv": [".csv"] } }],
    });
    // the picker creates a file that was not there, at zero bytes. one that was
    // empty when picked is one an aborted export may take away again; a file
    // with content is the person's and is left as it was
    empty = ((await handle.getFile?.().catch(() => undefined))?.size ?? 1) === 0;
    writable = await handle.createWritable();
  } catch (error) {
    if (error instanceof DOMException && error.name === "AbortError") return null;
    throw error;
  }
  return {
    write: (chunk) => writable.write(chunk),
    close: () => writable.close(),
    abort: async () => {
      // the writable stages into a swap file, so aborting drops what was written
      await writable.abort().catch(() => undefined);
      // and the empty file the picker made is not left behind, where the browser
      // lets a page remove it (best effort: the rest of the export is unaffected)
      if (empty) await handle.remove?.().catch(() => undefined);
    },
  };
}

/**
 * Gather `stream` into a Blob without holding it in the page. The body is
 * piped through a counter into the browser's own `Response.blob()`, which keeps
 * it outside the JavaScript heap and reports the bytes as they pass. Aborting
 * `signal` stops it with the `AbortError`; a stream that fails any other way is
 * a body cut short, which is {@link ExportTruncatedError}.
 */
export async function collectBlob(
  stream: ReadableStream<Uint8Array> | null,
  options: { signal?: AbortSignal; onProgress?: (bytes: number) => void } = {},
): Promise<Blob> {
  const { signal, onProgress } = options;
  if (!stream) return new Blob([]);
  let seen = 0;
  const counter = new TransformStream<Uint8Array, Uint8Array>({
    transform(chunk, controller) {
      seen += chunk.byteLength;
      onProgress?.(seen);
      controller.enqueue(chunk);
    },
  });
  try {
    return await new Response(stream.pipeThrough(counter, { signal })).blob();
  } catch (error) {
    if (signal?.aborted) throw signal.reason ?? error;
    if (error instanceof DOMException && error.name === "AbortError") throw error;
    if (error instanceof TypeError) throw new ExportTruncatedError();
    throw error;
  }
}

/**
 * Copy `stream` into `sink`, reporting the bytes written so far. Aborting
 * `signal` stops it with the `AbortError`; a stream that fails any other way
 * is a body cut short, which is {@link ExportTruncatedError}. Either way the
 * sink is aborted first, so a half-written file is never kept.
 */
export async function pump(
  stream: ReadableStream<Uint8Array> | null,
  sink: DownloadSink,
  options: { signal?: AbortSignal; onProgress?: (bytes: number) => void } = {},
): Promise<void> {
  const { signal, onProgress } = options;
  const reader = stream?.getReader();
  let written = 0;
  // a read parked on a connection that went quiet is ended by the abort, not
  // left waiting for a chunk that will not come
  const aborted = new Promise<never>((_, reject) => {
    signal?.addEventListener("abort", () => reject(signal.reason), { once: true });
  });
  aborted.catch(() => undefined);
  try {
    for (;;) {
      signal?.throwIfAborted();
      const next = reader ? await Promise.race([reader.read(), aborted]) : undefined;
      if (!next || next.done) break;
      await sink.write(next.value);
      written += next.value.byteLength;
      onProgress?.(written);
    }
    await sink.close();
  } catch (error) {
    await sink.abort();
    await reader?.cancel().catch(() => undefined);
    if (signal?.aborted) throw signal.reason ?? error;
    if (error instanceof DOMException && error.name === "AbortError") throw error;
    // a read that fails is the connection or the server ending the body early;
    // a failing write is the disk, which is the person's to hear about as it is
    if (error instanceof TypeError) throw new ExportTruncatedError();
    throw error;
  }
}
