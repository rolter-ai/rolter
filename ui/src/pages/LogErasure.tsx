import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Eraser, ShieldAlert } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { superadminOnly } from "@/components/ForbiddenScreen";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { ERASURES_KEY, ErasureStatus } from "@/components/LogErasure";
import {
  ListCell,
  ListEmptyRow,
  ListHeader,
  ListHeaderCell,
  ListLoadingRow,
  ListRow,
  ListSummary,
  ListTable,
  PageBody,
  primaryColumn,
} from "@/components/screen";
import { Combobox } from "@/components/ui/combobox";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { describedBy, FieldError } from "@/components/ui/field-error";
import { Input } from "@/components/ui/input";
import { SettingsPanel } from "@/components/ui/settings-panel";
import { SwitchRow } from "@/components/ui/switch-row";
import {
  createLogErasure,
  fetchLogErasures,
  type LogErasureRow,
  type LogErasureScope,
} from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { useToast } from "@/lib/toast";
import { useScreenReady } from "@/lib/ux-react";

const SCOPES = ["virtual_key", "user", "org"] as const;
type Scope = (typeof SCOPES)[number];

const SCOPE_KEY: Record<LogErasureRow["scope_kind"], string> = {
  virtual_key: "pages.logErasure.scope.virtual_key",
  user: "pages.logErasure.scope.user",
  org: "pages.logErasure.scope.org",
  request: "pages.logErasure.scope.request",
};

const NOUN_KEY: Record<Scope, string> = {
  virtual_key: "pages.logErasure.confirm.noun.virtual_key",
  user: "pages.logErasure.confirm.noun.user",
  org: "pages.logErasure.confirm.noun.org",
};

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

const GRID = `${primaryColumn(1)} 1.2fr 1.4fr 1fr 1.3fr`;

function EraseForm({ onErased }: { onErased: (row: LogErasureRow) => void }) {
  const { t } = useTranslation();
  const toast = useToast();
  const [scope, setScope] = React.useState<Scope>("virtual_key");
  const [subject, setSubject] = React.useState("");
  const [reason, setReason] = React.useState("");
  const [includeLogs, setIncludeLogs] = React.useState(true);
  const [confirming, setConfirming] = React.useState(false);
  const idError = React.useId();
  const idHint = React.useId();

  const id = subject.trim();
  const idInvalid = id !== "" && !UUID.test(id);
  const ready = UUID.test(id) && reason.trim() !== "";

  const erase = useMutation({
    mutationFn: () => {
      const target: LogErasureScope = { kind: scope, id };
      return createLogErasure({ scope: target, reason: reason.trim(), include_logs: includeLogs });
    },
    onSuccess: (row) => {
      setConfirming(false);
      setSubject("");
      setReason("");
      toast.push({ tone: "info", title: t("pages.logErasure.submittedToast") });
      onErased(row);
    },
  });

  return (
    <SettingsPanel
      title={t("pages.logErasure.form.title")}
      description={t("pages.logErasure.form.hint")}
    >
      <div className="flex w-full flex-col gap-3">
        <Field label={t("pages.logErasure.form.scope")}>
          <Combobox
            value={scope}
            onChange={(next) => setScope(next as Scope)}
            options={SCOPES.map((s) => ({
              value: s,
              label: t(SCOPE_KEY[s]),
              description: t(`pages.logErasure.scopeBody.${s}`),
            }))}
          />
        </Field>
        <Field label={t("pages.logErasure.form.subject")} htmlFor="log-erasure-subject">
          <Input
            id="log-erasure-subject"
            className="font-mono"
            value={subject}
            aria-invalid={idInvalid || undefined}
            aria-describedby={describedBy(idHint, idInvalid && idError)}
            onChange={(e) => setSubject(e.target.value)}
            placeholder={t("pages.logErasure.form.subjectPlaceholder")}
          />
          <p id={idHint} className="text-xs text-muted-foreground">
            {t(`pages.logErasure.subjectHint.${scope}`)}
          </p>
          <FieldError
            id={idError}
            error={idInvalid ? t("pages.logErasure.form.subjectInvalid") : undefined}
          />
        </Field>
        <Field label={t("pages.logErasure.reason")} hint={t("pages.logErasure.reasonHint")}>
          <Input
            value={reason}
            onChange={(e) => setReason(e.target.value)}
            placeholder={t("pages.logErasure.reasonPlaceholder")}
          />
        </Field>
        <SwitchRow
          title={t("pages.logErasure.form.includeLogs")}
          hint={
            includeLogs
              ? t("pages.logErasure.form.includeLogsOn")
              : t("pages.logErasure.form.includeLogsOff")
          }
          checked={includeLogs}
          onChange={setIncludeLogs}
        />
        <div>
          <GatedButton
            gate="log_erasure:create"
            control="log-erasure-submit"
            variant="destructive"
            disabled={!ready}
            onClick={() => {
              erase.reset();
              setConfirming(true);
            }}
          >
            <Eraser className="mr-2 h-4 w-4" aria-hidden />
            {t("pages.logErasure.form.submit")}
          </GatedButton>
        </div>
      </div>

      <ConfirmDialog
        name="log-erasure"
        open={confirming}
        onOpenChange={(open) => {
          if (open) return;
          setConfirming(false);
          erase.reset();
        }}
        title={t(
          includeLogs
            ? "pages.logErasure.confirm.titleLogs"
            : "pages.logErasure.confirm.titleBodies",
          { noun: t(NOUN_KEY[scope]), id },
        )}
        description={
          includeLogs
            ? t("pages.logErasure.confirm.bodyAll")
            : t("pages.logErasure.confirm.bodyBodies")
        }
        confirmLabel={t("pages.logErasure.confirm.confirm")}
        pending={erase.isPending}
        error={erase.error}
        onConfirm={() => erase.mutate()}
      >
        <p className="text-xs text-muted-foreground">{t("pages.logErasure.confirm.notInstant")}</p>
      </ConfirmDialog>
    </SettingsPanel>
  );
}

