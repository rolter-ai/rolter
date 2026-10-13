import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { Loader2, PlugZap, Plus, ShieldAlert, ShieldOff, ShieldQuestion } from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  GuardrailBanner,
  GuardrailEmpty,
  GuardrailLoading,
  PolicyCard,
} from "@/components/GuardrailPanel";
import {
  GuardrailOutcomesNotice,
  OutcomeCounts,
  PROVIDER_ACTIONS,
  UnnamedProviderCounts,
  useGuardrailOutcomes,
} from "@/components/GuardrailOutcomes";
import { GuardrailProviderTrial } from "@/components/GuardrailProviderTrial";
import { LoadError } from "@/components/LoadError";
import { superadminOnly } from "@/components/ForbiddenScreen";
import { GatedButton } from "@/components/GatedButton";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Combobox } from "@/components/ui/combobox";
import { Field } from "@/components/ui/field";
import { FormSection } from "@/components/ui/form-section";
import { Input } from "@/components/ui/input";

import { SwitchRow } from "@/components/ui/switch-row";
import {
  createGuardrailProvider,
  deleteGuardrailProvider,
  fetchConfig,
  fetchEffectiveGuardrails,
  fetchGuardrailProviders,
  updateGuardrailProvider,
  type GuardrailProviderInput,
  type GuardrailProviderRow,
} from "@/lib/api";
import {
  providerStatus,
  readEffectiveWebhook,
  readWebhookSource,
  resolveEnforcement,
  type Enforcement,
  type ProviderStatus,
} from "@/lib/guardrail-enforcement";
import { errorText } from "@/lib/error-copy";
import { useFormat } from "@/lib/i18n/format";
import { useToast } from "@/lib/toast";
import { useScreenReady } from "@/lib/ux-react";

// the shape of an evaluation URL, shown as a placeholder and never as a value:
// a prefilled host saves as the endpoint when nobody edits it, and a
// fail-closed provider pointed at a host that does not exist refuses every
// request (#2163)
const URL_EXAMPLE = "https://guardrails.internal/v1/evaluate";
const HTTP_URL = /^https?:\/\//;

