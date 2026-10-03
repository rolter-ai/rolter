import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Gavel, History, Loader2, Megaphone, Play } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { EditorSheet } from "@/components/EditorSheet";
import { superadminOnly } from "@/components/ForbiddenScreen";
import { GatedButton } from "@/components/GatedButton";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { GatedSwitch } from "@/components/GatedSwitch";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton, TableSkeleton } from "@/components/LoadingState";
import {
  ListCell,
  ListHeader,
  ListHeaderCell,
  ListRow,
  ListSummary,
  ListTable,
  PageBody,
  Pill,
  StatusDot,
  Toolbar,
} from "@/components/screen";
import { Combobox } from "@/components/ui/combobox";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  ALERT_SIGNALS,
  createAlertChannel,
  createAlertRule,
  deleteAlertChannel,
  deleteAlertRule,
  evaluateAlertRule,
  fetchAlertChannels,
  fetchAlertHistory,
  fetchAlertRules,
  updateAlertChannel,
  updateAlertRule,
  type AlertChannelRow,
  type AlertRuleRow,
} from "@/lib/api";
import {
  ALERT_SIGNAL_SPECS,
  defaultThresholdInput,
  formatSignalValue,
  fromFormValue,
  isAlertSignal,
  signalDescription,
  signalLabel,
  thresholdInputMax,
  thresholdLabel,
  thresholdRangeKey,
  thresholdValid,
  type AlertSignal,
} from "@/lib/alert-signals";
import { useCurrencyCode } from "@/lib/currency";
import { useFormat } from "@/lib/i18n/format";
import { errorDetail, useToast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

// `[label, tint]`: the label colour is the -text half of the hue, because a
// state pill is a glyph on a tint rather than a shape (#1181). a rule is
// `unknown`, `ok`, `firing` or `error` (its evaluation failed); a history row
// is `firing` or `resolved`
const STATE_TONE: Record<string, [string, string]> = {
  ok: ["var(--status-success-text)", "rgba(22,163,74,.14)"],
  resolved: ["var(--status-success-text)", "rgba(22,163,74,.14)"],
  firing: ["var(--status-danger-text)", "var(--red-tint)"],
  error: ["var(--status-warning-text)", "rgba(245,158,11,.14)"],
  unknown: ["var(--text-secondary)", "var(--surface-subtle)"],
};

const stateTone = (state: string) => STATE_TONE[state] ?? STATE_TONE.unknown;

// a `failed` delivery is an alert nobody received, so it reads as danger; a
// `skipped` one is a rule with no live channel, which is a choice, not a fault
const DELIVERY_TONE: Record<string, string> = {
  delivered: "var(--status-success-text)",
  failed: "var(--status-danger-text)",
  skipped: "var(--text-secondary)",
};

const deliveryTone = (status: string) => DELIVERY_TONE[status] ?? DELIVERY_TONE.skipped;

// the bounds `validate_rule` enforces on `window_secs`
const WINDOW_MIN_SECS = 60;
const WINDOW_MAX_SECS = 86_400;

// the signal a new rule starts from
const DEFAULT_SIGNAL: AlertSignal = ALERT_SIGNALS[0];

// ---------------------------------------------------------------------------
// channels: webhook destinations alerts are delivered to

function AlertChannelsScreen() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();
  const channels = useQuery({
    queryKey: ["alert-channels"],
    queryFn: fetchAlertChannels,
    retry: false,
  });

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider
  useScreenReady(!channels.isLoading);
  useErrorState(!!channels.error, "alert-channels");
  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["alert-channels"] });

  const toggle = useMutation({
    mutationFn: (c: AlertChannelRow) =>
      updateAlertChannel(c.id, { name: c.name, endpoint: c.endpoint, enabled: !c.enabled }),
    onSuccess: invalidate,
    // a switch that went through says so by staying flipped; one that
    // bounced back says nothing at all without this (#1197)
    onError: (error, c) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: c.name }),
        detail: errorDetail(error),
      });
    },
  });
  const remove = useMutation({ mutationFn: deleteAlertChannel, onSuccess: invalidate });

  const [addOpen, setAddOpen] = React.useState(false);
  // deleting a channel silently strands every rule delivering through it, so
  // the name and the consequence are stated before the request (#1179)
  const [deleteTarget, setDeleteTarget] = React.useState<AlertChannelRow | null>(null);
  const startDelete = (channel: AlertChannelRow) => {
    remove.reset();
    setDeleteTarget(channel);
  };
  // the row controls take the same deployment-wide authority the add button
  // does — alerting has no tenancy scope to be an admin of (#1258)

  return (
    <PageBody>
      <Toolbar>
        <ListSummary data={channels.data}>
          {(rows) => t("pages.alerting.channelSummary", { count: rows.length })}
        </ListSummary>
        <GatedButton
          gate="alert_channel:create"
          control="alert-channel-new"
          className="ml-auto"
          onClick={() => setAddOpen(true)}
        >
          + {t("pages.alerting.channels.add")}
        </GatedButton>
      </Toolbar>

      {channels.isLoading && <CardGridSkeleton cards={3} height={168} min={340} />}
      {channels.isError && (
        <LoadError
          error={channels.error}
          resource={t("errors.resources.alertChannels")}
          onRetry={() => void channels.refetch()}
        />
      )}
      {channels.data && channels.data.length === 0 && (
        <EmptyState
          uxTarget="alert-channels"
          icon={<Megaphone />}
          title={t("pages.alerting.channels.emptyTitle")}
          description={t("pages.alerting.channels.emptyBody")}
          actions={
            <GatedButton
              gate="alert_channel:create"
              control="alert-channel-new-empty"
              onClick={() => setAddOpen(true)}
            >
              {t("pages.alerting.channels.add")}
            </GatedButton>
          }
        />
      )}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(340px,100%),1fr))]">
        {(channels.data ?? []).map((c) => (
          <div
            key={c.id}
            className="flex flex-col gap-3 rounded-[10px] border border-[color:var(--border-default)] bg-card p-4"
          >
            <div className="flex items-center gap-2.5">
              <span className="flex h-[34px] w-[34px] flex-none items-center justify-center rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] text-[color:var(--text-secondary)]">
                <Megaphone className="h-4 w-4" />
              </span>
              <div className="min-w-0 flex-1">
                <div className="font-mono text-sm font-semibold">{c.name}</div>
                <div className="truncate text-xs text-muted-foreground">{c.endpoint}</div>
              </div>
              <GatedSwitch
                gate="alert_channel:update"
                control="alert-channel-toggle"
                checked={c.enabled}
                disabled={toggle.isPending}
                aria-label={t("pages.alerting.channels.toggleAria", { name: c.name })}
                onCheckedChange={() => toggle.mutate(c)}
              />
            </div>
            <div className="flex items-center gap-2 border-t border-[color:var(--border-subtle)] pt-3">
              <Pill color="var(--text-secondary)" tint="var(--surface-subtle)">
                {c.kind}
              </Pill>
              {c.secret_configured && (
                <Pill color="var(--status-info-text)" tint="rgba(59,130,246,.14)">
                  {t("pages.alerting.channels.secretSet")}
                </Pill>
              )}
              {/* the label names the channel: a column of cards each
                  offering "Delete channel" is N buttons a screen reader
                  cannot tell apart (#1214) */}
              <DeleteIconButton
                gate="alert_channel:delete"
                control="alert-channel-delete"
                className="ml-auto"
                label={t("pages.alerting.channels.deleteAria", { name: c.name })}
                pending={remove.isPending && remove.variables === c.id}
                onClick={() => startDelete(c)}
              />
            </div>
          </div>
        ))}
      </div>

      <ConfirmDialog
        name="alert-channel-delete"
        open={!!deleteTarget}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title={t("pages.alerting.confirm.channelTitle", { name: deleteTarget?.name })}
        description={t("pages.alerting.confirm.channelBody")}
        confirmLabel={t("pages.alerting.confirm.channelConfirm")}
        pending={remove.isPending}
        error={remove.error}
        onConfirm={() => {
          if (!deleteTarget) return;
          const what = deleteTarget.name;
          remove.mutate(deleteTarget.id, {
            onSuccess: () => {
              setDeleteTarget(null);
              toast.push({ tone: "success", title: t("toast.deleted", { what }) });
            },
            onError: (error) => {
              toast.push({
                tone: "error",
                title: t("toast.deleteFailed", { what }),
                detail: errorDetail(error),
              });
            },
          });
        }}
      />

      <AddChannelDialog open={addOpen} onOpenChange={setAddOpen} onDone={invalidate} />
    </PageBody>
  );
}

