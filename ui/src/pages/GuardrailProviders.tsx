import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { Loader2, PlugZap, Plus, ShieldOff, ShieldQuestion } from "lucide-react";
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
import { Segmented } from "@/components/ui/segmented";

import { SwitchRow } from "@/components/ui/switch-row";
import {
  createGuardrailProvider,
  deleteGuardrailProvider,
  fetchEffectiveGuardrails,
  fetchGuardrailPolicies,
  fetchGuardrailProviders,
  updateGuardrailProvider,
  type GuardrailPolicyRow,
  type GuardrailProviderInput,
  type GuardrailProviderRow,
} from "@/lib/api";
import {
  policyScopeLabel,
  providerStatus,
  readSemanticGuards,
  resolveEnforcement,
  type Enforcement,
  type ProviderStatus,
} from "@/lib/guardrail-enforcement";
import { errorText } from "@/lib/error-copy";
import { useFormat } from "@/lib/i18n/format";
import { useToast } from "@/lib/toast";
import { useScreenReady } from "@/lib/ux-react";
import { cn } from "@/lib/utils";

// the shape of an evaluation URL, shown as a placeholder and never as a value:
// a prefilled host saves as the endpoint when nobody edits it, and a
// fail-closed provider pointed at a host that does not exist refuses every
// request (#2163)
const URL_EXAMPLE = "https://guardrails.internal/v1/evaluate";
const HTTP_URL = /^https?:\/\//;
const SHA256 = /^[0-9a-fA-F]{64}$/;

const EMPTY: GuardrailProviderInput = {
  name: "",
  enabled: false,
  kind: "webhook",
  url: "",
  model_path: null,
  model_sha256: null,
  stage: "pre_call",
  timeout_ms: 2000,
  max_retries: 0,
  failure_mode: "fail_closed",
  max_body_bytes: 65536,
  max_scored_bytes: null,
  on_truncated: "score",
  auth_kind: "none",
  auth_env: null,
};

