import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import {
  fetchSecuritySettings,
  updateSecuritySettings,
  type SecuritySettingsDto,
} from "@/lib/api";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

interface FormState {
  authEnabled: boolean;
  credentialRef: string;
  managedSecret: string;
  enforceVk: boolean;
  allowedOrigins: string;
  allowedHeaders: string;
  requiredHeaders: string;
  bypassRoutes: string;
}

const splitList = (value: string) =>
  value
    .split(",")
    .map((v) => v.trim())
    .filter(Boolean);

const parseRequiredHeaders = (value: string): Record<string, string> => {
  const headers: Record<string, string> = {};
  for (const pair of splitList(value)) {
    const idx = pair.indexOf(":");
    if (idx > 0) {
      headers[pair.slice(0, idx).trim()] = pair.slice(idx + 1).trim();
    }
  }
  return headers;
};

const fromDto = (dto: SecuritySettingsDto): FormState => ({
  authEnabled: dto.dashboard_auth_enabled,
  credentialRef: dto.dashboard_credential_ref ?? "",
  managedSecret: "",
  enforceVk: dto.virtual_key_required,
  allowedOrigins: dto.allowed_origins.join(", "),
  allowedHeaders: dto.allowed_headers.join(", "),
  requiredHeaders: Object.entries(
    (dto.required_headers ?? {}) as Record<string, string>,
  )
    .map(([k, v]) => `${k}: ${v}`)
    .join(", "),
  bypassRoutes: dto.auth_bypass_routes.join(", "),
});

