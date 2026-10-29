import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Eraser, Loader2 } from "lucide-react";
import * as React from "react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { Badge, type BadgeTone } from "@/components/ui/badge";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  createLogErasure,
  fetchLogErasure,
  type InvocationRow,
  type LogErasureRow,
  type LogErasureStatus,
  type LogErasureStore,
} from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { erasureRefetchInterval, erasureTimestamp } from "@/lib/log-erasure";

/** the list the Log erasure screen reads; a record settling refreshes it */
export const ERASURES_KEY = "log-erasures";

const STATUS_TONE: Record<LogErasureStatus, BadgeTone> = {
  pending: "info",
  completed: "success",
  failed: "danger",
  unverified: "warning",
};

const STATUS_KEY: Record<LogErasureStatus, string> = {
  pending: "pages.logErasure.status.pending",
  completed: "pages.logErasure.status.completed",
  failed: "pages.logErasure.status.failed",
  unverified: "pages.logErasure.status.unverified",
};

const STATUS_HINT_KEY: Record<LogErasureStatus, string> = {
  pending: "pages.logErasure.statusHint.pending",
  completed: "pages.logErasure.statusHint.completed",
  failed: "pages.logErasure.statusHint.failed",
  unverified: "pages.logErasure.statusHint.unverified",
};

/** a ClickHouse store in the dashboard's language; one it has no name for reads as itself */
function storeName(store: string, t: TFunction): string {
  return t(`pages.logErasure.storeName.${store}`, { defaultValue: store });
}

/** why a store was not searched: the translated code, else the server's own words */
function skipReason(store: LogErasureStore, t: TFunction): string {
  const fallback = store.reason ?? "";
  if (!store.code) return fallback;
  return t(`pages.logErasure.skipReason.${store.code}`, { defaultValue: fallback });
}

/**
 * Where an erasure stands (#3279).
 *
 * ClickHouse applies a delete as a background mutation, so a submission is
 * `pending` and says so; this asks the record again until it is not, which is
 * the only thing that moves it on. Nothing is called erased before the record
 * says `completed`. The read is the one the page-level refetch convention
 * uses (`erasureRefetchInterval`), and a record that settles refreshes the
 * recent-erasures list.
 */