function AddChannelDialog({
  open,
  onOpenChange,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [name, setName] = React.useState("");
  const [endpoint, setEndpoint] = React.useState("");
  const [secret, setSecret] = React.useState("");

  React.useEffect(() => {
    if (open) {
      setName("");
      setEndpoint("");
      setSecret("");
    }
  }, [open]);

  const create = useMutation({
    mutationFn: () =>
      createAlertChannel({
        name,
        endpoint,
        enabled: true,
        ...(secret.trim() ? { managed_secret: secret } : {}),
      }),
    onSuccess: () => {
      // the sheet closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push({ tone: "success", title: t("toast.created", { what: name }) });
      onDone();
      onOpenChange(false);
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: name }),
        detail: errorDetail(error),
      });
    },
  });

  return (
    <EditorSheet
      name="alert-channel-create"
      open={open}
      onOpenChange={onOpenChange}
      title={t("pages.alerting.channels.sheetTitle")}
      subtitle={t("pages.alerting.channels.sheetSubtitle")}
      dirty={Boolean(name || endpoint || secret)}
      errorMessage={create.isError ? (create.error as Error).message : undefined}
      saveLabel={t("common.create")}
      canSave={Boolean(name.trim() && endpoint.trim())}
      saving={create.isPending}
      onSave={() => create.mutate()}
    >
      <div className="space-y-3">
        <Field label={t("pages.alerting.channels.fieldName")}>
          <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="ops-webhook" />
        </Field>
        <Field label={t("pages.alerting.channels.fieldEndpoint")}>
          <Input
            className="font-mono"
            value={endpoint}
            onChange={(e) => setEndpoint(e.target.value)}
            placeholder="https://alerts.example.com/rolter"
          />
        </Field>
        <Field label={t("pages.alerting.channels.fieldSecret")}>
          <Input
            type="password"
            value={secret}
            onChange={(e) => setSecret(e.target.value)}
            placeholder={t("pages.alerting.channels.secretPlaceholder")}
          />
        </Field>
      </div>
    </EditorSheet>
  );
}

