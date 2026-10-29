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
import { Combobox, type ComboboxOption } from "@/components/ui/combobox";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { describedBy, FieldError } from "@/components/ui/field-error";
import { Input } from "@/components/ui/input";
import { SettingsPanel } from "@/components/ui/settings-panel";
import { SwitchRow } from "@/components/ui/switch-row";
import {
  createLogErasure,
  fetchLogErasures,
  findLogErasureSubjects,
  type LogErasureRow,
  type LogErasureScope,
  type LogErasureSubject,
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

/**
 * The named subjects of one kind (#3385). A hand-typed UUID that is wrong but
 * valid erases another tenant's logs, and an id says nothing of whose data it
 * is, so the form offers a list by name and email and puts the name in the
 * confirmation. An id pasted instead still works: it is looked up and named
 * when the control plane knows it, and stays usable when it does not.
 */
function useSubjects(scope: Scope, id: string, search: string) {
  // the control plane lists the first few hundred; what is typed narrows that
  // list on the server, so a name past the cap is still found
  const list = useQuery({
    queryKey: ["log-erasure-subjects", scope, search],
    queryFn: () => findLogErasureSubjects(scope, { q: search }),
    // the list being typed into stays up while its narrower answer loads, but
    // never the list of another kind
    placeholderData: (previous, previousQuery) =>
      previousQuery?.queryKey[1] === scope ? previous : undefined,
    retry: false,
  });
  const listed = list.data?.data ?? [];
  const known = listed.find((subject) => subject.id === id);
  // an id the list does not show (more subjects than it carries, or one typed
  // from a ticket) is asked for by itself, whether or not the list loaded
  const lookup = useQuery({
    queryKey: ["log-erasure-subject", scope, id],
    queryFn: () => findLogErasureSubjects(scope, { id }),
    enabled: UUID.test(id) && !known,
    retry: false,
  });
  const found: LogErasureSubject | undefined = known ?? lookup.data?.data[0];
  return { list, listed, found, lookup };
}

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
  const [search, setSearch] = React.useState("");
  const [debounced, setDebounced] = React.useState("");
  React.useEffect(() => {
    const timer = window.setTimeout(() => setDebounced(search.trim()), 250);
    return () => window.clearTimeout(timer);
  }, [search]);
  const { list, listed, found, lookup } = useSubjects(scope, id, debounced);
  const ready = UUID.test(id) && reason.trim() !== "" && !lookup.isFetching;
  const unknownId = UUID.test(id) && !found && lookup.isSuccess;
  // no answer is not "no such subject": the confirmation must not say it
  const unchecked = UUID.test(id) && !found && lookup.isError;
  const subjectOptions: ComboboxOption[] = [
    ...listed,
    ...(found && !listed.some((s) => s.id === found.id) ? [found] : []),
  ].map((s) => ({ value: s.id, label: s.name, description: s.detail || undefined }));

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
            onChange={(next) => {
              setScope(next as Scope);
              setSubject("");
              setSearch("");
            }}
            options={SCOPES.map((s) => ({
              value: s,
              label: t(SCOPE_KEY[s]),
              description: t(`pages.logErasure.scopeBody.${s}`),
            }))}
          />
        </Field>
        <Field label={t("pages.logErasure.form.subject")} htmlFor="log-erasure-subject">
          <Combobox
            id="log-erasure-subject"
            value={id}
            onChange={(next) => {
              setSubject(next);
              setSearch("");
            }}
            onQueryChange={setSearch}
            options={subjectOptions}
            allowCustom
            clearable
            disabled={list.isPending}
            placeholder={
              list.isPending
                ? t("pages.logErasure.form.subjectLoading")
                : t("pages.logErasure.form.subjectPlaceholder")
            }
            emptyText={
              list.isPending
                ? t("pages.logErasure.form.subjectLoading")
                : t("pages.logErasure.form.subjectNone")
            }
            aria-invalid={idInvalid || unknownId || undefined}
            aria-describedby={describedBy(idHint, (idInvalid || unknownId) && idError)}
          />
          <p id={idHint} className="text-xs text-muted-foreground">
            {t(`pages.logErasure.subjectHint.${scope}`)}
          </p>
          <FieldError
            id={idError}
            error={
              idInvalid
                ? t("pages.logErasure.form.subjectInvalid")
                : unknownId
                  ? t("pages.logErasure.form.subjectUnknown", { noun: t(NOUN_KEY[scope]) })
                  : undefined
            }
          />
          {list.data?.truncated && (
            <p className="text-xs text-muted-foreground">
              {t("pages.logErasure.form.subjectTruncated", { count: listed.length })}
            </p>
          )}
          {list.error && (
            // load-error-allow: the list only decorates the form; a pasted id still works
            <LoadError
              error={list.error}
              resource={t("errors.resources.logErasureSubjects")}
              onRetry={() => void list.refetch()}
              target="log-erasure-subjects"
            />
          )}
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
          { noun: t(NOUN_KEY[scope]), name: found?.name ?? id },
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
        <p className="text-sm text-foreground">
          {found
            ? t(
                found.detail
                  ? "pages.logErasure.confirm.subjectLine"
                  : "pages.logErasure.confirm.subjectLineBare",
                { name: found.name, detail: found.detail, id },
              )
            : unchecked
              ? t("pages.logErasure.confirm.subjectUnchecked", { noun: t(NOUN_KEY[scope]), id })
              : t("pages.logErasure.confirm.subjectUnknown", { noun: t(NOUN_KEY[scope]), id })}
        </p>
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
