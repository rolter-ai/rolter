import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ArrowRight, Check, ShieldCheck } from "lucide-react";
import { Link } from "react-router";

import { CopyButton } from "@/components/CopyButton";
import { LoadError } from "@/components/LoadError";
import { CodeBlock } from "@/components/ui/code-block";
import { Skeleton } from "@/components/ui/skeleton";
import { fetchConfig, type GatewayConfigDto } from "@/lib/api";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const SECTION_TH =
  "border-b border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3.5 py-2 text-[0.6875rem] uppercase tracking-[0.07em] text-[color:var(--text-subtle)]";

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
    ? `${cfg.providers.length} providers · ${cfg.routes.length} routes · ${cfg.virtual_keys.length} virtual keys`
    : "";

  return (
    <div className="grid items-start gap-4 p-[22px] xl:grid-cols-[1.5fr_1fr]">
      <div className="flex min-w-0 flex-col gap-3">
        <div className="flex flex-wrap items-center gap-2.5">
          <h2 className="text-base font-medium">Effective config</h2>
          <span className="font-mono text-xs text-[color:var(--text-subtle)]">{summary}</span>
          <span className="ml-auto inline-flex items-center gap-1.5 text-xs text-[color:var(--status-success-text)]">
            <span className="h-[7px] w-[7px] rounded-full bg-[color:var(--status-success)]" />
            reload-free
          </span>
        </div>
        <div className="inline-flex items-center gap-2 rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3 py-2 text-xs text-muted-foreground">
          <ShieldCheck className="h-3.5 w-3.5 flex-none text-[color:var(--red-folk-text)]" />
          Admin-only · read-only view. Config is applied from{" "}
          <span className="font-mono text-[color:var(--text-secondary)]">rolter.toml</span> or the
          control-plane store and synced on reload.
        </div>

        {config.isError && (
          <LoadError
            error={config.error}
            resource={t("errors.resources.config")}
            onRetry={() => config.refetch()}
          />
        )}
        {config.isLoading && <Skeleton height={280} radius={10} />}

        {cfg && (
          <div className="overflow-hidden rounded-[10px] border border-[color:var(--border-subtle)]">
            <div className={SECTION_TH}>Providers</div>
            <div className="grid grid-cols-[1fr_1.1fr_2fr] gap-3 border-b border-[color:var(--border-subtle)] px-3.5 py-2 text-[0.6875rem] uppercase tracking-[0.06em] text-[color:var(--text-subtle)]">
              <span>Name</span>
              <span>Kind</span>
              <span>API base</span>
            </div>
            {cfg.providers.map((p) => (
              <div
                key={p.name}
                className="grid grid-cols-[1fr_1.1fr_2fr] items-center gap-3 border-b border-[color:var(--border-subtle)] px-3.5 py-[9px] font-mono text-xs"
              >
                <span>{p.name}</span>
                <span className="text-[color:var(--text-secondary)]">{p.kind}</span>
                <span className="truncate text-muted-foreground">{p.api_base}</span>
              </div>
            ))}
            <div className={SECTION_TH}>Routes</div>
            <div className="grid grid-cols-[1.2fr_1.1fr_2fr] gap-3 border-b border-[color:var(--border-subtle)] px-3.5 py-2 text-[0.6875rem] uppercase tracking-[0.06em] text-[color:var(--text-subtle)]">
              <span>Model</span>
              <span>Strategy</span>
              <span>Targets</span>
            </div>
            {cfg.routes.map((r) => (
              <div
                key={r.model}
                className="grid grid-cols-[1.2fr_1.1fr_2fr] items-center gap-3 border-b border-[color:var(--border-subtle)] px-3.5 py-[9px] font-mono text-xs last:border-b-0"
              >
                <span>{r.model}</span>
                <span className="text-[color:var(--text-secondary)]">{r.strategy}</span>
                <span className="truncate text-muted-foreground">
                  {r.targets
                    .map((t) => `${t.provider}${t.weight ? ` ${t.weight}` : ""}`)
                    .join(" · ") || "—"}
                </span>
              </div>
            ))}
          </div>
        )}

        <p className="flex items-start gap-2 text-xs text-muted-foreground">
          <Check className="mt-0.5 h-3.5 w-3.5 flex-none text-[color:var(--status-success-text)]" />
          Config hot-swaps with no restart — the gateway polls the control plane's snapshot
          endpoint.
        </p>

        {cfg && <AllSections config={cfg} />}
      </div>

      <div className="flex flex-col gap-3">
        <h2 className="text-base font-medium">Related settings</h2>
        <RelatedLink
          to="/feature-flags"
          title="Feature flags"
          desc="Toggle experimental gateway features. Persisted and hot-reloaded — no restart."
        />
        <RelatedLink
          to="/client-settings"
          title="Client settings"
          desc="Base URL, forwarded and injected headers, request correlation."
        />
        <RelatedLink
          to="/model-settings"
          title="Model settings"
          desc="Deployment-wide defaults for sampling parameters and the model."
        />
        <RelatedLink
          to="/performance"
          title="Performance tuning"
          desc="Upstream retries, timeouts, and the bounded admission queue."
        />
      </div>
    </div>
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
  const sections = Object.entries(config).filter(
    ([key]) => !TABLED.has(key) && !HIDDEN.has(key),
  );
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
          <details key={key} className="group border-b border-[color:var(--border-subtle)] last:border-b-0">
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

function summarize(value: unknown, t: (key: string, opts?: Record<string, unknown>) => string): string {
  if (Array.isArray(value)) return t("pages.config.allSections.entries", { count: value.length });
  if (value && typeof value === "object") {
    return t("pages.config.allSections.fields", { count: Object.keys(value).length });
  }
  return String(value);
}