// ---------------------------------------------------------------------------
// rules: threshold rules over gateway signals

function AlertRulesScreen() {
  const { t } = useTranslation();
  const fmt = useFormat();
  // `spend_velocity` is spend in the settlement currency, not in dollars
  const currency = useCurrencyCode();
  const queryClient = useQueryClient();
  const toast = useToast();
  const rules = useQuery({ queryKey: ["alert-rules"], queryFn: fetchAlertRules, retry: false });

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider
  useScreenReady(!rules.isLoading);
  useErrorState(!!rules.error, "alert-rules");
  const channels = useQuery({
    queryKey: ["alert-channels"],
    queryFn: fetchAlertChannels,
    retry: false,
  });
  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["alert-rules"] });

  const channelName = (id: string | null) => channels.data?.find((c) => c.id === id)?.name ?? "—";
  // the evaluate action is fired by id, and the toast names the rule
  const ruleName = (id: string) => rules.data?.find((r) => r.id === id)?.name ?? id;
  // a threshold or reading in the rule's own unit, read over its own window
  const reading = (r: AlertRuleRow, value: number) =>
    formatSignalValue(r.signal, value, { fmt, t, currency, windowSecs: r.window_secs });

  const asInput = (r: AlertRuleRow) => ({
    name: r.name,
    signal: r.signal,
    threshold: r.threshold,
    window_secs: r.window_secs,
    channel_id: r.channel_id,
    enabled: r.enabled,
  });
  const toggle = useMutation({
    mutationFn: (r: AlertRuleRow) => updateAlertRule(r.id, { ...asInput(r), enabled: !r.enabled }),
    onSuccess: invalidate,
    // a switch that bounced back reads as nothing having happened (#1197)
    onError: (error, r) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: r.name }),
        detail: errorDetail(error),
      });
    },
  });
  const evaluate = useMutation({
    mutationFn: evaluateAlertRule,
    onSuccess: (result, id) => {
      invalidate();
      // an evaluation that was a transition wrote a history row
      void queryClient.invalidateQueries({ queryKey: ["alert-history"] });
      // a transition says what became of it: a delivery that failed is an
      // alert nobody received, which a green "evaluated" toast would hide
      const n = result.notification;
      const vars = { state: n?.state, detail: n?.detail ?? "—" };
      toast.push({
        tone: n?.delivery_status === "failed" ? "error" : "success",
        title: t("pages.alerting.rules.evaluated", { name: ruleName(id) }),
        detail: !n
          ? undefined
          : n.delivery_status === "delivered"
            ? t("pages.alerting.rules.reportedDelivered", vars)
            : n.delivery_status === "failed"
              ? t("pages.alerting.rules.reportedFailed", vars)
              : t("pages.alerting.rules.reportedSkipped", vars),
      });
    },
    onError: (error, id) => {
      // a failed evaluation still wrote the rule's error state and last_error,
      // which the card shows only once it is read again
      invalidate();
      void queryClient.invalidateQueries({ queryKey: ["alert-history"] });
      toast.push({
        tone: "error",
        title: t("pages.alerting.rules.evaluateFailed", { name: ruleName(id) }),
        detail: errorDetail(error),
      });
    },
  });
  const remove = useMutation({ mutationFn: deleteAlertRule, onSuccess: invalidate });

  const [addOpen, setAddOpen] = React.useState(false);
  const [deleteTarget, setDeleteTarget] = React.useState<AlertRuleRow | null>(null);
  const startDelete = (rule: AlertRuleRow) => {
    remove.reset();
    setDeleteTarget(rule);
  };

  return (
    <PageBody>
      <Toolbar>
        <ListSummary data={rules.data}>
          {(rows) => t("pages.alerting.ruleSummary", { count: rows.length })}
        </ListSummary>
        <GatedButton
          gate="alert_rule:create"
          control="alert-rule-new"
          className="ml-auto"
          onClick={() => setAddOpen(true)}
        >
          + {t("pages.alerting.rules.add")}
        </GatedButton>
      </Toolbar>

      {rules.isLoading && <CardGridSkeleton cards={3} height={196} min={380} />}
      {rules.isError && (
        <LoadError
          error={rules.error}
          resource={t("errors.resources.alertRules")}
          onRetry={() => void rules.refetch()}
        />
      )}
      {rules.data && rules.data.length === 0 && (
        <EmptyState
          uxTarget="alert-rules"
          icon={<Gavel />}
          title={t("pages.alerting.rules.emptyTitle")}
          description={
            channels.data && channels.data.length === 0
              ? t("pages.alerting.rules.emptyBodyNoChannel")
              : t("pages.alerting.rules.emptyBody")
          }
          actions={
            <GatedButton
              gate="alert_rule:create"
              control="alert-rule-new-empty"
              onClick={() => setAddOpen(true)}
            >
              {t("pages.alerting.rules.add")}
            </GatedButton>
          }
        />
      )}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(380px,100%),1fr))]">
        {(rules.data ?? []).map((r) => {
          const tone = stateTone(r.state);
          const nameId = `alert-rule-${r.id}-name`;
          return (
            <article
              key={r.id}
              aria-labelledby={nameId}
              className="flex flex-col gap-3 rounded-[10px] border border-[color:var(--border-default)] bg-card p-4"
            >
              <div className="flex items-center gap-2.5">
                <StatusDot color={tone[0]} />
                <span id={nameId} className="min-w-0 truncate font-mono text-sm font-semibold">
                  {r.name}
                </span>
                <Pill color={tone[0]} tint={tone[1]}>
                  {r.state}
                </Pill>
                <GatedSwitch
                  gate="alert_rule:update"
                  control="alert-rule-toggle"
                  className="ml-auto"
                  checked={r.enabled}
                  disabled={toggle.isPending}
                  aria-label={t("pages.alerting.rules.toggleAria", { name: r.name })}
                  onCheckedChange={() => toggle.mutate(r)}
                />
              </div>
              <dl className="grid grid-cols-1 gap-2.5 sm:grid-cols-3">
                {/* a known signal reads as its name; an unknown one keeps its
                    id, which is what the API and the docs call it */}
                <RuleStat
                  label={t("pages.alerting.rules.statSignal")}
                  value={signalLabel(r.signal, t)}
                  mono={!isAlertSignal(r.signal)}
                />
                <RuleStat
                  label={t("pages.alerting.rules.statThreshold")}
                  value={reading(r, r.threshold)}
                />
                <RuleStat
                  label={t("pages.alerting.rules.statWindow")}
                  value={fmt.duration(r.window_secs)}
                />
                <RuleStat
                  label={t("pages.alerting.rules.statLastValue")}
                  value={r.last_value === null ? "—" : reading(r, r.last_value)}
                />
                <RuleStat
                  label={t("pages.alerting.rules.statEvaluated")}
                  value={
                    r.last_evaluated_at
                      ? fmt.time(r.last_evaluated_at)
                      : t("pages.alerting.rules.statNever")
                  }
                />
                <RuleStat
                  label={t("pages.alerting.rules.statChannel")}
                  value={channelName(r.channel_id)}
                />
              </dl>
              {r.last_error && (
                <p className="text-xs text-[color:var(--status-danger-text)]">{r.last_error}</p>
              )}
              <div className="flex items-center gap-2 border-t border-[color:var(--border-subtle)] pt-3">
                {/* running a rule writes an alert-history row, which is the
                    capability the control plane guards it with */}
                <GatedButton
                  gate="alert_history:create"
                  control="alert-rule-evaluate"
                  size="sm"
                  variant="outline"
                  aria-label={t("pages.alerting.rules.evaluateAria", { name: r.name })}
                  disabled={evaluate.isPending}
                  onClick={() => evaluate.mutate(r.id)}
                >
                  {evaluate.isPending && evaluate.variables === r.id ? (
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  ) : (
                    <Play className="h-3.5 w-3.5" />
                  )}
                  {t("pages.alerting.rules.evaluateNow")}
                </GatedButton>
                <DeleteIconButton
                  gate="alert_rule:delete"
                  control="alert-rule-delete"
                  className="ml-auto"
                  label={t("pages.alerting.rules.deleteAria", { name: r.name })}
                  pending={remove.isPending && remove.variables === r.id}
                  onClick={() => startDelete(r)}
                />
              </div>
            </article>
          );
        })}
      </div>

      <ConfirmDialog
        name="alert-rule-delete"
        open={!!deleteTarget}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title={t("pages.alerting.confirm.ruleTitle", { name: deleteTarget?.name })}
        description={t("pages.alerting.confirm.ruleBody")}
        confirmLabel={t("pages.alerting.confirm.ruleConfirm")}
        pending={remove.isPending}
        error={remove.error}
        onConfirm={() => {
          if (!deleteTarget) return;
          const what = deleteTarget.name;
          remove.mutate(deleteTarget.id, {
            onSuccess: () => {
              setDeleteTarget(null);
              toast.push({ tone: "success", title: t("toast.deleted", { what }) });
            },
            onError: (error) => {
              toast.push({
                tone: "error",
                title: t("toast.deleteFailed", { what }),
                detail: errorDetail(error),
              });
            },
          });
        }}
      />

      <AddRuleDialog
        open={addOpen}
        onOpenChange={setAddOpen}
        channels={channels.data ?? []}
        onDone={invalidate}
      />
    </PageBody>
  );
}

