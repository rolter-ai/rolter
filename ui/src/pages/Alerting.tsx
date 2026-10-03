import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Gavel, History, Loader2, Megaphone, Pencil, Play, Plus } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { Overline } from "@/components/ui/overline";
import { IconFrame } from "@/components/ui/icon-frame";
import { CardStack } from "@/components/ui/card";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { EditorSheet } from "@/components/EditorSheet";
import { superadminOnly } from "@/components/ForbiddenScreen";
import { GatedButton } from "@/components/GatedButton";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { GatedSwitch } from "@/components/GatedSwitch";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton, ListSkeleton } from "@/components/LoadingState";
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
  Pill,
  RowIconButton,
  StatusDot,
  Toolbar,
} from "@/components/screen";
import { Button } from "@/components/ui/button";
import { Combobox } from "@/components/ui/combobox";
import { EmptyState, EmptyStateLink } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { FieldLabel } from "@/components/ui/field-label";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/components/ui/segmented";
import {
  ALERT_COMPARISONS,
  ALERT_NO_DATA_POLICIES,
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
  type AlertComparison,
  type AlertNoDataPolicy,
  type AlertRuleRow,
} from "@/lib/api";
import {
  defaultThresholdInput,
  formatSignalValue,
  fromFormValue,
  isAlertSignal,
  signalDescription,
  signalLabel,
  signalSpec,
  thresholdInputMax,
  thresholdLabel,
  thresholdRangeKey,
  supportsNoData,
  thresholdValid,
  toFormValue,
  type AlertSignal,
} from "@/lib/alert-signals";
import {
  channelKindLabel,
  deliveryLabel,
  DELIVERY_STATUSES,
  HISTORY_STATES,
  stateLabel,
} from "@/lib/alert-states";
import { useCurrencyCode } from "@/lib/currency";
import { useFormat } from "@/lib/i18n/format";
import { movesOrigin } from "@/lib/origin";
import { errorDetail, useToast } from "@/lib/toast";
import { useNow } from "@/lib/use-now";
import { cn } from "@/lib/utils";
import { useScreenReady } from "@/lib/ux-react";

// a state's three colours. the pill label is the -text half of the hue, because
// a label is a glyph on a tint rather than a shape (#1181); the dot is a shape,
// so it takes the fill. a rule is `unknown`, `ok`, `firing` or `error` (its
// evaluation failed); a history row is `firing` or `resolved`
interface StateTone {
  fill: string;
  text: string;
  tint: string;
}

