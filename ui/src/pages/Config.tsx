import { useQuery } from "@tanstack/react-query";
import { Check, ShieldCheck } from "lucide-react";
import * as React from "react";

import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { fetchConfig } from "@/lib/api";
import { FEATURE_FLAGS, type FeatureFlag } from "@/lib/mock";

const SECTION_TH =
  "border-b border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3.5 py-2 text-[0.6875rem] uppercase tracking-[0.07em] text-[color:var(--text-subtle)]";

// effective config from the design prototype: structured read-only provider /
// route tables on the left, feature-flag cards on the right
export default function Config() {
  const config = useQuery({ queryKey: ["config"], queryFn: fetchConfig });
  const [flags, setFlags] = React.useState<FeatureFlag[]>(FEATURE_FLAGS);

  const cfg = config.data;
  const summary = cfg
    ? `${cfg.providers.length} providers · ${cfg.routes.length} routes · ${cfg.virtual_keys.length} virtual keys`
    : "";

  return (
    <div className="grid items-start gap-4 p-[22px] xl:grid-cols-[1.5fr_1fr]">
      <div className="flex min-w-0 flex-col gap-3">
        <div className="flex items-center gap-2.5">
          <h2 className="text-base font-medium">Effective config</h2>
          <span className="font-mono text-xs text-[color:var(--text-subtle)]">{summary}</span>
          <span className="ml-auto inline-flex items-center gap-1.5 text-xs text-[color:var(--status-success)]">
            <span className="h-[7px] w-[7px] rounded-full bg-[color:var(--status-success)]" />
            reload-free
          </span>
        </div>
        <div className="inline-flex items-center gap-2 rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3 py-2 text-xs text-muted-foreground">
          <ShieldCheck className="h-3.5 w-3.5 flex-none text-[color:var(--red-folk)]" />
          Admin-only · read-only view. Config is applied from{" "}
          <span className="font-mono text-[color:var(--text-secondary)]">rolter.toml</span> or the
          control-plane store and synced on reload.
        </div>

        {config.isError && (
          <p className="text-sm text-destructive">
            Failed to load config: {(config.error as Error).message}
          </p>
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
          <Check className="mt-0.5 h-3.5 w-3.5 flex-none text-[color:var(--status-success)]" />
          Config hot-swaps with no restart — the gateway polls the control plane's snapshot
          endpoint.
        </p>
      </div>

      <div className="flex flex-col gap-3">
        <div className="flex items-center gap-2">
          <h2 className="text-base font-medium">Feature flags</h2>
          <Badge tone="warning" className="font-mono text-[10px] uppercase">
            preview
          </Badge>
        </div>
        <div className="flex flex-col gap-2.5">
          {flags.map((f) => (
            <div
              key={f.key}
              className="flex items-center gap-3 rounded-[10px] border border-[color:var(--border-default)] bg-card px-4 py-3.5"
            >
              <div className="min-w-0 flex-1">
                <div className="text-sm">{f.label}</div>
                <div className="text-xs text-muted-foreground">{f.desc}</div>
              </div>
              <Switch
                checked={f.on}
                onCheckedChange={(on) =>
                  setFlags((fs) => fs.map((x) => (x.key === f.key ? { ...x, on } : x)))
                }
              />
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