// a figure with its unit wraps rather than truncates: `10 failed health events
// in 5m` cut to `10 failed hea…` is a number with its meaning cut off
function RuleStat({ label, value, mono = true }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="min-w-0">
      <dt className="mb-0.5 text-[0.6875rem] uppercase tracking-[0.05em] text-[color:var(--text-subtle)]">
        {label}
      </dt>
      <dd
        className={cn(
          "break-words text-xs text-[color:var(--text-secondary)]",
          mono && "font-mono",
        )}
      >
        {value}
      </dd>
    </div>
  );
}

function AddRuleDialog({
  open,
  onOpenChange,
  channels,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  channels: AlertChannelRow[];
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const currency = useCurrencyCode();
  const [name, setName] = React.useState("");
  const [signal, setSignal] = React.useState<AlertSignal>(DEFAULT_SIGNAL);
  // typed in the signal's form unit: a percentage for `error_rate`
  const [threshold, setThreshold] = React.useState(defaultThresholdInput(DEFAULT_SIGNAL));
  const [windowSecs, setWindowSecs] = React.useState("300");
  const [channelId, setChannelId] = React.useState("");

  React.useEffect(() => {
    if (open) {
      setName("");
      setSignal(DEFAULT_SIGNAL);
      setThreshold(defaultThresholdInput(DEFAULT_SIGNAL));
      setWindowSecs("300");
      setChannelId(channels[0]?.id ?? "");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // a threshold means something only in its signal's unit, so another signal
  // starts from its own default rather than carrying 5 % over as 5 ms
  const chooseSignal = (next: string) => {
    if (!isAlertSignal(next)) return;
    setSignal(next);
    setThreshold(defaultThresholdInput(next));
  };

  const create = useMutation({
    mutationFn: () =>
      createAlertRule({
        name,
        signal,
        threshold: fromFormValue(signal, Number(threshold)),
        window_secs: Number(windowSecs),
        channel_id: channelId || null,
        enabled: true,
      }),
    onSuccess: () => {
      // the sheet closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push({ tone: "success", title: t("toast.created", { what: name }) });
      onDone();
      onOpenChange(false);
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: name }),
        detail: errorDetail(error),
      });
    },
  });

  // the same bounds the API checks, so a window the form accepts is never a 400
  const windowNumber = Number(windowSecs);
  const windowValid =
    windowSecs.trim() !== "" &&
    Number.isInteger(windowNumber) &&
    windowNumber >= WINDOW_MIN_SECS &&
    windowNumber <= WINDOW_MAX_SECS;
  const windowRange = t("pages.alerting.rules.windowRange", {
    min: WINDOW_MIN_SECS,
    max: WINDOW_MAX_SECS,
  });
  const thresholdOk = thresholdValid(signal, threshold);
  const thresholdRange = t(thresholdRangeKey(signal));
  const thresholdMax = thresholdInputMax(signal);

  // the draft is seeded with defaults rather than blanks, so "dirty" is a diff
  // against the seed instead of a plain emptiness check
  const dirty =
    name !== "" ||
    signal !== DEFAULT_SIGNAL ||
    threshold !== defaultThresholdInput(signal) ||
    windowSecs !== "300" ||
    channelId !== (channels[0]?.id ?? "");

  return (
    <EditorSheet
      name="alert-rule-create"
      open={open}
      onOpenChange={onOpenChange}
      title={t("pages.alerting.rules.sheetTitle")}
      subtitle={t("pages.alerting.rules.sheetSubtitle")}
      dirty={dirty}
      errorMessage={create.isError ? (create.error as Error).message : undefined}
      saveLabel={t("common.create")}
      canSave={Boolean(name.trim() && thresholdOk && windowValid)}
      saving={create.isPending}
      onSave={() => create.mutate()}
    >
      <div className="space-y-3">
        <Field label={t("pages.alerting.rules.fieldName")}>
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={t("pages.alerting.rules.namePlaceholder")}
          />
        </Field>
        {/* the option's second line is the id the API and the docs use */}
        <Field
          label={t("pages.alerting.rules.fieldSignal")}
          hint={signalDescription(signal, t, currency)}
        >
          <Combobox
            value={signal}
            onChange={chooseSignal}
            options={ALERT_SIGNALS.map((s) => ({
              value: s,
              label: signalLabel(s, t),
              description: s,
            }))}
          />
        </Field>
        {/* one per row: the label carries the unit, and the longest one would
            wrap beside the window and push its input out of line */}
        <Field
          label={thresholdLabel(signal, t, currency)}
          hint={thresholdOk && thresholdMax !== undefined ? thresholdRange : undefined}
          error={thresholdOk ? undefined : thresholdRange}
        >
          <Input
            type="number"
            min={0}
            max={thresholdMax}
            step={ALERT_SIGNAL_SPECS[signal].step}
            value={threshold}
            onChange={(e) => setThreshold(e.target.value)}
          />
        </Field>
        <Field
          label={t("pages.alerting.rules.fieldWindow")}
          hint={windowValid ? windowRange : undefined}
          error={windowValid ? undefined : windowRange}
        >
          <Input
            type="number"
            min={WINDOW_MIN_SECS}
            max={WINDOW_MAX_SECS}
            step={1}
            value={windowSecs}
            onChange={(e) => setWindowSecs(e.target.value)}
          />
        </Field>
        <Field label={t("pages.alerting.rules.fieldChannel")}>
          <Combobox
            value={channelId}
            onChange={setChannelId}
            options={[
              { value: "", label: t("pages.alerting.rules.channelNone") },
              ...channels.map((c) => ({ value: c.id, label: c.name })),
            ]}
          />
        </Field>
      </div>
    </EditorSheet>
  );
}

