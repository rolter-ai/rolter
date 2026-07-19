import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Megaphone, Play, Trash2 } from "lucide-react";
import * as React from "react";

import { ListHeader, ListRow, ListTable, PageBody, Pill, StatusDot } from "@/components/screen";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
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

const STATE_TONE: Record<string, [string, string]> = {
  ok: ["var(--status-success)", "rgba(22,163,74,.14)"],
  firing: ["var(--status-danger)", "var(--red-tint)"],
  pending: ["var(--status-warning)", "rgba(245,158,11,.14)"],
  unknown: ["var(--text-secondary)", "var(--surface-subtle)"],
};

const stateTone = (state: string) => STATE_TONE[state] ?? STATE_TONE.unknown;

// ---------------------------------------------------------------------------
// channels: webhook destinations alerts are delivered to

export function AlertChannels() {
  const queryClient = useQueryClient();
  const channels = useQuery({ queryKey: ["alert-channels"], queryFn: fetchAlertChannels, retry: false });
  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["alert-channels"] });

  const toggle = useMutation({
    mutationFn: (c: AlertChannelRow) =>
      updateAlertChannel(c.id, { name: c.name, endpoint: c.endpoint, enabled: !c.enabled }),
    onSuccess: invalidate,
  });
  const remove = useMutation({ mutationFn: deleteAlertChannel, onSuccess: invalidate });

  const [addOpen, setAddOpen] = React.useState(false);

  return (
    <PageBody>
      <div className="flex items-center gap-3">
        <span className="text-sm text-muted-foreground">
          {channels.data?.length ?? 0} channels · webhook destinations for alert delivery
        </span>
        <Button className="ml-auto" onClick={() => setAddOpen(true)}>
          + Add channel
        </Button>
      </div>

      {channels.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
      {channels.isError && (
        <p className="text-sm text-muted-foreground">
          Alert channels need superadmin access: {(channels.error as Error).message}
        </p>
      )}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(340px,1fr))]">
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
              <Switch
                checked={c.enabled}
                disabled={toggle.isPending}
                onCheckedChange={() => toggle.mutate(c)}
              />
            </div>
            <div className="flex items-center gap-2 border-t border-[color:var(--border-subtle)] pt-3">
              <Pill color="var(--text-secondary)" tint="var(--surface-subtle)">
                {c.kind}
              </Pill>
              {c.secret_configured && (
                <Pill color="var(--status-info)" tint="rgba(59,130,246,.14)">
                  secret set
                </Pill>
              )}
              <button
                type="button"
                title="Delete channel"
                onClick={() => remove.mutate(c.id)}
                className="ml-auto flex h-[30px] items-center rounded-[6px] border border-[color:var(--border-subtle)] px-2 text-[color:var(--status-danger)] transition-colors hover:bg-[color:var(--red-tint)]"
              >
                <Trash2 className="h-3.5 w-3.5" />
              </button>
            </div>
          </div>
        ))}
      </div>

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
      onDone();
      onOpenChange(false);
    },
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>Add channel</DialogTitle>
        <DialogDescription>Webhook endpoint alerts are POSTed to.</DialogDescription>
      </DialogHeader>
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
        {create.isError && (
          <p className="text-xs text-destructive">{(create.error as Error).message}</p>
        )}
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button
          disabled={!name.trim() || !endpoint.trim() || create.isPending}
          onClick={() => create.mutate()}
        >
          Create
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// rules: threshold rules over gateway signals