// global gateway security policy, persisted via /api/v1/security-settings
// (superadmin only). dashboard secret is write-only: the server seals it and
// reports only whether one is configured.
function SecurityScreen() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();
  const settings = useQuery({
    queryKey: ["security-settings"],
    queryFn: fetchSecuritySettings,
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `settings` is the query the user is actually waiting on for this screen
  useScreenReady(!settings.isLoading);
  useErrorState(!!settings.error, "security");

  const [form, setForm] = React.useState<FormState | null>(null);
  React.useEffect(() => {
    if (settings.data && form === null) {
      setForm(fromDto(settings.data));
    }
  }, [settings.data, form]);

  const save = useMutation({
    mutationFn: (f: FormState) =>
      updateSecuritySettings({
        virtual_key_required: f.enforceVk,
        allowed_origins: splitList(f.allowedOrigins),
        allowed_headers: splitList(f.allowedHeaders),
        required_headers: parseRequiredHeaders(f.requiredHeaders),
        auth_bypass_routes: splitList(f.bypassRoutes),
        dashboard_auth_enabled: f.authEnabled,
        dashboard_credential_ref: f.credentialRef.trim() || null,
        ...(f.managedSecret.trim()
          ? { managed_dashboard_secret: f.managedSecret }
          : {}),
      }),
    onSuccess: (dto) => {
      queryClient.setQueryData(["security-settings"], dto);
      // the cached write alone left every other reader of this key on the
      // value it already had; the refetch is what makes the save stick (#1197)
      void queryClient.invalidateQueries({ queryKey: ["security-settings"] });
      setForm(fromDto(dto));
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: t("errors.resources.securitySettings") }),
      });
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: t("errors.resources.securitySettings") }),
        detail: errorDetail(error),
      });
    },
  });

  if (settings.isLoading) {
    return (
      <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
        <PanelSkeleton panels={3} height={148} />
      </div>
    );
  }
  if (settings.isError) {
    return (
      <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
        <LoadError
          error={settings.error}
          resource={t("errors.resources.securitySettings")}
          onRetry={() => void settings.refetch()}
        />
      </div>
    );
  }
  if (!form) return null;

  const set = (patch: Partial<FormState>) => {
    setForm((f) => (f ? { ...f, ...patch } : f));
  };
  const disabledAuth = !form.authEnabled;
  const secretConfigured = settings.data?.dashboard_secret_configured ?? false;

  return (
    <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
      <section className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div className="flex items-start gap-4">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span id="security-auth-label" className="text-sm font-medium">
                {t("pages.security.dashboardAuth")}
              </span>
              <Badge tone="info">BETA</Badge>
            </div>
            <p className="mt-1 text-sm text-muted-foreground">
              {t("pages.security.dashboardAuthHint")}
            </p>
          </div>
          <Switch
            checked={form.authEnabled}
            aria-labelledby="security-auth-label"
            onCheckedChange={(v) => set({ authEnabled: v })}
          />
        </div>
        {/* a disabled fieldset rather than a dimmed div: the inputs inside
            already carry `disabled`, and fading a live div drags its labels and
            hints below 4.5:1 while telling assistive tech nothing (#1181) */}
        <fieldset
          className="flex min-w-0 flex-col gap-1.5"
          disabled={disabledAuth}
          style={{ opacity: disabledAuth ? 0.55 : 1 }}
        >
          <label htmlFor="security-credential-ref" className="text-xs font-medium text-[color:var(--text-secondary)]">
            {t("pages.security.credentialRef")}
          </label>
          <Input
            id="security-credential-ref"
            value={form.credentialRef}
            disabled={disabledAuth}
            placeholder="vault://secrets/rolter-dashboard"
            onChange={(e) => set({ credentialRef: e.target.value })}
          />
        </fieldset>
        <fieldset
          className="flex min-w-0 flex-col gap-1.5"
          disabled={disabledAuth}
          style={{ opacity: disabledAuth ? 0.55 : 1 }}
        >
          <label htmlFor="security-managed-secret" className="text-xs font-medium text-[color:var(--text-secondary)]">
            {t("pages.security.managedSecret")}
          </label>
          <Input
            id="security-managed-secret"
            type="password"
            value={form.managedSecret}
            disabled={disabledAuth}
            placeholder={
              secretConfigured
                ? t("pages.security.secretConfigured")
                : t("pages.security.secretPlaceholder")
            }
            onChange={(e) => set({ managedSecret: e.target.value })}
          />
          <span className="text-[0.6875rem] text-[color:var(--text-subtle)]">
            {t("pages.security.secretHint")}
          </span>
        </fieldset>
      </section>

      <ToggleCard
        title={t("pages.security.enforceVk")}
        desc={t("pages.security.enforceVkHint")}
        checked={form.enforceVk}
        onChange={(v) => set({ enforceVk: v })}
      />
      <TextCard
        title={t("pages.security.allowedOrigins")}
        desc={t("pages.security.allowedOriginsHint")}
        value={form.allowedOrigins}
        placeholder={t("pages.security.allowedOriginsPlaceholder")}
        onChange={(v) => set({ allowedOrigins: v })}
      />
      <TextCard
        title={t("pages.security.allowedHeaders")}
        desc={t("pages.security.allowedHeadersHint")}
        value={form.allowedHeaders}
        placeholder={t("pages.security.allowedHeadersPlaceholder")}
        onChange={(v) => set({ allowedHeaders: v })}
      />
      <TextCard
        title={t("pages.security.requiredHeaders")}
        desc={t("pages.security.requiredHeadersHint")}
        value={form.requiredHeaders}
        placeholder={t("pages.security.requiredHeadersPlaceholder")}
        onChange={(v) => set({ requiredHeaders: v })}
      />
      <TextCard
        title={t("pages.security.bypassRoutes")}
        desc={t("pages.security.bypassRoutesHint")}
        value={form.bypassRoutes}
        placeholder={t("pages.security.bypassRoutesPlaceholder")}
        onChange={(v) => set({ bypassRoutes: v })}
      />

      <div className="sticky bottom-0 flex items-center justify-end gap-3 border-t border-[color:var(--border-subtle)] bg-background py-3">
        <Button disabled={save.isPending} onClick={() => save.mutate(form)}>
          {save.isPending ? t("common.saving") : t("common.saveChanges")}
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
      <Switch checked={checked} onCheckedChange={onChange} aria-label={title} />
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

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(SecurityScreen, "errors.resources.securitySettings");
