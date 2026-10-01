import { useMutation, useQuery } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { AlertTriangle, CheckCircle2, Loader2, XCircle } from "lucide-react";

import { CopyButton } from "@/components/CopyButton";
import { useDiscardGuard } from "@/components/DiscardGuard";
import { DocsLink } from "@/components/DocsLink";
import { ProjectScopeField, useMayWiden } from "@/components/ProjectScopeField";
import { Button } from "@/components/ui/button";
import { Combobox } from "@/components/ui/combobox";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Sheet,
  SheetActions,
  SheetBody,
  SheetError,
  SheetFooter,
  SheetHeader,
} from "@/components/ui/sheet";
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
        <p className="mt-1 font-mono text-[11px] text-muted-foreground">{result.probed_url}</p>
      )}
    </div>
  );
}

/**
 * What a create leaves on screen (#2142).
 *
 * The sheet stays open on the provider it just made, since the next step is to
 * check it before a route depends on it, and that check is a button in this
 * footer. It does not run by itself: the test is a call to the upstream.
 */
function CreatedNote({ name }: { name: string }) {
  const { t } = useTranslation();
  return (
    <div
      role="status"
      className={`mx-[22px] mt-2.5 rounded-md border px-3 py-2 text-xs ${OUTCOME_STYLES.ok}`}
    >
      <div className="flex items-center gap-1.5 font-medium">
        <CheckCircle2 className="size-3.5 text-[color:var(--status-success-text)]" />
        <span className="min-w-0 break-words">{t("providerSheet.created", { name })}</span>
      </div>
      <p className="mt-1 text-muted-foreground">{t("providerSheet.createdNext")}</p>
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
  /** the project the provider is scoped to; `""` is the whole organization */
  projectId: string;
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
    projectId: "",
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
    projectId: p.project_id ?? "",
  };
}

export interface ProviderSheetProps {
  open: boolean;
  mode: ProviderSheetMode;
  onOpenChange: (open: boolean) => void;
  orgId: string | null;
  provider?: ProviderRow | null;
  /**
   * The project the dashboard is open on. Someone who may not make a provider
   * org-wide has to name a project, and this is the one they start on (#1919).
   */
  defaultProjectId?: string | null;
  onDone: (created?: ProviderRow) => void;
}

