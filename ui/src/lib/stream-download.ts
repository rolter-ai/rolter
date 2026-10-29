// Saving a streamed file without holding it (#3434). The request-log export can
// run to a million rows, so where the browser lets a page write straight to a
// file it chosen (the File System Access API) the body is copied there a chunk
// at a time; elsewhere it is gathered into a Blob and handed over at the end,
// which is the only way left, and costs the memory the other path saves.

/** where a streamed body goes */
export interface DownloadSink {
  write(chunk: Uint8Array): Promise<void>;
  /** the body ended whole */
  close(): Promise<void>;
  /** the body did not end whole: drop what was written so far */
  abort(): Promise<void>;
}

/** a sink that gathers the body in memory, for browsers that cannot write to disk */
export interface BlobSink extends DownloadSink {
  /** the gathered body, once `close` has run */
  blob(type: string): Blob;
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
interface SavePicker {
  (options: {
    suggestedName?: string;
    types?: { description?: string; accept: Record<string, string[]> }[];
  }): Promise<{ createWritable(): Promise<WritableFile> }>;
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
  try {
    const handle = await pick({
      suggestedName,
      types: [{ description: "CSV", accept: { "text/csv": [".csv"] } }],
    });
    writable = await handle.createWritable();
  } catch (error) {
    if (error instanceof DOMException && error.name === "AbortError") return null;
    throw error;
  }
  return {
    write: (chunk) => writable.write(chunk),
    close: () => writable.close(),
    // the writable stages into a swap file, so aborting leaves nothing behind
    abort: () => writable.abort().catch(() => undefined),
  };
}

/** a sink that gathers the body in memory */
export function blobSink(): BlobSink {
  let chunks: Uint8Array[] = [];
  return {
    write: async (chunk) => {
      chunks.push(chunk);
    },
    close: async () => undefined,
    abort: async () => {
      chunks = [];
    },
    blob: (type) => new Blob(chunks as BlobPart[], { type }),
  };
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
