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
  blobSink,
  chooseDiskSink,
  diskSaveSupported,
  ExportTruncatedError,
  pump,
  type DownloadSink,
} from "@/lib/stream-download";
import { useToast } from "@/lib/toast";

/** how an export ended, when it ended without a failure */
type Outcome = "saved" | "cancelled";

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
  const exportFile = useMutation<Outcome>({
    mutationFn: async () => {
      let sink: DownloadSink;
      const gathered = blobSink();
      if (toDisk && diskSaveSupported()) {
        const chosen = await chooseDiskSink(fallbackName);
        // the dialog was closed: nothing was asked of the server
        if (!chosen) return "cancelled";
        sink = chosen;
      } else {
        sink = gathered;
      }
      const abort = new AbortController();
      controller.current = abort;
      setBytes(0);
      try {
        const file = await run(abort.signal);
        await pump(file.stream, sink, { signal: abort.signal, onProgress: setBytes });
        if (sink === gathered) downloadBlob(file.name ?? fallbackName, gathered.blob("text/csv"));
        return "saved";
      } catch (error) {
        await sink.abort();
        if (abort.signal.aborted) return "cancelled";
        throw error;
      } finally {
        controller.current = null;
      }
    },
    onSuccess: (outcome) => {
      if (outcome === "saved") {
        toast.push({ tone: "success", title: t("analytics.export.downloaded") });
      } else {
        toast.push({ tone: "info", title: t("analytics.export.cancelled") });
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
