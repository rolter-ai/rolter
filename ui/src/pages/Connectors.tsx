import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Cable, FlaskConical, Trash2 } from "lucide-react";
import * as React from "react";

import { PageBody, Pill, StatusDot } from "@/components/screen";
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
import { Switch } from "@/components/ui/switch";
import {
  createConnector,
  deleteConnector,
  fetchConnectors,
  testConnector,
  updateConnector,
  type ConnectorRow,
} from "@/lib/api";

const HEALTH_TONE: Record<string, [string, string]> = {
  healthy: ["var(--status-success)", "rgba(22,163,74,.14)"],
  unhealthy: ["var(--status-danger)", "var(--red-tint)"],
  unknown: ["var(--text-secondary)", "var(--surface-subtle)"],
};

const healthTone = (status: string) => HEALTH_TONE[status] ?? HEALTH_TONE.unknown;

const asInput = (c: ConnectorRow) => ({
  name: c.name,
  kind: "otlp_http" as const,
  endpoint: c.endpoint,
  enabled: c.enabled,
  sampling_rate: c.sampling_rate,
  auth_secret_ref: c.auth_secret_ref,
});

// OTLP log-shipping connectors: request logs mirrored to Datadog, Langfuse,
// or any OTLP/HTTP collector, with per-connector sampling and health checks
export default function Connectors() {
  const queryClient = useQueryClient();
  const connectors = useQuery({
    queryKey: ["connectors"],
    queryFn: fetchConnectors,
    retry: false,
  });
  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["connectors"] });

  const toggle = useMutation({
    mutationFn: (c: ConnectorRow) => updateConnector(c.id, { ...asInput(c), enabled: !c.enabled }),
    onSuccess: invalidate,
  });
  const test = useMutation({ mutationFn: testConnector, onSuccess: invalidate });
  const remove = useMutation({ mutationFn: deleteConnector, onSuccess: invalidate });

  const [addOpen, setAddOpen] = React.useState(false);

  return (
    <PageBody>
      <div className="flex items-center gap-3">
        <span className="text-sm text-muted-foreground">
          {connectors.data?.length ?? 0} connectors · OTLP/HTTP sinks for request logs
        </span>
        <Button className="ml-auto" onClick={() => setAddOpen(true)}>
          + Add connector
        </Button>
      </div>

      {connectors.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
      {connectors.isError && (
        <p className="text-sm text-muted-foreground">
          Connectors need superadmin access: {(connectors.error as Error).message}
        </p>
      )}
      {connectors.data && connectors.data.length === 0 && (
        <p className="text-sm text-muted-foreground">
          No connectors yet. Add one to ship request logs to your observability stack.
        </p>
      )}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(380px,1fr))]">
        {(connectors.data ?? []).map((c) => {
          const tone = healthTone(c.health_status);
          return (
            <div
              key={c.id}
              className="flex flex-col gap-3 rounded-[10px] border border-[color:var(--border-default)] bg-card p-4"
            >
              <div className="flex items-center gap-2.5">
                <span className="flex h-[34px] w-[34px] flex-none items-center justify-center rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] text-[color:var(--text-secondary)]">
                  <Cable className="h-4 w-4" />
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
              <div className="flex flex-wrap items-center gap-2">
                <Pill color="var(--text-secondary)" tint="var(--surface-subtle)">
                  {c.kind}
                </Pill>
                <Pill color={tone[0]} tint={tone[1]}>
                  <StatusDot color={tone[0]} className="h-1.5 w-1.5" />
                  {c.health_status}
                </Pill>
                <Pill color="var(--status-info)" tint="rgba(59,130,246,.14)">
                  {Math.round(c.sampling_rate * 100)}% sampled
                </Pill>
                {c.auth_secret_configured && (
                  <Pill color="var(--text-secondary)" tint="var(--surface-subtle)">
                    secret set
                  </Pill>
                )}
              </div>
              {c.health_error && (
                <p className="text-xs text-destructive">{c.health_error}</p>
              )}
              <div className="flex items-center gap-2 border-t border-[color:var(--border-subtle)] pt-3">
                <Button
                  size="sm"
                  variant="outline"
                  disabled={test.isPending}
                  onClick={() => test.mutate(c.id)}
                >
                  <FlaskConical className="h-3.5 w-3.5" />
                  Test delivery
                </Button>
                {c.health_checked_at && (
                  <span className="text-[0.6875rem] text-[color:var(--text-subtle)]">
                    checked {c.health_checked_at.slice(11, 19)}
                  </span>
                )}
                <button
                  type="button"
                  title="Delete connector"
                  onClick={() => remove.mutate(c.id)}
                  className="ml-auto flex h-[30px] items-center rounded-[6px] border border-[color:var(--border-subtle)] px-2 text-[color:var(--status-danger)] transition-colors hover:bg-[color:var(--red-tint)]"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </button>
              </div>
            </div>
          );
        })}
      </div>
      {(test.isError || remove.isError) && (
        <p className="text-xs text-destructive">
          {((test.error ?? remove.error) as Error).message}
        </p>
      )}

      <AddConnectorDialog open={addOpen} onOpenChange={setAddOpen} onDone={invalidate} />
    </PageBody>
  );
}

function AddConnectorDialog({
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
  const [sampling, setSampling] = React.useState("100");
  const [secret, setSecret] = React.useState("");

  React.useEffect(() => {
    if (open) {
      setName("");
      setEndpoint("");
      setSampling("100");
      setSecret("");
    }
  }, [open]);

  const create = useMutation({
    mutationFn: () =>
      createConnector({
        name,
        kind: "otlp_http",
        endpoint,
        enabled: true,
        sampling_rate: Math.min(100, Math.max(0, Number(sampling) || 100)) / 100,
        ...(secret.trim() ? { managed_auth_secret: secret } : {}),
      }),
    onSuccess: () => {
      onDone();
      onOpenChange(false);
    },
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>Add connector</DialogTitle>
        <DialogDescription>
          OTLP/HTTP collector endpoint request logs are exported to.
        </DialogDescription>
      </DialogHeader>
      <div className="space-y-3">
        <Field label="Name">
          <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="datadog" />
        </Field>
        <Field label="Endpoint URL">
          <Input
            className="font-mono"
            value={endpoint}
            onChange={(e) => setEndpoint(e.target.value)}
            placeholder="https://otlp.example.com/v1/logs"
          />
        </Field>
        <div className="grid grid-cols-2 gap-3">
          <Field label="Sampling (%)">
            <Input
              type="number"
              min={0}
              max={100}
              value={sampling}
              onChange={(e) => setSampling(e.target.value)}
            />
          </Field>
          <Field label="Bearer secret (optional)">
            <Input
              type="password"
              value={secret}
              onChange={(e) => setSecret(e.target.value)}
              placeholder="stored encrypted"
            />
          </Field>
        </div>
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