export function ProviderSheet({
  open,
  mode,
  onOpenChange,
  orgId,
  provider,
  defaultProjectId,
  onDone,
}: ProviderSheetProps) {
  const [draft, setDraft] = React.useState<ProviderDraft>(() => blankDraft());
  const initialRef = React.useRef("");
  // the row a create just made. while it is set the sheet is an edit sheet for
  // that row, open on the test instead of closing over it (#2142)
  const [created, setCreated] = React.useState<ProviderRow | null>(null);
  const editing = mode === "edit" || created !== null;
  // the row the control plane holds, which is what the connection test probes
  const stored = created ?? (mode === "edit" ? (provider ?? null) : null);

  const seededRef = React.useRef(false);
  // a caller who may not make a provider org-wide can only scope it to a
  // project, so the picker has no org option for them and their current
  // project stands in until they pick another
  const mayWiden = useMayWiden("provider", editing ? "edit" : "add");
  const scopeValue = draft.projectId || (!stored && !mayWiden ? (defaultProjectId ?? "") : "");
  // the server keeps `api_key_env` at org level, since it reads the control
  // plane's own environment, so a scoped provider's is an org admin's to set
  const envLocked = !mayWiden && scopeValue !== "";

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
    const known = kinds.data?.length ? kinds.data.map((k) => k.kind) : [...PROVIDER_KINDS];
    return known.includes(draft.kind) || !draft.kind ? known : [draft.kind, ...known];
  }, [kinds.data, draft.kind]);
  // default to the openai-shaped rule: it is the default kind, and it is the
  // one the old static ".../v1" placeholder got wrong
  const baseIncludesV1 = kinds.data?.find((k) => k.kind === draft.kind)?.base_includes_v1 ?? false;
  const resolvedUrl = resolveUpstreamUrl(draft.apiBase, baseIncludesV1);
  const baseDoublesV1 = apiBaseDoublesV1(draft.apiBase, baseIncludesV1);

  const dirty = initialRef.current !== "" && JSON.stringify(draft) !== initialRef.current;
  const { t } = useTranslation();

  // edit mode uses the backend's tri-state semantics: omit a field to leave it
  // unchanged, send "" to clear it, send a value to set/rotate it. api_key is
  // left out entirely unless the operator typed a new one — never pre-filled,
  // so an empty submit must not clear a credential that's just not being rotated
  // form lifecycle for the UX stream (#805). the target names the form and the
  // mode, never anything the operator typed into it — this sheet holds provider
  // credentials, so the distinction is not academic
  const ux = useFormTelemetry(editing ? "provider-edit" : "provider-create", open, {
    dirty,
  });

  // probes the *stored* row, so it answers "does what I saved work", not "does
  // what I have typed work". that is the honest question — the credential is
  // sealed and never leaves the server, so the form could not test a draft key
  // without shipping it somewhere first
  const toast = useToast();

  const test = useMutation({ mutationFn: () => testProvider(stored!.id) });

  const save = useMutation({
    mutationFn: () => {
      if (!stored) {
        return createProvider(orgId as string, {
          name: draft.name,
          slug: draft.slug.trim() || undefined,
          kind: draft.kind,
          api_base: draft.apiBase,
          api_key: draft.apiKey || undefined,
          api_key_env: draft.apiKeyEnv || undefined,
          egress_proxy: draft.egressProxy || undefined,
          project_id: scopeValue || undefined,
        });
      }
      const p = stored;
      return updateProvider(p.id, {
        kind: draft.kind !== p.kind ? draft.kind : undefined,
        api_base: draft.apiBase !== p.api_base ? draft.apiBase : undefined,
        api_key: draft.apiKey ? draft.apiKey : undefined,
        api_key_env: draft.apiKeyEnv !== (p.api_key_env ?? "") ? draft.apiKeyEnv : undefined,
        egress_proxy: draft.egressProxy !== (p.egress_proxy ?? "") ? draft.egressProxy : undefined,
        // sent only when it moved: `null` is the word for org-wide again, and a
        // project admin's unchanged edit must not carry a scope the server refuses
        project_id: scopeValue !== (p.project_id ?? "") ? scopeValue || null : undefined,
      });
    },
    onSuccess: (row) => {
      ux.saved();
      if (!stored) {
        // a create keeps the sheet open on the new row, so the outcome is
        // announced by the sheet itself rather than by a toast that would
        // say it a second time. the form is re-seeded from what the control
        // plane stored, which also clears the typed key
        const seeded = fromProvider(row);
        setCreated(row);
        setDraft(seeded);
        initialRef.current = JSON.stringify(seeded);
        test.reset();
        onDone(row);
        return;
      }
      // the sheet closes on a save, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: row.name }),
      });
      onDone(row);
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

  // seeded once per opening. a result or a refusal belongs to the opening that
  // produced it, so it is cleared as the sheet closes: a probe of one provider
  // must not greet the next one opened, not even for the frame before an effect
  // on opening would have cleared it
  React.useEffect(() => {
    if (!open) {
      seededRef.current = false;
      setCreated(null);
      test.reset();
      save.reset();
      return;
    }
    if (seededRef.current) return;
    seededRef.current = true;
    const d = mode === "edit" && provider ? fromProvider(provider) : blankDraft();
    setDraft(d);
    initialRef.current = JSON.stringify(d);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, mode, provider]);

  // the next step after a create is the test, so focus moves to it: the button
  // that was pressed is now Save, with nothing to save
  const testRef = React.useRef<HTMLButtonElement>(null);
  React.useEffect(() => {
    if (created) testRef.current?.focus();
  }, [created]);

  const testHintId = React.useId();

  const title = editing
    ? t("providerSheet.title.edit", { name: stored?.name ?? "" })
    : t("providerSheet.title.add");
  const subtitle = editing
    ? `${draft.slug || "—"} · ${draft.kind}`
    : t("providerSheet.subtitle.add");
  // the sheet's own dismissal paths (Escape, scrim, close, Cancel) all run
  // through the shared discard prompt (#1463)
  const { guard, close, locked, prompt } = useDiscardGuard({
    dirty,
    saving: save.isPending,
    onOpenChange,
  });

  const cta = editing ? t("providerSheet.cta.save") : t("providerSheet.cta.create");
  const canSave =
    !!draft.name.trim() &&
    !!draft.apiBase.trim() &&
    !save.isPending &&
    (editing ? true : !!orgId) &&
    // no org-wide option to fall back on: a project has to be named
    (mayWiden || scopeValue !== "") &&
    // right after a create there is nothing to save until something is edited
    (created === null || dirty);

  return (
    <Sheet open={open} onOpenChange={onOpenChange} onDismiss={guard}>
      <SheetHeader title={title} subtitle={subtitle} onClose={close} closeDisabled={locked} />
      <SheetBody>
        <p className="text-xs leading-snug text-muted-foreground">
          {editing ? t("providerSheet.fields.edit") : t("providerSheet.fields.add")}
        </p>

        <Field label={t("providerSheet.fields.name")} hint={t("providerSheet.fields.nameHint")}>
          <Input
            value={draft.name}
            onChange={(e) => set({ name: e.target.value })}
            placeholder="openai-primary"
            disabled={editing}
          />
        </Field>

        {!editing ? (
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
              <Input
                id="provider-slug"
                value={draft.slug}
                readOnly
                disabled
                className="font-mono"
              />
              {stored && (
                <CopyButton
                  value={`${stored.slug}/`}
                  label={t("providerSheet.fields.copyPrefix")}
                />
              )}
            </div>
          </Field>
        )}

        <ProjectScopeField
          resource="provider"
          mode={editing ? "edit" : "add"}
          orgId={orgId}
          id="provider-scope"
          value={scopeValue}
          onChange={(projectId) => set({ projectId })}
          mayWiden={mayWiden}
        />

        <Field label={t("providerSheet.fields.kind")}>
          <Combobox
            value={draft.kind}
            onChange={(kind) => set({ kind })}
            options={kindOptions.map((k) => ({ value: k, label: k }))}
          />
        </Field>

        {/* two children — the input and the resolved url — so the id is
            written out rather than left to the field's fallback (#1264). the
            doubled-/v1 warning is the field's error rather than a loose <p>, so
            it lands in the input's description and flips aria-invalid (#1544) */}
        <Field
          label={t("providerSheet.fields.apiBase")}
          htmlFor="provider-api-base"
          hint={t(
            baseIncludesV1
              ? "providerSheet.apiBase.includesV1"
              : "providerSheet.apiBase.excludesV1",
            { kind: draft.kind },
          )}
          error={baseDoublesV1 ? t("providerSheet.apiBase.doubled") : undefined}
        >
          <Input
            id="provider-api-base"
            value={draft.apiBase}
            onChange={(e) => set({ apiBase: e.target.value })}
            placeholder={baseIncludesV1 ? "https://api.example.com/v1" : "https://api.example.com"}
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
        </Field>

        <Field
          label={t("providerSheet.fields.providerKey")}
          hint={
            <>
              {editing
                ? t("providerSheet.fields.providerKeyHintEdit")
                : t("providerSheet.fields.providerKeyHintAdd")}{" "}
              {/* the hint stands alone; the link only adds depth, and is absent
                  on a deployment that configured no documentation host (#1164) */}
              <DocsLink page="whichKey" label={t("docs.link.whichKey")} />
            </>
          }
        >
          <Input
            type="password"
            value={draft.apiKey}
            onChange={(e) => set({ apiKey: e.target.value })}
            autoComplete="off"
            placeholder={editing ? t("providerSheet.fields.apiKeyUnchanged") : undefined}
          />
        </Field>

        <Field
          label={t("providerSheet.fields.providerKeyEnv")}
          hint={
            envLocked
              ? t("providerSheet.fields.providerKeyEnvScoped")
              : t("providerSheet.fields.providerKeyEnvHint")
          }
        >
          <Input
            value={draft.apiKeyEnv}
            onChange={(e) => set({ apiKeyEnv: e.target.value })}
            placeholder="OPENAI_API_KEY"
            disabled={envLocked}
          />
        </Field>

        <Field
          label={t("providerSheet.fields.egressProxy")}
          hint={t("providerSheet.fields.egressProxyHint")}
        >
          <Input
            value={draft.egressProxy}
            onChange={(e) => set({ egressProxy: e.target.value })}
            placeholder="http://proxy.internal:8080"
          />
        </Field>
      </SheetBody>

      <SheetFooter>
        <SheetError message={save.isError ? (save.error as Error).message : undefined} />
        {created && test.isIdle && <CreatedNote name={created.name} />}
        {test.data && <TestOutcome result={test.data} />}
        <SheetError message={test.isError ? (test.error as Error).message : undefined} />
        {/* the probe reads the stored row, so with edits sitting in the form its
            answer would speak for the old values: the button is off and this
            says why, where a disabled button cannot */}
        {stored && dirty && (
          <p id={testHintId} className="px-[22px] pt-2.5 text-xs text-muted-foreground">
            {t("providerSheet.testSavedOnly")}
          </p>
        )}
        <SheetActions
          start={
            // only for a saved provider: the probe reads the stored row, so it
            // cannot speak for edits still sitting in the form
            stored && (
              <Button
                ref={testRef}
                variant="outline"
                className="mr-auto"
                disabled={test.isPending || dirty}
                aria-describedby={dirty ? testHintId : undefined}
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
            )
          }
        >
          <Button variant="ghost" disabled={locked} onClick={close}>
            {created ? t("common.done") : t("common.cancel")}
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
        </SheetActions>
      </SheetFooter>
      {prompt}
    </Sheet>
  );
}