export function AlertRules() {
  const queryClient = useQueryClient();
  const rules = useQuery({ queryKey: ["alert-rules"], queryFn: fetchAlertRules, retry: false });
  const channels = useQuery({ queryKey: ["alert-channels"], queryFn: fetchAlertChannels, retry: false });
  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["alert-rules"] });

  const channelName = (id: string | null) =>
    channels.data?.find((c) => c.id === id)?.name ?? "—";

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
  });
  const evaluate = useMutation({ mutationFn: evaluateAlertRule, onSuccess: invalidate });
  const remove = useMutation({ mutationFn: deleteAlertRule, onSuccess: invalidate });

  const [addOpen, setAddOpen] = React.useState(false);

  return (
    <PageBody>
      <div className="flex items-center gap-3">
        <span className="text-sm text-muted-foreground">
          {rules.data?.length ?? 0} rules · evaluated every 60s against gateway analytics
        </span>
        <Button className="ml-auto" onClick={() => setAddOpen(true)}>
          + Add rule
        </Button>
      </div>

      {rules.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
      {rules.isError && (
        <p className="text-sm text-muted-foreground">
          Alert rules need superadmin access: {(rules.error as Error).message}
        </p>
      )}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(380px,1fr))]">
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
                <Switch
                  className="ml-auto"
                  checked={r.enabled}
                  disabled={toggle.isPending}
                  onCheckedChange={() => toggle.mutate(r)}
                />
              </div>
              <div className="grid grid-cols-3 gap-2.5">
                <RuleStat label="Signal" value={r.signal} />
                <RuleStat label="Threshold" value={String(r.threshold)} />
                <RuleStat label="Window" value={`${r.window_secs}s`} />
                <RuleStat
                  label="Last value"
                  value={r.last_value === null ? "—" : String(r.last_value)}
                />
                <RuleStat
                  label="Evaluated"
                  value={r.last_evaluated_at ? r.last_evaluated_at.slice(11, 19) : "never"}
                />
                <RuleStat label="Channel" value={channelName(r.channel_id)} />
              </div>
              {r.last_error && (
                <p className="text-xs text-destructive">{r.last_error}</p>
              )}
              <div className="flex items-center gap-2 border-t border-[color:var(--border-subtle)] pt-3">
                <Button
                  size="sm"
                  variant="outline"
                  disabled={evaluate.isPending}
                  onClick={() => evaluate.mutate(r.id)}
                >
                  <Play className="h-3.5 w-3.5" />
                  Evaluate now
                </Button>
                <button
                  type="button"
                  title="Delete rule"
                  onClick={() => remove.mutate(r.id)}
                  className="ml-auto flex h-[30px] items-center rounded-[6px] border border-[color:var(--border-subtle)] px-2 text-[color:var(--status-danger)] transition-colors hover:bg-[color:var(--red-tint)]"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </button>
              </div>
            </div>
          );
        })}
      </div>

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
      onDone();
      onOpenChange(false);
    },
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>Add rule</DialogTitle>
        <DialogDescription>
          Fires when the signal crosses the threshold within the window.
        </DialogDescription>
      </DialogHeader>
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
        <div className="grid grid-cols-2 gap-3">
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
        {create.isError && (
          <p className="text-xs text-destructive">{(create.error as Error).message}</p>
        )}
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button
          disabled={!name.trim() || !threshold.trim() || create.isPending}
          onClick={() => create.mutate()}
        >
          Create
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// history: every notification the evaluator delivered (or failed to)

const HISTORY_GRID = "150px 1.4fr 110px 130px 2fr";

export function AlertHistory() {
  const history = useQuery({
    queryKey: ["alert-history"],
    queryFn: () => fetchAlertHistory(200),
    retry: false,
  });
  const rules = useQuery({ queryKey: ["alert-rules"], queryFn: fetchAlertRules, retry: false });
  const ruleName = (id: string) => rules.data?.find((r) => r.id === id)?.name ?? id.slice(0, 8);

  return (
    <PageBody>
      <span className="text-sm text-muted-foreground">
        {history.data?.length ?? 0} notifications · newest first
      </span>
      {history.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
      {history.isError && (
        <p className="text-sm text-muted-foreground">
          Alert history needs superadmin access: {(history.error as Error).message}
        </p>
      )}
      {history.data && history.data.length === 0 && (
        <p className="text-sm text-muted-foreground">No alerts delivered yet.</p>
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
                  {n.sent_at.slice(0, 19).replace("T", " ")}
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