const EMPTY: GuardrailProviderInput = {
  name: "",
  enabled: false,
  url: "",
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
  const { number } = useFormat();
  // the registry and the effective webhook are read together, so the banner
  // never pairs a fresh list with a stale webhook between two refetches. an
  // unreadable config is `null`: the list still renders and the banner says it
  // cannot confirm enforcement (#2162)
  const query = useQuery({
    queryKey: ["guardrail-providers"],
    queryFn: async () => {
      const [providers, webhook, source] = await Promise.all([
        fetchGuardrailProviders(),
        fetchConfig().then(readEffectiveWebhook, () => null),
        // where the webhook comes from (#2249); without it the screen compares
        // fields, which cannot tell a file copy of the enabled row from the row
        fetchEffectiveGuardrails().then(readWebhookSource, () => null),
      ]);
      return { providers, webhook, source };
    },
    retry: false,
  });
  // what the active provider did over the last week (#2158)
  const outcomes = useGuardrailOutcomes();

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `query` is the query the user is actually waiting on for this screen
  useScreenReady(!query.isLoading);
  const [editing, setEditing] = React.useState<GuardrailProviderRow | null | undefined>();
  const save = useMutation({
    mutationFn: (body: GuardrailProviderInput) =>
      editing ? updateGuardrailProvider(editing.id, body) : createGuardrailProvider(body),
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
        detail: errorText(error, t),
      });
    },
  });
  const remove = useMutation({
    mutationFn: deleteGuardrailProvider,
    onSuccess: () => void client.invalidateQueries({ queryKey: ["guardrail-providers"] }),
  });
  // was a bare window.confirm; an external enforcement point going away is
  // exactly the kind of change that deserves a styled, translated dialog (#1179)
  const [deleteTarget, setDeleteTarget] = React.useState<GuardrailProviderRow | null>(null);
  const startDelete = (provider: GuardrailProviderRow) => {
    remove.reset();
    setDeleteTarget(provider);
  };

  const providers = query.data?.providers ?? [];
  const enforcement = query.data
    ? resolveEnforcement(query.data.providers, query.data.webhook, query.data.source)
    : null;
  // `undefined` while the registry has not loaded: "none is active" would be a
  // guess, and the confirmation below must not guess
  const active = query.data ? (providers.find((provider) => provider.enabled) ?? null) : undefined;
  // an enabled config-file webhook wins over the registry, whichever row is on
  const fileInForce =
    (enforcement?.state === "enforced" || enforcement?.state === "inert") &&
    enforcement.provider === null;

  // switching a provider on hands it every request (#2163), and switching the
  // active one off leaves them to nothing (#2271), so both saves go through a
  // confirmation naming the provider and what traffic goes through afterwards.
  // a save that leaves the switch where it was is not a hand-over and saves
  // directly
  const [handingOver, setHandingOver] = React.useState<GuardrailProviderInput | null>(null);
  // not cleared as the confirmation closes: the landing is reported on the
  // closing edge, and a UX stream key that flipped to the activation's there
  // would file the pause under it
  const [pausing, setPausing] = React.useState(false);
  const requestSave = (body: GuardrailProviderInput) => {
    if (body.enabled === Boolean(editing?.enabled)) {
      save.mutate(body);
      return;
    }
    setPausing(!body.enabled);
    setHandingOver(body);
  };
  // what requests go through once the active row is paused. no other row
  // takes its place, so only an enabled config-file webhook still checks
  // them, and an unreadable effective config cannot say whether one is
  const pauseConsequence = fileInForce
    ? t("pages.guardrailProviders.confirm.pauseFileStays", { name: handingOver?.name })
    : enforcement === null || enforcement.state === "unknown"
      ? t("pages.guardrailProviders.confirm.pauseUnknown")
      : t("pages.guardrailProviders.confirm.pauseUnchecked");

  return (
    <div className="mx-auto flex max-w-[1120px] flex-col gap-5 p-[22px]">
      <div className="flex flex-col gap-3 border-b border-[color:var(--border-subtle)] pb-5 sm:flex-row sm:items-end sm:justify-between">
        <div>
          <p className="max-w-2xl text-sm text-muted-foreground">
            {t("pages.guardrailProviders.intro")}
          </p>
        </div>
        <GatedButton
          gate="guardrail_provider:create"
          control="guardrail-provider-new"
          onClick={() => setEditing(null)}
        >
          <Plus className="h-4 w-4" aria-hidden /> {t("pages.guardrailProviders.addProvider")}
        </GatedButton>
      </div>

      {/* an empty registry with nothing on in the config file is the empty
          state's to explain; every other state is worth a line here */}
      {enforcement && (enforcement.state !== "off" || providers.length > 0) && (
        <EnforcementBanner enforcement={enforcement} onRetry={() => void query.refetch()} />
      )}

      {!query.isLoading && !query.isError && providers.length > 0 && (
        <GuardrailOutcomesNotice query={outcomes} />
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
          target="guardrail-providers"
        />
      ) : providers.length === 0 ? (
        <GuardrailEmpty
          uxTarget="guardrail-providers"
          title={t("pages.guardrailProviders.emptyTitle")}
          description={t("pages.guardrailProviders.emptyBody")}
          action={
            <GatedButton
              gate="guardrail_provider:create"
              control="guardrail-provider-new-empty"
              onClick={() => setEditing(null)}
            >
              {t("pages.guardrailProviders.emptyAction")}
            </GatedButton>
          }
        />
      ) : (
        <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
          {providers.map((provider) => (
            <PolicyCard
              key={provider.id}
              title={provider.name}
              description={provider.url}
              descriptionMono
              enabled={provider.enabled}
              status={
                enforcement ? cardStatus(providerStatus(provider, enforcement), t) : undefined
              }
              badges={
                <>
                  {/* the safe default is neutral; only fail-open is worth a colour */}
                  <Badge tone={provider.failure_mode === "fail_closed" ? "neutral" : "warning"}>
                    {provider.failure_mode.replace("_", "-")}
                  </Badge>
                  <Badge tone="outline">{provider.stage.replace("_", "-")}</Badge>
                  {/* colour says a credential is attached; no auth says nothing */}
                  <Badge tone={provider.auth_kind === "none" ? "outline" : "info"}>
                    {provider.auth_kind.replace("_", " ")}
                  </Badge>
                </>
              }
              details={
                <>
                  {t("pages.guardrailProviders.detailLine", {
                    timeout: number(provider.timeout_ms),
                    count: provider.max_retries,
                    kib: number(Math.round(provider.max_body_bytes / 1024)),
                  })}
                  {/* said on a paused row too: activating it is how a working
                      provider gets replaced by one that checks nothing */}
                  {provider.stage === "post_call" && (
                    <p className="mt-1 text-[color:var(--status-warning-text)]">
                      {t("pages.guardrailProviders.postCallCardNote")}
                    </p>
                  )}
                  {/* outcomes are recorded under the provider's name, so a card
                      counts its own and a switched-away provider's history
                      stays off the new one's. a paused provider the gateway
                      never calls says nothing; rows from before the name was
                      recorded are stated once, on the provider in force */}
                  {enforcement && providerStatus(provider, enforcement) === "enforced" && (
                    <>
                      <OutcomeCounts
                        query={outcomes}
                        rule={provider.name}
                        actions={PROVIDER_ACTIONS}
                      />
                      <UnnamedProviderCounts query={outcomes} />
                    </>
                  )}
                </>
              }
              actions={
                <>
                  <GatedButton
                    gate="guardrail_provider:delete"
                    control="guardrail-provider-delete"
                    variant="ghost"
                    aria-label={t("pages.guardrailProviders.deleteAria", {
                      name: provider.name,
                    })}
                    onClick={() => startDelete(provider)}
                    disabled={remove.isPending && remove.variables === provider.id}
                  >
                    {remove.isPending && remove.variables === provider.id && (
                      <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" />
                    )}
                    {t("common.delete")}
                  </GatedButton>
                  <GatedButton
                    gate="guardrail_provider:update"
                    control="guardrail-provider-edit"
                    variant="outline"
                    aria-label={t("pages.guardrailProviders.editAria", {
                      name: provider.name,
                    })}
                    onClick={() => setEditing(provider)}
                  >
                    {t("pages.guardrailProviders.editProvider")}
                  </GatedButton>
                </>
              }
            />
          ))}
        </div>
      )}

      <ConfirmDialog
        name="guardrail-provider-delete"
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
                detail: errorText(error, t),
              });
            },
          });
        }}
      />

      <ProviderDialog
        key={editing?.id ?? (editing === null ? "new" : "closed")}
        open={editing !== undefined}
        initial={editing ?? null}
        active={active}
        pending={save.isPending}
        error={save.isError ? errorText(save.error, t) : null}
        onClose={() => setEditing(undefined)}
        onSave={requestSave}
      />

      {/* raised over the provider dialog, which stays open behind it: a
          cancel goes back to the form with every field as it was. mounted
          outside that dialog, whose key changes as it closes, so it sees the
          save land */}
      <ConfirmDialog
        name={pausing ? "guardrail-provider-pause" : "guardrail-provider-activate"}
        open={!!handingOver}
        onOpenChange={(open) => {
          if (open) return;
          setHandingOver(null);
          save.reset();
        }}
        // activating is a hand-over, not a removal: the provider it pauses can
        // be switched back on. a pause that leaves requests unchecked is as
        // red as the delete, and one under the config-file webhook changes
        // nothing for traffic
        tone={pausing && !fileInForce ? "danger" : "default"}
        title={
          pausing
            ? t("pages.guardrailProviders.confirm.pauseTitle", { name: handingOver?.name })
            : t("pages.guardrailProviders.confirm.activateTitle", { name: handingOver?.name })
        }
        description={
          handingOver &&
          (pausing ? (
            pauseConsequence
          ) : (
            <ActivationConsequence
              provider={handingOver}
              replaces={active === undefined ? undefined : (active?.name ?? null)}
              fileWins={fileInForce}
            />
          ))
        }
        confirmLabel={
          pausing
            ? t("pages.guardrailProviders.confirm.pauseConfirm")
            : t("pages.guardrailProviders.confirm.activateConfirm")
        }
        pending={save.isPending}
        error={save.error}
        onConfirm={() => {
          if (!handingOver) return;
          save.mutate(handingOver, { onSuccess: () => setHandingOver(null) });
        }}
      />
    </div>
  );
}