function LogErasureScreen() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const queryClient = useQueryClient();
  const erasures = useQuery({
    queryKey: [ERASURES_KEY],
    queryFn: fetchLogErasures,
    retry: false,
  });
  useScreenReady(!erasures.isLoading);
  const rows = erasures.data?.data;

  return (
    <PageBody className="max-w-[1040px] gap-3.5">
      <p className="flex items-start gap-2 text-sm text-muted-foreground">
        <ShieldAlert className="mt-0.5 h-4 w-4 flex-none" aria-hidden />
        {t("pages.logErasure.intro")}
      </p>

      <EraseForm
        onErased={() => void queryClient.invalidateQueries({ queryKey: [ERASURES_KEY] })}
      />

      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <h2 className="text-sm font-medium text-foreground">{t("pages.logErasure.recent")}</h2>
        <ListSummary data={rows} className="text-xs">
          {(all) => t("pages.logErasure.count", { count: all.length })}
        </ListSummary>
      </div>

      {erasures.error && (
        <LoadError
          error={erasures.error}
          resource={t("errors.resources.logErasures")}
          onRetry={() => void erasures.refetch()}
          target="log-erasures"
        />
      )}

      {!erasures.isError && (
        <ListTable label={t("screens.log-erasure.title")} minWidth={900}>
          <ListHeader grid={GRID}>
            <ListHeaderCell>{t("pages.logErasure.colRequested")}</ListHeaderCell>
            <ListHeaderCell>{t("pages.logErasure.colScope")}</ListHeaderCell>
            <ListHeaderCell>{t("pages.logErasure.colReason")}</ListHeaderCell>
            <ListHeaderCell>{t("pages.logErasure.colRemoves")}</ListHeaderCell>
            <ListHeaderCell>{t("pages.logErasure.colStatus")}</ListHeaderCell>
          </ListHeader>
          <ListLoadingRow read={erasures}>
            <ListSkeleton rows={3} className="p-3" />
          </ListLoadingRow>
          {(rows ?? []).map((row) => (
            <ListRow key={row.id} grid={GRID}>
              <ListCell className="font-mono text-xs text-muted-foreground">
                {fmt.dateTime(row.created_at)}
              </ListCell>
              <ListCell className="min-w-0">
                <div className="text-sm">{t(SCOPE_KEY[row.scope_kind])}</div>
                <div
                  className="truncate font-mono text-xs text-muted-foreground"
                  title={row.scope_id}
                >
                  {row.scope_id}
                </div>
              </ListCell>
              <ListCell className="min-w-0 truncate text-sm" title={row.reason}>
                {row.reason}
              </ListCell>
              <ListCell className="text-xs text-muted-foreground">
                {row.include_logs
                  ? t("pages.logErasure.removesAll")
                  : t("pages.logErasure.removesBodies")}
              </ListCell>
              <ListCell className="min-w-0">
                <ErasureStatus row={row} />
              </ListCell>
            </ListRow>
          ))}
          <ListEmptyRow read={erasures} rows={rows?.length ?? 0}>
            <EmptyState
              uxTarget="log-erasures"
              className="py-6"
              icon={<Eraser />}
              title={t("pages.logErasure.empty.title")}
              description={t("pages.logErasure.empty.body")}
            />
          </ListEmptyRow>
        </ListTable>
      )}
    </PageBody>
  );
}

// deployment-scoped, superadmin-only in the capability table: a lesser caller
// sees the refusal instead of a screen that loads and then 403s (#1183)
export default superadminOnly(LogErasureScreen, "errors.resources.logErasures", "log_erasure:read");
