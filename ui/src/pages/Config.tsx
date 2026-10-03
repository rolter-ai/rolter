import { useMutation, useQuery } from "@tanstack/react-query";
import { Trans, useTranslation } from "react-i18next";
import { ArrowRight, Check, Download, ShieldCheck } from "lucide-react";
import { Link, useNavigate } from "react-router";

import { CopyButton } from "@/components/CopyButton";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { TableSkeleton } from "@/components/LoadingState";
import {
  ListCell,
  ListEmptyRow,
  ListHeader,
  ListHeaderCell,
  ListRow,
  ListTable,
} from "@/components/screen";
import { CodeBlock } from "@/components/ui/code-block";
import { EmptyState } from "@/components/ui/empty-state";
import {
  exportConfigToml,
  fetchConfig,
  type GatewayConfigDto,
  type ProviderDto,
  type RouteDto,
  type VirtualKeyDto,
} from "@/lib/api";
import { errorDetail, useToast } from "@/lib/toast";
import type { ReadState } from "@/lib/read-state";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

// effective config: structured read-only provider / route tables. feature flags
// used to render here from a mock; they are persisted and hot-reloaded now, so
// this screen points at the real one rather than shipping a second, fake copy
// of the same switches (#564)
export default function Config() {
  const { t } = useTranslation();
  const config = useQuery({ queryKey: ["config"], queryFn: fetchConfig });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `config` is the query the user is actually waiting on for this screen
  useScreenReady(!config.isLoading);
  useErrorState(!!config.error, "config");

  const cfg = config.data;
  const summary = cfg
    ? t("pages.config.summary.line", {
        providers: t("pages.config.summary.providers", { count: cfg.providers.length }),
        routes: t("pages.config.summary.routes", { count: cfg.routes.length }),
        keys: t("pages.config.summary.keys", { count: cfg.virtual_keys.length }),
      })
    : "";

  return (
    <div className="grid items-start gap-4 p-[22px] xl:grid-cols-[1.5fr_1fr]">
      <div className="flex min-w-0 flex-col gap-3">
        <div className="flex flex-wrap items-center gap-2.5">
          <h2 className="text-base font-medium">{t("pages.config.heading")}</h2>
          <span className="font-mono text-xs text-[color:var(--text-subtle)]">{summary}</span>
          <span className="ml-auto inline-flex items-center gap-1.5 text-xs text-[color:var(--status-success-text)]">
            <span className="h-[7px] w-[7px] rounded-full bg-[color:var(--status-success)]" />
            {t("pages.config.reloadFree")}
          </span>
          <ExportButton />
        </div>
        <div className="inline-flex items-center gap-2 rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3 py-2 text-xs text-muted-foreground">
          <ShieldCheck className="h-3.5 w-3.5 flex-none text-[color:var(--red-folk-text)]" />
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
          <LoadError
            error={config.error}
            resource={t("errors.resources.config")}
            onRetry={() => config.refetch()}
          />
        )}
        {config.isLoading && <TableSkeleton rows={6} />}

        {cfg && (
          <>
            <ProvidersTable read={config} providers={cfg.providers} />
            <RoutesTable read={config} routes={cfg.routes} />
            {cfg.virtual_keys.length > 0 && <VirtualKeysTable keys={cfg.virtual_keys} />}
          </>
        )}

        <p className="flex items-start gap-2 text-xs text-muted-foreground">
          <Check className="mt-0.5 h-3.5 w-3.5 flex-none text-[color:var(--status-success-text)]" />
          {t("pages.config.hotSwapNote")}
        </p>

        {cfg && <AllSections config={cfg} />}
      </div>

      <div className="flex flex-col gap-3">
        <h2 className="text-base font-medium">{t("pages.config.related.title")}</h2>
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
    </div>
  );
}

const PROVIDER_GRID = "1fr 1.1fr 2fr";
const ROUTE_GRID = "1.2fr 1.1fr 2fr";
const KEY_GRID = "1fr 2fr";
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

