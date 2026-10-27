import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  ArrowRight,
  ChevronRight,
  Loader2,
  Lock,
  Plus,
  ShieldOff,
  ShieldQuestion,
} from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";
import { Link } from "react-router";

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
  RULE_ACTIONS,
  useGuardrailOutcomes,
} from "@/components/GuardrailOutcomes";
import { GuardrailPolicyTrial } from "@/components/GuardrailPolicyTrial";
import { GuardrailRuleTrial } from "@/components/GuardrailRuleTrial";
import { LoadError } from "@/components/LoadError";
import { superadminOnly } from "@/components/ForbiddenScreen";
import { GatedButton } from "@/components/GatedButton";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
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
import { Tag } from "@/components/ui/tag";
import { Textarea } from "@/components/ui/textarea";
import {
  createGuardrailRule,
  deleteGuardrailRule,
  fetchConfig,
  fetchEffectiveGuardrails,
  fetchGuardrailRules,
  updateGuardrailRule,
  type GuardrailRuleInput,
  type GuardrailRuleRow,
} from "@/lib/api";
import {
  readEffectivePolicy,
  readRuleSources,
  resolvePolicy,
  type RuleSource,
  type EffectiveRule,
  type RowState,
} from "@/lib/guardrail-policy";
import { errorText } from "@/lib/error-copy";
import { defaultToken, replacementToken, ruleBody, withSource } from "@/lib/guardrail-replacement";
import { useFormat } from "@/lib/i18n/format";
import { useToast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useScreenReady } from "@/lib/ux-react";

const EMPTY: GuardrailRuleInput = {
  name: "",
  enabled: true,
  source_type: "builtin",
  builtin: "email",
  pattern: null,
  stage: "pre_call",
  action: "redact",
  // empty, so the gateway writes the detector's own token (#2160)
  replacement: null,
  include_system: false,
  position: 0,
};

/** the gap between the positions a new rule is numbered by, so one can slip between two */
const POSITION_STEP = 10;

/** after the last rule, so a new rule never ties with one and falls back to name order (#2161) */
function nextPosition(rules: GuardrailRuleRow[]): number {
  return rules.reduce((last, rule) => Math.max(last, rule.position), 0) + POSITION_STEP;
}

/**
 * What a post-call rule does to a streamed request on this deployment (#2156).
 *
 * A post-call rule masks the buffered response, which a stream never is, so
 * the gateway settles it with the deployment-wide
 * `[guardrails] streaming_post_call`: `reject` refuses the request with a 400,
 * `passthrough` serves the stream with the rule skipped. That is a property
 * of the deployment rather than of the rule, so it is read from the effective
 * config, and `unknown` stands for a config that could not be read or did not
 * say.
 */
type StreamingMode = "reject" | "passthrough" | "unknown";

/** the parts of a rule a card shows, shared by dashboard rows and file rules */
type RuleShape = Pick<GuardrailRuleRow, "stage" | "action" | "include_system"> & {
  builtin?: GuardrailRuleRow["builtin"];
  pattern?: string | null;
  replacement?: string | null;
};

/** one line on what each built-in detector matches, so the picker does not oversell it (#2159) */
const DETECTOR_HINT = {
  email: "pages.guardrailRules.detectorHint.email",
  phone: "pages.guardrailRules.detectorHint.phone",
  api_token: "pages.guardrailRules.detectorHint.apiToken",
  payment_card: "pages.guardrailRules.detectorHint.paymentCard",
} as const;

const ACTION_HINT = {
  annotate: "pages.guardrailRules.actionHint.annotate",
  block: "pages.guardrailRules.actionHint.block",
  redact: "pages.guardrailRules.actionHint.redact",
} as const;

const STREAMING_COPY = {
  card: {
    reject: "pages.guardrailRules.streaming.cardReject",
    passthrough: "pages.guardrailRules.streaming.cardPassthrough",
    unknown: "pages.guardrailRules.streaming.cardUnknown",
  },
  note: {
    reject: "pages.guardrailRules.streaming.noteReject",
    passthrough: "pages.guardrailRules.streaming.notePassthrough",
    unknown: "pages.guardrailRules.streaming.noteUnknown",
  },
} as const;

