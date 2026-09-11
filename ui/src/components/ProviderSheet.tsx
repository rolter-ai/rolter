import { useMutation, useQuery } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { AlertTriangle, CheckCircle2, Loader2, XCircle } from "lucide-react";

import { CopyButton } from "@/components/CopyButton";
import { Button } from "@/components/ui/button";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Sheet, SheetBody, SheetFooter, SheetHeader } from "@/components/ui/sheet";
import { errorDetail, useToast } from "@/lib/toast";
import { useFormTelemetry } from "@/lib/ux-react";
import {
  apiBaseDoublesV1,
  createProvider,
  fetchProviderKinds,
  PROVIDER_KINDS,
  resolveUpstreamUrl,
  testProvider,
  updateProvider,
  type ProviderRow,
  type ProviderTestResult,
} from "@/lib/api";

export type ProviderSheetMode = "add" | "edit";

/**
 * The result of a connection probe.
 *
 * Always names the URL that was tried. "It failed" is not actionable when the
 * operator cannot see how their `api_base` was turned into an endpoint — a
 * doubled `/v1` is the single most common cause and is invisible otherwise.
 */
/**
 * Which of the probe's three outcomes a result is.
 *
 * The API reports "answered, but not a model list" as `reachable: false` with a
 * 2xx status (#980). Collapsing that into the same red as a refused connection
 * is accurate and useless: the host is up and it is the URL or the service
 * behind it that is wrong, which is a different next action from "could not
 * connect" or "the upstream refused us" (#1034).
 */
function probeOutcome(result: ProviderTestResult): "ok" | "answered" | "failed" {
  if (result.reachable) return "ok";
  const status = result.status ?? 0;
  return status >= 200 && status < 300 ? "answered" : "failed";
}

const OUTCOME_STYLES: Record<ReturnType<typeof probeOutcome>, string> = {
  ok: "border-[color:var(--status-success)]/40 bg-[color:var(--green-tint)]",
  answered: "border-[color:var(--status-warning)]/40 bg-[color:var(--status-warning)]/10",
  failed: "border-[color:var(--status-danger)]/40 bg-destructive/10",
};

function TestOutcome({ result }: { result: ProviderTestResult }) {
  const { t } = useTranslation();
  const outcome = probeOutcome(result);
  return (
    <div
      role="status"
      className={`mx-[22px] mt-2.5 rounded-md border px-3 py-2 text-xs ${OUTCOME_STYLES[outcome]}`}
    >
      <div className="flex items-center gap-1.5 font-medium">
        {outcome === "ok" && (
          <CheckCircle2 className="size-3.5 text-[color:var(--status-success-text)]" />
        )}
        {outcome === "answered" && (
          <AlertTriangle className="size-3.5 text-[color:var(--status-warning-text)]" />
        )}
        {outcome === "failed" && (
          <XCircle className="size-3.5 text-[color:var(--status-danger-text)]" />
        )}
        <span>
          {outcome === "ok"
            ? t("providerSheet.testOk", {
                count: result.models_found ?? 0,
                ms: result.latency_ms,
              })
            : outcome === "answered"
              ? t("providerSheet.testAnswered")
              : t("providerSheet.testFailed")}
        </span>
      </div>
      {result.error && <p className="mt-1 text-muted-foreground">{result.error}</p>}
      {result.probed_url && (
        <p className="mt-1 font-mono text-[11px] text-muted-foreground">
          {result.probed_url}
        </p>
      )}
    </div>
  );
}

interface ProviderDraft {
  name: string;
  slug: string;
  kind: string;
  apiBase: string;
  apiKey: string;
  apiKeyEnv: string;
  egressProxy: string;
}

function blankDraft(): ProviderDraft {
  return {
    name: "",
    slug: "",
    kind: PROVIDER_KINDS[0],
    apiBase: "",
    apiKey: "",
    apiKeyEnv: "",
    egressProxy: "",
  };
}

function fromProvider(p: ProviderRow): ProviderDraft {
  return {
    name: p.name,
    slug: p.slug,
    kind: p.kind,
    apiBase: p.api_base,
    apiKey: "",
    apiKeyEnv: p.api_key_env ?? "",
    egressProxy: p.egress_proxy ?? "",
  };
}

export interface ProviderSheetProps {
  open: boolean;
  mode: ProviderSheetMode;
  onOpenChange: (open: boolean) => void;
  orgId: string | null;
  provider?: ProviderRow | null;
  onDone: (created?: ProviderRow) => void;
}

