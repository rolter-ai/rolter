import { useMutation } from "@tanstack/react-query";
import { Download, X } from "lucide-react";
import * as React from "react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { ApiError, isEndpointNotMounted, streamExport, type ExportRequest } from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { useFormat } from "@/lib/i18n/format";
import {
  chooseDiskSink,
  diskSaveSupported,
  ExportTruncatedError,
  pump,
  type DownloadSink,
  type ExportFileType,
} from "@/lib/stream-download";
import {
  DownloadTruncatedError,
  DownloadStoppedError,
  downloadWithTicket,
  ExportNotStartedError,
  ExportRefusedError,
  ExportStalledError,
  isUnknownExport,
} from "@/lib/ticket-download";
import { useToast } from "@/lib/toast";

/**
 * How an export ended, when it ended without a failure. `dismissed` is the save
 * dialog being closed before anything was asked: a no that needs no toast.
 * `unwatched` is the page no longer following a download the browser still
 * has, and `stopped` the browser's download ending early, most likely because
 * the person cancelled it there.
 */
type Outcome =
  | "saved"
  | "downloaded"
  | "cancelled"
  | "dismissed"
  | "unwatched"
  | "stopped"
  | { fallback: FallbackDone };

/** how a screen's own way of exporting ended, for its toast */
export interface FallbackDone {
  title: string;
  tone: "success" | "info";
}

/** how often the byte count under an export is redrawn */
const PROGRESS_EVERY_MS = 1000;

/**
 * The Export button of a screen whose figures end in a spreadsheet (#1838).
 *
 * `request` names the export the control plane builds, under the caller's own
 * row scope, named after its window and currency. A failure is a toast, since
 * the screen has nowhere else for it.
 *
 * With `toDisk`, a browser that can write to a chosen file (the File System
 * Access API) streams the body there as it arrives, so a request log of a
 * million rows is never held in the tab (#3434); it asks where to save first,
 * under `fallbackName`, since the dialog needs the click and the server's own
 * name is only known once the request has answered. A screen passes the name
 * the server will give as a function, built with `exportFileName` from the
 * bounds it sends and read at the click, so both ways of saving offer one name
 * (#3498). It shows the bytes received and a Cancel that stops the request.
 *
 * Every other case hands the file to the browser's download manager through a
 * single-use download ticket (#3504), so the tab never holds the body: the
 * button mints the ticket, follows its link, and reads the ticket's status
 * about once a second for the rows written and how the download ended, which
 * is where a `429` or a body cut short is learned. The browser's own download
 * list is where that download is cancelled; the button's Cancel there only
 * stops following it, and says so.
 *
 * The Audit Log's CSV and JSON exports use it too (#3545): `label`, `fileType`
 * and `done` give the button its own words and the save dialog its own type,
 * and `fallback` is what a screen does when an older control plane has no such
 * export at all.
 */
