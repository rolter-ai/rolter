import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Gavel, History, Loader2, Megaphone, Play, Trash2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { EditorSheet } from "@/components/EditorSheet";
import { superadminOnly } from "@/components/ForbiddenScreen";
import { GatedButton } from "@/components/GatedButton";
import { GatedSwitch } from "@/components/GatedSwitch";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton, TableSkeleton } from "@/components/LoadingState";
import { ListHeader, ListRow, ListTable, PageBody, Pill, StatusDot, Toolbar } from "@/components/screen";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
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
import { useGate } from "@/lib/can";
import { useFormat } from "@/lib/i18n/format";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

// `[label, tint]`: the label colour is the -text half of the hue, because a
// state pill is a glyph on a tint rather than a shape (#1181)
const STATE_TONE: Record<string, [string, string]> = {
  ok: ["var(--status-success-text)", "rgba(22,163,74,.14)"],
  firing: ["var(--status-danger-text)", "var(--red-tint)"],
  pending: ["var(--status-warning-text)", "rgba(245,158,11,.14)"],
  unknown: ["var(--text-secondary)", "var(--surface-subtle)"],
};

const stateTone = (state: string) => STATE_TONE[state] ?? STATE_TONE.unknown;

// ---------------------------------------------------------------------------
// channels: webhook destinations alerts are delivered to