// `error` is amber and `firing` is red on purpose: a rule that could not be
// evaluated has no reading to compare, and the two must not look alike. the
// same amber colours the `last_error` line under the card, so one fault has one
// tone
const STATE_TONE: Record<string, StateTone> = {
  ok: {
    fill: "var(--status-success)",
    text: "var(--status-success-text)",
    tint: "rgba(22,163,74,.14)",
  },
  resolved: {
    fill: "var(--status-success)",
    text: "var(--status-success-text)",
    tint: "rgba(22,163,74,.14)",
  },
  firing: {
    fill: "var(--status-danger)",
    text: "var(--status-danger-text)",
    tint: "var(--red-tint)",
  },
  error: {
    fill: "var(--status-warning)",
    text: "var(--status-warning-text)",
    tint: "rgba(245,158,11,.14)",
  },
  unknown: {
    fill: "var(--zinc-500)",
    text: "var(--text-secondary)",
    tint: "var(--surface-subtle)",
  },
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

// the signal and window a new rule starts from
const DEFAULT_SIGNAL: AlertSignal = ALERT_SIGNALS[0];
const DEFAULT_WINDOW_SECS = 300;

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

  const [sheetOpen, setSheetOpen] = React.useState(false);
  // the channel the sheet edits, or `null` when it adds one. kept after the
  // sheet closes, so a closing edit does not turn into the add form on its way out
  const [editTarget, setEditTarget] = React.useState<AlertChannelRow | null>(null);
  const openSheet = (channel: AlertChannelRow | null) => {
    setEditTarget(channel);
    setSheetOpen(true);
  };
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
          onClick={() => openSheet(null)}
        >
          <Plus className="h-4 w-4" />
          {t("pages.alerting.channels.add")}
        </GatedButton>
      </Toolbar>

      {channels.isLoading && <CardGridSkeleton cards={3} height={168} min={340} />}
      {channels.isError && (
        <LoadError
          error={channels.error}
          resource={t("errors.resources.alertChannels")}
          onRetry={() => void channels.refetch()}
          target="alert-channels"
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
              onClick={() => openSheet(null)}
            >
              {t("pages.alerting.channels.add")}
            </GatedButton>
          }
        />
      )}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(340px,100%),1fr))]">
        {(channels.data ?? []).map((c) => (
          <CardStack key={c.id}>
            <div className="flex items-center gap-2.5">
              <IconFrame>
                <Megaphone className="h-4 w-4" />
              </IconFrame>
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
                {channelKindLabel(c.kind, t)}
              </Pill>
              {c.secret_configured && (
                <Pill color="var(--status-info-text)" tint="rgba(59,130,246,.14)">
                  {t("pages.alerting.channels.secretSet")}
                </Pill>
              )}
              {/* the labels name the channel: a column of cards each
                  offering "Delete channel" is N buttons a screen reader
                  cannot tell apart (#1214) */}
              <div className="ml-auto flex items-center gap-1.5">
                <RowIconButton
                  gate="alert_channel:update"
                  control="alert-channel-edit"
                  className="p-1.5"
                  title={t("pages.alerting.channels.editAria", { name: c.name })}
                  aria-label={t("pages.alerting.channels.editAria", { name: c.name })}
                  onClick={() => openSheet(c)}
                >
                  <Pencil className="h-3.5 w-3.5" />
                </RowIconButton>
                <DeleteIconButton
                  gate="alert_channel:delete"
                  control="alert-channel-delete"
                  label={t("pages.alerting.channels.deleteAria", { name: c.name })}
                  pending={remove.isPending && remove.variables === c.id}
                  onClick={() => startDelete(c)}
                />
              </div>
            </div>
          </CardStack>
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

      <ChannelSheet
        open={sheetOpen}
        onOpenChange={setSheetOpen}
        existing={editTarget}
        onDone={invalidate}
      />
    </PageBody>
  );
}

function ChannelSheet({
  open,
  onOpenChange,
  existing,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** the channel to edit, or `null` to add one */
  existing: AlertChannelRow | null;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [name, setName] = React.useState("");
  const [endpoint, setEndpoint] = React.useState("");
  // write-only: an edit starts blank, because the stored secret is never read back
  const [secret, setSecret] = React.useState("");

  const save = useMutation({
    mutationFn: () => {
      // a blank secret is left out, which an update reads as "keep the stored one"
      const input = { name, endpoint, ...(secret.trim() ? { managed_secret: secret } : {}) };
      // PUT replaces the whole row, so an edit sends the switch back as it
      // found it; a new channel starts on
      return existing
        ? updateAlertChannel(existing.id, { ...input, enabled: existing.enabled })
        : createAlertChannel({ ...input, enabled: true });
    },
    onSuccess: () => {
      // the sheet closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push(
        existing
          ? {
              tone: "success",
              title: t("toast.saved"),
              detail: t("toast.savedDetail", { what: name }),
            }
          : { tone: "success", title: t("toast.created", { what: name }) },
      );
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

  React.useEffect(() => {
    if (open) {
      setName(existing?.name ?? "");
      setEndpoint(existing?.endpoint ?? "");
      setSecret("");
      // a refusal for one channel must not greet the next one opened
      save.reset();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, existing]);

  // what happens to the stored secret on save, said beside the field that
  // decides it
  const secretHint = !existing
    ? undefined
    : !existing.secret_configured
      ? t("pages.alerting.channels.secretNoneHint")
      : secret.trim() === "" && movesOrigin(existing.endpoint, endpoint)
        ? t("pages.alerting.channels.secretDroppedHint")
        : t("pages.alerting.channels.secretKeepHint");

  return (
    <EditorSheet
      name={existing ? "alert-channel-edit" : "alert-channel-create"}
      open={open}
      onOpenChange={onOpenChange}
      title={
        existing
          ? t("pages.alerting.channels.editTitle", { name: existing.name })
          : t("pages.alerting.channels.sheetTitle")
      }
      subtitle={t("pages.alerting.channels.sheetSubtitle")}
      dirty={
        name !== (existing?.name ?? "") || endpoint !== (existing?.endpoint ?? "") || secret !== ""
      }
      errorMessage={save.isError ? (save.error as Error).message : undefined}
      saveLabel={existing ? t("common.save") : t("common.create")}
      canSave={Boolean(name.trim() && endpoint.trim())}
      saving={save.isPending}
      onSave={() => save.mutate()}
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
        <Field label={t("pages.alerting.channels.fieldSecret")} hint={secretHint}>
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
  // the "Evaluated" figures are relative, so they read the same clock
  const now = useNow();
  // `spend_velocity` is spend in the settlement currency, not in dollars
  const currency = useCurrencyCode();
  const queryClient = useQueryClient();
  const toast = useToast();
  const rules = useQuery({ queryKey: ["alert-rules"], queryFn: fetchAlertRules, retry: false });

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider
  useScreenReady(!rules.isLoading);
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
    // `no_data` is left out: a PUT keeps it, and a signal without the policy
    // would answer 400
    comparison: r.comparison,
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
      // the state reads mid-sentence, so it is lowercased in the locale's own rules
      const vars = {
        state: n ? stateLabel(n.state, t).toLocaleLowerCase(fmt.locale) : undefined,
        detail: n?.detail ?? "—",
      };
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

  const [sheetOpen, setSheetOpen] = React.useState(false);
  // the rule the sheet edits, or `null` when it adds one, kept after the sheet
  // closes for the reason the channel's is
  const [editTarget, setEditTarget] = React.useState<AlertRuleRow | null>(null);
  const openSheet = (rule: AlertRuleRow | null) => {
    setEditTarget(rule);
    setSheetOpen(true);
  };
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
          onClick={() => openSheet(null)}
        >
          <Plus className="h-4 w-4" />
          {t("pages.alerting.rules.add")}
        </GatedButton>
      </Toolbar>

      {rules.isLoading && <CardGridSkeleton cards={3} height={196} min={380} />}
      {rules.isError && (
        <LoadError
          error={rules.error}
          resource={t("errors.resources.alertRules")}
          onRetry={() => void rules.refetch()}
          target="alert-rules"
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
              onClick={() => openSheet(null)}
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
                <StatusDot color={tone.fill} />
                <span id={nameId} className="min-w-0 truncate font-mono text-sm font-semibold">
                  {r.name}
                </span>
                <Pill color={tone.text} tint={tone.tint}>
                  {stateLabel(r.state, t)}
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
                  value={t(
                    `pages.alerting.rules.reading.${r.comparison === "below" ? "below" : "above"}`,
                    {
                      value: reading(r, r.threshold),
                    },
                  )}
                />
                <RuleStat
                  label={t("pages.alerting.rules.statWindow")}
                  value={fmt.duration(r.window_secs)}
                />
                <RuleStat
                  label={t("pages.alerting.rules.statLastValue")}
                  value={
                    r.last_value !== null
                      ? reading(r, r.last_value)
                      : r.last_evaluated_at
                        ? // evaluated, but the window held nothing to measure
                          t("pages.alerting.rules.noDataReading")
                        : "—"
                  }
                  mono={r.last_value !== null || !r.last_evaluated_at}
                />
                <RuleStat
                  label={t("pages.alerting.rules.statEvaluated")}
                  value={
                    r.last_evaluated_at ? (
                      // a clock time alone read the same a minute or three days
                      // on; the full stamp is on hover
                      <time
                        dateTime={r.last_evaluated_at}
                        title={fmt.dateTime(r.last_evaluated_at)}
                      >
                        {fmt.relative(r.last_evaluated_at, now)}
                      </time>
                    ) : (
                      t("pages.alerting.rules.statNever")
                    )
                  }
                  mono={false}
                />
                <RuleStat
                  label={t("pages.alerting.rules.statChannel")}
                  value={channelName(r.channel_id)}
                />
              </dl>
              {r.last_error && (
                <p className="text-xs" style={{ color: STATE_TONE.error.text }}>
                  {r.last_error}
                </p>
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
                    <Loader2 className="h-3.5 w-3.5 motion-safe:animate-spin" />
                  ) : (
                    <Play className="h-3.5 w-3.5" />
                  )}
                  {t("pages.alerting.rules.evaluateNow")}
                </GatedButton>
                <div className="ml-auto flex items-center gap-1.5">
                  <RowIconButton
                    gate="alert_rule:update"
                    control="alert-rule-edit"
                    className="p-1.5"
                    title={t("pages.alerting.rules.editAria", { name: r.name })}
                    aria-label={t("pages.alerting.rules.editAria", { name: r.name })}
                    onClick={() => openSheet(r)}
                  >
                    <Pencil className="h-3.5 w-3.5" />
                  </RowIconButton>
                  <DeleteIconButton
                    gate="alert_rule:delete"
                    control="alert-rule-delete"
                    label={t("pages.alerting.rules.deleteAria", { name: r.name })}
                    pending={remove.isPending && remove.variables === r.id}
                    onClick={() => startDelete(r)}
                  />
                </div>
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

      <RuleSheet
        open={sheetOpen}
        onOpenChange={setSheetOpen}
        existing={editTarget}
        channels={channels.data ?? []}
        onDone={invalidate}
      />
    </PageBody>
  );
}

// a figure with its unit wraps rather than truncates: `10 failed health events
// in 5m` cut to `10 failed hea…` is a number with its meaning cut off
function RuleStat({
  label,
  value,
  mono = true,
}: {
  label: string;
  value: React.ReactNode;
  mono?: boolean;
}) {
  return (
    <div className="min-w-0">
      <Overline as="dt">{label}</Overline>
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

interface RuleDraft {
  name: string;
  signal: string;
  /** typed in the signal's form unit: a percentage for `error_rate` */
  threshold: string;
  comparison: AlertComparison;
  noData: AlertNoDataPolicy;
  windowSecs: string;
  channelId: string;
}

// the form as it opens: an existing rule's own values, with the threshold in
// the form's unit (a stored 0.05 error rate opens as 5), or a new rule's
// defaults
function ruleSeed(existing: AlertRuleRow | null, channels: AlertChannelRow[]): RuleDraft {
  return existing
    ? {
        name: existing.name,
        signal: existing.signal,
        threshold: String(toFormValue(existing.signal, existing.threshold)),
        comparison: existing.comparison ?? "above",
        noData: existing.no_data ?? "ignore",
        windowSecs: String(existing.window_secs),
        channelId: existing.channel_id ?? "",
      }
    : {
        name: "",
        signal: DEFAULT_SIGNAL,
        threshold: defaultThresholdInput(DEFAULT_SIGNAL),
        comparison: "above",
        noData: "ignore",
        windowSecs: String(DEFAULT_WINDOW_SECS),
        channelId: channels[0]?.id ?? "",
      };
}

function RuleSheet({
  open,
  onOpenChange,
  existing,
  channels,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** the rule to edit, or `null` to add one */
  existing: AlertRuleRow | null;
  channels: AlertChannelRow[];
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const currency = useCurrencyCode();
  const seed = ruleSeed(existing, channels);
  const [name, setName] = React.useState(seed.name);
  // a string rather than an `AlertSignal`: an existing rule may carry a signal
  // this build does not know, and an edit sends it back as it found it
  const [signal, setSignal] = React.useState(seed.signal);
  const [threshold, setThreshold] = React.useState(seed.threshold);
  const [comparison, setComparison] = React.useState<AlertComparison>(seed.comparison);
  const [noData, setNoData] = React.useState<AlertNoDataPolicy>(seed.noData);
  const [windowSecs, setWindowSecs] = React.useState(seed.windowSecs);
  const [channelId, setChannelId] = React.useState(seed.channelId);
  const comparisonLabelId = React.useId();
  const noDataLabelId = React.useId();

  // a threshold means something only in its signal's unit, so another signal
  // starts from its own default rather than carrying 5 % over as 5 ms. picking
  // the signal already chosen is no change, and keeps what the rule had
  const chooseSignal = (next: string) => {
    if (!isAlertSignal(next) || next === signal) return;
    setSignal(next);
    setThreshold(defaultThresholdInput(next));
  };

  const save = useMutation({
    mutationFn: () => {
      const input = {
        name,
        signal,
        // an untouched threshold goes back as stored rather than through the
        // form's twelve digits, so renaming a rule cannot nudge it
        threshold:
          existing && signal === existing.signal && threshold === seed.threshold
            ? existing.threshold
            : fromFormValue(signal, Number(threshold)),
        comparison,
        // the API answers 400 for a signal with no data policy, so it is not sent
        ...(supportsNoData(signal) ? { no_data: noData } : {}),
        window_secs: Number(windowSecs),
        channel_id: channelId || null,
      };
      // PUT replaces the whole row, so an edit sends the switch back as it
      // found it; a new rule starts on
      return existing
        ? updateAlertRule(existing.id, { ...input, enabled: existing.enabled })
        : createAlertRule({ ...input, enabled: true });
    },
    onSuccess: () => {
      // the sheet closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push(
        existing
          ? {
              tone: "success",
              title: t("toast.saved"),
              detail: t("toast.savedDetail", { what: name }),
            }
          : { tone: "success", title: t("toast.created", { what: name }) },
      );
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

  // seeded straight from the row rather than through `chooseSignal`, so an
  // existing rule opens on its own threshold instead of its signal's default
  React.useEffect(() => {
    if (open) {
      setName(seed.name);
      setSignal(seed.signal);
      setThreshold(seed.threshold);
      setComparison(seed.comparison);
      setNoData(seed.noData);
      setWindowSecs(seed.windowSecs);
      setChannelId(seed.channelId);
      // a refusal for one rule must not greet the next one opened
      save.reset();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, existing]);

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

  // the draft is seeded with the row or with defaults rather than blanks, so
  // "dirty" is a diff against the seed instead of a plain emptiness check
  const dirty =
    name !== seed.name ||
    signal !== seed.signal ||
    threshold !== seed.threshold ||
    comparison !== seed.comparison ||
    noData !== seed.noData ||
    windowSecs !== seed.windowSecs ||
    channelId !== seed.channelId;

  return (
    <EditorSheet
      name={existing ? "alert-rule-edit" : "alert-rule-create"}
      open={open}
      onOpenChange={onOpenChange}
      title={
        existing
          ? t("pages.alerting.rules.editTitle", { name: existing.name })
          : t("pages.alerting.rules.sheetTitle")
      }
      subtitle={
        existing ? t("pages.alerting.rules.editSubtitle") : t("pages.alerting.rules.sheetSubtitle")
      }
      dirty={dirty}
      errorMessage={save.isError ? (save.error as Error).message : undefined}
      saveLabel={existing ? t("common.save") : t("common.create")}
      canSave={Boolean(name.trim() && thresholdOk && windowValid)}
      saving={save.isPending}
      onSave={() => save.mutate()}
    >
      <div className="space-y-3">
        <Field label={t("pages.alerting.rules.fieldName")}>
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={t("pages.alerting.rules.namePlaceholder")}
          />
        </Field>
        {/* the option's second line is the id the API and the docs use; a
            signal this build does not know is offered under its id alone */}
        <Field
          label={t("pages.alerting.rules.fieldSignal")}
          hint={signalDescription(signal, t, currency)}
        >
          <Combobox
            value={signal}
            onChange={chooseSignal}
            options={[
              ...(isAlertSignal(signal) ? [] : [{ value: signal, label: signal }]),
              ...ALERT_SIGNALS.map((s) => ({
                value: s,
                label: signalLabel(s, t),
                description: s,
              })),
            ]}
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
            step={signalSpec(signal)?.step ?? "any"}
            value={threshold}
            onChange={(e) => setThreshold(e.target.value)}
          />
        </Field>
        <div className="space-y-1.5">
          <FieldLabel label={t("pages.alerting.rules.fieldComparison")} id={comparisonLabelId} />
          <Segmented
            labelledBy={comparisonLabelId}
            value={comparison}
            onChange={setComparison}
            options={ALERT_COMPARISONS.map((c) => ({
              value: c,
              label: t(`pages.alerting.rules.comparison.${c}`),
            }))}
          />
          <p className="text-xs text-muted-foreground">
            {t("pages.alerting.rules.thresholdInclusive")}
          </p>
          {signal === "request_volume" && comparison === "below" && Number(threshold) === 0 && (
            <p className="text-xs text-muted-foreground">
              {t("pages.alerting.rules.trafficStoppedHelp")}
            </p>
          )}
        </div>
        {supportsNoData(signal) && (
          <div className="space-y-1.5">
            <FieldLabel label={t("pages.alerting.rules.fieldNoData")} id={noDataLabelId} />
            <Segmented
              labelledBy={noDataLabelId}
              value={noData}
              onChange={setNoData}
              options={ALERT_NO_DATA_POLICIES.map((p) => ({
                value: p,
                label: t(`pages.alerting.rules.noData.${p}`),
              }))}
            />
            <p className="text-xs text-muted-foreground">{t("pages.alerting.rules.noDataHint")}</p>
          </div>
        )}
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

// how many of the newest rows the screen asks for. the endpoint clamps to 500
// and has no cursor, so this is also the most the screen can ever show: when a
// read comes back this full, older rows exist that nothing here reaches
const HISTORY_LIMIT = 200;

// state and delivery lead because they are what the table is for, and because
// the list scrolls sideways on a phone: a later column sits off the right edge
// at 375px, so these two are the ones that have to fit in the first screenful
const HISTORY_GRID = "100px 130px 150px 1.4fr 2fr";

function AlertHistoryScreen() {
  const { t } = useTranslation();
  const fmt = useFormat();
  // the rule filter is sent to the API, so it reaches that rule's own newest
  // rows instead of filtering what the newest 200 of every rule happen to hold.
  // the endpoint has no state or delivery filter, so those two narrow the rows
  // already read
  const [ruleId, setRuleId] = React.useState("");
  const [state, setState] = React.useState("");
  const [delivery, setDelivery] = React.useState("");
  const history = useQuery({
    queryKey: ["alert-history", ruleId],
    queryFn: () => fetchAlertHistory(HISTORY_LIMIT, ruleId || undefined),
    retry: false,
  });

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider
  useScreenReady(!history.isLoading);
  const rules = useQuery({ queryKey: ["alert-rules"], queryFn: fetchAlertRules, retry: false });
  const ruleName = (id: string) => rules.data?.find((r) => r.id === id)?.name ?? id.slice(0, 8);

  const rows = (history.data ?? []).filter(
    (n) =>
      (state === "" || n.state === state) && (delivery === "" || n.delivery_status === delivery),
  );
  const filtering = ruleId !== "" || state !== "" || delivery !== "";
  const capped = history.data !== undefined && history.data.length >= HISTORY_LIMIT;
  const clearFilters = () => {
    setRuleId("");
    setState("");
    setDelivery("");
  };

  return (
    <PageBody>
      <Toolbar>
        <ListSummary data={history.data}>
          {() => t("pages.alerting.historySummary", { count: rows.length })}
        </ListSummary>
        {/* half a row each on a phone, so the two short pickers share a line
            under the rule picker instead of each taking its own */}
        <div className="flex w-full flex-wrap items-center gap-3 sm:ml-auto sm:w-auto">
          {rules.data && rules.data.length > 0 && (
            <Combobox
              className="w-full sm:w-56"
              aria-label={t("pages.alerting.history.ruleFilterAria")}
              value={ruleId}
              onChange={setRuleId}
              options={[
                { value: "", label: t("pages.alerting.history.allRules") },
                ...rules.data.map((r) => ({ value: r.id, label: r.name })),
              ]}
            />
          )}
          <Combobox
            className="w-[calc(50%-0.375rem)] sm:w-44"
            aria-label={t("pages.alerting.history.stateFilterAria")}
            value={state}
            onChange={setState}
            options={[
              { value: "", label: t("pages.alerting.history.allStates") },
              ...HISTORY_STATES.map((s) => ({ value: s, label: stateLabel(s, t) })),
            ]}
          />
          <Combobox
            className="w-[calc(50%-0.375rem)] sm:w-44"
            aria-label={t("pages.alerting.history.deliveryFilterAria")}
            value={delivery}
            onChange={setDelivery}
            options={[
              { value: "", label: t("pages.alerting.history.allDeliveries") },
              ...DELIVERY_STATUSES.map((d) => ({ value: d, label: deliveryLabel(d, t) })),
            ]}
          />
        </div>
      </Toolbar>
      {history.isError && (
        <LoadError
          error={history.error}
          resource={t("errors.resources.alertHistory")}
          onRetry={() => void history.refetch()}
          target="alert-history"
        />
      )}
      {capped && (
        <p className="text-xs text-muted-foreground">
          {ruleId === ""
            ? t("pages.alerting.history.cappedAll", { limit: fmt.number(HISTORY_LIMIT) })
            : t("pages.alerting.history.cappedRule", { limit: fmt.number(HISTORY_LIMIT) })}
        </p>
      )}
      <ListTable label={t("screens.alerting-history.title")}>
        <ListHeader grid={HISTORY_GRID}>
          <ListHeaderCell>{t("pages.alerting.history.colState")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.alerting.history.colDelivery")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.alerting.history.colSent")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.alerting.history.colRule")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.alerting.history.colDetail")}</ListHeaderCell>
        </ListHeader>
        <ListLoadingRow read={history}>
          <ListSkeleton rows={5} className="p-3" />
        </ListLoadingRow>
        {rows.map((n) => {
          const tone = stateTone(n.state);
          return (
            <ListRow key={n.id} grid={HISTORY_GRID}>
              <ListCell className="grid">
                <Pill color={tone.text} tint={tone.tint}>
                  {stateLabel(n.state, t)}
                </Pill>
              </ListCell>
              <ListCell className="grid">
                <Pill color={deliveryTone(n.delivery_status)} tint="var(--surface-subtle)">
                  {deliveryLabel(n.delivery_status, t)}
                </Pill>
              </ListCell>
              <ListCell className="font-mono text-xs text-[color:var(--text-secondary)]">
                {fmt.dateTime(n.sent_at)}
              </ListCell>
              {/* the rule name is what tells two rows of one state apart, so it wraps
                  rather than truncates (#2428) */}
              <ListCell className="min-w-0 break-words font-mono text-xs">
                {ruleName(n.rule_id)}
              </ListCell>
              {/* the diagnosis of a failed delivery wraps rather than truncates:
                  `channel secret could not be unsealed; check ROLTER_KEK` cut to
                  `channel secret could not be…` names the fault and hides what to
                  do about it, and a title is a hover a keyboard or a phone never
                  reaches. `min-w-0` lets a long unbroken token break inside its
                  column instead of widening it (#2335) */}
              <ListCell className="min-w-0 break-words text-xs text-muted-foreground">
                {n.detail ?? "—"}
              </ListCell>
            </ListRow>
          );
        })}
        <ListEmptyRow read={history} rows={rows.length}>
          <EmptyState
            uxTarget="alert-history"
            icon={<History />}
            title={
              filtering
                ? t("pages.alerting.history.noMatchTitle")
                : t("pages.alerting.history.emptyTitle")
            }
            description={
              filtering
                ? t("pages.alerting.history.noMatchBody")
                : t("pages.alerting.history.emptyBody")
            }
            actions={
              filtering ? (
                <Button variant="outline" onClick={clearFilters}>
                  {t("common.clearFilters")}
                </Button>
              ) : (
                <EmptyStateLink to="/alerting-rules">
                  {t("pages.alerting.history.emptyAction")}
                </EmptyStateLink>
              )
            }
          />
        </ListEmptyRow>
      </ListTable>
    </PageBody>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export const AlertChannels = superadminOnly(AlertChannelsScreen, "errors.resources.alertChannels");
export const AlertRules = superadminOnly(AlertRulesScreen, "errors.resources.alertRules");
export const AlertHistory = superadminOnly(AlertHistoryScreen, "errors.resources.alertHistory");
