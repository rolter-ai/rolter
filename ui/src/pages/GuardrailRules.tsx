import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, Plus } from "lucide-react";
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
import { Textarea } from "@/components/ui/textarea";
import {
  createGuardrailRule,
  deleteGuardrailRule,
  fetchGuardrailRules,
  updateGuardrailRule,
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
  const [editing, setEditing] = React.useState<
    GuardrailRuleRow | null | undefined
  >();

  const save = useMutation({
    mutationFn: (body: GuardrailRuleInput) =>
      editing
        ? updateGuardrailRule(editing.id, body)
        : createGuardrailRule(body),
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
    onSuccess: () =>
      void client.invalidateQueries({ queryKey: ["guardrail-rules"] }),
  });

  // was a bare window.confirm: unstyled, untranslatable, and invisible to the
  // story runner, which is the one place this path is ever exercised (#1179)
  const [deleteTarget, setDeleteTarget] = React.useState<GuardrailRuleRow | null>(
    null,
  );
  const startDelete = (rule: GuardrailRuleRow) => {
    remove.reset();
    setDeleteTarget(rule);
  };

  const open = editing !== undefined;
  const rules = query.data ?? [];
  return (
    <div className="mx-auto flex max-w-[1120px] flex-col gap-5 p-[22px]">
      <div className="flex flex-col gap-3 border-b border-[color:var(--border-subtle)] pb-5 sm:flex-row sm:items-end sm:justify-between">
        <div>
          <p className="font-mono text-[0.6875rem] uppercase tracking-[0.16em] text-[color:var(--status-danger-text)]">
            Ordered policy stack
          </p>
          <h1 className="mt-1 text-xl font-semibold tracking-tight">
            Traffic inspection rules
          </h1>
          <p className="mt-1 max-w-2xl text-sm text-muted-foreground">
            Rules run from the lowest position upward. Paused rules stay visible
            but never inspect traffic.
          </p>
        </div>
        <GatedButton gate="guardrail_rule:create" onClick={() => setEditing(null)}>
          <Plus className="h-4 w-4" aria-hidden /> Add rule
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
          title="No inspection rules"
          description="Add a built-in detector or a bounded custom regex before enabling the deployment guardrails flag."
          action={
            <GatedButton gate="guardrail_rule:create" onClick={() => setEditing(null)}>Add first rule</GatedButton>
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
                  ? `Built-in ${rule.builtin?.replace(/_/g, " ")} detector`
                  : (rule.pattern ?? "Custom regular expression")
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
                    <Badge tone="accent">system messages</Badge>
                  )}
                </>
              }
              details={
                rule.replacement
                  ? `Replacement · ${rule.replacement}`
                  : "Content is not rewritten"
              }
              actions={
                <>
                  <GatedButton
                    gate="guardrail_rule:delete"
                    variant="ghost"
                    aria-label={t("pages.guardrailRules.deleteAria", { name: rule.name })}
                    onClick={() => startDelete(rule)}
                    disabled={remove.isPending && remove.variables === rule.id}
                  >
                    {remove.isPending && remove.variables === rule.id && (
                      <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                    )}
                    Delete
                  </GatedButton>
                  <GatedButton
                    gate="guardrail_rule:update"
                    variant="outline"
                    aria-label={t("pages.guardrailRules.editAria", { name: rule.name })}
                    onClick={() => setEditing(rule)}
                  >
                    Edit rule
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
  pending,
  error,
  onClose,
  onSave,
}: {
  open: boolean;
  initial: GuardrailRuleRow | null;
  pending: boolean;
  error: string | null;
  onClose: () => void;
  onSave: (body: GuardrailRuleInput) => void;
}) {
  const [form, setForm] = React.useState<GuardrailRuleInput>(initial ?? EMPTY);
  const set = (patch: Partial<GuardrailRuleInput>) =>
    setForm((value) => ({ ...value, ...patch }));
  const valid =
    form.name.trim() !== "" &&
    (form.source_type === "builtin" || Boolean(form.pattern?.trim()));
  return (
    <Dialog open={open} onOpenChange={(value) => !value && onClose()}>
      <DialogHeader>
        <DialogTitle>
          {initial ? "Edit inspection rule" : "Add inspection rule"}
        </DialogTitle>
        <DialogDescription>
          Use a built-in detector or one bounded regular expression. The gateway
          validates it before publishing.
        </DialogDescription>
      </DialogHeader>
      <div className="max-h-[65vh] space-y-4 overflow-y-auto pr-1">
        <Field label="Rule name" htmlFor="rule-name">
          <Input
            id="rule-name"
            value={form.name}
            onChange={(event) => set({ name: event.target.value })}
          />
        </Field>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <Field label="Source" htmlFor="rule-source">
            <Select
              id="rule-source"
              value={form.source_type}
              onChange={(event) =>
                set({
                  source_type: event.target
                    .value as GuardrailRuleInput["source_type"],
                  builtin: event.target.value === "builtin" ? "email" : null,
                  pattern: event.target.value === "pattern" ? "" : null,
                })
              }
            >
              <option value="builtin">Built-in detector</option>
              <option value="pattern">Custom regex</option>
            </Select>
          </Field>
          <Field
            label="Position"
            htmlFor="rule-position"
            hint="Lower positions run first."
          >
            <Input
              id="rule-position"
              type="number"
              min={0}
              value={form.position}
              onChange={(event) =>
                set({ position: Number(event.target.value) })
              }
            />
          </Field>
        </div>
        {form.source_type === "builtin" ? (
          <Field label="Detector" htmlFor="rule-builtin">
            <Select
              id="rule-builtin"
              value={form.builtin ?? "email"}
              onChange={(event) =>
                set({
                  builtin: event.target.value as GuardrailRuleInput["builtin"],
                })
              }
            >
              <option value="email">Email address</option>
              <option value="phone">Phone number</option>
              <option value="api_token">API token</option>
              <option value="payment_card">Payment card</option>
            </Select>
          </Field>
        ) : (
          <Field
            label="Regular expression"
            htmlFor="rule-pattern"
            hint="Uses the gateway’s linear-time regex engine."
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
          <Field label="Stage" htmlFor="rule-stage">
            <Select
              id="rule-stage"
              value={form.stage}
              onChange={(event) =>
                set({
                  stage: event.target.value as GuardrailRuleInput["stage"],
                })
              }
            >
              <option value="pre_call">Before upstream</option>
              <option value="post_call">Before response</option>
            </Select>
          </Field>
          <Field label="Action" htmlFor="rule-action">
            <Select
              id="rule-action"
              value={form.action}
              onChange={(event) =>
                set({
                  action: event.target.value as GuardrailRuleInput["action"],
                })
              }
            >
              <option value="annotate">Annotate only</option>
              <option value="block">Block traffic</option>
              <option value="redact">Redact matches</option>
            </Select>
          </Field>
        </div>
        {form.action === "redact" && (
          <Field label="Replacement token" htmlFor="rule-replacement">
            <Input
              id="rule-replacement"
              value={form.replacement ?? ""}
              onChange={(event) =>
                set({ replacement: event.target.value || null })
              }
            />
          </Field>
        )}
        <ToggleRow
          label="Rule enforced"
          description="Paused rules remain stored and ordered."
          checked={form.enabled}
          onChange={(enabled) => set({ enabled })}
        />
        <ToggleRow
          label="Inspect system messages"
          description="Off by default because operator-authored instructions are trusted."
          checked={form.include_system}
          onChange={(include_system) => set({ include_system })}
        />
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
          {pending ? "Publishing…" : "Publish rule"}
        </Button>
      </DialogFooter>
    </Dialog>
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
