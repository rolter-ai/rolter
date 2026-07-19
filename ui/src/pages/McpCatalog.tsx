import * as React from "react";

import { HEALTH_COLOR, PageBody, StatusDot } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { MCP_SERVERS, type MockMcpServer } from "@/lib/mock";

// MCP catalog from the design prototype. the gateway has no MCP registry API
// yet, so this renders the prototype's preview data with local-only toggles.
export default function McpCatalog() {
  const [servers, setServers] = React.useState<MockMcpServer[]>(MCP_SERVERS);
  const toolCount = servers.reduce((a, s) => a + s.tools.length, 0);

  const toggle = (id: string, enabled: boolean) =>
    setServers((ss) => ss.map((s) => (s.id === id ? { ...s, enabled } : s)));

  return (
    <PageBody>
      <div className="flex items-center gap-3">
        <span className="text-sm text-muted-foreground">
          {servers.length} servers · {toolCount} tools exposed through the gateway
        </span>
        <Badge tone="warning" className="font-mono text-[10px] uppercase">
          preview data
        </Badge>
        <Button className="ml-auto" disabled title="MCP registry API is not available yet">
          + Register server
        </Button>
      </div>

      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(320px,1fr))]">
        {servers.map((m) => (
          <div
            key={m.id}
            className="flex flex-col gap-3 rounded-[10px] border border-[color:var(--border-default)] bg-card p-4"
            style={{ opacity: m.enabled ? 1 : 0.55 }}
          >
            <div className="flex items-center gap-2.5">
              <StatusDot color={HEALTH_COLOR[m.status]} className="h-2 w-2" />
              <span className="font-mono text-sm font-semibold">{m.name}</span>
              <Badge tone="outline">{m.transport}</Badge>
              <Switch
                className="ml-auto"
                checked={m.enabled}
                onCheckedChange={(v) => toggle(m.id, v)}
              />
            </div>
            <div className="flex flex-wrap gap-1.5">
              {m.tools.map((tool) => (
                <span
                  key={tool}
                  className="rounded-[6px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-2 py-0.5 font-mono text-xs text-[color:var(--text-secondary)]"
                >
                  {tool}
                </span>
              ))}
            </div>
            <div className="flex items-center gap-2 border-t border-[color:var(--border-subtle)] pt-2.5">
              <span className="text-xs text-muted-foreground">{m.tools.length} tools</span>
              <span className="ml-auto font-mono text-xs text-[color:var(--text-secondary)]">
                {m.calls24h.toLocaleString()} calls · 24h
              </span>
            </div>
          </div>
        ))}
      </div>
    </PageBody>
  );
}
