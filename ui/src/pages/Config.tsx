import { useQuery } from "@tanstack/react-query";
import { Check } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { InfoHint } from "@/components/ui/info-hint";
import { PageHeader } from "@/components/ui/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { StatusRow } from "@/components/ui/status-row";
import { Textarea } from "@/components/ui/textarea";
import { fetchConfig, type GatewayConfigDto } from "@/lib/api";

// render the effective config as TOML for a read-only snapshot. secrets
// (virtual keys) are counted, never serialized.
function toToml(cfg: GatewayConfigDto): string {
  const lines: string[] = [];
  for (const p of cfg.providers) {
    lines.push("[[providers]]");
    lines.push(`name = ${JSON.stringify(p.name)}`);
    lines.push(`kind = ${JSON.stringify(p.kind)}`);
    lines.push(`api_base = ${JSON.stringify(p.api_base)}`);
    lines.push("");
  }
  for (const r of cfg.routes) {
    lines.push("[[routes]]");
    lines.push(`model = ${JSON.stringify(r.model)}`);
    lines.push(`strategy = ${JSON.stringify(r.strategy)}`);
    for (const t of r.targets) {
      lines.push("[[routes.targets]]");
      lines.push(`provider = ${JSON.stringify(t.provider)}`);
      if (t.model) lines.push(`model = ${JSON.stringify(t.model)}`);
      lines.push(`weight = ${t.weight}`);
    }
    lines.push("");
  }
  return lines.join("\n").trim() || "# no providers or routes configured";
}

export default function Config() {
  const config = useQuery({ queryKey: ["config"], queryFn: fetchConfig });

  const cfg = config.data;
  const toml = cfg ? toToml(cfg) : "";

  return (
    <div className="space-y-6">
      <PageHeader
        title="Config"
        description="Effective routing config served to the gateway. Managed in rolter.toml or the control-plane store; changes hot-reload with no gateway restart."
        actions={
          <span className="inline-flex items-center gap-1.5">
            <Badge tone="success" dot>
              read-only
            </Badge>
            <InfoHint text="Editing config live from the dashboard isn't wired yet — update rolter.toml or the control-plane store and the gateway picks it up via snapshot reload. Tracked as a follow-up." />
          </span>
        }
      />

      {config.isError ? (
        <Card>
          <CardContent className="py-10 text-center text-sm text-muted-foreground">
            Failed to load config: {(config.error as Error).message}
          </CardContent>
        </Card>
      ) : (
        <div className="grid gap-4 lg:grid-cols-[1fr_280px]">
          <Card>
            <CardHeader>
              <CardTitle className="text-base">rolter.toml</CardTitle>
              <CardDescription>
                Providers and routes as the gateway sees them
              </CardDescription>
            </CardHeader>
            <CardContent>
              {config.isLoading ? (
                <Skeleton height={320} />
              ) : (
                <Textarea
                  readOnly
                  value={toml}
                  className="min-h-[320px] font-mono text-xs"
                  spellCheck={false}
                />
              )}
            </CardContent>
          </Card>

          <div className="space-y-4">
            <Card>
              <CardHeader>
                <CardTitle className="text-base">Snapshot</CardTitle>
                <CardDescription>What's live right now</CardDescription>
              </CardHeader>
              <CardContent className="flex flex-col gap-1">
                {config.isLoading ? (
                  <Skeleton height={72} />
                ) : (
                  <>
                    <StatusRow
                      status="success"
                      chevron={false}
                      label={`${cfg?.providers.length ?? 0} providers`}
                    />
                    <StatusRow
                      status="success"
                      chevron={false}
                      label={`${cfg?.routes.length ?? 0} routes`}
                    />
                    <StatusRow
                      status="success"
                      chevron={false}
                      label={`${cfg?.virtual_keys.length ?? 0} virtual keys`}
                    />
                  </>
                )}
              </CardContent>
            </Card>
            <Card>
              <CardContent className="flex items-start gap-2 py-4 text-xs text-muted-foreground">
                <Check className="mt-0.5 h-3.5 w-3.5 flex-none text-[color:var(--status-success)]" />
                <span>
                  Config hot-swaps with no restart — the gateway polls the
                  control plane's snapshot endpoint.
                </span>
              </CardContent>
            </Card>
          </div>
        </div>
      )}
    </div>
  );
}
