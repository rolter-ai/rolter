import * as React from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";

interface SecurityState {
  authEnabled: boolean;
  adminUser: string;
  adminPass: string;
  enforceVk: boolean;
  allowDirect: boolean;
  allowedOrigins: string;
  allowedHeaders: string;
  requiredHeaders: string;
  whitelistedRoutes: string;
}

const DEFAULTS: SecurityState = {
  authEnabled: true,
  adminUser: "",
  adminPass: "",
  enforceVk: true,
  allowDirect: false,
  allowedOrigins: "",
  allowedHeaders: "",
  requiredHeaders: "",
  whitelistedRoutes: "",
};

// security settings from the design prototype. the control plane has no
// security-settings API yet — the form is fully rendered but saves locally
// only, labelled as preview.
export default function Security() {
  const [sec, setSec] = React.useState<SecurityState>(DEFAULTS);
  const [dirty, setDirty] = React.useState(false);
  const [saved, setSaved] = React.useState(false);

  const set = (patch: Partial<SecurityState>) => {
    setSec((s) => ({ ...s, ...patch }));
    setDirty(true);
    setSaved(false);
  };

  const disabledAuth = !sec.authEnabled;

  return (
    <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
      <div className="flex items-center gap-2">
        <Badge tone="warning" className="font-mono text-[10px] uppercase">
          preview — not yet persisted
        </Badge>
        <span className="text-xs text-muted-foreground">
          Backend security-settings API pending; changes stay in this session.
        </span>
      </div>

      <section className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div className="flex items-start gap-4">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="text-sm font-medium">Password protect the dashboard</span>
              <Badge tone="info">BETA</Badge>
            </div>
            <p className="mt-1 text-sm text-muted-foreground">
              Set up authentication credentials to protect your dashboard. Once configured, use
              the generated token for all admin API calls.
            </p>
          </div>
          <Switch checked={sec.authEnabled} onCheckedChange={(v) => set({ authEnabled: v })} />
        </div>
        <div className="flex flex-col gap-1.5" style={{ opacity: disabledAuth ? 0.55 : 1 }}>
          <label className="text-xs font-medium text-[color:var(--text-secondary)]">Username</label>
          <Input
            value={sec.adminUser}
            disabled={disabledAuth}
            placeholder="Enter admin username or env.VAR_NAME"
            onChange={(e) => set({ adminUser: e.target.value })}
          />
        </div>
        <div className="flex flex-col gap-1.5" style={{ opacity: disabledAuth ? 0.55 : 1 }}>
          <label className="text-xs font-medium text-[color:var(--text-secondary)]">Password</label>
          <Input
            type="password"
            value={sec.adminPass}
            disabled={disabledAuth}
            placeholder="Enter admin password or env.VAR_NAME"
            onChange={(e) => set({ adminPass: e.target.value })}
          />
          <span className="text-[0.6875rem] text-[color:var(--text-subtle)]">
            Use at least 12 characters with uppercase, lowercase, number, and special character.
            Env var references are accepted.
          </span>
        </div>
      </section>

      <ToggleCard
        title="Enforce Virtual Keys on Inference"
        desc="Require a virtual key for all inference requests."
        checked={sec.enforceVk}
        onChange={(v) => set({ enforceVk: v })}
      />
      <ToggleCard
        title="Allow Direct API Keys"
        desc="When enabled, callers can pass a provider API key directly in the Authorization header, bypassing the registered key pool."
        checked={sec.allowDirect}
        onChange={(v) => set({ allowDirect: v })}
      />

      <TextCard
        title="Allowed Origins"
        desc="Comma-separated list of allowed origins for CORS and WebSocket connections. Localhost origins are always allowed. Wildcards are supported for subdomains (e.g. https://*.example.com) or use “*” to allow all origins."
        value={sec.allowedOrigins}
        placeholder="https://app.example.com, https://*.example.com, *"
        onChange={(v) => set({ allowedOrigins: v })}
      />
      <TextCard
        title="Allowed Headers"
        desc="Comma-separated list of allowed headers for CORS."
        value={sec.allowedHeaders}
        placeholder="X-Stainless-Timeout"
        onChange={(v) => set({ allowedHeaders: v })}
      />
      <TextCard
        title="Required Headers"
        desc="Comma-separated list of headers that must be present on every request. Requests missing any of these headers are rejected with a 400 error."
        value={sec.requiredHeaders}
        placeholder="X-Tenant-ID, X-Custom-Header"
        onChange={(v) => set({ requiredHeaders: v })}
      />
      <TextCard
        title="Whitelisted Routes"
        desc="Comma-separated list of routes that bypass the auth middleware. System routes like /health and the login endpoints are always whitelisted."
        value={sec.whitelistedRoutes}
        placeholder="/api/custom-webhook, /api/public-endpoint"
        onChange={(v) => set({ whitelistedRoutes: v })}
      />

      <div className="sticky bottom-0 flex items-center justify-end gap-3 border-t border-[color:var(--border-subtle)] bg-background py-3">
        {saved && (
          <span className="text-xs text-[color:var(--status-success)]">
            Security settings updated.
          </span>
        )}
        <Button
          disabled={!dirty}
          onClick={() => {
            setDirty(false);
            setSaved(true);
          }}
        >
          Save Changes
        </Button>
      </div>
    </div>
  );
}

function ToggleCard({
  title,
  desc,
  checked,
  onChange,
}: {
  title: string;
  desc: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <section className="flex items-start gap-4 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
      <div className="min-w-0 flex-1">
        <span className="text-sm font-medium">{title}</span>
        <p className="mt-1 text-sm text-muted-foreground">{desc}</p>
      </div>
      <Switch checked={checked} onCheckedChange={onChange} />
    </section>
  );
}

function TextCard({
  title,
  desc,
  value,
  placeholder,
  onChange,
}: {
  title: string;
  desc: string;
  value: string;
  placeholder: string;
  onChange: (v: string) => void;
}) {
  return (
    <section className="flex flex-col gap-2.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
      <div>
        <span className="text-sm font-medium">{title}</span>
        <p className="mt-1 text-sm text-muted-foreground">{desc}</p>
      </div>
      <Textarea
        className="min-h-[76px] font-mono text-xs"
        value={value}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
      />
    </section>
  );
}