/**
 * What activating `provider` does, for the confirmation: the registry row it
 * pauses, then what every request goes through afterwards. `replaces` is the
 * active row's name, `null` when none is active, and `undefined` when the
 * registry did not load.
 *
 * Rendered inside the dialog's description paragraph, so each sentence is a
 * block `span` rather than a `p`.
 */
function ActivationConsequence({
  provider,
  replaces,
  fileWins,
}: {
  provider: GuardrailProviderInput;
  replaces: string | null | undefined;
  fileWins: boolean;
}) {
  const { t } = useTranslation();
  const name = provider.name;
  return (
    <>
      <span className="block">
        {replaces === undefined
          ? t("pages.guardrailProviders.confirm.activateOwnerUnknown")
          : replaces === null
            ? t("pages.guardrailProviders.confirm.activateNoneActive")
            : t("pages.guardrailProviders.confirm.activateReplaces", { current: replaces })}
      </span>
      <span className="mt-2 block">
        {provider.stage === "post_call" ? (
          t("pages.guardrailProviders.confirm.activatePostCall")
        ) : provider.failure_mode === "fail_closed" ? (
          <Trans
            i18nKey="pages.guardrailProviders.confirm.activateFailClosed"
            values={{ name }}
            components={[<code key="code" className="font-mono text-xs" />]}
          />
        ) : (
          t("pages.guardrailProviders.confirm.activateFailOpen", { name })
        )}
      </span>
      {fileWins && (
        <span className="mt-2 block">
          {t("pages.guardrailProviders.confirm.activateFileWins", { name })}
        </span>
      )}
    </>
  );
}

