import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, Loader2, Plus } from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { GuardrailEmpty, GuardrailLoading, PolicyCard } from "@/components/GuardrailPanel";
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
import { Input } from "@/components/ui/input";

import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import {
  createGuardrailRule,
  deleteGuardrailRule,
  fetchConfig,
  fetchGuardrailRules,
  updateGuardrailRule,
  type GatewayConfigDto,
  type GuardrailRuleInput,
  type GuardrailRuleRow,
} from "@/lib/api";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const EMPTY: GuardrailRuleInput = {
  name: "",
  enabled: true,
  source_type: "builtin",
  builtin: "email",
  pattern: null,
  stage: "pre_call",
  action: "redact",
  replacement: "[REDACTED:EMAIL]",
  include_system: false,
  position: 0,
};

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

function streamingMode(config: GatewayConfigDto | undefined): StreamingMode {
  const guardrails = config?.guardrails as { streaming_post_call?: unknown } | null | undefined;
  const mode = guardrails?.streaming_post_call;
  return mode === "reject" || mode === "passthrough" ? mode : "unknown";
}

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

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `query` is the query the user is actually waiting on for this screen
  useScreenReady(!query.isLoading);
  useErrorState(!!query.error, "guardrail-rules");
  // the same cache entry the Effective config and Models screens read.
  // undefined until it answers, so a note never says "could not be read"
  // about a request still in flight
  const config = useQuery({ queryKey: ["config"], queryFn: fetchConfig, retry: false });
  const streaming = config.isPending ? undefined : streamingMode(config.data);
  const [editing, setEditing] = React.useState<GuardrailRuleRow | null | undefined>();

  const save = useMutation({
    mutationFn: (body: GuardrailRuleInput) =>
      editing ? updateGuardrailRule(editing.id, body) : createGuardrailRule(body),
    onSuccess: (_result, body) => {
      void client.invalidateQueries({ queryKey: ["guardrail-rules"] });
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
    mutationFn: deleteGuardrailRule,
    onSuccess: () => void client.invalidateQueries({ queryKey: ["guardrail-rules"] }),
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

  const open = editing !== undefined;
  const rules = query.data ?? [];
  return (
    <div className="mx-auto flex max-w-[1120px] flex-col gap-5 p-[22px]">
      <div className="flex flex-col gap-3 border-b border-[color:var(--border-subtle)] pb-5 sm:flex-row sm:items-end sm:justify-between">
        <div>
          <p className="font-mono text-[0.6875rem] uppercase tracking-[0.16em] text-[color:var(--status-danger-text)]">
            {t("pages.guardrailRules.eyebrow")}
          </p>
          <h1 className="mt-1 text-xl font-semibold tracking-tight">
            {t("pages.guardrailRules.heading")}
          </h1>
          <p className="mt-1 max-w-2xl text-sm text-muted-foreground">
            {t("pages.guardrailRules.intro")}
          </p>
        </div>
        <GatedButton
          gate="guardrail_rule:create"
          control="guardrail-rule-new"
          onClick={() => setEditing(null)}
        >
          <Plus className="h-4 w-4" aria-hidden /> {t("pages.guardrailRules.addRule")}
        </GatedButton>
      </div>

      {query.isLoading ? (
        <GuardrailLoading />
      ) : query.isError ? (
        // never hand-rolled: a 403 is what a non-superadmin gets on this
        // deployment-scoped screen, and the bespoke panel offered it a retry
        // that could not ever work (#1259)
        <LoadError
          error={query.error}
          resource={t("errors.resources.guardrailRules")}
          onRetry={() => void query.refetch()}
        />
      ) : rules.length === 0 ? (
        <GuardrailEmpty
          title={t("pages.guardrailRules.emptyTitle")}
          description={t("pages.guardrailRules.emptyBody")}
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
        <div className="grid gap-3 md:grid-cols-2">
          {rules.map((rule) => (
            <PolicyCard
              key={rule.id}
              title={`${rule.position.toString().padStart(2, "0")} · ${rule.name}`}
              description={
                rule.source_type === "builtin"
                  ? builtinDescription(rule.builtin)
                  : (rule.pattern ?? t("pages.guardrailRules.customRegex"))
              }
              enabled={rule.enabled}
              badges={
                <>
                  <Badge
                    tone={
                      rule.action === "block"
                        ? "danger"
                        : rule.action === "redact"
                          ? "warning"
                          : "info"
                    }
                  >
                    {rule.action}
                  </Badge>
                  <Badge tone="outline">{rule.stage.replace("_", "-")}</Badge>
                  {rule.include_system && (
                    <Badge tone="accent">{t("pages.guardrailRules.systemBadge")}</Badge>
                  )}
                </>
              }
              details={
                <>
                  <p>
                    {rule.replacement
                      ? t("pages.guardrailRules.replacementDetail", {
                          token: rule.replacement,
                        })
                      : t("pages.guardrailRules.noRewrite")}
                  </p>
                  {rule.stage === "post_call" && streaming && (
                    <StreamingEffect mode={streaming} variant="card" />
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
                      <Loader2 className="mr-2 h-4 w-4 animate-spin" />
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
          ))}
        </div>
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
                detail: errorDetail(error),
              });
            },
          });
        }}
      />

      <RuleDialog
        key={editing?.id ?? (editing === null ? "new" : "closed")}
        open={open}
        initial={editing ?? null}
        streaming={streaming}
        pending={save.isPending}
        error={save.isError ? (save.error as Error).message : null}
        onClose={() => setEditing(undefined)}
        onSave={(body) => save.mutate(body)}
      />
    </div>
  );
}