function AlertChannelsScreen() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();
  const channels = useQuery({ queryKey: ["alert-channels"], queryFn: fetchAlertChannels, retry: false });

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
  const channelDeleteGate = useGate("alert_channel:delete");

  return (
    <PageBody>
      <Toolbar>
        <span className="text-sm text-muted-foreground">
          {channels.data?.length ?? 0} channels · webhook destinations for alert delivery
        </span>
        <GatedButton gate="alert_channel:create" className="ml-auto" onClick={() => setAddOpen(true)}>
          + Add channel
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
          actions={<GatedButton gate="alert_channel:create" onClick={() => setAddOpen(true)}>{t("pages.alerting.channels.add")}</GatedButton>}
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
                  secret set
                </Pill>
              )}
              {/* the label names the channel: a column of cards each
                  offering "Delete channel" is N buttons a screen reader
                  cannot tell apart (#1214) */}
              <button
                type="button"
                title={
                  channelDeleteGate.reason ??
                  t("pages.alerting.channels.deleteAria", { name: c.name })
                }
                aria-label={t("pages.alerting.channels.deleteAria", { name: c.name })}
                disabled={
                  channelDeleteGate.denied ||
                  (remove.isPending && remove.variables === c.id)
                }
                onClick={() => startDelete(c)}
                className="ml-auto flex h-[30px] items-center rounded-[6px] border border-[color:var(--border-subtle)] px-2 text-[color:var(--status-danger-text)] transition-colors hover:bg-[color:var(--red-tint)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
              >
                {remove.isPending && remove.variables === c.id ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <Trash2 className="h-3.5 w-3.5" />
                )}
              </button>
            </div>
          </div>
        ))}
      </div>

      <ConfirmDialog
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
      open={open}
      onOpenChange={onOpenChange}
      title="Add channel"
      subtitle="Webhook endpoint alerts are POSTed to."
      dirty={Boolean(name || endpoint || secret)}
      errorMessage={create.isError ? (create.error as Error).message : undefined}
      saveLabel="Create"
      canSave={Boolean(name.trim() && endpoint.trim())}
      saving={create.isPending}
      onSave={() => create.mutate()}
    >
      <div className="space-y-3">
        <Field label="Name">
          <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="ops-slack" />
        </Field>
        <Field label="Endpoint URL">
          <Input
            className="font-mono"
            value={endpoint}
            onChange={(e) => setEndpoint(e.target.value)}
            placeholder="https://hooks.slack.com/services/…"
          />
        </Field>
        <Field label="Bearer secret (optional, write-only)">
          <Input
            type="password"
            value={secret}
            onChange={(e) => setSecret(e.target.value)}
            placeholder="stored encrypted"
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
  const queryClient = useQueryClient();
  const toast = useToast();
  const rules = useQuery({ queryKey: ["alert-rules"], queryFn: fetchAlertRules, retry: false });

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider
  useScreenReady(!rules.isLoading);
  useErrorState(!!rules.error, "alert-rules");
  const channels = useQuery({ queryKey: ["alert-channels"], queryFn: fetchAlertChannels, retry: false });
  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["alert-rules"] });

  const channelName = (id: string | null) =>
    channels.data?.find((c) => c.id === id)?.name ?? "—";
  // the evaluate action is fired by id, and the toast names the rule
  const ruleName = (id: string) => rules.data?.find((r) => r.id === id)?.name ?? id;

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
    onSuccess: (_result, id) => {
      invalidate();
      toast.push({
        tone: "success",
        title: t("pages.alerting.rules.evaluated", { name: ruleName(id) }),
      });
    },
    onError: (error, id) => {
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
  const ruleDeleteGate = useGate("alert_rule:delete");

  return (
    <PageBody>
      <Toolbar>
        <span className="text-sm text-muted-foreground">
          {rules.data?.length ?? 0} rules · evaluated every 60s against gateway analytics
        </span>
        <GatedButton gate="alert_rule:create" className="ml-auto" onClick={() => setAddOpen(true)}>
          + Add rule
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
          actions={<GatedButton gate="alert_rule:create" onClick={() => setAddOpen(true)}>{t("pages.alerting.rules.add")}</GatedButton>}
        />
      )}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(380px,100%),1fr))]">
        {(rules.data ?? []).map((r) => {
          const tone = stateTone(r.state);
          return (
            <div
              key={r.id}
              className="flex flex-col gap-3 rounded-[10px] border border-[color:var(--border-default)] bg-card p-4"
            >
              <div className="flex items-center gap-2.5">
                <StatusDot color={tone[0]} />
                <span className="min-w-0 truncate font-mono text-sm font-semibold">{r.name}</span>
                <Pill color={tone[0]} tint={tone[1]}>
                  {r.state}
                </Pill>
                <GatedSwitch
                  gate="alert_rule:update"
                  className="ml-auto"
                  checked={r.enabled}
                  disabled={toggle.isPending}
                  aria-label={t("pages.alerting.rules.toggleAria", { name: r.name })}
                  onCheckedChange={() => toggle.mutate(r)}
                />
              </div>
              <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-3">
                <RuleStat label="Signal" value={r.signal} />
                <RuleStat label="Threshold" value={String(r.threshold)} />
                <RuleStat label="Window" value={`${r.window_secs}s`} />
                <RuleStat
                  label="Last value"
                  value={r.last_value === null ? "—" : String(r.last_value)}
                />
                <RuleStat
                  label="Evaluated"
                  value={r.last_evaluated_at ? fmt.time(r.last_evaluated_at) : "never"}
                />
                <RuleStat label="Channel" value={channelName(r.channel_id)} />
              </div>
              {r.last_error && (
                <p className="text-xs text-[color:var(--status-danger-text)]">{r.last_error}</p>
              )}
              <div className="flex items-center gap-2 border-t border-[color:var(--border-subtle)] pt-3">
                {/* running a rule writes an alert-history row, which is the
                    capability the control plane guards it with */}
                <GatedButton
                  gate="alert_history:create"
                  size="sm"
                  variant="outline"
                  aria-label={t("pages.alerting.rules.evaluateAria", { name: r.name })}
                  disabled={evaluate.isPending}
                  onClick={() => evaluate.mutate(r.id)}
                >
                  <Play className="h-3.5 w-3.5" />
                  Evaluate now
                </GatedButton>
                <button
                  type="button"
                  title={
                    ruleDeleteGate.reason ??
                    t("pages.alerting.rules.deleteAria", { name: r.name })
                  }
                  aria-label={t("pages.alerting.rules.deleteAria", { name: r.name })}
                  disabled={
                    ruleDeleteGate.denied ||
                    (remove.isPending && remove.variables === r.id)
                  }
                  onClick={() => startDelete(r)}
                  className="ml-auto flex h-[30px] items-center rounded-[6px] border border-[color:var(--border-subtle)] px-2 text-[color:var(--status-danger-text)] transition-colors hover:bg-[color:var(--red-tint)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
                >
                  {remove.isPending && remove.variables === r.id ? (
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  ) : (
                    <Trash2 className="h-3.5 w-3.5" />
                  )}
                </button>
              </div>
            </div>
          );
        })}
      </div>

      <ConfirmDialog
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