export function ExportCsvButton({
  request,
  fallbackName,
  what,
  disabled,
  toDisk = false,
  label,
  fileType = "csv",
  done,
  fallback,
  bounded = true,
}: {
  /** the export to run, read when the button is pressed */
  request: () => ExportRequest;
  /**
   * the name the save dialog suggests when saving to disk (a download through
   * the browser keeps the server's own name); a function is read when the
   * button is pressed, so a rolling window is named for the moment it leaves
   */
  fallbackName: string | (() => string);
  /** the translated noun a failure names, such as "request log export" */
  what: string;
  disabled?: boolean;
  /** stream to a file the person picks where the browser allows it */
  toDisk?: boolean;
  /** the button's text, "Export CSV" when absent */
  label?: string;
  /** what the save dialog offers to save as */
  fileType?: ExportFileType;
  /** the success toast's title for each way of saving, the analytics wording when absent */
  done?: { saved: string; downloaded: string };
  /**
   * what to do instead when the control plane answers that the export, or the
   * ticket for it, is not there (an older one): resolves with the toast to show
   */
  fallback?: () => Promise<FallbackDone>;
  /**
   * the export has a row and time limit, so a body cut short may have hit
   * one; the audit log's has neither, and its message does not say so
   */
  bounded?: boolean;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const toast = useToast();
  const [bytes, setBytes] = React.useState(0);
  // rows the control plane has written, on the download ticket path; `null`
  // on the save-to-file path, which counts bytes instead
  const [rows, setRows] = React.useState<number | null>(null);
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
        disk = await chooseDiskSink(
          typeof fallbackName === "function" ? fallbackName() : fallbackName,
          fileType,
        );
        // the dialog was closed: nothing was asked of the server
        if (!disk) return "dismissed";
      }
      const abort = new AbortController();
      controller.current = abort;
      shownAt.current = 0;
      setBytes(0);
      setRows(disk ? null : 0);
      try {
        if (!disk) {
          // the browser's download manager receives the file; the page only
          // follows the ticket's status
          try {
            await downloadWithTicket(request(), { signal: abort.signal, onProgress: setRows });
          } catch (error) {
            if (error instanceof DownloadStoppedError) return "stopped";
            if (abort.signal.aborted) return "unwatched";
            throw error;
          }
          return "downloaded";
        }
        const file = await streamExport(request(), { signal: abort.signal });
        await pump(file.stream, disk, { signal: abort.signal, onProgress: progress });
        return "saved";
      } catch (error) {
        // pump has dropped the file already when it failed; a request that
        // failed before any byte came (a 429, a refusal) has not
        await disk?.abort();
        if (abort.signal.aborted) return "cancelled";
        // an older control plane: no such export or ticket route (`404`), or a
        // mint that does not know this export yet (`400`)
        if (fallback && (isEndpointNotMounted(error) || isUnknownExport(error))) {
          return { fallback: await fallback() };
        }
        throw error;
      } finally {
        controller.current = null;
      }
    },
    onSuccess: (outcome) => {
      if (outcome === "dismissed" || !mounted.current) return;
      if (typeof outcome === "object") {
        toast.push(outcome.fallback);
      } else if (outcome === "cancelled") {
        toast.push({ tone: "info", title: t("analytics.export.cancelled") });
      } else if (outcome === "unwatched" || outcome === "stopped") {
        toast.push({ tone: "info", title: t(`analytics.export.${outcome}`) });
      } else {
        // both paths offer the name the server gives (#3498), so both say what
        // it carries; the disk path's says it was the suggestion, since the
        // person may have typed another
        toast.push({
          tone: "success",
          title: done
            ? done[outcome]
            : t(outcome === "saved" ? "analytics.export.saved" : "analytics.export.downloaded"),
        });
      }
    },
    onError: (error) =>
      toast.push({
        tone: "error",
        title: t("toast.exportFailed", { what }),
        detail: exportError(error, t, bounded),
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
        {pending ? t("analytics.export.pending") : (label ?? t("analytics.export.csv"))}
      </Button>
      {pending && rows !== null && (
        <>
          {/* the rows the control plane has written: the browser's download
              list shows the bytes, and the server does not say how many rows
              there are */}
          <span role="status" className="text-xs tabular-nums text-muted-foreground">
            {rows > 0 ? t("analytics.export.rows", { count: rows, rows: fmt.number(rows) }) : ""}
          </span>
          {/* the browser owns the download: this only stops following it */}
          <Button
            size="sm"
            variant="ghost"
            onClick={() => controller.current?.abort()}
            aria-label={t("analytics.export.stopWatchingLabel", { what })}
          >
            <X className="h-3.5 w-3.5" />
            {t("analytics.export.stopWatching")}
          </Button>
        </>
      )}
      {pending && rows === null && (
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

/** the sentence for a failed export: the ways an export ends early have their own */
function exportError(error: unknown, t: TFunction, bounded: boolean) {
  if (error instanceof ApiError && error.status === 429) {
    return error.retryAfterSeconds
      ? t("analytics.export.busyRetry", { seconds: error.retryAfterSeconds })
      : t("analytics.export.busy");
  }
  // before the plain truncated error, which it extends: here the browser holds
  // the partial file, and the page cannot take it away
  if (error instanceof DownloadTruncatedError) {
    return t(
      bounded
        ? "analytics.export.truncatedDownload"
        : "analytics.export.truncatedDownloadUnbounded",
    );
  }
  if (error instanceof ExportTruncatedError) {
    return t(bounded ? "analytics.export.truncated" : "analytics.export.truncatedUnbounded");
  }
  if (error instanceof ExportRefusedError) {
    return t("analytics.export.refused", { status: error.status });
  }
  if (error instanceof ExportNotStartedError) return t("analytics.export.notStarted");
  if (error instanceof ExportStalledError) return t("analytics.export.stalled");
  return errorText(error, t);
}