function ProvidersTable({ read, providers }: { read: ReadState; providers: ProviderDto[] }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const title = t("pages.config.tables.providers");
  return (
    <TableSection title={title}>
      <ListTable label={title} minWidth={560}>
        <ListHeader grid={PROVIDER_GRID}>
          <ListHeaderCell>{t("pages.config.tables.name")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.config.tables.kind")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.config.tables.apiBase")}</ListHeaderCell>
        </ListHeader>
        {providers.map((p, i) => (
          <ListRow key={`${p.name}-${i}`} grid={PROVIDER_GRID}>
            <ListCell className={VALUE}>{p.name}</ListCell>
            <ListCell className={`${VALUE} text-[color:var(--text-secondary)]`}>{p.kind}</ListCell>
            <ListCell className={`${VALUE} text-muted-foreground`}>{p.api_base}</ListCell>
          </ListRow>
        ))}
        <ListEmptyRow read={read} rows={providers.length}>
          <EmptyState
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

function RoutesTable({ read, routes }: { read: ReadState; routes: RouteDto[] }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const title = t("pages.config.tables.routes");
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
        {routes.map((r, i) => (
          <ListRow key={`${r.model}-${i}`} grid={ROUTE_GRID} className="items-start">
            <ListCell className={VALUE}>{r.model}</ListCell>
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
            </ListCell>
          </ListRow>
        ))}
        <ListEmptyRow read={read} rows={routes.length}>
          <EmptyState
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
function VirtualKeysTable({ keys }: { keys: VirtualKeyDto[] }) {
  const { t } = useTranslation();
  const title = t("pages.config.tables.virtualKeys");
  return (
    <TableSection title={title}>
      <ListTable label={title} minWidth={480}>
        <ListHeader grid={KEY_GRID}>
          <ListHeaderCell>{t("pages.config.tables.name")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.config.tables.allowedModels")}</ListHeaderCell>
        </ListHeader>
        {keys.map((k, i) => (
          <ListRow key={`${k.name ?? ""}-${i}`} grid={KEY_GRID}>
            <ListCell className={VALUE}>{k.name || "—"}</ListCell>
            <ListCell className={`${VALUE} text-muted-foreground`}>
              {k.models.length > 0 ? k.models.join(" · ") : t("pages.config.tables.allModels")}
            </ListCell>
          </ListRow>
        ))}
      </ListTable>
    </TableSection>
  );
}

/**
 * Save the deployment's configuration as an importable `rolter.toml` (#1313).
 *
 * The document is rendered by the control plane, not by this screen: what the
 * viewer above shows is the *effective* config the gateway runs, and what
 * `rolter-seed --import` accepts is a narrower file. Downloading the rendered
 * JSON would hand the operator something that looks importable and is not.
 *
 * Gated on `config_export:read` — the whole-deployment, superadmin-only pair
 * the endpoint itself enforces (`crates/rolter-control/src/rbac_matrix.rs`).
 */
function ExportButton() {
  const { t } = useTranslation();
  const toast = useToast();

  const exportConfig = useMutation({
    mutationFn: exportConfigToml,
    onSuccess: (toml) => {
      // an object URL rather than a data URI: the document is the whole
      // deployment's config and can run to hundreds of kilobytes, which some
      // browsers refuse to navigate to as a URI
      const url = URL.createObjectURL(new Blob([toml], { type: "application/toml" }));
      const a = document.createElement("a");
      a.href = url;
      // dated, because the reason to keep one of these is to diff it against
      // the next; `rolter.toml` alone would overwrite the last export
      a.download = `rolter-config-${new Date().toISOString().slice(0, 10)}.toml`;
      a.click();
      URL.revokeObjectURL(url);
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.exportFailed", { what: t("errors.resources.configExport") }),
        detail: errorDetail(error),
      });
    },
  });

  return (
    <GatedButton
      gate="config_export:read"
      control="config-export"
      variant="outline"
      size="sm"
      disabled={exportConfig.isPending}
      onClick={() => exportConfig.mutate()}
    >
      <Download className="h-3.5 w-3.5" />
      {exportConfig.isPending ? t("pages.config.export.pending") : t("pages.config.export.action")}
    </GatedButton>
  );
}

function RelatedLink({ to, title, desc }: { to: string; title: string; desc: string }) {
  return (
    <Link
      to={to}
      className="group flex items-center gap-3 rounded-[10px] border border-[color:var(--border-default)] bg-card px-4 py-3.5 transition-colors hover:border-[color:var(--red-folk)]"
    >
      <div className="min-w-0 flex-1">
        <div className="text-sm">{title}</div>
        <div className="text-xs text-muted-foreground">{desc}</div>
      </div>
      <ArrowRight className="h-4 w-4 flex-none text-[color:var(--text-subtle)] transition-colors group-hover:text-[color:var(--red-folk-text)]" />
    </Link>
  );
}

// the three tables above are the parts an operator reads daily; the document
// the gateway actually serves has some forty sections, and until now the
// screen called itself "effective config" while showing three of them (#1204).
// every remaining section renders here, collapsed, as the JSON the control
// plane returns — credentials are redacted before it leaves the server
const TABLED = new Set(["providers", "routes", "virtual_keys"]);
// carried for the gateway, meaningless to a reader: digests and an
// always-empty (redacted) session list
const HIDDEN = new Set(["db_virtual_keys", "mcp_oauth_sessions"]);

function AllSections({ config }: { config: GatewayConfigDto }) {
  const { t } = useTranslation();
  const sections = Object.entries(config).filter(([key]) => !TABLED.has(key) && !HIDDEN.has(key));
  const json = JSON.stringify(config, null, 2);
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
      <div className="overflow-hidden rounded-[10px] border border-[color:var(--border-subtle)]">
        {sections.map(([key, value]) => (
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
