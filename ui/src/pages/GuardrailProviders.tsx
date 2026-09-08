import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, PlugZap, Plus } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  GuardrailEmpty,
  GuardrailLoading,
  PolicyCard,
} from "@/components/GuardrailPanel";
import { LoadError } from "@/components/LoadError";
import { superadminOnly } from "@/components/ForbiddenScreen";
import { GatedButton } from "@/components/GatedButton";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import {
  createGuardrailProvider,
  deleteGuardrailProvider,
  fetchGuardrailProviders,
  updateGuardrailProvider,
  type GuardrailProviderInput,
  type GuardrailProviderRow,
} from "@/lib/api";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const EMPTY: GuardrailProviderInput = {
  name: "",
  enabled: false,
  url: "https://guardrails.internal/v1/evaluate",
  stage: "pre_call",
  timeout_ms: 2000,
  max_retries: 0,
  failure_mode: "fail_closed",
  max_body_bytes: 65536,
  auth_kind: "none",
  auth_env: null,
};

function GuardrailProvidersScreen() {
  const { t } = useTranslation();
  const client = useQueryClient();
  const toast = useToast();
  const query = useQuery({
    queryKey: ["guardrail-providers"],
    queryFn: fetchGuardrailProviders,
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `query` is the query the user is actually waiting on for this screen
  useScreenReady(!query.isLoading);
  useErrorState(!!query.error, "guardrail-providers");
  const [editing, setEditing] = React.useState<
    GuardrailProviderRow | null | undefined
  >();
  const save = useMutation({
    mutationFn: (body: GuardrailProviderInput) =>
      editing
        ? updateGuardrailProvider(editing.id, body)
        : createGuardrailProvider(body),
    onSuccess: (_result, body) => {
      void client.invalidateQueries({ queryKey: ["guardrail-providers"] });
      // the dialog closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push(
        editing
          ? {
              tone: "success",
              title: t("toast.saved"),
              detail: t("toast.savedDetail", { what: body.name }),
            }
          : { tone: "success", title: t("toast.created", { what: body.name }) },
      );
      setEditing(undefined);
    },
    onError: (error, body) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: body.name }),
        detail: errorDetail(error),
      });
    },
  });
  const remove = useMutation({
    mutationFn: deleteGuardrailProvider,
    onSuccess: () =>
      void client.invalidateQueries({ queryKey: ["guardrail-providers"] }),
  });
  // was a bare window.confirm; an external enforcement point going away is
  // exactly the kind of change that deserves a styled, translated dialog (#1179)
  const [deleteTarget, setDeleteTarget] =
    React.useState<GuardrailProviderRow | null>(null);
  const startDelete = (provider: GuardrailProviderRow) => {
    remove.reset();
    setDeleteTarget(provider);
  };

  const providers = query.data ?? [];
  const active = providers.find((provider) => provider.enabled);

  return (
    <div className="mx-auto flex max-w-[1120px] flex-col gap-5 p-[22px]">
      <div className="flex flex-col gap-3 border-b border-[color:var(--border-subtle)] pb-5 sm:flex-row sm:items-end sm:justify-between">
        <div>
          <p className="font-mono text-[0.6875rem] uppercase tracking-[0.16em] text-[color:var(--status-danger-text)]">
            External enforcement
          </p>
          <h1 className="mt-1 text-xl font-semibold tracking-tight">
            Guardrail provider registry
          </h1>
          <p className="mt-1 max-w-2xl text-sm text-muted-foreground">
            Register services freely; activate one at a time. Enabling a
            provider atomically hands enforcement over from the current service.
          </p>
        </div>
        <GatedButton gate="guardrail_provider:create" onClick={() => setEditing(null)}>
          <Plus className="h-4 w-4" aria-hidden /> Add provider
        </GatedButton>
      </div>

      {active && (
        <section className="flex items-center gap-3 rounded-[10px] border border-[color:var(--status-success)]/30 bg-[color:var(--status-success)]/5 p-4">
          <PlugZap
            className="h-5 w-5 text-[color:var(--status-success-text)]"
            aria-hidden
          />
          <div>
            <p className="text-sm font-medium">
              {active.name} owns external enforcement
            </p>
            <p className="text-xs text-muted-foreground">
              {active.failure_mode === "fail_closed"
                ? "Requests stop when the service cannot decide."
                : "Requests continue when the service cannot decide."}
            </p>
          </div>
        </section>
      )}

      {query.isLoading ? (
        <GuardrailLoading />
      ) : query.isError ? (
        // never hand-rolled: a 403 is what a non-superadmin gets on this
        // deployment-scoped screen, and the bespoke panel offered it a retry
        // that could not ever work (#1259)
        <LoadError
          error={query.error}
          resource={t("errors.resources.guardrailProviders")}
          onRetry={() => void query.refetch()}
        />
      ) : providers.length === 0 ? (
        <GuardrailEmpty
          title="No guardrail providers"
          description="Register a self-hosted HTTP guardrail service. Credentials remain environment references, never stored secret values."
          action={
            <GatedButton gate="guardrail_provider:create" onClick={() => setEditing(null)}>
              Register first provider
            </GatedButton>
          }
        />
      ) : (
        <div className="grid gap-3 md:grid-cols-2">
          {providers.map((provider) => (
            <PolicyCard
              key={provider.id}
              title={provider.name}
              description={provider.url}
              enabled={provider.enabled}
              badges={
                <>
                  <Badge
                    tone={
                      provider.failure_mode === "fail_closed"
                        ? "danger"
                        : "warning"
                    }
                  >
                    {provider.failure_mode.replace("_", "-")}
                  </Badge>
                  <Badge tone="outline">
                    {provider.stage.replace("_", "-")}
                  </Badge>
                  <Badge tone="info">
                    {provider.auth_kind.replace("_", " ")}
                  </Badge>
                </>
              }
              details={`${provider.timeout_ms} ms timeout · ${provider.max_retries} retries · ${Math.round(provider.max_body_bytes / 1024)} KiB cap`}
              actions={
                <>
                  <GatedButton
                    gate="guardrail_provider:delete"
                    variant="ghost"
                    aria-label={t("pages.guardrailProviders.deleteAria", {
                      name: provider.name,
                    })}
                    onClick={() => startDelete(provider)}
                    disabled={
                      remove.isPending && remove.variables === provider.id
                    }
                  >
                    {remove.isPending && remove.variables === provider.id && (
                      <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                    )}
                    Delete
                  </GatedButton>
                  <GatedButton
                    gate="guardrail_provider:update"
                    variant="outline"
                    aria-label={t("pages.guardrailProviders.editAria", {
                      name: provider.name,
                    })}
                    onClick={() => setEditing(provider)}
                  >
                    Edit provider
                  </GatedButton>
                </>
              }
            />
          ))}
        </div>
      )}

      <ConfirmDialog
        open={!!deleteTarget}
        onOpenChange={(o) => !o && setDeleteTarget(null)}
        title={t("pages.guardrailProviders.confirm.title", {
          name: deleteTarget?.name,
        })}
        description={t("pages.guardrailProviders.confirm.body")}
        confirmLabel={t("common.delete")}
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

      <ProviderDialog
        key={editing?.id ?? (editing === null ? "new" : "closed")}
        open={editing !== undefined}
        initial={editing ?? null}
        pending={save.isPending}
        error={save.isError ? (save.error as Error).message : null}
        onClose={() => setEditing(undefined)}
        onSave={(body) => save.mutate(body)}
      />
    </div>
  );
}