function RuleStat({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <div className="mb-0.5 text-[0.6875rem] uppercase tracking-[0.05em] text-[color:var(--text-subtle)]">
        {label}
      </div>
      <div className="truncate font-mono text-xs text-[color:var(--text-secondary)]">{value}</div>
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
  const [name, setName] = React.useState("");
  const [signal, setSignal] = React.useState<string>(ALERT_SIGNALS[0]);
  const [threshold, setThreshold] = React.useState("0.05");
  const [windowSecs, setWindowSecs] = React.useState("300");
  const [channelId, setChannelId] = React.useState("");

  React.useEffect(() => {
    if (open) {
      setName("");
      setSignal(ALERT_SIGNALS[0]);
      setThreshold("0.05");
      setWindowSecs("300");
      setChannelId(channels[0]?.id ?? "");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const create = useMutation({
    mutationFn: () =>
      createAlertRule({
        name,
        signal,
        threshold: Number(threshold),
        window_secs: Number(windowSecs) || 300,
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

  // the draft is seeded with defaults rather than blanks, so "dirty" is a diff
  // against the seed instead of a plain emptiness check
  const dirty =
    name !== "" ||
    signal !== ALERT_SIGNALS[0] ||
    threshold !== "0.05" ||
    windowSecs !== "300" ||
    channelId !== (channels[0]?.id ?? "");

  return (
    <EditorSheet
      open={open}
      onOpenChange={onOpenChange}
      title="Add rule"
      subtitle="Fires when the signal crosses the threshold within the window."
      dirty={dirty}
      errorMessage={create.isError ? (create.error as Error).message : undefined}
      saveLabel="Create"
      canSave={Boolean(name.trim() && threshold.trim())}
      saving={create.isPending}
      onSave={() => create.mutate()}
    >
      <div className="space-y-3">
        <Field label="Name">
          <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="high error rate" />
        </Field>
        <Field label="Signal">
          <Select value={signal} onChange={(e) => setSignal(e.target.value)}>
            {ALERT_SIGNALS.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </Select>
        </Field>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <Field label="Threshold">
            <Input
              type="number"
              step="any"
              value={threshold}
              onChange={(e) => setThreshold(e.target.value)}
            />
          </Field>
          <Field label="Window (seconds)">
            <Input
              type="number"
              min={30}
              value={windowSecs}
              onChange={(e) => setWindowSecs(e.target.value)}
            />
          </Field>
        </div>
        <Field label="Channel">
          <Select value={channelId} onChange={(e) => setChannelId(e.target.value)}>
            <option value="">none (record only)</option>
            {channels.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </Select>
        </Field>
      </div>
    </EditorSheet>
  );
}

// ---------------------------------------------------------------------------
// history: every notification the evaluator delivered (or failed to)

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
      <span className="text-sm text-muted-foreground">
        {history.data?.length ?? 0} notifications · newest first
      </span>
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
        <ListTable>
          <ListHeader grid={HISTORY_GRID}>
            <span>Sent</span>
            <span>Rule</span>
            <span>State</span>
            <span>Delivery</span>
            <span>Detail</span>
          </ListHeader>
          {history.data.map((n) => {
            const tone = stateTone(n.state);
            return (
              <ListRow key={n.id} grid={HISTORY_GRID}>
                <span className="font-mono text-xs text-[color:var(--text-secondary)]">
                  {fmt.dateTime(n.sent_at)}
                </span>
                <span className="truncate font-mono text-xs">{ruleName(n.rule_id)}</span>
                <Pill color={tone[0]} tint={tone[1]}>
                  {n.state}
                </Pill>
                <Pill
                  color={
                    n.delivery_status === "delivered"
                      ? "var(--status-success)"
                      : "var(--status-warning)"
                  }
                  tint="var(--surface-subtle)"
                >
                  {n.delivery_status}
                </Pill>
                <span className="truncate text-xs text-muted-foreground">{n.detail ?? "—"}</span>
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