export function ProviderSheet({
  open,
  mode,
  onOpenChange,
  orgId,
  provider,
  onDone,
}: ProviderSheetProps) {
  const [draft, setDraft] = React.useState<ProviderDraft>(() => blankDraft());
  const initialRef = React.useRef("");

  const seededRef = React.useRef(false);
  React.useEffect(() => {
    if (!open) {
      seededRef.current = false;
      return;
    }
    if (seededRef.current) return;
    seededRef.current = true;
    const d = mode === "edit" && provider ? fromProvider(provider) : blankDraft();
    setDraft(d);
    initialRef.current = JSON.stringify(d);
  }, [open, mode, provider]);

  const set = (patch: Partial<ProviderDraft>) => setDraft((d) => ({ ...d, ...patch }));

  // whether /v1 belongs in api_base depends on the kind, so the hint, the
  // placeholder and the preview all follow the selected one (#947). deployment
  // metadata, so it never goes stale within a session
  const kinds = useQuery({
    queryKey: ["provider-kinds"],
    queryFn: fetchProviderKinds,
    staleTime: Infinity,
    retry: false,
  });
  // the picker is built from the deployment's own list, not from the bundled
  // constant: a kind the backend gained and the constant had not (#1178 found
  // `gemini_interactions`) was otherwise unselectable. PROVIDER_KINDS stays as
  // the fallback for a control plane that cannot answer, and the current draft
  // kind is always offered so editing a provider never silently rewrites it
  const kindOptions = React.useMemo(() => {
    const known = kinds.data?.length
      ? kinds.data.map((k) => k.kind)
      : [...PROVIDER_KINDS];
    return known.includes(draft.kind) || !draft.kind
      ? known
      : [draft.kind, ...known];
  }, [kinds.data, draft.kind]);
  // default to the openai-shaped rule: it is the default kind, and it is the
  // one the old static ".../v1" placeholder got wrong
  const baseIncludesV1 =
    kinds.data?.find((k) => k.kind === draft.kind)?.base_includes_v1 ?? false;
  const resolvedUrl = resolveUpstreamUrl(draft.apiBase, baseIncludesV1);
  const baseDoublesV1 = apiBaseDoublesV1(draft.apiBase, baseIncludesV1);

  const dirty = initialRef.current !== "" && JSON.stringify(draft) !== initialRef.current;
  const { t } = useTranslation();
  const guard = React.useCallback(() => {
    if (!dirty) return true;
    return window.confirm(t("common.discardChanges"));
  }, [dirty, t]);

  // edit mode uses the backend's tri-state semantics: omit a field to leave it
  // unchanged, send "" to clear it, send a value to set/rotate it. api_key is
  // left out entirely unless the operator typed a new one — never pre-filled,
  // so an empty submit must not clear a credential that's just not being rotated
  // form lifecycle for the UX stream (#805). the target names the form and the
  // mode, never anything the operator typed into it — this sheet holds provider
  // credentials, so the distinction is not academic
  const ux = useFormTelemetry(mode === "add" ? "provider-create" : "provider-edit", open);

  // probes the *stored* row, so it answers "does what I saved work", not "does
  // what I have typed work". that is the honest question — the credential is
  // sealed and never leaves the server, so the form could not test a draft key
  // without shipping it somewhere first
  const toast = useToast();

  const test = useMutation({ mutationFn: () => testProvider(provider!.id) });

  const save = useMutation({
    mutationFn: () => {
      if (mode === "add") {
        return createProvider(orgId as string, {
          name: draft.name,
          slug: draft.slug.trim() || undefined,
          kind: draft.kind,
          api_base: draft.apiBase,
          api_key: draft.apiKey || undefined,
          api_key_env: draft.apiKeyEnv || undefined,
          egress_proxy: draft.egressProxy || undefined,
        });
      }
      const p = provider!;
      return updateProvider(p.id, {
        kind: draft.kind !== p.kind ? draft.kind : undefined,
        api_base: draft.apiBase !== p.api_base ? draft.apiBase : undefined,
        api_key: draft.apiKey ? draft.apiKey : undefined,
        api_key_env: draft.apiKeyEnv !== (p.api_key_env ?? "") ? draft.apiKeyEnv : undefined,
        egress_proxy:
          draft.egressProxy !== (p.egress_proxy ?? "") ? draft.egressProxy : undefined,
      });
    },
    onSuccess: (created) => {
      ux.saved();
      // the sheet closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push(
        mode === "add"
          ? { tone: "success", title: t("toast.created", { what: created.name }) }
          : {
              tone: "success",
              title: t("toast.saved"),
              detail: t("toast.savedDetail", { what: created.name }),
            },
      );
      onDone(created);
      onOpenChange(false);
    },
    onError: (error) => {
      ux.failed();
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: draft.name }),
        detail: errorDetail(error),
      });
    },
  });

  const title = mode === "add" ? "Add provider" : `Edit ${provider?.name ?? ""}`;
  const subtitle =
    mode === "add"
      ? "an upstream provider used as a route target"
      : `${draft.slug || "—"} · ${draft.kind}`;
  const cta = mode === "add" ? "Create provider" : "Save provider";
  const canSave =
    !!draft.name.trim() && !!draft.apiBase.trim() && !save.isPending &&
    (mode === "add" ? !!orgId : true);

  return (
    <Sheet open={open} onOpenChange={onOpenChange} onDismiss={guard}>
      <SheetHeader
        title={title}
        subtitle={subtitle}
        onClose={() => guard() && onOpenChange(false)}
      />
      <SheetBody>
        <p className="text-xs leading-snug text-muted-foreground">
          {mode === "add"
            ? t("providerSheet.fields.add")
            : t("providerSheet.fields.edit")}
        </p>

        <Field label={t("providerSheet.fields.name")}>
          <Input
            value={draft.name}
            onChange={(e) => set({ name: e.target.value })}
            placeholder="openai-primary"
            disabled={mode === "edit"}
          />
        </Field>

        {mode === "add" ? (
          <Field
            label={t("providerSheet.fields.slugOptional")}
            hint={t("providerSheet.fields.slugOptionalHint")}
          >
            <Input
              value={draft.slug}
              onChange={(e) => set({ slug: e.target.value })}
              placeholder="openai-primary"
              className="font-mono"
            />
          </Field>
        ) : (
          <Field
            label={t("providerSheet.fields.slug")}
            hint={t("providerSheet.fields.slugHint")}
            // the child here is a row, not the control, so Field cannot find
            // the input to hang the id on — say which one the label means
            htmlFor="provider-slug"
          >
            <div className="flex items-center gap-2">
              <Input id="provider-slug" value={draft.slug} readOnly disabled className="font-mono" />
              {provider && <CopyButton value={`${provider.slug}/`} label={t("providerSheet.fields.copyPrefix")} />}
            </div>
          </Field>
        )}

        <Field label={t("providerSheet.fields.kind")}>
          <Select value={draft.kind} onChange={(e) => set({ kind: e.target.value })}>
            {kindOptions.map((k) => (
              <option key={k} value={k}>
                {k}
              </option>
            ))}
          </Select>
        </Field>

        {/* three children — the input, the resolved url and the doubled-/v1
            warning — so the id is written out rather than left to the field's
            fallback (#1264) */}
        <Field
          label={t("providerSheet.fields.apiBase")}
          htmlFor="provider-api-base"
          hint={t(
            baseIncludesV1
              ? "providerSheet.apiBase.includesV1"
              : "providerSheet.apiBase.excludesV1",
            { kind: draft.kind },
          )}
        >
          <Input
            id="provider-api-base"
            value={draft.apiBase}
            onChange={(e) => set({ apiBase: e.target.value })}
            placeholder={
              baseIncludesV1 ? "https://api.example.com/v1" : "https://api.example.com"
            }
          />
          {resolvedUrl && (
            <p
              className={
                baseDoublesV1
                  ? "mt-1.5 text-xs text-[color:var(--status-danger-text)]"
                  : "mt-1.5 text-xs text-muted-foreground"
              }
            >
              {t("providerSheet.apiBase.resolvesTo")}{" "}
              <span className="font-mono break-all">{resolvedUrl}</span>
            </p>
          )}
          {baseDoublesV1 && (
            <p className="mt-1 text-xs text-[color:var(--status-danger-text)]">
              {t("providerSheet.apiBase.doubled")}
            </p>
          )}
        </Field>

        <Field
          label={t("providerSheet.fields.providerKey")}
          hint={
            mode === "add"
              ? t("providerSheet.fields.providerKeyHintAdd")
              : t("providerSheet.fields.providerKeyHintEdit")
          }
        >
          <Input
            type="password"
            value={draft.apiKey}
            onChange={(e) => set({ apiKey: e.target.value })}
            autoComplete="off"
            placeholder={mode === "edit" ? "unchanged" : undefined}
          />
        </Field>

        <Field
          label={t("providerSheet.fields.providerKeyEnv")}
          hint={t("providerSheet.fields.providerKeyEnvHint")}
        >
          <Input
            value={draft.apiKeyEnv}
            onChange={(e) => set({ apiKeyEnv: e.target.value })}
            placeholder="OPENAI_API_KEY"
          />
        </Field>

        <Field label={t("providerSheet.fields.egressProxy")}>
          <Input
            value={draft.egressProxy}
            onChange={(e) => set({ egressProxy: e.target.value })}
            placeholder="http://proxy.internal:8080"
          />
        </Field>
      </SheetBody>

      <SheetFooter>
        {save.isError && (
          <p className="px-[22px] pt-2.5 text-xs text-[color:var(--status-danger-text)]">
            {(save.error as Error).message}
          </p>
        )}
        {test.data && <TestOutcome result={test.data} />}
        {test.isError && (
          <p className="px-[22px] pt-2.5 text-xs text-[color:var(--status-danger-text)]">
            {(test.error as Error).message}
          </p>
        )}
        <div className="flex items-center justify-end gap-2.5 px-[22px] py-3.5">
          {/* only for a saved provider: the probe reads the stored row, so it
              cannot speak for edits still sitting in the form */}
          {mode === "edit" && provider && (
            <Button
              variant="outline"
              className="mr-auto"
              disabled={test.isPending}
              onClick={() => test.mutate()}
            >
              {test.isPending ? (
                <>
                  <Loader2 className="size-4 animate-spin" />
                  {t("providerSheet.testing")}
                </>
              ) : (
                t("providerSheet.testConnection")
              )}
            </Button>
          )}
          <Button variant="ghost" onClick={() => guard() && onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button
            disabled={!canSave}
            onClick={() => {
              ux.submitted();
              save.mutate();
            }}
          >
            {cta}
          </Button>
        </div>
      </SheetFooter>
    </Sheet>
  );
}
