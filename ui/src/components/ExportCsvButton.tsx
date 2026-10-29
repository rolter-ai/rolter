import { useMutation } from "@tanstack/react-query";
import { Download, X } from "lucide-react";
import * as React from "react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { ApiError, type AnalyticsExportStream } from "@/lib/api";
import { downloadBlob } from "@/lib/audit-export";
import { errorText } from "@/lib/error-copy";
import { useFormat } from "@/lib/i18n/format";
import {
  chooseDiskSink,
  collectBlob,
  diskSaveSupported,
  ExportTruncatedError,
  pump,
  type DownloadSink,
} from "@/lib/stream-download";
import { useToast } from "@/lib/toast";

/**
 * How an export ended, when it ended without a failure. `dismissed` is the save
 * dialog being closed before anything was asked: a no that needs no toast.
 */
type Outcome = "saved" | "downloaded" | "cancelled" | "dismissed";

/** how often the byte count under an export is redrawn */
const PROGRESS_EVERY_MS = 1000;

/**
 * The Export button of a screen whose figures end in a spreadsheet (#1838).
 *
 * `run` asks the control plane for the file, which it builds under the caller's
 * own row scope and names after its window and currency; the button hands it
 * to the browser. A failure is a toast, since the screen has nowhere else for
 * it, and a file never starts downloading half built.
 *
 * With `toDisk`, a browser that can write to a chosen file (the File System
 * Access API) streams the body there as it arrives, so a request log of a
 * million rows is never held in the tab (#3434); it asks where to save first,
 * under `fallbackName`, since the dialog needs the click and the server's own
 * name is only known once the request has answered. Any other browser gathers
 * the body and downloads it under the server's name, as before. Either way the
 * button shows the bytes received and a Cancel that stops the request.
 */
export function ExportCsvButton({
  run,
  fallbackName,
  what,
  disabled,
  toDisk = false,
}: {
  run: (signal: AbortSignal) => Promise<AnalyticsExportStream>;
  /** used only if the server sent no file name, and as the suggestion when saving to disk */
  fallbackName: string;
  /** the translated noun a failure names, such as "request log export" */
  what: string;
  disabled?: boolean;
  /** stream to a file the person picks where the browser allows it */
  toDisk?: boolean;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const toast = useToast();
  const [bytes, setBytes] = React.useState(0);
  const controller = React.useRef<AbortController | null>(null);
  // the count is redrawn about once a second, not per chunk: a screen reader
  // announces each change of a live region, and a chunk is a few kilobytes
  const shownAt = React.useRef(0);
  const progress = (received: number) => {
    const now = Date.now();
    if (shownAt.current === 0 || now - shownAt.current >= PROGRESS_EVERY_MS) {
      shownAt.current = now;
      setBytes(received);
    }
  };
  // an export outlives nothing: leaving the screen stops the request and drops
  // the half-written file instead of finishing a download nobody is waiting on
  const mounted = React.useRef(true);
  React.useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      controller.current?.abort();
    };
  }, []);
  const exportFile = useMutation<Outcome>({
    mutationFn: async () => {
      let disk: DownloadSink | null = null;
      if (toDisk && diskSaveSupported()) {
        disk = await chooseDiskSink(fallbackName);
        // the dialog was closed: nothing was asked of the server
        if (!disk) return "dismissed";
      }
      const abort = new AbortController();
      controller.current = abort;
      shownAt.current = 0;
      setBytes(0);
      try {
        const file = await run(abort.signal);
        if (disk) {
          await pump(file.stream, disk, { signal: abort.signal, onProgress: progress });
          return "saved";
        }
        const blob = await collectBlob(file.stream, {
          signal: abort.signal,
          onProgress: progress,
        });
        downloadBlob(file.name ?? fallbackName, blob);
        return "downloaded";
      } catch (error) {
        // pump has dropped the file already when it failed; a request that
        // failed before any byte came (a 429, a refusal) has not
        await disk?.abort();
        if (abort.signal.aborted) return "cancelled";
        throw error;
      } finally {
        controller.current = null;
      }
    },
    onSuccess: (outcome) => {
      if (outcome === "dismissed" || !mounted.current) return;
      if (outcome === "cancelled") {
        toast.push({ tone: "info", title: t("analytics.export.cancelled") });
      } else {
        // the disk path saves under the name the person picked, so the sentence
        // about the server's file name is only for the download
        toast.push({
          tone: "success",
          title: t(outcome === "saved" ? "analytics.export.saved" : "analytics.export.downloaded"),
        });
      }
    },
    onError: (error) =>
      toast.push({
        tone: "error",
        title: t("toast.exportFailed", { what }),
        detail: exportError(error, t),
      }),
  });
  const pending = exportFile.isPending;
  return (
    <span className="inline-flex items-center gap-1">
      <Button
        size="sm"
        variant="outline"
        disabled={disabled || pending}
        onClick={() => exportFile.mutate()}
      >
        <Download className="h-3.5 w-3.5" />
        {pending ? t("analytics.export.pending") : t("analytics.export.csv")}
      </Button>
      {pending && (
        <>
          {/* a live count of what has arrived: the server does not say how big
              the file will be, so there is no percentage to give */}
          <span role="status" className="text-xs tabular-nums text-muted-foreground">
            {bytes > 0
              ? t("analytics.export.received", {
                  size: fmt.number(bytes / 1_048_576, {
                    maximumFractionDigits: 1,
                    minimumFractionDigits: 1,
                  }),
                })
              : ""}
          </span>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => controller.current?.abort()}
            aria-label={t("analytics.export.cancel", { what })}
          >
            <X className="h-3.5 w-3.5" />
            {t("common.cancel")}
          </Button>
        </>
      )}
    </span>
  );
}

/** the sentence for a failed export: the two this endpoint can answer have their own */
function exportError(error: unknown, t: TFunction) {
  if (error instanceof ApiError && error.status === 429) {
    return error.retryAfterSeconds
      ? t("analytics.export.busyRetry", { seconds: error.retryAfterSeconds })
      : t("analytics.export.busy");
  }
  if (error instanceof ExportTruncatedError) return t("analytics.export.truncated");
  return errorText(error, t);
}