export function ErasureStatus({
  row,
  detail = false,
  pollMs,
}: {
  row: LogErasureRow;
  /** how often a pending record is asked again; a story shortens it */
  pollMs?: number;
  /** also write the sentence under the badge, for a place with room for it */
  detail?: boolean;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const settled = row.status !== "pending";
  const record = useQuery({
    queryKey: ["log-erasure", row.id],
    queryFn: () => fetchLogErasure(row.id),
    initialData: row,
    // the row in hand is a snapshot; a pending one is stale at once
    initialDataUpdatedAt: 0,
    enabled: !settled,
    retry: false,
    refetchInterval: (query) => erasureRefetchInterval(query.state.data, pollMs),
  });
  const current = record.data ?? row;
  const status = current.status;
  const skipped = (current.stores ?? row.stores ?? []).filter((s) => !s.searched);

  const previous = React.useRef(status);
  React.useEffect(() => {
    if (previous.current === "pending" && status !== "pending") {
      void queryClient.invalidateQueries({ queryKey: [ERASURES_KEY] });
    }
    previous.current = status;
  }, [status, queryClient]);

  if (record.isError && status === "pending") {
    return (
      <LoadError
        error={record.error}
        resource={t("errors.resources.logErasure")}
        onRetry={() => void record.refetch()}
        target="log-erasure-status"
      />
    );
  }
  return (
    <div role="status" className="flex min-w-0 flex-col gap-1">
      <div className="flex flex-wrap items-center gap-2">
        <Badge dot={status === "completed"} tone={STATUS_TONE[status]}>
          {status === "pending" && (
            <Loader2 className="mr-1 h-3 w-3 motion-safe:animate-spin" aria-hidden />
          )}
          {t(STATUS_KEY[status])}
        </Badge>
        {status === "pending" && current.statements > 0 && (
          <span className="text-xs text-muted-foreground">
            {t("pages.logErasure.progress", {
              done: current.mutations_done ?? 0,
              total: current.statements,
            })}
          </span>
        )}
      </div>
      {detail && (
        <p className="text-xs leading-relaxed text-muted-foreground">
          {t(STATUS_HINT_KEY[status])}
        </p>
      )}
      {skipped.length > 0 && (
        <p className="text-xs text-[color:var(--status-warning-text)]">
          {t("pages.logErasure.skipped", {
            stores: skipped.map((s) => storeName(s.store, t)).join(", "),
            reasons: [...new Set(skipped.map((s) => skipReason(s, t)).filter(Boolean))].join(" "),
          })}
        </p>
      )}
      {status === "pending" && current.last_error && (
        <p className="text-xs text-[color:var(--status-warning-text)]">{current.last_error}</p>
      )}
      {status === "failed" && current.error && (
        <p className="text-xs text-[color:var(--status-danger-text)]">{current.error}</p>
      )}
    </div>
  );
}

/**
 * "Erase captured payload" in the LLM Logs detail (#3279): the row with the
 * secret pasted into a prompt is the place an operator holds the request id and
 * its instant. It erases that one request's bodies and keeps its metadata
 * (`include_logs: false`), asks for the reason the audit trail keeps, and then
 * follows the record until ClickHouse has finished.
 *
 * Superadmin only (`log_erasure:create`), refused for everyone else the way
 * every gated control is. A row with no organization cannot be addressed by
 * request, since the id alone is not unique, so it says so.
 */
export function ErasePayloadAction({ row, pollMs }: { row: InvocationRow; pollMs?: number }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const [open, setOpen] = React.useState(false);
  const [reason, setReason] = React.useState("");
  const [record, setRecord] = React.useState<LogErasureRow | null>(null);
  const reasonId = React.useId();

  // the drawer is reused for the next row, which is not this one's erasure
  React.useEffect(() => {
    setRecord(null);
    setOpen(false);
    setReason("");
  }, [row.request_id, row.ts]);

  const erase = useMutation({
    mutationFn: () =>
      createLogErasure({
        scope: {
          kind: "request",
          request_id: row.request_id,
          org_id: row.org_id,
          ts,
        },
        reason: reason.trim(),
        include_logs: false,
      }),
    onSuccess: (created) => {
      setRecord(created);
      setOpen(false);
      setReason("");
    },
  });

  // a request id is client-supplied and not unique, so an erasure that names no
  // instant would delete every row of the organization that shares it: a row
  // whose time does not parse is not offered
  const ts = erasureTimestamp(row.ts);
  const addressable = !!row.org_id && ts !== undefined;
  return (
    <div className="flex flex-col gap-2 border-t border-[color:var(--border-subtle)] pt-3">
      <GatedButton
        gate="log_erasure:create"
        control="log-erasure-payload"
        variant="outline"
        disabled={!addressable || erase.isPending}
        title={
          addressable
            ? undefined
            : row.org_id
              ? t("pages.logErasure.action.noTime")
              : t("pages.logErasure.action.noOrg")
        }
        onClick={() => {
          erase.reset();
          setOpen(true);
        }}
      >
        <Eraser className="mr-2 h-4 w-4" aria-hidden />
        {t("pages.logErasure.action.erase")}
      </GatedButton>
      {record && <ErasureStatus row={record} detail pollMs={pollMs} />}

      <ConfirmDialog
        name="log-erasure-payload"
        open={open}
        onOpenChange={(next) => {
          if (next) return;
          setOpen(false);
          erase.reset();
        }}
        title={t("pages.logErasure.action.confirmTitle", { id: row.request_id })}
        description={t("pages.logErasure.action.confirmBody", { time: fmt.dateTime(row.ts) })}
        confirmLabel={t("pages.logErasure.action.confirm")}
        confirmDisabled={!reason.trim()}
        pending={erase.isPending}
        error={erase.error}
        onConfirm={() => erase.mutate()}
      >
        <Field
          label={t("pages.logErasure.reason")}
          hint={t("pages.logErasure.reasonHint")}
          htmlFor={reasonId}
        >
          <Input
            id={reasonId}
            value={reason}
            onChange={(e) => setReason(e.target.value)}
            placeholder={t("pages.logErasure.reasonPlaceholder")}
          />
        </Field>
      </ConfirmDialog>
    </div>
  );
}