/**
 * The card badge for a row, or `undefined` for the two states `PolicyCard`
 * already says from `enabled` alone.
 */
function cardStatus(
  status: ProviderStatus,
  t: TFunction,
): { tone: "warning" | "neutral"; label: string } | undefined {
  switch (status) {
    case "inert":
      return { tone: "warning", label: t("pages.guardrailProviders.statusInert") };
    case "overridden":
      return { tone: "neutral", label: t("pages.guardrailProviders.statusOverridden") };
    case "unknown":
      return { tone: "neutral", label: t("pages.guardrailProviders.statusUnknown") };
    default:
      return undefined;
  }
}

/**
 * What the gateway enforces, read from the effective webhook rather than from
 * the registry's switch. Green only when requests are really sent to a
 * guardrail service before they go upstream (#2162).
 */
function EnforcementBanner({
  enforcement,
  onRetry,
}: {
  enforcement: Enforcement;
  onRetry: () => void;
}) {
  const { t } = useTranslation();
  let tone: "success" | "warning" | "neutral" = "neutral";
  let icon: React.ReactNode;
  let title: string;
  const lines: React.ReactNode[] = [];

  if (enforcement.state === "unknown") {
    icon = <ShieldQuestion className="h-5 w-5 text-muted-foreground" aria-hidden />;
    title = t("pages.guardrailProviders.unknownBanner");
    lines.push(t("pages.guardrailProviders.unknownBody"));
  } else if (enforcement.state === "off") {
    icon = <ShieldOff className="h-5 w-5 text-muted-foreground" aria-hidden />;
    title = t("pages.guardrailProviders.offBanner");
    lines.push(t("pages.guardrailProviders.offBody"));
  } else {
    const { provider, overridden, webhook } = enforcement;
    if (enforcement.state === "enforced") {
      tone = "success";
      icon = <PlugZap className="h-5 w-5 text-[color:var(--status-success-text)]" aria-hidden />;
      title = provider
        ? t("pages.guardrailProviders.activeBanner", { name: provider.name })
        : t("pages.guardrailProviders.fileBanner");
    } else {
      tone = "warning";
      icon = (
        <ShieldAlert className="h-5 w-5 text-[color:var(--status-warning-text)]" aria-hidden />
      );
      title = provider
        ? t("pages.guardrailProviders.inertBanner", { name: provider.name })
        : t("pages.guardrailProviders.fileInertBanner");
    }
    if (!provider) {
      // the file's webhook is not a row on this screen, so its endpoint is
      // the only way to tell which service it is
      lines.push(<span className="font-mono">{webhook.url}</span>);
    }
    if (enforcement.state === "enforced") {
      lines.push(
        webhook.failure_mode === "fail_closed"
          ? t("pages.guardrailProviders.activeFailClosed")
          : t("pages.guardrailProviders.activeFailOpen"),
      );
    } else {
      lines.push(
        provider
          ? t("pages.guardrailProviders.inertBody")
          : t("pages.guardrailProviders.fileInertBody"),
      );
    }
    if (!provider) {
      lines.push(
        overridden
          ? t("pages.guardrailProviders.overriddenNote", { name: overridden.name })
          : t("pages.guardrailProviders.fileNote"),
      );
    }
  }

  return (
    <GuardrailBanner
      tone={tone}
      icon={icon}
      title={title}
      action={
        enforcement.state === "unknown" ? (
          <Button variant="outline" size="sm" onClick={onRetry}>
            {t("errors.load.retry")}
          </Button>
        ) : undefined
      }
    >
      {lines.map((line, index) => (
        <p key={index}>{line}</p>
      ))}
    </GuardrailBanner>
  );
}