function GuardrailProvidersScreen() {
  const { t } = useTranslation();
  const client = useQueryClient();
  const toast = useToast();
  const { number } = useFormat();
  // the registry, the effective document and the policies are read together,
  // so the banner never pairs a fresh list with a stale document between two
  // refetches. an unreadable document is `null`: the list still renders and the
  // banner says it cannot confirm enforcement (#2162); unreadable policies are
  // `null` too, and a confirmation then says it cannot name the traffic
  const query = useQuery({
    queryKey: ["guardrail-providers"],
    queryFn: async () => {
      const [providers, semantic, policies] = await Promise.all([
        fetchGuardrailProviders(),
        // which providers the gateways run (#459); without it the screen would
        // trust the registry's switch, which is not the same thing
        fetchEffectiveGuardrails().then(readSemanticGuards, () => null),
        fetchGuardrailPolicies().catch(() => null),
      ]);
      return { providers, semantic, policies };
    },
    retry: false,
  });
  // what the active providers did over the last week (#2158)
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
    ? resolveEnforcement(query.data.providers, query.data.semantic)
    : null;
  // the providers running besides the one being edited; `undefined` while the
  // effective document has not said, since "none" would be a guess
  const othersOf = (id: string | undefined): string[] | undefined =>
    enforcement?.state === "enforced"
      ? enforcement.providers.filter((p) => p.provider_id !== id).map((p) => p.name)
      : enforcement?.state === "off"
        ? []
        : undefined;

  // switching a provider on adds it to the active set (#459) and switching
  // one off takes it out of it (#2271), so both saves go through a confirmation
  // naming the provider and the traffic it sees or stops seeing. a save that
  // leaves the switch where it was changes neither and saves directly
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
  // pausing the last running provider leaves requests unchecked, which is as
  // red as the delete; with others still running it changes less
  const othersRemain = (othersOf(editing?.id) ?? []).length > 0;

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
              description={provider.kind === "local" ? (provider.model_path ?? "") : provider.url}
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
                  <Badge tone={provider.kind === "local" ? "info" : "outline"}>
                    {t(`pages.guardrailProviders.kind.${provider.kind}`)}
                  </Badge>
                  {/* colour says a credential is attached; no auth says nothing */}
                  {provider.kind === "webhook" && (
                    <Badge tone={provider.auth_kind === "none" ? "outline" : "info"}>
                      {provider.auth_kind.replace("_", " ")}
                    </Badge>
                  )}
                </>
              }
              details={
                <>
                  {provider.kind === "local"
                    ? t("pages.guardrailProviders.detailLineLocal", {
                        timeout: number(provider.timeout_ms),
                        kib: number(Math.round(provider.max_body_bytes / 1024)),
                      })
                    : t("pages.guardrailProviders.detailLine", {
                        timeout: number(provider.timeout_ms),
                        count: provider.max_retries,
                        kib: number(Math.round(provider.max_body_bytes / 1024)),
                      })}
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
        others={othersOf(editing?.id)}
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
        // red as the delete, and one with other providers running is not
        tone={pausing && !othersRemain ? "danger" : "default"}
        title={
          pausing
            ? t("pages.guardrailProviders.confirm.pauseTitle", { name: handingOver?.name })
            : t("pages.guardrailProviders.confirm.activateTitle", { name: handingOver?.name })
        }
        description={
          handingOver &&
          (pausing ? (
            <PauseConsequence
              others={othersOf(editing?.id)}
              reach={providerReach(query.data?.policies ?? null, editing?.id)}
            />
          ) : (
            <ActivationConsequence
              provider={handingOver}
              others={othersOf(editing?.id)}
              reach={providerReach(query.data?.policies ?? null, editing?.id)}
              fileWins={
                enforcement?.state === "enforced" &&
                enforcement.providers.some(
                  (p) => p.source === "file" && p.name === handingOver.name,
                )
              }
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
 * Which requests a provider sees, from the policies that list it. A provider no
 * policy lists, enabled or not, is unattached and runs on every request no
 * policy matches (reported as "default"); one that policies list runs only for
 * those policies' scopes, so pausing a policy never widens it.
 */
type Reach =
  | { kind: "unknown" }
  | { kind: "unattached" }
  | { kind: "scoped"; enabled: GuardrailPolicyRow[]; paused: GuardrailPolicyRow[] };

function providerReach(policies: GuardrailPolicyRow[] | null, id: string | undefined): Reach {
  if (policies === null) return { kind: "unknown" };
  const listing = id ? policies.filter((policy) => policy.provider_ids.includes(id)) : [];
  if (listing.length === 0) return { kind: "unattached" };
  return {
    kind: "scoped",
    enabled: listing.filter((policy) => policy.enabled),
    paused: listing.filter((policy) => !policy.enabled),
  };
}

/** the scopes of some policies, named, for a sentence */
function useScopeNames(policies: GuardrailPolicyRow[]): string {
  const { t } = useTranslation();
  return policies
    .map((policy) =>
      policyScopeLabel(policy, (kind) => t(`pages.guardrailPolicies.scopes.${kind}`)),
    )
    .join(", ");
}

/**
 * What activating `provider` does, for the confirmation: it joins the active
 * set without pausing another, then which requests it sees and what a failure
 * does. `others` is the names of the providers already running, `undefined`
 * when the effective document did not say.
 *
 * Rendered inside the dialog's description paragraph, so each sentence is a
 * block `span` rather than a `p`.
 */
function ActivationConsequence({
  provider,
  others,
  reach,
  fileWins,
}: {
  provider: GuardrailProviderInput;
  others: string[] | undefined;
  reach: Reach;
  fileWins: boolean;
}) {
  const { t } = useTranslation();
  const name = provider.name;
  const scopes = useScopeNames(reach.kind === "scoped" ? reach.enabled : []);
  return (
    <>
      <span className="block">
        {others === undefined
          ? t("pages.guardrailProviders.confirm.activateOthersUnknown")
          : others.length === 0
            ? t("pages.guardrailProviders.confirm.activateAlone")
            : t("pages.guardrailProviders.confirm.activateAlongside", {
                names: others.join(", "),
              })}
      </span>
      <span className="mt-2 block">
        {reach.kind === "unknown"
          ? t("pages.guardrailProviders.confirm.reachUnknown")
          : reach.kind === "unattached"
            ? t("pages.guardrailProviders.confirm.reachUnattached")
            : reach.enabled.length === 0
              ? t("pages.guardrailProviders.confirm.reachPaused")
              : t("pages.guardrailProviders.confirm.reachScoped", { scopes })}
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
 * What pausing the provider does: the other active providers keep running, and
 * the policies that list it lose it. `others` is `undefined` when the effective
 * document did not say, which is not the same as none.
 */
function PauseConsequence({ others, reach }: { others: string[] | undefined; reach: Reach }) {
  const { t } = useTranslation();
  const policies = reach.kind === "scoped" ? reach.enabled.map((policy) => policy.name) : [];
  return (
    <>
      <span className="block">
        {others === undefined
          ? t("pages.guardrailProviders.confirm.pauseUnknown")
          : others.length === 0
            ? t("pages.guardrailProviders.confirm.pauseUnchecked")
            : t("pages.guardrailProviders.confirm.pauseOthersKeep", { names: others.join(", ") })}
      </span>
      {policies.length > 0 && (
        <span className="mt-2 block">
          {t("pages.guardrailProviders.confirm.pausePolicies", { policies: policies.join(", ") })}
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
    case "overridden":
      return { tone: "neutral", label: t("pages.guardrailProviders.statusOverridden") };
    case "unknown":
      return { tone: "neutral", label: t("pages.guardrailProviders.statusUnknown") };
    default:
      return undefined;
  }
}

/**
 * What the gateways enforce, read from the providers they run rather than from
 * the registry's switch. Green only when requests are really sent to a
 * guardrail service (#2162); several providers may be running at once (#459).
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
    const { providers, unattached } = enforcement;
    tone = "success";
    icon = <PlugZap className="h-5 w-5 text-[color:var(--status-success-text)]" aria-hidden />;
    title = t("pages.guardrailProviders.activeBanner", {
      names: providers.map((provider) => provider.name).join(", "),
    });
    const closed = providers.filter((p) => p.failure_mode === "fail_closed").map((p) => p.name);
    const open = providers.filter((p) => p.failure_mode === "fail_open").map((p) => p.name);
    if (closed.length > 0) {
      lines.push(t("pages.guardrailProviders.activeFailClosed", { names: closed.join(", ") }));
    }
    if (open.length > 0) {
      lines.push(t("pages.guardrailProviders.activeFailOpen", { names: open.join(", ") }));
    }
    if (unattached.length > 0) {
      lines.push(t("pages.guardrailProviders.unattachedNote", { names: unattached.join(", ") }));
    }
    const fromFile = providers.filter((p) => p.source === "file").map((p) => p.name);
    if (fromFile.length > 0) {
      lines.push(t("pages.guardrailProviders.fileNote", { names: fromFile.join(", ") }));
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
  others,
  pending,
  error,
  onClose,
  onSave,
}: {
  open: boolean;
  initial: GuardrailProviderRow | null;
  /** the other providers running, `undefined` before the effective document says */
  others: string[] | undefined;
  pending: boolean;
  error: string | null;
  onClose: () => void;
  onSave: (body: GuardrailProviderInput) => void;
}) {
  const { t } = useTranslation();
  const [form, setForm] = React.useState<GuardrailProviderInput>(initial ?? EMPTY);
  const set = (patch: Partial<GuardrailProviderInput>) =>
    setForm((value) => ({ ...value, ...patch }));
  const [trialOpen, setTrialOpen] = React.useState(false);
  const local = form.kind === "local";
  // a kind switch resets the fields the other kind owns, so a stale URL never
  // rides along with a local provider (the server rejects it) and vice versa
  const setKind = (kind: GuardrailProviderInput["kind"]) =>
    set(
      kind === "local"
        ? {
            kind,
            url: "",
            auth_kind: "none",
            auth_env: null,
            max_retries: 0,
            model_path: form.model_path ?? "",
            model_sha256: form.model_sha256 ?? "",
          }
        : { kind, model_path: null, model_sha256: null },
    );
  const urlValid = HTTP_URL.test(form.url);
  const pathValid = (form.model_path ?? "").trim() !== "";
  const shaValid = SHA256.test(form.model_sha256 ?? "");
  const [pathLeft, setPathLeft] = React.useState(false);
  const [shaLeft, setShaLeft] = React.useState(false);
  const pathError =
    pathLeft && !pathValid ? t("pages.guardrailProviders.modelPathRequired") : undefined;
  const shaError = shaLeft && !shaValid ? t("pages.guardrailProviders.modelShaInvalid") : undefined;
  const scoredValid =
    form.max_scored_bytes === null ||
    (form.max_scored_bytes >= form.max_body_bytes && form.max_scored_bytes <= 16 * 1024 * 1024);
  const [scoredLeft, setScoredLeft] = React.useState(false);
  const scoredError =
    scoredLeft && !scoredValid ? t("pages.guardrailProviders.scoredBytesInvalid") : undefined;
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
    (local ? pathValid && shaValid && scoredValid : urlValid) &&
    form.timeout_ms > 0 &&
    form.max_body_bytes > 0 &&
    (form.auth_kind === "none" || Boolean(form.auth_env?.trim()));
  // says the switch adds this provider to the running ones; the save that
  // flips it is confirmed on the screen, with the failure policy the form ends
  // up with
  const activateHint = initial?.enabled
    ? t("pages.guardrailProviders.activateHintSelf")
    : others === undefined
      ? t("pages.guardrailProviders.activateHint")
      : others.length === 0
        ? t("pages.guardrailProviders.activateHintNone")
        : t("pages.guardrailProviders.activateHintAlongside", { names: others.join(", ") });
  return (
    <Dialog
      open={open}
      onOpenChange={(value) => !value && onClose()}
      onSubmit={() =>
        onSave(
          local
            ? {
                ...form,
                model_path: (form.model_path ?? "").trim(),
                model_sha256: (form.model_sha256 ?? "").toLowerCase(),
              }
            : form,
        )
      }
      canSubmit={valid}
      busy={pending}
    >
      <DialogHeader>
        <DialogTitle>
          {initial
            ? t("pages.guardrailProviders.dialogEditTitle")
            : t("pages.guardrailProviders.dialogAddTitle")}
        </DialogTitle>
        <DialogDescription>{t("pages.guardrailProviders.dialogBody")}</DialogDescription>
      </DialogHeader>
      <DialogBody className="space-y-4">
        <Field
          label={t("pages.guardrailProviders.fieldName")}
          htmlFor="provider-name"
          info={t("pages.guardrailProviders.nameInfo")}
          required
        >
          <Input
            id="provider-name"
            value={form.name}
            onChange={(event) => set({ name: event.target.value })}
          />
        </Field>
        <Field
          label={t("pages.guardrailProviders.fieldKind")}
          info={t("pages.guardrailProviders.kindInfo")}
        >
          <Segmented<GuardrailProviderInput["kind"]>
            ariaLabel={t("pages.guardrailProviders.fieldKind")}
            value={form.kind}
            options={[
              { value: "webhook", label: t("pages.guardrailProviders.kind.webhook") },
              { value: "local", label: t("pages.guardrailProviders.kind.local") },
            ]}
            onChange={setKind}
          />
        </Field>
        {local ? (
          <>
            <Field
              label={t("pages.guardrailProviders.fieldModelPath")}
              htmlFor="provider-model-path"
              hint={t("pages.guardrailProviders.modelPathHint")}
              error={pathError}
              required
            >
              <Input
                id="provider-model-path"
                className="font-mono"
                placeholder="/etc/rolter/models/injection.json"
                value={form.model_path ?? ""}
                onChange={(event) => set({ model_path: event.target.value })}
                onBlur={() => setPathLeft(true)}
              />
            </Field>
            <Field
              label={t("pages.guardrailProviders.fieldModelSha")}
              htmlFor="provider-model-sha"
              hint={t("pages.guardrailProviders.modelShaHint")}
              error={shaError}
              required
            >
              <Input
                id="provider-model-sha"
                className="font-mono"
                value={form.model_sha256 ?? ""}
                onChange={(event) => set({ model_sha256: event.target.value.trim() })}
                onBlur={() => setShaLeft(true)}
              />
            </Field>
          </>
        ) : (
          <Field
            label={t("pages.guardrailProviders.fieldUrl")}
            htmlFor="provider-url"
            hint={t("pages.guardrailProviders.urlHint")}
            error={urlError}
            required
          >
            <Input
              id="provider-url"
              type="url"
              className="font-mono"
              placeholder={URL_EXAMPLE}
              value={form.url}
              onChange={(event) => set({ url: event.target.value })}
              onBlur={() => setUrlLeft(true)}
            />
          </Field>
        )}
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <Field
            label={t("pages.guardrailProviders.fieldStage")}
            htmlFor="provider-stage"
            info={t("pages.guardrailProviders.stageInfo")}
          >
            <Combobox
              id="provider-stage"
              value={form.stage}
              onChange={(picked) =>
                set({
                  stage: picked as GuardrailProviderInput["stage"],
                })
              }
              options={[
                { value: "pre_call", label: t("pages.guardrailProviders.stagePre") },
                // the gateway checks the response body at this stage too (#459)
                { value: "post_call", label: t("pages.guardrailProviders.stagePost") },
              ]}
            />
          </Field>
          <Field
            label={t("pages.guardrailProviders.fieldFailure")}
            htmlFor="provider-failure"
            info={t("pages.guardrailProviders.failureInfo")}
          >
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
        <div className={cn("grid grid-cols-1 gap-3", local ? "sm:grid-cols-2" : "sm:grid-cols-3")}>
          <Field
            label={t("pages.guardrailProviders.fieldTimeout")}
            htmlFor="provider-timeout"
            info={t("pages.guardrailProviders.timeoutInfo")}
          >
            <Input
              id="provider-timeout"
              type="number"
              min={1}
              value={form.timeout_ms}
              onChange={(event) => set({ timeout_ms: Number(event.target.value) })}
            />
          </Field>
          {!local && (
            <Field
              label={t("pages.guardrailProviders.fieldRetries")}
              htmlFor="provider-retries"
              info={t("pages.guardrailProviders.retriesInfo")}
            >
              <Input
                id="provider-retries"
                type="number"
                min={0}
                value={form.max_retries}
                onChange={(event) => set({ max_retries: Number(event.target.value) })}
              />
            </Field>
          )}
          <Field
            label={t("pages.guardrailProviders.fieldBodyCap")}
            htmlFor="provider-cap"
            info={t("pages.guardrailProviders.bodyCapInfo")}
          >
            <Input
              id="provider-cap"
              type="number"
              min={1}
              value={form.max_body_bytes}
              onChange={(event) => set({ max_body_bytes: Number(event.target.value) })}
            />
          </Field>
        </div>
        {local && (
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            <Field
              label={t("pages.guardrailProviders.fieldScoredBytes")}
              htmlFor="provider-scored-cap"
              info={t("pages.guardrailProviders.scoredBytesInfo")}
              error={scoredError}
            >
              <Input
                id="provider-scored-cap"
                type="number"
                min={form.max_body_bytes}
                max={16 * 1024 * 1024}
                value={form.max_scored_bytes ?? ""}
                onChange={(event) =>
                  set({
                    max_scored_bytes: event.target.value === "" ? null : Number(event.target.value),
                  })
                }
                onBlur={() => setScoredLeft(true)}
              />
            </Field>
            <Field
              label={t("pages.guardrailProviders.fieldOnTruncated")}
              htmlFor="provider-on-truncated"
              info={t("pages.guardrailProviders.onTruncatedInfo")}
            >
              <Combobox
                id="provider-on-truncated"
                value={form.on_truncated}
                onChange={(picked) =>
                  set({ on_truncated: picked as GuardrailProviderInput["on_truncated"] })
                }
                options={[
                  { value: "score", label: t("pages.guardrailProviders.truncatedScore") },
                  {
                    value: "fail_closed",
                    label: t("pages.guardrailProviders.truncatedFailClosed"),
                  },
                ]}
              />
            </Field>
          </div>
        )}
        {!local && (
          <Field
            label={t("pages.guardrailProviders.fieldAuth")}
            htmlFor="provider-auth"
            info={t("pages.guardrailProviders.authInfo")}
          >
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
        )}
        {!local && form.auth_kind !== "none" && (
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
        {local ? (
          <p className="text-xs text-muted-foreground">
            {t("pages.guardrailProviders.trial.localNote")}
          </p>
        ) : (
          <FormSection
            title={t("pages.guardrailProviders.trial.title")}
            open={trialOpen}
            onToggle={() => setTrialOpen((value) => !value)}
          >
            <GuardrailProviderTrial draft={form} />
          </FormSection>
        )}
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
        <Button
          type="submit"
          disabled={pending}
          aria-disabled={!valid || undefined}
          className="aria-disabled:opacity-50"
        >
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
export default superadminOnly(
  GuardrailProvidersScreen,
  "errors.resources.guardrailProviders",
  "guardrail_provider:read",
);