// ---------------------------------------------------------------------------
// history: every state change a rule recorded, with what became of its delivery

const HISTORY_GRID = "150px 1.4fr 110px 130px 2fr";

function AlertHistoryScreen() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const history = useQuery({
    queryKey: ["alert-history"],
    queryFn: () => fetchAlertHistory(200),
    retry: false,
  });

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider
  useScreenReady(!history.isLoading);
  useErrorState(!!history.error, "alert-history");
  const rules = useQuery({ queryKey: ["alert-rules"], queryFn: fetchAlertRules, retry: false });
  const ruleName = (id: string) => rules.data?.find((r) => r.id === id)?.name ?? id.slice(0, 8);

  return (
    <PageBody>
      <ListSummary data={history.data}>
        {(rows) => t("pages.alerting.historySummary", { count: rows.length })}
      </ListSummary>
      {history.isLoading && <TableSkeleton rows={5} />}
      {history.isError && (
        <LoadError
          error={history.error}
          resource={t("errors.resources.alertHistory")}
          onRetry={() => void history.refetch()}
        />
      )}
      {history.data && history.data.length === 0 && (
        <EmptyState
          uxTarget="alert-history"
          icon={<History />}
          title={t("pages.alerting.history.emptyTitle")}
          description={t("pages.alerting.history.emptyBody")}
          actions={
            <a
              href="/alerting-rules"
              className="text-sm font-medium text-foreground underline decoration-[color:var(--border-strong)] underline-offset-4 transition-colors hover:decoration-current focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              {t("pages.alerting.history.emptyAction")}
            </a>
          }
        />
      )}
      {history.data && history.data.length > 0 && (
        <ListTable label={t("screens.alerting-history.title")}>
          <ListHeader grid={HISTORY_GRID}>
            <ListHeaderCell>{t("pages.alerting.history.colSent")}</ListHeaderCell>
            <ListHeaderCell>{t("pages.alerting.history.colRule")}</ListHeaderCell>
            <ListHeaderCell>{t("pages.alerting.history.colState")}</ListHeaderCell>
            <ListHeaderCell>{t("pages.alerting.history.colDelivery")}</ListHeaderCell>
            <ListHeaderCell>{t("pages.alerting.history.colDetail")}</ListHeaderCell>
          </ListHeader>
          {history.data.map((n) => {
            const tone = stateTone(n.state);
            return (
              <ListRow key={n.id} grid={HISTORY_GRID}>
                <ListCell className="font-mono text-xs text-[color:var(--text-secondary)]">
                  {fmt.dateTime(n.sent_at)}
                </ListCell>
                <ListCell className="truncate font-mono text-xs">{ruleName(n.rule_id)}</ListCell>
                <ListCell className="grid">
                  <Pill color={tone[0]} tint={tone[1]}>
                    {n.state}
                  </Pill>
                </ListCell>
                <ListCell className="grid">
                  <Pill color={deliveryTone(n.delivery_status)} tint="var(--surface-subtle)">
                    {n.delivery_status}
                  </Pill>
                </ListCell>
                <ListCell className="truncate text-xs text-muted-foreground">
                  {n.detail ?? "—"}
                </ListCell>
              </ListRow>
            );
          })}
        </ListTable>
      )}
    </PageBody>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export const AlertChannels = superadminOnly(AlertChannelsScreen, "errors.resources.alertChannels");
export const AlertRules = superadminOnly(AlertRulesScreen, "errors.resources.alertRules");
export const AlertHistory = superadminOnly(AlertHistoryScreen, "errors.resources.alertHistory");