function ProviderDialog({
  open,
  initial,
  active,
  pending,
  error,
  onClose,
  onSave,
}: {
  open: boolean;
  initial: GuardrailProviderRow | null;
  /** the registry's active row, `null` for none, `undefined` before it loads */
  active: GuardrailProviderRow | null | undefined;
  pending: boolean;
  error: string | null;
  onClose: () => void;
  onSave: (body: GuardrailProviderInput) => void;
}) {
  const { t } = useTranslation();
  const [form, setForm] = React.useState<GuardrailProviderInput>(initial ?? EMPTY);
  const set = (patch: Partial<GuardrailProviderInput>) =>
    setForm((value) => ({ ...value, ...patch }));
  const postCall = form.stage === "post_call";
  const [trialOpen, setTrialOpen] = React.useState(false);
  const urlValid = HTTP_URL.test(form.url);
  // said once the field is left, not on the first keystroke of a URL that is
  // still being typed
  const [urlLeft, setUrlLeft] = React.useState(false);
  const urlError =
    urlLeft && !urlValid
      ? form.url.trim() === ""
        ? t("pages.guardrailProviders.urlRequired")
        : t("pages.guardrailProviders.urlInvalid")
      : undefined;
  const valid =
    form.name.trim() !== "" &&
    urlValid &&
    form.timeout_ms > 0 &&
    form.max_body_bytes > 0 &&
    (form.auth_kind === "none" || Boolean(form.auth_env?.trim()));
  // names the provider the switch would pause; the save that turns it on is
  // confirmed on the screen, with the failure policy the form ends up with
  const activateHint = initial?.enabled
    ? t("pages.guardrailProviders.activateHintSelf")
    : active === undefined
      ? t("pages.guardrailProviders.activateHint")
      : active === null
        ? t("pages.guardrailProviders.activateHintNone")
        : t("pages.guardrailProviders.activateHintReplaces", { name: active.name });
  return (
    <Dialog open={open} onOpenChange={(value) => !value && onClose()}>
      <DialogHeader>
        <DialogTitle>
          {initial
            ? t("pages.guardrailProviders.dialogEditTitle")
            : t("pages.guardrailProviders.dialogAddTitle")}
        </DialogTitle>
        <DialogDescription>{t("pages.guardrailProviders.dialogBody")}</DialogDescription>
      </DialogHeader>
      <DialogBody className="space-y-4">
        <Field label={t("pages.guardrailProviders.fieldName")} htmlFor="provider-name">
          <Input
            id="provider-name"
            required
            value={form.name}
            onChange={(event) => set({ name: event.target.value })}
          />
        </Field>
        <Field
          label={t("pages.guardrailProviders.fieldUrl")}
          htmlFor="provider-url"
          hint={t("pages.guardrailProviders.urlHint")}
          error={urlError}
        >
          <Input
            id="provider-url"
            type="url"
            required
            className="font-mono"
            placeholder={URL_EXAMPLE}
            value={form.url}
            onChange={(event) => set({ url: event.target.value })}
            onBlur={() => setUrlLeft(true)}
          />
        </Field>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <Field label={t("pages.guardrailProviders.fieldStage")} htmlFor="provider-stage">
            <Combobox
              id="provider-stage"
              value={form.stage}
              onChange={(picked) =>
                set({
                  stage: picked as GuardrailProviderInput["stage"],
                })
              }
              aria-describedby={postCall ? "provider-stage-note" : undefined}
              options={[
                { value: "pre_call", label: t("pages.guardrailProviders.stagePre") },
                // the gateway runs only the pre-call stage (`consult_pre_call`
                // returns early for any other), so this is not offered until
                // it runs the output stage too (#2162)
                {
                  value: "post_call",
                  label: t("pages.guardrailProviders.stagePost"),
                  description: t("pages.guardrailProviders.stagePostUnsupported"),
                  disabled: true,
                },
              ]}
            />
          </Field>
          <Field label={t("pages.guardrailProviders.fieldFailure")} htmlFor="provider-failure">
            <Combobox
              id="provider-failure"
              aria-describedby={
                form.failure_mode === "fail_open" ? "provider-failure-note" : undefined
              }
              value={form.failure_mode}
              onChange={(picked) =>
                set({
                  failure_mode: picked as GuardrailProviderInput["failure_mode"],
                })
              }
              options={[
                { value: "fail_closed", label: t("pages.guardrailProviders.failClosed") },
                { value: "fail_open", label: t("pages.guardrailProviders.failOpen") },
              ]}
            />
          </Field>
        </div>
        {/* beside the failure policy it explains, not under the switch */}
        {form.failure_mode === "fail_open" && (
          <p
            id="provider-failure-note"
            className="rounded-lg bg-[color:var(--status-warning)]/10 p-3 text-xs text-[color:var(--status-warning-text)]"
          >
            {t("pages.guardrailProviders.failOpenWarning")}
          </p>
        )}
        {/* a row can still carry the stage: the API accepts it, and rows saved
            from this dialog before it was disabled keep it */}
        {postCall && (
          <p
            id="provider-stage-note"
            className="rounded-lg bg-[color:var(--status-warning)]/10 p-3 text-xs text-[color:var(--status-warning-text)]"
          >
            {t("pages.guardrailProviders.postCallDialogNote")}
          </p>
        )}
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
          <Field label={t("pages.guardrailProviders.fieldTimeout")} htmlFor="provider-timeout">
            <Input
              id="provider-timeout"
              type="number"
              min={1}
              value={form.timeout_ms}
              onChange={(event) => set({ timeout_ms: Number(event.target.value) })}
            />
          </Field>
          <Field label={t("pages.guardrailProviders.fieldRetries")} htmlFor="provider-retries">
            <Input
              id="provider-retries"
              type="number"
              min={0}
              value={form.max_retries}
              onChange={(event) => set({ max_retries: Number(event.target.value) })}
            />
          </Field>
          <Field label={t("pages.guardrailProviders.fieldBodyCap")} htmlFor="provider-cap">
            <Input
              id="provider-cap"
              type="number"
              min={1}
              value={form.max_body_bytes}
              onChange={(event) => set({ max_body_bytes: Number(event.target.value) })}
            />
          </Field>
        </div>
        <Field label={t("pages.guardrailProviders.fieldAuth")} htmlFor="provider-auth">
          <Combobox
            id="provider-auth"
            value={form.auth_kind}
            onChange={(picked) => {
              const auth_kind = picked as GuardrailProviderInput["auth_kind"];
              set({
                auth_kind,
                auth_env: auth_kind === "none" ? null : form.auth_env,
              });
            }}
            options={[
              { value: "none", label: t("pages.guardrailProviders.authNone") },
              { value: "bearer", label: t("pages.guardrailProviders.authBearer") },
              { value: "shared_secret", label: t("pages.guardrailProviders.authSharedSecret") },
            ]}
          />
        </Field>
        {form.auth_kind !== "none" && (
          <Field
            label={t("pages.guardrailProviders.fieldEnv")}
            htmlFor="provider-env"
            hint={t("pages.guardrailProviders.envHint")}
          >
            <Input
              id="provider-env"
              className="font-mono"
              placeholder="ROLTER_GUARDRAIL_TOKEN"
              value={form.auth_env ?? ""}
              onChange={(event) => set({ auth_env: event.target.value || null })}
            />
          </Field>
        )}
        <SwitchRow
          title={t("pages.guardrailProviders.activateLabel")}
          hint={activateHint}
          checked={form.enabled}
          onChange={(enabled) => set({ enabled })}
        />
        <FormSection
          title={t("pages.guardrailProviders.trial.title")}
          open={trialOpen}
          onToggle={() => setTrialOpen((value) => !value)}
        >
          <GuardrailProviderTrial draft={form} />
        </FormSection>
        {error && (
          <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">
            {error}
          </p>
        )}
      </DialogBody>
      <DialogFooter>
        <Button variant="ghost" onClick={onClose}>
          {t("common.cancel")}
        </Button>
        <Button disabled={!valid || pending} onClick={() => onSave(form)}>
          {pending && <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" aria-hidden />}
          {pending ? t("pages.guardrailProviders.publishing") : t("pages.guardrailProviders.save")}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(GuardrailProvidersScreen, "errors.resources.guardrailProviders");