function ProviderDialog({
  open,
  initial,
  pending,
  error,
  onClose,
  onSave,
}: {
  open: boolean;
  initial: GuardrailProviderRow | null;
  pending: boolean;
  error: string | null;
  onClose: () => void;
  onSave: (body: GuardrailProviderInput) => void;
}) {
  const [form, setForm] = React.useState<GuardrailProviderInput>(
    initial ?? EMPTY,
  );
  const set = (patch: Partial<GuardrailProviderInput>) =>
    setForm((value) => ({ ...value, ...patch }));
  const valid =
    form.name.trim() !== "" &&
    /^https?:\/\//.test(form.url) &&
    form.timeout_ms > 0 &&
    form.max_body_bytes > 0 &&
    (form.auth_kind === "none" || Boolean(form.auth_env?.trim()));
  return (
    <Dialog open={open} onOpenChange={(value) => !value && onClose()}>
      <DialogHeader>
        <DialogTitle>
          {initial ? "Edit guardrail provider" : "Register guardrail provider"}
        </DialogTitle>
        <DialogDescription>
          Use an HTTP endpoint and reference credentials by environment
          variable. Activating this provider pauses the current one.
        </DialogDescription>
      </DialogHeader>
      <div className="max-h-[65vh] space-y-4 overflow-y-auto pr-1">
        <Field label="Provider name" htmlFor="provider-name">
          <Input
            id="provider-name"
            value={form.name}
            onChange={(event) => set({ name: event.target.value })}
          />
        </Field>
        <Field
          label="Evaluation URL"
          htmlFor="provider-url"
          hint="The gateway posts a vendor-neutral decision envelope."
        >
          <Input
            id="provider-url"
            type="url"
            value={form.url}
            onChange={(event) => set({ url: event.target.value })}
          />
        </Field>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <Field label="Stage" htmlFor="provider-stage">
            <Select
              id="provider-stage"
              value={form.stage}
              onChange={(event) =>
                set({
                  stage: event.target.value as GuardrailProviderInput["stage"],
                })
              }
            >
              <option value="pre_call">Before upstream</option>
              <option value="post_call">Before response</option>
            </Select>
          </Field>
          <Field label="Failure policy" htmlFor="provider-failure">
            <Select
              id="provider-failure"
              value={form.failure_mode}
              onChange={(event) =>
                set({
                  failure_mode: event.target
                    .value as GuardrailProviderInput["failure_mode"],
                })
              }
            >
              <option value="fail_closed">Fail closed</option>
              <option value="fail_open">Fail open</option>
            </Select>
          </Field>
        </div>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
          <Field label="Timeout (ms)" htmlFor="provider-timeout">
            <Input
              id="provider-timeout"
              type="number"
              min={1}
              value={form.timeout_ms}
              onChange={(event) =>
                set({ timeout_ms: Number(event.target.value) })
              }
            />
          </Field>
          <Field label="Retries" htmlFor="provider-retries">
            <Input
              id="provider-retries"
              type="number"
              min={0}
              value={form.max_retries}
              onChange={(event) =>
                set({ max_retries: Number(event.target.value) })
              }
            />
          </Field>
          <Field label="Body cap" htmlFor="provider-cap">
            <Input
              id="provider-cap"
              type="number"
              min={1}
              value={form.max_body_bytes}
              onChange={(event) =>
                set({ max_body_bytes: Number(event.target.value) })
              }
            />
          </Field>
        </div>
        <Field label="Authentication" htmlFor="provider-auth">
          <Select
            id="provider-auth"
            value={form.auth_kind}
            onChange={(event) => {
              const auth_kind = event.target
                .value as GuardrailProviderInput["auth_kind"];
              set({
                auth_kind,
                auth_env: auth_kind === "none" ? null : form.auth_env,
              });
            }}
          >
            <option value="none">None</option>
            <option value="bearer">Bearer token</option>
            <option value="shared_secret">Shared secret header</option>
          </Select>
        </Field>
        {form.auth_kind !== "none" && (
          <Field
            label="Secret environment variable"
            htmlFor="provider-env"
            hint="Only the variable name is stored."
          >
            <Input
              id="provider-env"
              className="font-mono"
              placeholder="ROLTER_GUARDRAIL_TOKEN"
              value={form.auth_env ?? ""}
              onChange={(event) =>
                set({ auth_env: event.target.value || null })
              }
            />
          </Field>
        )}
        <div className="flex items-start justify-between gap-4 rounded-lg border border-[color:var(--border-subtle)] p-3">
          <div>
            <p className="text-sm font-medium">Activate provider</p>
            <p className="mt-0.5 text-xs text-muted-foreground">
              Takes enforcement ownership from the currently active provider.
            </p>
          </div>
          <Switch
            checked={form.enabled}
            aria-label="Activate provider"
            onCheckedChange={(enabled) => set({ enabled })}
          />
        </div>
        {form.failure_mode === "fail_open" && (
          <p className="rounded-lg bg-[color:var(--status-warning)]/10 p-3 text-xs text-[color:var(--status-warning-text)]">
            Fail-open favors availability: traffic continues if this provider
            times out or errors.
          </p>
        )}
        {error && (
          <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">
            {error}
          </p>
        )}
      </div>
      <DialogFooter>
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button disabled={!valid || pending} onClick={() => onSave(form)}>
          {pending ? "Publishing…" : "Save provider"}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(GuardrailProvidersScreen, "errors.resources.guardrailProviders");
