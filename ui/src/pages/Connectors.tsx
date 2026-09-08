import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Cable, FileCode2, FlaskConical, Trash2, Loader2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { EditorSheet } from "@/components/EditorSheet";
import { superadminOnly } from "@/components/ForbiddenScreen";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton, PanelSkeleton } from "@/components/LoadingState";
import { PageBody, Pill, StatusDot, Toolbar } from "@/components/screen";
import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/code-block";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import {
  createConnector,
  deleteConnector,
  fetchCollectorConfig,
  fetchConnectors,
  testConnector,
  updateConnector,
  type ConnectorRow,
} from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

// `[label, tint]`: the label colour is the -text half of the hue, because a
// health pill is a glyph on a tint rather than a shape (#1181)
const HEALTH_TONE: Record<string, [string, string]> = {
  healthy: ["var(--status-success-text)", "rgba(22,163,74,.14)"],
  unhealthy: ["var(--status-danger-text)", "var(--red-tint)"],
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

/**
 * The document the connectors are actually delivered through (#1195).
 *
 * ADR-0026 put the per-destination fan-out in an OpenTelemetry Collector, not
 * in N in-process exporters — so defining a connector here does nothing until
 * a collector is running this config. The screen used to define connectors and
 * never mention that, which left a freshly added connector with no visible way
 * to receive anything.
 *
 * The document is fetched only while the dialog is open: it is rendered from
 * the connector rows on every request and can carry a resolved bearer token,
 * so there is no reason to hold it in the cache behind a closed dialog.
 */
function CollectorConfigDialog({
  open,
  onOpenChange,
  connectorCount,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  connectorCount: number;
}) {
  const { t } = useTranslation();
  const config = useQuery({
    queryKey: ["collector-config"],
    queryFn: fetchCollectorConfig,
    enabled: open,
    gcTime: 0,
    retry: false,
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>{t("pages.connectors.collectorConfig.title")}</DialogTitle>
        <DialogDescription>
          {t("pages.connectors.collectorConfig.where")}
        </DialogDescription>
      </DialogHeader>

      {/* no connectors means no exporters and no pipelines: the document is
          valid and delivers nothing, which is worth saying rather than
          rendering as an almost-empty file */}
      {connectorCount === 0 ? (
        <EmptyState
          uxTarget="collector-config"
          icon={<FileCode2 />}
          title={t("pages.connectors.collectorConfig.emptyTitle")}
          description={t("pages.connectors.collectorConfig.emptyBody")}
        />
      ) : (
        <>
          {config.isLoading && <PanelSkeleton panels={1} height={240} />}
          {config.isError && (
            <LoadError
              error={config.error}
              resource={t("errors.resources.collectorConfig")}
              onRetry={() => void config.refetch()}
            />
          )}
          {config.data !== undefined && (
            <>
              <div className="flex items-center gap-2">
                <span className="font-mono text-xs text-[color:var(--text-subtle)]">
                  {t("pages.connectors.collectorConfig.endpoint")}
                </span>
              </div>
              {/* a collector document is YAML an operator pastes into a
                  deployment: highlighted, numbered and copyable, because a
                  badly indented exporter is the failure this screen exists to
                  prevent (#949). CodeBlock owns the copy button and the
                  focusable scroll region */}
              <CodeBlock
                value={config.data}
                language="yaml"
                label={t("pages.connectors.collectorConfig.title")}
                maxHeight={380}
                lineNumbers
              />
              <p className="text-sm text-muted-foreground">
                {t("pages.connectors.collectorConfig.deploy")}
              </p>
            </>
          )}
        </>
      )}

      <DialogFooter>
        <Button variant="ghost" onClick={() => onOpenChange(false)}>
          {t("common.close")}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

// OTLP log-shipping connectors: request logs mirrored to Datadog, Langfuse,
// or any OTLP/HTTP collector, with per-connector sampling and health checks
function ConnectorsScreen() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const queryClient = useQueryClient();
  const toast = useToast();
  const connectors = useQuery({
    queryKey: ["connectors"],
    queryFn: fetchConnectors,
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `connectors` is the query the user is actually waiting on for this screen
  useScreenReady(!connectors.isLoading);
  useErrorState(!!connectors.error, "connectors");
  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["connectors"] });

  const toggle = useMutation({
    mutationFn: (c: ConnectorRow) => updateConnector(c.id, { ...asInput(c), enabled: !c.enabled }),
    onSuccess: invalidate,
    // a switch that bounced back reads as nothing having happened (#1197)
    onError: (error, c) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: c.name }),
        detail: errorDetail(error),
      });
    },
  });
  const test = useMutation({ mutationFn: testConnector, onSuccess: invalidate });
  const remove = useMutation({ mutationFn: deleteConnector, onSuccess: invalidate });

  const [addOpen, setAddOpen] = React.useState(false);
  const [configOpen, setConfigOpen] = React.useState(false);
  // log shipping stops the moment the connector goes, and the delivery history
  // goes with it — worth saying before the click (#1179)
  const [deleteTarget, setDeleteTarget] = React.useState<ConnectorRow | null>(null);
  const startDelete = (connector: ConnectorRow) => {
    remove.reset();
    setDeleteTarget(connector);
  };

  return (
    <PageBody>
      <Toolbar>
        <span className="text-sm text-muted-foreground">
          {connectors.data?.length ?? 0} connectors · OTLP/HTTP sinks for request logs
        </span>
        {/* the config sits beside "add", because it is the other half of the
            job: a connector row does nothing until a collector runs this */}
        <Button
          className="ml-auto"
          variant="outline"
          disabled={connectors.isError}
          onClick={() => setConfigOpen(true)}
        >
          <FileCode2 className="h-4 w-4" aria-hidden />
          {t("pages.connectors.collectorConfig.open")}
        </Button>
        <GatedButton gate="connector:create" onClick={() => setAddOpen(true)}>+ Add connector</GatedButton>
      </Toolbar>

      {connectors.isLoading && <CardGridSkeleton cards={3} height={186} min={380} />}
      {/* the endpoint is superadmin-only, so a non-superadmin lands on the
          `forbidden` kind, which names who can widen the role rather than
          asserting a permission problem for every cause (#1180) */}
      {connectors.isError && (
        <LoadError
          error={connectors.error}
          resource={t("errors.resources.connectors")}
          onRetry={() => void connectors.refetch()}
        />
      )}
      {connectors.data && connectors.data.length === 0 && (
        <EmptyState
          uxTarget="connectors"
          icon={<Cable />}
          title={t("pages.connectors.emptyTitle")}
          description={t("pages.connectors.emptyBody")}
          actions={
            <GatedButton gate="connector:create" onClick={() => setAddOpen(true)}>
              {t("pages.connectors.emptyAction")}
            </GatedButton>
          }
        />
      )}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(380px,100%),1fr))]">
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
                  aria-label={t("pages.connectors.toggleAria", { name: c.name })}
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
                <Pill color="var(--status-info-text)" tint="rgba(59,130,246,.14)">
                  {Math.round(c.sampling_rate * 100)}% sampled
                </Pill>
                {c.auth_secret_configured && (
                  <Pill color="var(--text-secondary)" tint="var(--surface-subtle)">
                    secret set
                  </Pill>
                )}
              </div>
              {c.health_error && (
                <p className="text-xs text-[color:var(--status-danger-text)]">{c.health_error}</p>
              )}
              {/* the probe's own verdict, which the row only picks up after the
                  invalidated list comes back. saying why delivery failed at the
                  moment the operator pressed the button is the whole point of
                  the test (#1178) */}
              {test.data && test.variables === c.id && !test.data.delivered && (
                <p className="text-xs text-[color:var(--status-danger-text)]">
                  {t("pages.connectors.testFailed", {
                    message: test.data.health_error ?? test.data.health_status,
                  })}
                </p>
              )}
              <div className="flex items-center gap-2 border-t border-[color:var(--border-subtle)] pt-3">
                <Button
                  size="sm"
                  variant="outline"
                  disabled={test.isPending && test.variables === c.id}
                  onClick={() => test.mutate(c.id)}
                >
                  {test.isPending && test.variables === c.id ? (
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  ) : (
                    <FlaskConical className="h-3.5 w-3.5" />
                  )}
                  Test delivery
                </Button>
                {c.health_checked_at && (
                  <span className="text-[0.6875rem] text-[color:var(--text-subtle)]">
                    {t("pages.connectors.checkedAt", { time: fmt.time(c.health_checked_at) })}
                  </span>
                )}
                <button
                  type="button"
                  title="Delete connector"
                  aria-label={`Delete connector ${c.name}`}
                  disabled={remove.isPending && remove.variables === c.id}
                  onClick={() => startDelete(c)}
                  className="ml-auto flex h-[30px] items-center rounded-[6px] border border-[color:var(--border-subtle)] px-2 text-[color:var(--status-danger-text)] transition-colors hover:bg-[color:var(--red-tint)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50 disabled:pointer-events-none"
                >
                  {remove.isPending && remove.variables === c.id ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Trash2 className="h-3.5 w-3.5" />}
                </button>
              </div>
            </div>
          );
        })}
      </div>
      {(test.isError || (remove.isError && !deleteTarget)) && (
        <p className="text-xs text-[color:var(--status-danger-text)]">
          {((test.error ?? remove.error) as Error).message}
        </p>
      )}

      <ConfirmDialog
        open={!!deleteTarget}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title={t("pages.connectors.confirm.title", { name: deleteTarget?.name })}
        description={t("pages.connectors.confirm.body")}
        confirmLabel={t("pages.connectors.confirm.confirm")}
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

      <CollectorConfigDialog
        open={configOpen}
        onOpenChange={setConfigOpen}
        connectorCount={connectors.data?.length ?? 0}
      />

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
  const { t } = useTranslation();
  const toast = useToast();
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

  const dirty = !!(name.trim() || endpoint.trim() || secret.trim() || sampling !== "100");

  return (
    <EditorSheet
      open={open}
      onOpenChange={onOpenChange}
      title="Add connector"
      subtitle="OTLP/HTTP collector endpoint request logs are exported to."
      dirty={dirty}
      errorMessage={create.isError ? (create.error as Error).message : undefined}
      saveLabel="Create"
      canSave={!!name.trim() && !!endpoint.trim()}
      saving={create.isPending}
      onSave={() => create.mutate()}
    >
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
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
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
      </div>
    </EditorSheet>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(ConnectorsScreen, "errors.resources.connectors");