function RuleDialog({
  open,
  initial,
  streaming,
  pending,
  error,
  onClose,
  onSave,
}: {
  open: boolean;
  initial: GuardrailRuleRow | null;
  streaming: StreamingMode | undefined;
  pending: boolean;
  error: string | null;
  onClose: () => void;
  onSave: (body: GuardrailRuleInput) => void;
}) {
  const { t } = useTranslation();
  const [form, setForm] = React.useState<GuardrailRuleInput>(initial ?? EMPTY);
  const set = (patch: Partial<GuardrailRuleInput>) => setForm((value) => ({ ...value, ...patch }));
  const valid =
    form.name.trim() !== "" && (form.source_type === "builtin" || Boolean(form.pattern?.trim()));
  // shown before saving, and read out with the stage it depends on
  const noteId = React.useId();
  const streamingNote = form.stage === "post_call" ? streaming : undefined;
  return (
    <Dialog open={open} onOpenChange={(value) => !value && onClose()}>
      <DialogHeader>
        <DialogTitle>
          {initial
            ? t("pages.guardrailRules.dialogEditTitle")
            : t("pages.guardrailRules.dialogAddTitle")}
        </DialogTitle>
        <DialogDescription>{t("pages.guardrailRules.dialogBody")}</DialogDescription>
      </DialogHeader>
      <DialogBody className="space-y-4">
        <Field label={t("pages.guardrailRules.fieldName")} htmlFor="rule-name">
          <Input
            id="rule-name"
            value={form.name}
            onChange={(event) => set({ name: event.target.value })}
          />
        </Field>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <Field label={t("pages.guardrailRules.fieldSource")} htmlFor="rule-source">
            <Combobox
              id="rule-source"
              value={form.source_type}
              onChange={(picked) =>
                set({
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
          <Field label={t("pages.guardrailRules.fieldDetector")} htmlFor="rule-builtin">
            <Combobox
              id="rule-builtin"
              value={form.builtin ?? "email"}
              onChange={(picked) =>
                set({
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
          <Field label={t("pages.guardrailRules.fieldStage")} htmlFor="rule-stage">
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
          <Field label={t("pages.guardrailRules.fieldAction")} htmlFor="rule-action">
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
          <Field label={t("pages.guardrailRules.fieldReplacement")} htmlFor="rule-replacement">
            <Input
              id="rule-replacement"
              value={form.replacement ?? ""}
              onChange={(event) => set({ replacement: event.target.value || null })}
            />
          </Field>
        )}
        <ToggleRow
          label={t("pages.guardrailRules.toggleEnabled")}
          description={t("pages.guardrailRules.toggleEnabledHint")}
          checked={form.enabled}
          onChange={(enabled) => set({ enabled })}
        />
        <ToggleRow
          label={t("pages.guardrailRules.toggleSystem")}
          description={t("pages.guardrailRules.toggleSystemHint")}
          checked={form.include_system}
          onChange={(include_system) => set({ include_system })}
        />
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
          {pending ? t("pages.guardrailRules.publishing") : t("pages.guardrailRules.publish")}
        </Button>
      </DialogFooter>
    </Dialog>
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

function ToggleRow({
  label,
  description,
  checked,
  onChange,
}: {
  label: string;
  description: string;
  checked: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <div className="flex items-start justify-between gap-4 rounded-lg border border-[color:var(--border-subtle)] p-3">
      <div>
        <p className="text-sm font-medium">{label}</p>
        <p className="mt-0.5 text-xs text-muted-foreground">{description}</p>
      </div>
      <Switch checked={checked} aria-label={label} onCheckedChange={onChange} />
    </div>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(GuardrailRulesScreen, "errors.resources.guardrailRules");
