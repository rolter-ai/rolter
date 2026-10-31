import { useQuery } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";
import { AlertTriangle, ArrowRight, Download, RefreshCw, ShieldCheck } from "lucide-react";
import { Link, useNavigate } from "react-router";

import { ConfigExportDialog } from "@/components/ConfigExportDialog";
import { CopyButton } from "@/components/CopyButton";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { TableSkeleton } from "@/components/LoadingState";
import { UnservedConfigNotice } from "@/components/UnservedConfigNotice";
import {
  ListCell,
  ListEmptyRow,
  ListHeader,
  ListHeaderCell,
  ListRow,
  ListTable,
  primaryColumn,
  SearchInput,
} from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { CodeBlock } from "@/components/ui/code-block";
import { EmptyState } from "@/components/ui/empty-state";
import {
  fetchClusterNodes,
  fetchConfig,
  fetchConfigProblems,
  omittedRows,
  UNNAMED_KEY,
  type GatewayConfigDto,
  type ProviderDto,
  type RouteDto,
  type OmittedEntry,
  type VirtualKeyDto,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { useCan } from "@/lib/can";
import { gatewayPickup, type Pickup } from "@/lib/gateway-pickup";
import { providerKindName } from "@/lib/provider-kinds";
import { useToast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import type { ReadState } from "@/lib/read-state";
import { useScreenReady } from "@/lib/ux-react";

// how often the gateway line asks the inventory again. the Cluster screen polls
// on the same clock, and they share the query
const NODES_POLL_MS = 15_000;

// effective config: the stored config, read-only, with credentials removed.
// feature flags used to render here from a mock; they are persisted and
// hot-reloaded now, so this screen points at the real one rather than shipping
// a second, fake copy of the same switches (#564).
//
// `GET /api/v1/config` is what the control plane stores, not the snapshot the
// gateways receive: `sanitize_for_snapshot` leaves out an entry it cannot serve
// on the way there. `GET /api/v1/config/problems` says which, so every row it
// names is marked, and the gateways' own versions say whether they run it (#2085)
export default function Config() {
  const { t } = useTranslation();
  const can = useCan();
  const config = useQuery({ queryKey: ["config"], queryFn: fetchConfig });
  // the same key as Providers and the Playground, so the three agree. it is
  // asked even when the config read fails: since #2943 a setting the store
  // cannot read fails the whole load, and this is the read that says which
  const problems = useQuery({ queryKey: ["config-problems"], queryFn: fetchConfigProblems });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `config` is the query the user is actually waiting on for this screen
  useScreenReady(!config.isLoading);

  const cfg = config.data;
  const omitted = problems.data?.omitted;
  const summary = cfg
    ? t("pages.config.summary.line", {
        providers: t("pages.config.summary.providers", { count: cfg.providers.length }),
        routes: t("pages.config.summary.routes", { count: cfg.routes.length }),
        keys: t("pages.config.summary.keys", { count: cfg.virtual_keys.length }),
      })
    : "";

  return (
    <div className="flex flex-col gap-4 p-[22px]">
      <div className="flex min-w-0 flex-col gap-3">
        {/* the shell's header already names the screen, so this row is only its summary */}
        <div className="flex flex-wrap items-center gap-2.5">
          <span className="font-mono text-xs text-[color:var(--text-subtle)]">{summary}</span>
          <div className="ml-auto flex flex-wrap items-center gap-2.5">
            {/* only a caller who may read the inventory is told about it, and
                only over a config that loaded: a gateway line that reads
                healthy above a failed load is the claim #2085 removed */}
            {cfg && can("cluster_node", "read") === true && <GatewayLine />}
            <ExportButton />
          </div>
        </div>
        <div className="inline-flex items-center gap-2 rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3 py-2 text-xs text-muted-foreground">
          <ShieldCheck className="h-3.5 w-3.5 flex-none text-[color:var(--text-subtle)]" />
          <span>
            <Trans
              i18nKey="pages.config.readOnlyNotice"
              components={[
                <span key="file" className="font-mono text-[color:var(--text-secondary)]" />,
              ]}
            />
          </span>
        </div>

        {config.isError && (
          // load-error-allow: one read feeds both tables (config-providers, config-routes); neither owns it
          <LoadError
            error={config.error}
            resource={t("errors.resources.config")}
            onRetry={() => config.refetch()}
            target="config"
          />
        )}
        {/* a store that is down fails both reads, and that is one outage: the
            config's alert above says it, so this one speaks only alone */}
        {problems.isError && !config.isError && (
          // load-error-allow: the omissions only decorate the tables, whose own states live on the config read
          <LoadError
            error={problems.error}
            resource={t("errors.resources.configProblems")}
            onRetry={() => problems.refetch()}
            target="config-problems"
          />
        )}
        <UnservedConfigNotice problems={problems.data?.problems ?? []} />
        {config.isLoading && <TableSkeleton rows={6} />}

        {cfg && (
          <>
            <ProvidersTable read={config} providers={cfg.providers} omitted={omitted} />
            <RoutesTable read={config} routes={cfg.routes} omitted={omitted} />
            {cfg.virtual_keys.length > 0 && (
              <VirtualKeysTable keys={cfg.virtual_keys} omitted={omitted} />
            )}
          </>
        )}

        <p className="flex items-start gap-2 text-xs text-muted-foreground">
          <RefreshCw
            aria-hidden
            className="mt-0.5 h-3.5 w-3.5 flex-none text-[color:var(--text-subtle)]"
          />
          {t("pages.config.hotSwapNote")}
        </p>

        {cfg && <AllSections config={cfg} />}
      </div>

      {/* four short links sit in a row under the tables rather than taking a
          column of the page from them */}
      <section className="flex flex-col gap-2">
        <h3 className="text-sm font-medium">{t("pages.config.related.title")}</h3>
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          <RelatedLink
            to="/feature-flags"
            title={t("pages.config.related.featureFlags.title")}
            desc={t("pages.config.related.featureFlags.desc")}
          />
          <RelatedLink
            to="/client-settings"
            title={t("pages.config.related.clientSettings.title")}
            desc={t("pages.config.related.clientSettings.desc")}
          />
          <RelatedLink
            to="/model-settings"
            title={t("pages.config.related.modelSettings.title")}
            desc={t("pages.config.related.modelSettings.desc")}
          />
          <RelatedLink
            to="/performance"
            title={t("pages.config.related.performance.title")}
            desc={t("pages.config.related.performance.desc")}
          />
        </div>
      </section>
    </div>
  );
}

/**
 * How many live gateways run the control plane's current config version, read
 * from the cluster inventory, with the way to the Cluster screen beside it.
 *
 * Only `converged` is coloured as a success and only `lagging` as a warning:
 * a read that is out, failed or found no gateway knows nothing about the
 * fleet, so it says so in plain text.
 */
function GatewayLine() {
  const { t } = useTranslation();
  const nodes = useQuery({
    queryKey: ["cluster-nodes"],
    queryFn: fetchClusterNodes,
    retry: false,
    // a failed first read stops asking, so its line does not flicker back to
    // "checking" on every poll; the next visit asks again
    refetchInterval: (query) =>
      query.state.status === "error" && query.state.data === undefined ? false : NODES_POLL_MS,
  });
  const pickup = gatewayPickup(nodes.data, nodes.isError);
  const tone =
    pickup.state === "converged"
      ? "text-[color:var(--status-success-text)]"
      : pickup.state === "lagging"
        ? "text-[color:var(--status-warning-text)]"
        : "text-[color:var(--text-subtle)]";
  const dot =
    pickup.state === "converged"
      ? "bg-[color:var(--status-success)]"
      : pickup.state === "lagging"
        ? "bg-[color:var(--status-warning)]"
        : null;
  return (
    <span className="inline-flex flex-wrap items-center gap-x-2 gap-y-1 text-xs">
      <span className={`inline-flex items-center gap-1.5 ${tone}`}>
        {dot && <span aria-hidden className={`h-[7px] w-[7px] flex-none rounded-full ${dot}`} />}
        {gatewayLine(t, pickup)}
      </span>
      <Link
        to="/cluster"
        className="rounded-sm font-medium text-foreground underline decoration-[color:var(--border-strong)] underline-offset-4 transition-colors hover:decoration-current focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      >
        {t("pages.config.gateways.link")}
      </Link>
    </span>
  );
}

function gatewayLine(t: TFunction, pickup: Pickup): string {
  switch (pickup.state) {
    case "checking":
      return t("pages.config.gateways.checking");
    case "unavailable":
      return t("pages.config.gateways.unavailable");
    case "none":
      return t("pages.config.gateways.none");
    case "converged":
      return t("pages.config.gateways.converged", { count: pickup.total });
    case "lagging":
      return t("pages.config.gateways.lagging", {
        count: pickup.total,
        converged: pickup.converged,
      });
  }
}

/** The badge on a row the snapshot leaves out. */
function NotServed() {
  const { t } = useTranslation();
  return <Badge tone="warning">{t("pages.config.tables.notServed")}</Badge>;
}

/**
 * The control plane's reason a row is not served, in its own words.
 *
 * The sentence comes from `sanitize_for_snapshot` and is English in every
 * locale, so it says so to a screen reader.
 */
function OmissionReason({ reason }: { reason: string }) {
  return (
    <span className="flex items-start gap-1.5 font-sans text-[color:var(--status-warning-text)]">
      <AlertTriangle aria-hidden className="mt-px h-3.5 w-3.5 flex-none" />
      <span lang="en">{reason}</span>
    </span>
  );
}

const PROVIDER_GRID = `${primaryColumn(1)} 1.1fr 2fr`;
const ROUTE_GRID = `${primaryColumn(1.2)} 1.1fr 2fr`;
const KEY_GRID = `${primaryColumn(1)} 2fr`;
// values here are config the operator reads in full: a base url or a model name
// wraps rather than truncating, so nothing is only readable by resizing
const VALUE = "min-w-0 break-all font-mono text-xs";

function TableSection({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="flex flex-col gap-2">
      <h3 className="text-sm font-medium">{title}</h3>
      {children}
    </section>
  );
}

function ProvidersTable({
  read,
  providers,
  omitted,
}: {
  read: ReadState;
  providers: ProviderDto[];
  omitted: readonly OmittedEntry[] | undefined;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const title = t("pages.config.tables.providers");
  const notServed = omittedRows(
    omitted,
    ["provider"],
    providers.map((p) => p.name),
  );
  return (
    <TableSection title={title}>
      <ListTable label={title} minWidth={560}>
        <ListHeader grid={PROVIDER_GRID}>
          <ListHeaderCell>{t("pages.config.tables.name")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.config.tables.kind")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.config.tables.apiBase")}</ListHeaderCell>
        </ListHeader>
        {providers.map((p, i) => {
          const omission = notServed.get(i)?.reason;
          return (
            <ListRow
              key={`${p.name}-${i}`}
              grid={PROVIDER_GRID}
              className={omission === undefined ? undefined : "items-start"}
            >
              <ListCell className={`${VALUE} flex flex-col items-start gap-1`}>
                <span>{p.name}</span>
                {omission !== undefined && <NotServed />}
              </ListCell>
              <ListCell className="min-w-0 text-xs text-[color:var(--text-secondary)]">
                {providerKindName(p.kind, t)}
              </ListCell>
              <ListCell className={`${VALUE} flex flex-col gap-1 text-muted-foreground`}>
                <span>{p.api_base}</span>
                {omission !== undefined && <OmissionReason reason={omission} />}
              </ListCell>
            </ListRow>
          );
        })}
        <ListEmptyRow read={read} rows={providers.length}>
          <EmptyState
            thread
            uxTarget="config-providers"
            title={t("pages.config.tables.emptyProvidersTitle")}
            description={t("pages.config.tables.emptyProvidersBody")}
            actions={
              <GatedButton
                gate="provider:create"
                control="config-providers-empty"
                onClick={() => navigate("/providers")}
              >
                {t("pages.config.tables.addProvider")}
              </GatedButton>
            }
          />
        </ListEmptyRow>
      </ListTable>
    </TableSection>
  );
}

function RoutesTable({
  read,
  routes,
  omitted,
}: {
  read: ReadState;
  routes: RouteDto[];
  omitted: readonly OmittedEntry[] | undefined;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const title = t("pages.config.tables.routes");
  const notServed = omittedRows(
    omitted,
    ["route"],
    routes.map((r) => r.model),
  );
  return (
    <TableSection title={title}>
      <ListTable label={title} minWidth={600}>
        <ListHeader grid={ROUTE_GRID}>
          <ListHeaderCell>{t("pages.config.tables.model")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.config.tables.strategy")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.config.tables.targets")}</ListHeaderCell>
        </ListHeader>
        {/* the endpoint strips tenancy, so two orgs' routes can share a public
            name: the position keeps the keys apart */}
        {routes.map((r, i) => {
          const reason = notServed.get(i)?.reason;
          return (
            <ListRow key={`${r.model}-${i}`} grid={ROUTE_GRID} className="items-start">
              <ListCell className={`${VALUE} flex flex-col items-start gap-1`}>
                <span>{r.model}</span>
                {reason !== undefined && <NotServed />}
              </ListCell>
              <ListCell className={`${VALUE} text-[color:var(--text-secondary)]`}>
                {r.strategy}
              </ListCell>
              <ListCell className={`${VALUE} flex flex-col gap-1 text-muted-foreground`}>
                {r.targets.length === 0 && <span>{t("pages.config.tables.noTargets")}</span>}
                {r.targets.map((target, j) => (
                  <span key={`${target.provider}-${j}`}>
                    <span className="text-foreground">{target.provider}</span>
                    {target.model ? (
                      <> · {t("pages.config.tables.upstreamModel", { model: target.model })}</>
                    ) : null}
                    {" · "}
                    {t("pages.config.tables.weight", { value: target.weight })}
                  </span>
                ))}
                {reason !== undefined && <OmissionReason reason={reason} />}
              </ListCell>
            </ListRow>
          );
        })}
        <ListEmptyRow read={read} rows={routes.length}>
          <EmptyState
            thread
            uxTarget="config-routes"
            title={t("pages.config.tables.emptyRoutesTitle")}
            description={t("pages.config.tables.emptyRoutesBody")}
            actions={
              <GatedButton
                gate="route:create"
                control="config-routes-empty"
                onClick={() => navigate("/routing-rules")}
              >
                {t("pages.config.tables.addRoute")}
              </GatedButton>
            }
          />
        </ListEmptyRow>
      </ListTable>
    </TableSection>
  );
}

// virtual keys defined in the config file. the secret is never drawn: a name
// and what the key may call is what a reader needs, and the table has no column
// the secret could land in
function VirtualKeysTable({
  keys,
  omitted,
}: {
  keys: VirtualKeyDto[];
  omitted: readonly OmittedEntry[] | undefined;
}) {
  const { t } = useTranslation();
  const title = t("pages.config.tables.virtualKeys");
  const notServed = omittedRows(
    omitted,
    ["virtual_key", "public_example_key"],
    keys.map((k) => k.name || UNNAMED_KEY),
  );
  return (
    <TableSection title={title}>
      <ListTable label={title} minWidth={480}>
        <ListHeader grid={KEY_GRID}>
          <ListHeaderCell>{t("pages.config.tables.name")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.config.tables.allowedModels")}</ListHeaderCell>
        </ListHeader>
        {keys.map((k, i) => {
          const omission = notServed.get(i)?.reason;
          return (
            <ListRow
              key={`${k.name ?? ""}-${i}`}
              grid={KEY_GRID}
              className={omission === undefined ? undefined : "items-start"}
            >
              <ListCell className={`${VALUE} flex flex-col items-start gap-1`}>
                <span>{k.name || "—"}</span>
                {omission !== undefined && <NotServed />}
              </ListCell>
              <ListCell className={`${VALUE} flex flex-col gap-1 text-muted-foreground`}>
                <span>
                  {k.models.length > 0 ? k.models.join(" · ") : t("pages.config.tables.allModels")}
                </span>
                {omission !== undefined && <OmissionReason reason={omission} />}
              </ListCell>
            </ListRow>
          );
        })}
      </ListTable>
    </TableSection>
  );
}

/**
 * Save the deployment's configuration as an importable `rolter.toml` (#1313).
 *
 * The document is rendered by the control plane, not by this screen: what the
 * viewer above shows is the whole stored config with credentials removed, and
 * what `rolter-seed --import` accepts is a narrower file. Downloading the rendered
 * JSON would hand the operator something that looks importable and is not.
 *
 * Gated on `config_export:read` — the whole-deployment, superadmin-only pair
 * the endpoint itself enforces (`crates/rolter-control/src/rbac_matrix.rs`).
 */
function ExportButton() {
  const { t } = useTranslation();
  const toast = useToast();
  const [open, setOpen] = React.useState(false);

  return (
    <>
      <GatedButton
        gate="config_export:read"
        control="config-export"
        variant="outline"
        size="sm"
        onClick={() => setOpen(true)}
      >
        <Download className="h-3.5 w-3.5" />
        {t("pages.config.export.action")}
      </GatedButton>
      <ConfigExportDialog
        open={open}
        onOpenChange={setOpen}
        onExported={(toml, org) => {
          // an object URL rather than a data URI: the document is the whole
          // deployment's config and can run to hundreds of kilobytes, which some
          // browsers refuse to navigate to as a URI
          const url = URL.createObjectURL(new Blob([toml], { type: "application/toml" }));
          const a = document.createElement("a");
          a.href = url;
          // dated, because the reason to keep one of these is to diff it against
          // the next; `rolter.toml` alone would overwrite the last export
          a.download = `rolter-config-${org ? `${org}-` : ""}${new Date().toISOString().slice(0, 10)}.toml`;
          a.click();
          URL.revokeObjectURL(url);
        }}
        onFailed={(error) =>
          toast.push({
            tone: "error",
            title: t("toast.exportFailed", { what: t("errors.resources.configExport") }),
            detail: errorText(error, t),
          })
        }
      />
    </>
  );
}

function RelatedLink({ to, title, desc }: { to: string; title: string; desc: string }) {
  return (
    <Link
      to={to}
      className="group flex items-center gap-3 rounded-lg border border-[color:var(--border-default)] bg-card px-4 py-3.5 transition-colors hover:border-[color:var(--border-strong)]"
    >
      <div className="min-w-0 flex-1">
        <div className="text-sm">{title}</div>
        <div className="text-xs text-muted-foreground">{desc}</div>
      </div>
      <ArrowRight className="h-4 w-4 flex-none text-[color:var(--text-subtle)] transition-colors group-hover:text-foreground" />
    </Link>
  );
}

// the three tables above are the parts an operator reads daily; the stored
// document has some forty sections, and until now the screen called itself
// "effective config" while showing three of them (#1204).
// every remaining section renders here, collapsed, as the JSON the control
// plane returns — credentials are redacted before it leaves the server
const TABLED = new Set(["providers", "routes", "virtual_keys"]);
// carried for the gateway, meaningless to a reader: digests and an
// always-empty (redacted) session list
const HIDDEN = new Set(["db_virtual_keys", "mcp_oauth_sessions"]);

function AllSections({ config }: { config: GatewayConfigDto }) {
  const { t } = useTranslation();
  const [search, setSearch] = React.useState("");
  const sections = Object.entries(config).filter(([key]) => !TABLED.has(key) && !HIDDEN.has(key));
  const json = JSON.stringify(config, null, 2);
  // a section is found by its name or by anything written in it, so `retry`
  // reaches the section that holds a retry setting as well as one called that
  const q = search.trim().toLowerCase();
  const shown = sections.filter(
    ([key, value]) =>
      q === "" || key.toLowerCase().includes(q) || JSON.stringify(value).toLowerCase().includes(q),
  );
  return (
    <section className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        <h3 className="text-sm font-medium">{t("pages.config.allSections.title")}</h3>
        <span className="font-mono text-xs text-[color:var(--text-subtle)]">
          {t("pages.config.allSections.count", { count: sections.length })}
        </span>
        <CopyButton className="ml-auto" value={json} label={t("pages.config.allSections.copy")} />
      </div>
      <p className="text-xs text-muted-foreground">{t("pages.config.allSections.body")}</p>
      <SearchInput
        value={search}
        onChange={(e) => setSearch(e.target.value)}
        placeholder={t("pages.config.allSections.search")}
      />
      {q !== "" && (
        <p role="status" className="text-xs text-muted-foreground">
          {shown.length === 0
            ? t("pages.config.allSections.noMatch", { query: search.trim() })
            : t("pages.config.allSections.matching", {
                shown: shown.length,
                total: sections.length,
              })}
        </p>
      )}
      <div
        className={cn(
          "overflow-hidden rounded-lg border border-[color:var(--border-subtle)]",
          shown.length === 0 && "hidden",
        )}
      >
        {shown.map(([key, value]) => (
          <details
            key={key}
            className="group border-b border-[color:var(--border-subtle)] last:border-b-0"
          >
            <summary className="flex cursor-pointer items-center gap-3 px-3.5 py-2 font-mono text-xs hover:bg-[color:var(--surface-hover)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring">
              <span className="text-foreground">{key}</span>
              <span className="text-[color:var(--text-subtle)]">{summarize(value, t)}</span>
            </summary>
            {/* the section reads as JSON because that is what the gateway
                serves; the shared block gives it the same colours, copy button
                and focusable scroll region as every other payload (#949).
                numbered, because these are the sections an operator quotes in
                a ticket — "line 40 of routes" only means something with a
                gutter to count from */}
            <div className="border-t border-[color:var(--border-subtle)] p-2">
              <CodeBlock
                value={JSON.stringify(value, null, 2)}
                language="json"
                label={key}
                maxHeight={360}
                lineNumbers
              />
            </div>
          </details>
        ))}
      </div>
    </section>
  );
}

function summarize(
  value: unknown,
  t: (key: string, opts?: Record<string, unknown>) => string,
): string {
  if (Array.isArray(value)) return t("pages.config.allSections.entries", { count: value.length });
  if (value && typeof value === "object") {
    return t("pages.config.allSections.fields", { count: Object.keys(value).length });
  }
  return String(value);
}
