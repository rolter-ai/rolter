import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, Cable, FileCode2, FlaskConical, Loader2, Pencil, Plus } from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { CopyButton } from "@/components/CopyButton";
import { EditorSheet } from "@/components/EditorSheet";
import { superadminOnly } from "@/components/ForbiddenScreen";
import { GatedButton } from "@/components/GatedButton";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { GatedSwitch } from "@/components/GatedSwitch";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton, LoadingRegion, PanelSkeleton } from "@/components/LoadingState";
import {
  ListSummary,
  PageBody,
  Pill,
  RowIconButton,
  StatusDot,
  Toolbar,
} from "@/components/screen";
import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/code-block";
import {
  Dialog,
  DialogBody,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { FieldLabel } from "@/components/ui/field-label";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { SwitchRow } from "@/components/ui/switch-row";
import {
  collectorConfigUrl,
  createConnector,
  deleteConnector,
  fetchCollectorConfig,
  fetchConnectors,
  testConnector,
  updateConnector,
  type ConnectorRow,
} from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { movesOrigin } from "@/lib/origin";
import { parseSamplingPercent, samplingPercentText } from "@/lib/sampling";
import { errorDetail, useToast } from "@/lib/toast";
import { usePublicUrl } from "@/lib/use-public-url";
import { useScreenReady } from "@/lib/ux-react";

// the /15 wash of a status fill hue that a pill sits on (DESIGN.md, Status)
const statusTint = (hue: "success" | "danger" | "info") =>
  `color-mix(in srgb, var(--status-${hue}) 15%, transparent)`;

// `[label, tint]`: the label colour is the -text half of the hue, because a
// health pill is a glyph on a tint rather than a shape (#1181)
const HEALTH_TONE: Record<string, [string, string]> = {
  healthy: ["var(--status-success-text)", statusTint("success")],
  unhealthy: ["var(--status-danger-text)", statusTint("danger")],
  unknown: ["var(--text-secondary)", "var(--surface-subtle)"],
};

const healthTone = (status: string) => HEALTH_TONE[status] ?? HEALTH_TONE.unknown;

// the created-off toast carries an instruction, so it stays up as long as an
// error does rather than the few seconds a plain success gets
const NEXT_STEP_TOAST_MS = 8000;

const asInput = (c: ConnectorRow) => ({
  name: c.name,
  kind: "otlp_http" as const,
  endpoint: c.endpoint,
  enabled: c.enabled,
  sampling_rate: c.sampling_rate,
  auth_secret_ref: c.auth_secret_ref,
});

/**
 * The address the collector config is served from, and what it takes to read it
 * (#2106).
 *
 * The endpoint answers a superadmin principal and nothing narrower, and a
 * collector has no session to present one, so pointing a collector at it would
 * mean putting the admin token in the collector's own deployment. The row shows
 * the address for the operator's own tooling and says so; the document below it
 * is what goes into the collector's config file.
 *
 * The address is the control plane's public base, read from the control plane
 * (the query the Single Sign-On and User Provisioning screens share), never
 * `window.location`: the dashboard may be open under a different name than the
 * one a script calls. Pending holds the space and a failed read says so with a
 * retry rather than a URL that might be wrong. Unset, the base is the control
 * plane's default, which only a caller on its own host can reach: still shown
 * and copyable, with that said under it.
 *
 * It mounts only while the dialog is open, so the read happens when somebody
 * asks for the document and not on every visit to the screen.
 */
function CollectorEndpoint() {
  const { t } = useTranslation();
  const publicUrl = usePublicUrl();
  const labelId = React.useId();
  const value = publicUrl.data ? collectorConfigUrl(publicUrl.data.public_url) : null;
  return (
    <div role="group" aria-labelledby={labelId} className="flex min-w-0 flex-col gap-1.5">
      <FieldLabel id={labelId} label={t("pages.connectors.collectorConfig.endpoint")} />
      {publicUrl.isError ? (
        <LoadError
          error={publicUrl.error}
          resource={t("errors.resources.publicUrl")}
          onRetry={() => void publicUrl.refetch()}
          target="public-url"
        />
      ) : value ? (
        <div className="flex items-center justify-between gap-2 rounded-md border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] py-1.5 pl-3 pr-1.5">
          <code
            data-testid="collector-config-url"
            className="min-w-0 break-all font-mono text-sm text-foreground"
          >
            {value}
          </code>
          <CopyButton value={value} label={t("pages.connectors.collectorConfig.copyEndpoint")} />
        </div>
      ) : (
        <LoadingRegion className="w-full">
          <Skeleton height={46} radius={6} />
        </LoadingRegion>
      )}
      <p className="text-xs text-muted-foreground">
        {t("pages.connectors.collectorConfig.endpointHint")}
      </p>
      {publicUrl.data?.configured === false && (
        <p
          role="note"
          className="flex items-start gap-1.5 text-xs text-[color:var(--status-warning-text)]"
        >
          <AlertTriangle aria-hidden className="mt-px h-3.5 w-3.5 flex-none" />
          <span>
            <Trans
              i18nKey="pages.connectors.collectorConfig.urlUnset"
              components={{ code: <code className="font-mono" /> }}
            />
          </span>
        </p>
      )}
    </div>
  );
}

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
  enabledCount,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  connectorCount: number;
  /** how many of them are switched on: the only ones the document carries */
  enabledCount: number;
}) {
  const { t } = useTranslation();
  // the document is rendered from the enabled rows alone, so a list that is all
  // switched off has nothing in it to fetch or to show
  const deliverable = enabledCount > 0;
  const config = useQuery({
    queryKey: ["collector-config"],
    queryFn: fetchCollectorConfig,
    enabled: open && deliverable,
    gcTime: 0,
    retry: false,
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>{t("pages.connectors.collectorConfig.title")}</DialogTitle>
        <DialogDescription>{t("pages.connectors.collectorConfig.where")}</DialogDescription>
      </DialogHeader>

      {/* the body scrolls when the window is short, so the title and Close stay
          on screen: the address, its note and the document together are taller
          than a 640 px window (#2003) */}
      <DialogBody className="space-y-3">
        {/* no enabled connector means no exporters and no pipelines: the
            document is valid and delivers nothing, which is worth saying rather
            than rendering as an almost-empty file. a connector that is switched
            off is left out of it, so "there are connectors" is not enough, and
            the two causes have different remedies (#2364) */}
        {!deliverable ? (
          <EmptyState
            uxTarget={connectorCount === 0 ? "collector-config" : "collector-config-all-off"}
            icon={<FileCode2 />}
            title={t(
              connectorCount === 0
                ? "pages.connectors.collectorConfig.emptyTitle"
                : "pages.connectors.collectorConfig.allOffTitle",
            )}
            description={t(
              connectorCount === 0
                ? "pages.connectors.collectorConfig.emptyBody"
                : "pages.connectors.collectorConfig.allOffBody",
            )}
          />
        ) : (
          <>
            <CollectorEndpoint />
            {config.isLoading && <PanelSkeleton panels={1} height={240} />}
            {config.isError && (
              <LoadError
                error={config.error}
                resource={t("errors.resources.collectorConfig")}
                onRetry={() => void config.refetch()}
                target="collector-config"
              />
            )}
            {config.data !== undefined && (
              <>
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
      </DialogBody>

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
  // a probe that ran reports its verdict either way. a rejected one is written
  // on the card it belongs to; one that got through has no card line to appear
  // on (the pill only changes after the refetch, and not at all for a connector
  // that was already healthy), so it says so in a toast
  const test = useMutation({
    mutationFn: (c: ConnectorRow) => testConnector(c.id),
    onSuccess: (result, c) => {
      invalidate();
      if (result.delivered) {
        toast.push({
          tone: "success",
          title: t("pages.connectors.testDelivered", { name: c.name }),
        });
      }
    },
  });
  const remove = useMutation({ mutationFn: deleteConnector, onSuccess: invalidate });

  const [sheetOpen, setSheetOpen] = React.useState(false);
  // the connector the sheet edits, or `null` when it adds one. it outlives the
  // sheet closing, so the title does not flip to "Add connector" while the
  // sheet is still sliding away
  const [editTarget, setEditTarget] = React.useState<ConnectorRow | null>(null);
  const openSheet = (connector: ConnectorRow | null) => {
    setEditTarget(connector);
    setSheetOpen(true);
  };
  const [configOpen, setConfigOpen] = React.useState(false);
  // log shipping stops the moment the connector goes, and the delivery history
  // goes with it — worth saying before the click (#1179)
  const [deleteTarget, setDeleteTarget] = React.useState<ConnectorRow | null>(null);
  const startDelete = (connector: ConnectorRow) => {
    remove.reset();
    setDeleteTarget(connector);
  };
  // a connector has no tenancy scope, so its row controls are the superadmin's
  // exactly as the add button is (#1258)

  // what went wrong with this card's own test or delete, for the line under it.
  // a failed delete reports in the confirmation while that is open, so the card
  // takes over once it is closed. the list carries the probe's reason too once
  // it is refetched, and the same sentence twice says nothing new
  const problemWith = (c: ConnectorRow): string | null => {
    const probed = test.variables?.id === c.id;
    if (probed && test.data && !test.data.delivered) {
      const reason = test.data.health_error ?? test.data.health_status;
      return reason === c.health_error
        ? null
        : t("pages.connectors.testFailed", { message: reason });
    }
    if (probed && test.isError) {
      return t("pages.connectors.testError", { message: errorDetail(test.error) ?? "" });
    }
    if (remove.isError && remove.variables === c.id && !deleteTarget) {
      return t("pages.connectors.deleteError", { message: errorDetail(remove.error) ?? "" });
    }
    return null;
  };

  return (
    <PageBody>
      <Toolbar>
        <ListSummary data={connectors.data}>
          {(rows) => t("pages.connectors.summary", { count: rows.length })}
        </ListSummary>
        {/* the config sits beside "add", because it is the other half of the
            job: a connector row does nothing until a collector runs this */}
        <Button
          className="ml-auto"
          variant="outline"
          // the dialog says "no connectors" when handed none, so it waits for
          // a list that answered rather than one still loading or failed
          disabled={!connectors.isSuccess}
          onClick={() => setConfigOpen(true)}
        >
          <FileCode2 className="h-4 w-4" aria-hidden />
          {t("pages.connectors.collectorConfig.open")}
        </Button>
        <GatedButton
          gate="connector:create"
          control="connector-new"
          onClick={() => openSheet(null)}
        >
          <Plus className="h-4 w-4" aria-hidden />
          {t("pages.connectors.add")}
        </GatedButton>
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
          target="connectors"
        />
      )}
      {/* the empty state offers the same create as the toolbar, which stays
          where it sits on every other list */}
      {connectors.data && connectors.data.length === 0 && (
        <EmptyState
          uxTarget="connectors"
          icon={<Cable />}
          title={t("pages.connectors.emptyTitle")}
          description={t("pages.connectors.emptyBody")}
          actions={
            <GatedButton
              gate="connector:create"
              control="connector-new-empty"
              onClick={() => openSheet(null)}
            >
              {t("pages.connectors.emptyAction")}
            </GatedButton>
          }
        />
      )}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(380px,100%),1fr))]">
        {(connectors.data ?? []).map((c) => {
          const tone = healthTone(c.health_status);
          const problem = problemWith(c);
          return (
            <div
              key={c.id}
              // named after the connector, so the card's own test, delete and
              // error line read as belonging to it
              role="group"
              aria-label={c.name}
              className="flex flex-col gap-3 rounded-[10px] border border-[color:var(--border-default)] bg-card p-4"
            >
              <div className="flex items-center gap-2.5">
                <span className="flex h-[34px] w-[34px] flex-none items-center justify-center rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] text-[color:var(--text-secondary)]">
                  <Cable className="h-4 w-4" />
                </span>
                <div className="min-w-0 flex-1">
                  <div className="font-mono text-sm font-semibold">{c.name}</div>
                  {/* wraps rather than truncating: at 375 px a cut-off URL has no
                      other way to be read, and a host is checked by its end */}
                  <div className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
                    {c.endpoint}
                  </div>
                </div>
                <GatedSwitch
                  gate="connector:update"
                  control="connector-toggle"
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
                {/* health is its own axis: a connector that was never turned
                    on reads `unknown` because nothing has tested it, and the
                    switch alone does not say that nothing is being sent */}
                {!c.enabled && (
                  <Pill color="var(--text-secondary)" border="var(--border-strong)">
                    {t("pages.connectors.off")}
                  </Pill>
                )}
                <Pill color={tone[0]} tint={tone[1]}>
                  <StatusDot color={tone[0]} className="h-1.5 w-1.5" />
                  {c.health_status}
                </Pill>
                {/* the blue is for a rate that is being applied */}
                <Pill
                  color={c.enabled ? "var(--status-info-text)" : "var(--text-secondary)"}
                  tint={c.enabled ? statusTint("info") : "var(--surface-subtle)"}
                >
                  {/* a rate is not always a whole percent, and 0.4 % rounded to
                      "0% sampled" says nothing is sent while something is */}
                  {t("pages.connectors.sampled", {
                    percent: fmt.number(c.sampling_rate * 100, { maximumFractionDigits: 2 }),
                  })}
                </Pill>
                {c.auth_secret_configured && (
                  <Pill color="var(--text-secondary)" tint="var(--surface-subtle)">
                    {t("pages.connectors.secretSet")}
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
              {problem && (
                <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">
                  {problem}
                </p>
              )}
              <div className="flex flex-wrap items-center gap-2 border-t border-[color:var(--border-subtle)] pt-3">
                {/* the probe writes the connector's health back, so the
                    control plane guards it as an update */}
                <GatedButton
                  gate="connector:update"
                  control="connector-test"
                  size="sm"
                  variant="outline"
                  aria-label={t("pages.connectors.testAria", { name: c.name })}
                  disabled={test.isPending && test.variables?.id === c.id}
                  onClick={() => test.mutate(c)}
                >
                  {test.isPending && test.variables?.id === c.id ? (
                    <Loader2 className="h-3.5 w-3.5 motion-safe:animate-spin" />
                  ) : (
                    <FlaskConical className="h-3.5 w-3.5" />
                  )}
                  {t("pages.connectors.testDelivery")}
                </GatedButton>
                {/* a probe from last week must not read as one from today, so the
                    row says how long ago and keeps the full stamp for the hover */}
                {c.health_checked_at && (
                  <time
                    dateTime={c.health_checked_at}
                    title={fmt.dateTime(c.health_checked_at)}
                    className="text-[0.6875rem] text-[color:var(--text-subtle)]"
                  >
                    {t("pages.connectors.checkedAt", { time: fmt.relative(c.health_checked_at) })}
                  </time>
                )}
                {/* both name the connector: a grid of cards each offering "Edit"
                    is N buttons a screen reader cannot tell apart */}
                <div className="ml-auto flex items-center gap-1.5">
                  <RowIconButton
                    gate="connector:update"
                    control="connector-edit"
                    className="p-1.5"
                    title={t("pages.connectors.editAria", { name: c.name })}
                    aria-label={t("pages.connectors.editAria", { name: c.name })}
                    onClick={() => openSheet(c)}
                  >
                    <Pencil className="h-3.5 w-3.5" />
                  </RowIconButton>
                  <DeleteIconButton
                    gate="connector:delete"
                    control="connector-delete"
                    label={t("pages.connectors.deleteAria", { name: c.name })}
                    pending={remove.isPending && remove.variables === c.id}
                    onClick={() => startDelete(c)}
                  />
                </div>
              </div>
            </div>
          );
        })}
      </div>

      <ConfirmDialog
        name="connector-delete"
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
        enabledCount={connectors.data?.filter((c) => c.enabled).length ?? 0}
      />

      <ConnectorSheet
        open={sheetOpen}
        onOpenChange={setSheetOpen}
        existing={editTarget}
        onDone={invalidate}
      />
    </PageBody>
  );
}

/**
 * One sheet for adding a connector and editing one (#2101).
 *
 * An edit is a single `PUT` to the same id, so the connector keeps its health
 * history. The control plane replaces the whole row from the body, which is why
 * the fields the sheet has no control for go back as found: `enabled` (the
 * card's switch owns it) and `auth_secret_ref`. The bearer secret is write-only
 * and a blank field leaves it out of the body, which the control plane reads as
 * "keep the stored one". It cannot be cleared through the API, so the sheet
 * offers no clear, and it is not dropped when the endpoint moves to another
 * origin, which the hint under the field says.
 */
function ConnectorSheet({
  open,
  onOpenChange,
  existing,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** the connector to edit, or `null` to add one */
  existing: ConnectorRow | null;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  // what the sampling field opens on: every request for a new connector, the
  // stored rate read as a percentage for an edit
  const openingSampling = existing ? samplingPercentText(existing.sampling_rate) : "100";
  const [name, setName] = React.useState("");
  const [endpoint, setEndpoint] = React.useState("");
  const [sampling, setSampling] = React.useState(openingSampling);
  // write-only: an edit starts blank, because the stored secret is never read back
  const [secret, setSecret] = React.useState("");
  // a connector is an egress path, so it is created switched off unless the
  // operator says otherwise: the order the docs teach is test it, then turn it
  // on, and the first request log must not leave before the first test (#2349).
  // an edit has no such choice, because the card's switch owns `enabled`
  const [startNow, setStartNow] = React.useState(false);

  // the field is read as typed: 0 is a rate (nothing is sent), and a blank,
  // non-numeric or out-of-range value blocks Save instead of becoming one
  // (#2104). the control plane accepts 0 to 1 inclusive, so 0 is not refused
  const parsed = parseSamplingPercent(sampling);
  const samplingError = parsed.ok
    ? undefined
    : t(
        parsed.problem === "range"
          ? "pages.connectors.form.samplingRange"
          : "pages.connectors.form.samplingInvalid",
      );

  // the switch is read into the variables, not from state when the request
  // lands: a flip while it is in flight would otherwise announce a state the
  // request did not carry
  const save = useMutation({
    mutationFn: ({ rate, enabled }: { rate: number; enabled: boolean }) => {
      // a blank secret is left out, which an update reads as "keep the stored one"
      const replacement = secret.trim() ? { managed_auth_secret: secret } : {};
      return existing
        ? updateConnector(existing.id, {
            ...asInput(existing),
            name,
            endpoint,
            sampling_rate: rate,
            ...replacement,
          })
        : createConnector({
            name,
            kind: "otlp_http",
            endpoint,
            enabled,
            sampling_rate: rate,
            ...replacement,
          });
    },
    onSuccess: (_row, { enabled }) => {
      // the sheet closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      if (existing) {
        // a save keeps the health the last test recorded, and after a new
        // endpoint or secret that describes the old one
        const retest =
          !!existing.health_checked_at &&
          (endpoint.trim() !== existing.endpoint || !!secret.trim());
        toast.push({
          tone: "success",
          title: t("toast.saved"),
          detail: retest
            ? t("pages.connectors.savedRetest", { name })
            : t("toast.savedDetail", { what: name }),
        });
      } else {
        // one that was left off says so and what to do next, or nothing ever
        // arrives and nothing says why
        toast.push(
          enabled
            ? { tone: "success", title: t("toast.created", { what: name }) }
            : {
                tone: "success",
                title: t("pages.connectors.createdOff", { name }),
                detail: t("pages.connectors.createdOffNext"),
                duration: NEXT_STEP_TOAST_MS,
              },
        );
      }
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
      setSampling(openingSampling);
      setSecret("");
      setStartNow(false);
      // a refusal for one connector must not greet the next one opened
      save.reset();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, existing]);

  const dirty = existing
    ? name !== existing.name ||
      endpoint !== existing.endpoint ||
      sampling !== openingSampling ||
      secret !== ""
    : !!(name.trim() || endpoint.trim() || secret.trim() || sampling !== "100" || startNow);

  // what happens to the stored secret on save, said beside the field that
  // decides it. the control plane keeps it when the endpoint moves, so the
  // hint says where it would go rather than promising it is dropped
  const secretHint = !existing
    ? undefined
    : !existing.auth_secret_configured
      ? t("pages.connectors.form.secretNoneHint")
      : secret.trim()
        ? t("pages.connectors.form.secretReplaceHint")
        : movesOrigin(existing.endpoint, endpoint)
          ? t("pages.connectors.form.secretMovesHint")
          : t("pages.connectors.form.secretKeepHint");

  return (
    <EditorSheet
      name={existing ? "connector-edit" : "connector-create"}
      open={open}
      onOpenChange={onOpenChange}
      title={
        existing
          ? t("pages.connectors.editTitle", { name: existing.name })
          : t("pages.connectors.add")
      }
      subtitle={t("pages.connectors.sheetSubtitle")}
      dirty={dirty}
      errorMessage={save.isError ? (save.error as Error).message : undefined}
      saveLabel={existing ? t("common.save") : t("common.create")}
      canSave={!!name.trim() && !!endpoint.trim() && parsed.ok}
      saving={save.isPending}
      onSave={() => {
        if (!parsed.ok) return;
        // a field left as it opened sends the stored rate itself: its text is
        // a reading of the rate, and a rate with more digits than it shows
        // would come back changed
        const rate =
          existing && sampling === openingSampling ? existing.sampling_rate : parsed.rate;
        save.mutate({ rate, enabled: startNow });
      }}
    >
      <div className="space-y-3">
        <Field label={t("pages.connectors.form.name")}>
          <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="datadog" />
        </Field>
        <Field label={t("pages.connectors.form.endpoint")}>
          <Input
            className="font-mono"
            value={endpoint}
            onChange={(e) => setEndpoint(e.target.value)}
            placeholder="https://otlp.example.com/v1/logs"
          />
        </Field>
        <Field
          label={t("pages.connectors.form.sampling")}
          hint={t("pages.connectors.form.samplingHint")}
          error={samplingError}
        >
          <Input
            type="number"
            inputMode="decimal"
            min={0}
            max={100}
            step="any"
            value={sampling}
            onChange={(e) => setSampling(e.target.value)}
          />
        </Field>
        <Field
          label={t(existing ? "pages.connectors.form.secretEdit" : "pages.connectors.form.secret")}
          hint={secretHint}
        >
          <Input
            type="password"
            value={secret}
            onChange={(e) => setSecret(e.target.value)}
            placeholder={t("pages.connectors.form.secretPlaceholder")}
          />
        </Field>
        {/* said again when it is on, because that is the choice with a
            consequence: request logs leave for the endpoint as Create lands */}
        {!existing && (
          <SwitchRow
            title={t("pages.connectors.form.start")}
            hint={t(
              startNow ? "pages.connectors.form.startOnHint" : "pages.connectors.form.startHint",
            )}
            checked={startNow}
            onChange={setStartNow}
          />
        )}
      </div>
    </EditorSheet>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(ConnectorsScreen, "errors.resources.connectors");