function GuardrailRulesScreen() {
  const { t } = useTranslation();
  const client = useQueryClient();
  const toast = useToast();
  const query = useQuery({
    queryKey: ["guardrail-rules"],
    queryFn: fetchGuardrailRules,
    retry: false,
  });
  // what each rule did over the last week (#2158), read beside the rules
  const outcomes = useGuardrailOutcomes();

  // the same cache entry the Effective config and Models screens read, fetched
  // afresh on every visit: the config can also change outside this tab, which
  // the flag save's invalidation of ["config"] does not cover (#2284)
  const config = useQuery({
    queryKey: ["config"],
    queryFn: fetchConfig,
    retry: false,
    staleTime: 0,
  });
  // where each effective rule comes from (#2249). best-effort: when it cannot
  // be read, the cards infer the source from the rows, as they did before
  const sourcesQuery = useQuery({
    queryKey: ["guardrail-effective"],
    queryFn: () => fetchEffectiveGuardrails().then(readRuleSources),
    retry: false,
    staleTime: 0,
  });
  // every card compares the list with the effective policy, so both are read
  // as one settled pair. A refetch of one against a stale copy of the other
  // would flash a rule just edited as overridden, or a rule just deleted as
  // the config file's (#2157)
  const settled =
    query.isSuccess &&
    !query.isFetching &&
    !config.isFetching &&
    !sourcesQuery.isFetching &&
    (config.isSuccess || config.isError);
  const [shown, setShown] = React.useState<{
    rows: GuardrailRuleRow[];
    config: unknown;
    failed: boolean;
    sources: Map<string, RuleSource> | null;
  }>();
  const sources = sourcesQuery.data ?? null;
  if (
    settled &&
    (shown?.rows !== query.data ||
      shown.config !== config.data ||
      shown.failed !== config.isError ||
      shown.sources !== sources)
  ) {
    setShown({ rows: query.data, config: config.data, failed: config.isError, sources });
  }
  const resolution = React.useMemo(
    () =>
      shown &&
      resolvePolicy(
        shown.rows,
        shown.failed ? null : readEffectivePolicy(shown.config),
        shown.sources,
      ),
    [shown],
  );
  const policy = resolution?.policy ?? null;
  const loading = query.isLoading || (query.isSuccess && !resolution);
  // undefined until the pair settles, so a note never says "could not be read"
  // about a request still in flight
  const streaming: StreamingMode | undefined = resolution
    ? (policy?.streaming ?? "unknown")
    : undefined;

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `query` is the query the user is actually waiting on for this screen
  useScreenReady(!loading);
  const [editing, setEditing] = React.useState<GuardrailRuleRow | null | undefined>();
  const fileHeadingId = React.useId();
  const [policyTrialOpen, setPolicyTrialOpen] = React.useState(false);

  // a change to a row changes the effective policy too, and the cards need both
  const refresh = () => {
    void client.invalidateQueries({ queryKey: ["guardrail-rules"] });
    void client.invalidateQueries({ queryKey: ["config"] });
    void client.invalidateQueries({ queryKey: ["guardrail-effective"] });
  };

  const save = useMutation({
    mutationFn: (body: GuardrailRuleInput) =>
      editing ? updateGuardrailRule(editing.id, body) : createGuardrailRule(body),
    onSuccess: (_result, body) => {
      refresh();
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
    mutationFn: deleteGuardrailRule,
    onSuccess: refresh,
  });

  // was a bare window.confirm: unstyled, untranslatable, and invisible to the
  // story runner, which is the one place this path is ever exercised (#1179)
  const [deleteTarget, setDeleteTarget] = React.useState<GuardrailRuleRow | null>(null);
  const startDelete = (rule: GuardrailRuleRow) => {
    remove.reset();
    setDeleteTarget(rule);
  };

  const builtinDescription = (builtin: GuardrailRuleRow["builtin"]) => {
    switch (builtin) {
      case "email":
        return t("pages.guardrailRules.builtinCardEmail");
      case "phone":
        return t("pages.guardrailRules.builtinCardPhone");
      case "api_token":
        return t("pages.guardrailRules.builtinCardApiToken");
      case "payment_card":
        return t("pages.guardrailRules.builtinCardPaymentCard");
      default:
        return t("pages.guardrailRules.sourceBuiltin");
    }
  };

  // a custom pattern is copyable config, so its card draws it in mono (#2161)
  const isPattern = (rule: RuleShape) => !rule.builtin && Boolean(rule.pattern);
  const describe = (rule: RuleShape) =>
    rule.builtin
      ? builtinDescription(rule.builtin)
      : (rule.pattern ?? t("pages.guardrailRules.customRegex"));

  const notEnforced = {
    tone: "neutral" as const,
    label: t("pages.guardrailRules.status.notEnforced"),
  };
  // enforced and paused keep the card's own badge
  const rowStatus = (state: RowState | undefined) => {
    switch (state?.state) {
      case "off":
        return notEnforced;
      case "overridden":
        return { tone: "warning" as const, label: t("pages.guardrailRules.status.overridden") };
      case "unknown":
        return { tone: "neutral" as const, label: t("pages.guardrailRules.status.unknown") };
      default:
        return undefined;
    }
  };

  const open = editing !== undefined;
  const rules = shown?.rows ?? [];
  const fileRules = resolution?.fileRules ?? [];
  // the routes that switch an effective rule off, by its name. A row credited
  // with that rule shows them; an overridden row does not run, so the file
  // rule's card carries them, and a paused row is in no policy to be off in
  const offRoutes = (name: string) => resolution?.offRoutes.get(name) ?? [];
  return (
    <div className="mx-auto flex max-w-[1120px] flex-col gap-5 p-[22px]">
      <div className="flex flex-col gap-3 border-b border-[color:var(--border-subtle)] pb-5 sm:flex-row sm:items-end sm:justify-between">
        <div>
          <p className="max-w-2xl text-sm text-muted-foreground">
            {t("pages.guardrailRules.intro")}
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <GatedButton
            gate="guardrail_rule:update"
            control="guardrail-policy-trial-open"
            variant="outline"
            onClick={() => setPolicyTrialOpen(true)}
          >
            {t("pages.guardrailRules.policyTrial.open")}
          </GatedButton>
          <GatedButton
            gate="guardrail_rule:create"
            control="guardrail-rule-new"
            onClick={() => setEditing(null)}
          >
            <Plus className="h-4 w-4" aria-hidden /> {t("pages.guardrailRules.addRule")}
          </GatedButton>
        </div>
      </div>

      {/* the cards list what is stored; these two say it is not what runs */}
      {resolution && !query.isError && !policy && (
        <GuardrailBanner
          tone="neutral"
          icon={<ShieldQuestion className="h-5 w-5 text-muted-foreground" aria-hidden />}
          title={t("pages.guardrailRules.unknownPolicy.title")}
          action={
            <Button variant="outline" size="sm" onClick={() => void config.refetch()}>
              {t("errors.load.retry")}
            </Button>
          }
        >
          <p>{t("pages.guardrailRules.unknownPolicy.body")}</p>
        </GuardrailBanner>
      )}
      {resolution && !query.isError && policy && !policy.on && (
        <GuardrailBanner
          tone="warning"
          icon={
            <ShieldOff className="h-5 w-5 text-[color:var(--status-warning-text)]" aria-hidden />
          }
          title={t("pages.guardrailRules.flagOff.title")}
          action={
            <Link
              to="/feature-flags"
              className={cn(buttonVariants({ variant: "outline", size: "sm" }), "gap-1.5")}
            >
              {t("pages.guardrailRules.flagOff.link")}
              <ArrowRight className="h-3.5 w-3.5" aria-hidden />
            </Link>
          }
        >
          <p>{t("pages.guardrailRules.flagOff.body")}</p>
        </GuardrailBanner>
      )}

      {!loading && !query.isError && <GuardrailOutcomesNotice query={outcomes} />}

      {loading ? (
        <GuardrailLoading columns={1} />
      ) : query.isError ? (
        // never hand-rolled: a 403 is what a non-superadmin gets on this
        // deployment-scoped screen, and the bespoke panel offered it a retry
        // that could not ever work (#1259)
        <LoadError
          error={query.error}
          resource={t("errors.resources.guardrailRules")}
          onRetry={() => void query.refetch()}
          target="guardrail-rules"
        />
      ) : (
        <>
          {rules.length === 0 ? (
            // with config-file rules below, "no rules" would misstate the policy
            <GuardrailEmpty
              uxTarget="guardrail-rules"
              title={
                fileRules.length > 0
                  ? t("pages.guardrailRules.emptyFileTitle")
                  : t("pages.guardrailRules.emptyTitle")
              }
              description={
                fileRules.length > 0
                  ? t("pages.guardrailRules.emptyFileBody")
                  : t("pages.guardrailRules.emptyBody")
              }
              action={
                <GatedButton
                  gate="guardrail_rule:create"
                  control="guardrail-rule-new-empty"
                  onClick={() => setEditing(null)}
                >
                  {t("pages.guardrailRules.addFirst")}
                </GatedButton>
              }
            />
          ) : (
            // one column: rules run by position, and a grid would read 10, 20
            // across and 30 under 10 (#2161)
            <ol aria-label={t("pages.guardrailRules.orderLabel")} className="flex flex-col gap-3">
              {rules.map((rule) => {
                const state = resolution?.rows.get(rule.id);
                return (
                  <li key={rule.id} className="min-w-0">
                    <PolicyCard
                      title={`${rule.position.toString().padStart(2, "0")} · ${rule.name}`}
                      description={describe(rule)}
                      descriptionMono={isPattern(rule)}
                      enabled={rule.enabled}
                      status={rowStatus(state)}
                      badges={<RuleBadges rule={rule} />}
                      details={
                        <>
                          {/* an overridden row does not run, so what it would do to a
                            stream is beside the point; the file rule's card says it */}
                          <RuleDetails
                            rule={rule}
                            streaming={state?.state === "overridden" ? undefined : streaming}
                          />
                          {(state?.state === "enforced" || state?.state === "off") && (
                            <OffRoutes routes={offRoutes(rule.name)} />
                          )}
                          {/* an overridden row never runs; the file rule's card has the counts */}
                          {state?.state !== "overridden" && (
                            <OutcomeCounts
                              query={outcomes}
                              rule={rule.name}
                              actions={RULE_ACTIONS}
                            />
                          )}
                          {state?.state === "overridden" && (
                            <FileRuleNote
                              i18nKey="pages.guardrailRules.overriddenDetail"
                              rule={state.by}
                              warning
                            />
                          )}
                          {state?.state === "paused" && state.clash && (
                            <FileRuleNote
                              i18nKey="pages.guardrailRules.pausedClashDetail"
                              rule={state.clash}
                            />
                          )}
                        </>
                      }
                      actions={
                        <>
                          <GatedButton
                            gate="guardrail_rule:delete"
                            control="guardrail-rule-delete"
                            variant="ghost"
                            aria-label={t("pages.guardrailRules.deleteAria", { name: rule.name })}
                            onClick={() => startDelete(rule)}
                            disabled={remove.isPending && remove.variables === rule.id}
                          >
                            {remove.isPending && remove.variables === rule.id && (
                              <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" />
                            )}
                            {t("common.delete")}
                          </GatedButton>
                          <GatedButton
                            gate="guardrail_rule:update"
                            control="guardrail-rule-edit"
                            variant="outline"
                            aria-label={t("pages.guardrailRules.editAria", { name: rule.name })}
                            onClick={() => setEditing(rule)}
                          >
                            {t("pages.guardrailRules.editRule")}
                          </GatedButton>
                        </>
                      }
                    />
                  </li>
                );
              })}
            </ol>
          )}

          {/* the rest of the policy: file-owned, read-only, and run first */}
          {policy && fileRules.length > 0 && (
            <section aria-labelledby={fileHeadingId} className="flex flex-col gap-3 pt-2">
              <div>
                <h2 id={fileHeadingId} className="text-base font-semibold">
                  {t("pages.guardrailRules.file.heading")}
                </h2>
                <p className="mt-1 max-w-2xl text-sm text-muted-foreground">
                  <Trans
                    i18nKey="pages.guardrailRules.file.intro"
                    components={[<code key="key" className="font-mono text-xs" />]}
                  />
                </p>
              </div>
              <ul className="flex flex-col gap-3">
                {fileRules.map((rule) => (
                  <li key={rule.name} className="min-w-0">
                    <PolicyCard
                      headingLevel="h3"
                      title={rule.name}
                      description={describe(rule)}
                      descriptionMono={isPattern(rule)}
                      enabled
                      status={policy.on ? undefined : notEnforced}
                      badges={<RuleBadges rule={rule} />}
                      details={
                        <>
                          <RuleDetails rule={rule} streaming={streaming} />
                          <OffRoutes routes={offRoutes(rule.name)} />
                          <OutcomeCounts query={outcomes} rule={rule.name} actions={RULE_ACTIONS} />
                        </>
                      }
                      actions={
                        <p className="flex items-center gap-1.5 text-xs text-[color:var(--text-subtle)]">
                          <Lock className="h-3.5 w-3.5" aria-hidden />
                          {t("pages.guardrailRules.file.readOnly")}
                        </p>
                      }
                    />
                  </li>
                ))}
              </ul>
            </section>
          )}
        </>
      )}

      <ConfirmDialog
        name="guardrail-rule-delete"
        open={!!deleteTarget}
        onOpenChange={(o) => !o && setDeleteTarget(null)}
        title={t("pages.guardrailRules.confirm.title", { name: deleteTarget?.name })}
        description={t("pages.guardrailRules.confirm.body")}
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

      <Dialog open={policyTrialOpen} onOpenChange={setPolicyTrialOpen} size="lg">
        <DialogHeader>
          <DialogTitle>{t("pages.guardrailRules.policyTrial.title")}</DialogTitle>
          <DialogDescription>{t("pages.guardrailRules.policyTrial.intro")}</DialogDescription>
        </DialogHeader>
        <DialogBody>
          <GuardrailPolicyTrial />
        </DialogBody>
        <DialogFooter>
          <Button variant="ghost" onClick={() => setPolicyTrialOpen(false)}>
            {t("common.close")}
          </Button>
        </DialogFooter>
      </Dialog>

      <RuleDialog
        key={editing?.id ?? (editing === null ? "new" : "closed")}
        open={open}
        initial={editing ?? null}
        nextPosition={nextPosition(rules)}
        streaming={streaming}
        fileNames={fileRules.map((rule) => rule.name)}
        pending={save.isPending}
        error={save.isError ? errorText(save.error, t) : null}
        onClose={() => setEditing(undefined)}
        onSave={(body) => save.mutate(body)}
      />
    </div>
  );
}

function RuleDialog({
  open,
  initial,
  nextPosition,
  streaming,
  fileNames,
  pending,
  error,
  onClose,
  onSave,
}: {
  open: boolean;
  initial: GuardrailRuleRow | null;
  /** the position a new rule starts at: after the last one */
  nextPosition: number;
  streaming: StreamingMode | undefined;
  /** names the config file already uses; a row under one of them never runs */
  fileNames: string[];
  pending: boolean;
  error: string | null;
  onClose: () => void;
  onSave: (body: GuardrailRuleInput) => void;
}) {
  const { t } = useTranslation();
  // a stored block or annotate row can still hold a token it never wrote, so
  // the field starts empty should the action become redact (#2160)
  const [form, setForm] = React.useState<GuardrailRuleInput>(() =>
    initial ? ruleBody(initial) : { ...EMPTY, position: nextPosition },
  );
  const set = (patch: Partial<GuardrailRuleInput>) => setForm((value) => ({ ...value, ...patch }));
  // a token the user never edited follows the detector
  const setSource = (patch: Parameters<typeof withSource>[1]) =>
    setForm((value) => withSource(value, patch));
  const fallback = defaultToken(form.builtin);
  const valid =
    form.name.trim() !== "" && (form.source_type === "builtin" || Boolean(form.pattern?.trim()));
  // shown before saving, and read out with the stage it depends on
  const noteId = React.useId();
  const [trialOpen, setTrialOpen] = React.useState(false);
  const streamingNote = form.stage === "post_call" ? streaming : undefined;
  return (
    <Dialog
      open={open}
      onOpenChange={(value) => !value && onClose()}
      onSubmit={() => onSave(ruleBody(form))}
      canSubmit={valid}
      busy={pending}
    >
      <DialogHeader>
        <DialogTitle>
          {initial
            ? t("pages.guardrailRules.dialogEditTitle")
            : t("pages.guardrailRules.dialogAddTitle")}
        </DialogTitle>
        <DialogDescription>{t("pages.guardrailRules.dialogBody")}</DialogDescription>
      </DialogHeader>
      <DialogBody className="space-y-4">
        <Field
          label={t("pages.guardrailRules.fieldName")}
          htmlFor="rule-name"
          required
          hint={
            fileNames.includes(form.name.trim()) ? t("pages.guardrailRules.nameClash") : undefined
          }
        >
          <Input
            id="rule-name"
            value={form.name}
            onChange={(event) => set({ name: event.target.value })}
          />
        </Field>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <Field
            label={t("pages.guardrailRules.fieldSource")}
            htmlFor="rule-source"
            info={t("pages.guardrailRules.sourceInfo")}
          >
            <Combobox
              id="rule-source"
              value={form.source_type}
              onChange={(picked) =>
                setSource({
                  source_type: picked as GuardrailRuleInput["source_type"],
                  builtin: picked === "builtin" ? "email" : null,
                  pattern: picked === "pattern" ? "" : null,
                })
              }
              options={[
                { value: "builtin", label: t("pages.guardrailRules.sourceBuiltin") },
                { value: "pattern", label: t("pages.guardrailRules.sourcePattern") },
              ]}
            />
          </Field>
          <Field
            label={t("pages.guardrailRules.fieldPosition")}
            htmlFor="rule-position"
            hint={t("pages.guardrailRules.positionHint")}
          >
            <Input
              id="rule-position"
              type="number"
              min={0}
              value={form.position}
              onChange={(event) => set({ position: Number(event.target.value) })}
            />
          </Field>
        </div>
        {form.source_type === "builtin" ? (
          <Field
            label={t("pages.guardrailRules.fieldDetector")}
            htmlFor="rule-builtin"
            hint={t(DETECTOR_HINT[form.builtin ?? "email"])}
          >
            <Combobox
              id="rule-builtin"
              value={form.builtin ?? "email"}
              onChange={(picked) =>
                setSource({
                  builtin: picked as GuardrailRuleInput["builtin"],
                })
              }
              options={[
                { value: "email", label: t("pages.guardrailRules.detectorEmail") },
                { value: "phone", label: t("pages.guardrailRules.detectorPhone") },
                { value: "api_token", label: t("pages.guardrailRules.detectorApiToken") },
                { value: "payment_card", label: t("pages.guardrailRules.detectorPaymentCard") },
              ]}
            />
          </Field>
        ) : (
          <Field
            label={t("pages.guardrailRules.fieldPattern")}
            htmlFor="rule-pattern"
            hint={t("pages.guardrailRules.patternHint")}
          >
            <Textarea
              id="rule-pattern"
              rows={3}
              value={form.pattern ?? ""}
              onChange={(event) => set({ pattern: event.target.value })}
            />
          </Field>
        )}
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <Field
            label={t("pages.guardrailRules.fieldStage")}
            htmlFor="rule-stage"
            info={t("pages.guardrailRules.stageInfo")}
          >
            <Combobox
              id="rule-stage"
              aria-describedby={streamingNote ? noteId : undefined}
              value={form.stage}
              onChange={(picked) =>
                set({
                  stage: picked as GuardrailRuleInput["stage"],
                })
              }
              options={[
                { value: "pre_call", label: t("pages.guardrailRules.stagePre") },
                { value: "post_call", label: t("pages.guardrailRules.stagePost") },
              ]}
            />
          </Field>
          <Field
            label={t("pages.guardrailRules.fieldAction")}
            htmlFor="rule-action"
            hint={t(ACTION_HINT[form.action])}
          >
            <Combobox
              id="rule-action"
              value={form.action}
              onChange={(picked) =>
                set({
                  action: picked as GuardrailRuleInput["action"],
                })
              }
              options={[
                { value: "annotate", label: t("pages.guardrailRules.actionAnnotate") },
                { value: "block", label: t("pages.guardrailRules.actionBlock") },
                { value: "redact", label: t("pages.guardrailRules.actionRedact") },
              ]}
            />
          </Field>
        </div>
        {streamingNote && <StreamingEffect id={noteId} mode={streamingNote} variant="note" />}
        {form.action === "redact" && (
          <Field
            label={t("pages.guardrailRules.fieldReplacement")}
            htmlFor="rule-replacement"
            hint={
              <Trans
                i18nKey="pages.guardrailRules.replacementHint"
                values={{ token: fallback }}
                components={[<code key="token" className="font-mono" />]}
              />
            }
          >
            <Input
              id="rule-replacement"
              value={form.replacement ?? ""}
              placeholder={fallback}
              onChange={(event) => set({ replacement: event.target.value || null })}
            />
          </Field>
        )}
        <SwitchRow
          title={t("pages.guardrailRules.toggleEnabled")}
          hint={t("pages.guardrailRules.toggleEnabledHint")}
          checked={form.enabled}
          onChange={(enabled) => set({ enabled })}
        />
        <SwitchRow
          title={t("pages.guardrailRules.toggleSystem")}
          hint={t("pages.guardrailRules.toggleSystemHint")}
          checked={form.include_system}
          onChange={(include_system) => set({ include_system })}
        />
        <FormSection
          title={t("pages.guardrailRules.trial.title")}
          open={trialOpen}
          onToggle={() => setTrialOpen((value) => !value)}
        >
          <GuardrailRuleTrial draft={ruleBody(form)} />
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
        <Button
          type="submit"
          disabled={pending}
          aria-disabled={!valid || undefined}
          className="aria-disabled:opacity-50"
        >
          {pending && <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" aria-hidden />}
          {pending ? t("pages.guardrailRules.publishing") : t("pages.guardrailRules.publish")}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

function RuleBadges({ rule }: { rule: RuleShape }) {
  const { t } = useTranslation();
  return (
    <>
      <Badge
        tone={rule.action === "block" ? "danger" : rule.action === "redact" ? "warning" : "info"}
      >
        {rule.action}
      </Badge>
      <Badge tone="outline">{rule.stage.replace("_", "-")}</Badge>
      {rule.include_system && <Badge tone="accent">{t("pages.guardrailRules.systemBadge")}</Badge>}
    </>
  );
}

function RuleDetails({
  rule,
  streaming,
}: {
  rule: RuleShape;
  streaming: StreamingMode | undefined;
}) {
  const { t } = useTranslation();
  // the token the gateway writes, so a block rule with a stale one says none
  const token = replacementToken(rule);
  return (
    <>
      <p>
        {token !== null
          ? t("pages.guardrailRules.replacementDetail", { token })
          : t("pages.guardrailRules.noRewrite")}
      </p>
      {rule.stage === "post_call" && streaming && (
        <StreamingEffect mode={streaming} variant="card" />
      )}
    </>
  );
}

/** a line on a dashboard card naming the config-file rule under its name */
function FileRuleNote({
  i18nKey,
  rule,
  warning,
}: {
  i18nKey: "pages.guardrailRules.overriddenDetail" | "pages.guardrailRules.pausedClashDetail";
  rule: EffectiveRule;
  warning?: boolean;
}) {
  return (
    <p
      className={cn(
        "mt-1 flex items-start gap-1.5",
        warning && "text-[color:var(--status-warning-text)]",
      )}
    >
      <Lock className="mt-px h-3.5 w-3.5 flex-none" aria-hidden />
      <span>
        <Trans
          i18nKey={i18nKey}
          values={{ name: rule.name }}
          components={[<code key="name" className="font-mono" />]}
        />
      </span>
    </p>
  );
}

/** how many route names a card lists before it folds them behind a disclosure */
const INLINE_ROUTES = 3;

/**
 * The routes whose `advanced.guardrails.disable` switches the rule off (#2283).
 *
 * The card's status stays what it was: the rule still runs everywhere else, so
 * this is a line under it rather than a different badge. A few names are
 * listed in the open; more fold behind a disclosure that keeps the count.
 */
function OffRoutes({ routes }: { routes: string[] }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const summaryId = React.useId();
  if (routes.length === 0) return null;
  const count = routes.length;
  const summary = t("pages.guardrailRules.offRoutes.summary", { count, value: fmt.number(count) });
  const icon = <ShieldOff className="mt-px h-3.5 w-3.5 flex-none" aria-hidden />;
  const body = (
    <>
      <ul aria-labelledby={summaryId} className="flex flex-wrap gap-1">
        {routes.map((route) => (
          <li key={route} className="min-w-0 max-w-full">
            <Tag className="max-w-full" title={route}>
              <span className="truncate">{route}</span>
            </Tag>
          </li>
        ))}
      </ul>
      <p>
        <Trans
          i18nKey="pages.guardrailRules.offRoutes.source"
          count={count}
          components={[<code key="key" className="font-mono" />]}
        />
      </p>
    </>
  );
  if (count <= INLINE_ROUTES) {
    return (
      <div className="mt-1 flex items-start gap-1.5">
        <span className="text-[color:var(--status-warning-text)]">{icon}</span>
        <div className="min-w-0 flex-1 space-y-1.5">
          <p id={summaryId} className="text-[color:var(--status-warning-text)]">
            {summary}
          </p>
          {body}
        </div>
      </div>
    );
  }
  return (
    <details className="group mt-1">
      <summary className="flex w-fit cursor-pointer list-none items-start gap-1.5 rounded-sm text-[color:var(--status-warning-text)] hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring [&::-webkit-details-marker]:hidden">
        {icon}
        <span id={summaryId}>{summary}</span>
        <ChevronRight
          className="mt-px h-3.5 w-3.5 flex-none group-open:rotate-90 motion-safe:transition-transform"
          aria-hidden
        />
      </summary>
      <div className="mt-1.5 space-y-1.5 pl-5">{body}</div>
    </details>
  );
}

function StreamingEffect({
  id,
  mode,
  variant,
}: {
  id?: string;
  mode: StreamingMode;
  /** one line on the rule's card, or the full note under the dialog's stage */
  variant: "card" | "note";
}) {
  const text = (
    <Trans
      i18nKey={STREAMING_COPY[variant][mode]}
      components={[<code key="setting" className="inline-block font-mono" />]}
    />
  );
  const icon = <AlertTriangle className="mt-px h-3.5 w-3.5 flex-none" aria-hidden />;
  return variant === "card" ? (
    <p className="mt-1 flex items-start gap-1.5 text-[color:var(--status-warning-text)]">
      {icon}
      <span>{text}</span>
    </p>
  ) : (
    <p
      id={id}
      role="note"
      className="flex items-start gap-2 rounded-lg bg-[color:var(--status-warning)]/10 p-3 text-xs text-[color:var(--status-warning-text)]"
    >
      {icon}
      <span>{text}</span>
    </p>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(
  GuardrailRulesScreen,
  "errors.resources.guardrailRules",
  "guardrail_rule:read",
);
