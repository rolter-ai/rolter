import { useMutation } from "@tanstack/react-query";
import { Plus, X } from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

import { CopyButton } from "@/components/CopyButton";
import { useDiscardGuard } from "@/components/DiscardGuard";
import { ProjectScopeField, useMayWiden } from "@/components/ProjectScopeField";
import { Button } from "@/components/ui/button";
import { Combobox } from "@/components/ui/combobox";
import { Field } from "@/components/ui/field";
import { describedBy, FieldError } from "@/components/ui/field-error";
import { Input } from "@/components/ui/input";
import {
  Sheet,
  SheetActions,
  SheetBody,
  SheetError,
  SheetFooter,
  SheetHeader,
} from "@/components/ui/sheet";
import { Switch } from "@/components/ui/switch";
import {
  createProviderGroup,
  STRATEGIES,
  updateProviderGroup,
  type GroupMemberInput,
  type ProviderGroupRow,
  type ProviderRow,
} from "@/lib/api";
import { StrategyHint } from "@/components/StrategyHint";
import { providersUsableFrom, usableFrom } from "@/lib/provider-scope";
import { strategyOptions } from "@/lib/strategies";
import { errorDetail, useToast } from "@/lib/toast";
import { useFormTelemetry } from "@/lib/ux-react";

export type ProviderGroupSheetMode = "add" | "edit";

// one editable membership row: which provider, optional upstream-model rewrite,
// and a relative weight for weighted balancing
interface DraftMember {
  provider_id: string;
  upstream_model: string;
  weight: string;
}

function emptyMember(providers: ProviderRow[]): DraftMember {
  return { provider_id: providers[0]?.id ?? "", upstream_model: "", weight: "1" };
}

function toMemberInputs(members: DraftMember[]): GroupMemberInput[] {
  return members
    .filter((m) => m.provider_id)
    .map((m) => ({
      provider_id: m.provider_id,
      upstream_model: m.upstream_model.trim() || undefined,
      weight: Math.max(1, Number.parseInt(m.weight, 10) || 1),
    }));
}

interface GroupDraft {
  name: string;
  slug: string;
  strategy: string;
  members: DraftMember[];
  allowSlugChange: boolean;
  /** the project the group is scoped to; `""` is the whole organization */
  projectId: string;
}

function blankDraft(): GroupDraft {
  return {
    name: "",
    slug: "",
    strategy: STRATEGIES[0],
    members: [],
    allowSlugChange: false,
    projectId: "",
  };
}

function fromGroup(group: ProviderGroupRow): GroupDraft {
  return {
    name: group.name,
    slug: group.slug,
    strategy: group.strategy,
    allowSlugChange: false,
    projectId: group.project_id ?? "",
    members: group.members.map((m) => ({
      provider_id: m.provider_id,
      upstream_model: m.upstream_model ?? "",
      weight: String(m.weight),
    })),
  };
}

function MemberEditor({
  providers,
  known,
  owner,
  members,
  onChange,
}: {
  /** the providers this group's scope may use: the ones the picker offers */
  providers: ProviderRow[];
  /** every provider of the org, so a member the scope no longer allows still has a name */
  known: ProviderRow[];
  /** the project the group is scoped to, `null` when org-wide */
  owner: string | null;
  members: DraftMember[];
  onChange: (next: DraftMember[]) => void;
}) {
  const { t } = useTranslation();
  const update = (i: number, patch: Partial<DraftMember>) =>
    onChange(members.map((m, idx) => (idx === i ? { ...m, ...patch } : m)));
  const remove = (i: number) => onChange(members.filter((_, idx) => idx !== i));

  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between">
        <span className="text-sm font-medium leading-none">
          {t("providerGroupSheet.members.title")}
        </span>
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="h-7"
          disabled={providers.length === 0}
          onClick={() => onChange([...members, emptyMember(providers)])}
        >
          <Plus className="h-3.5 w-3.5" />
          {t("providerGroupSheet.members.add")}
        </Button>
      </div>
      {providers.length === 0 && (
        <p className="text-xs text-muted-foreground">
          {t("providerGroupSheet.members.noProviders")}
        </p>
      )}
      {members.length === 0 && providers.length > 0 && (
        <p className="text-xs text-muted-foreground">{t("providerGroupSheet.members.none")}</p>
      )}
      {members.length > 0 && (
        <div
          className="grid gap-2 text-[11px] uppercase tracking-[0.06em] text-[color:var(--text-subtle)]"
          style={{ gridTemplateColumns: "1.4fr 1.4fr 64px 28px" }}
        >
          <span>{t("common.provider")}</span>
          <span>{t("providerGroupSheet.members.upstreamModel")}</span>
          <span>{t("providerGroupSheet.members.weight")}</span>
          <span />
        </div>
      )}
      {members.map((m, i) => {
        // a member picked under another scope stays listed, and says so: the
        // control plane refuses a group whose scope its provider does not fit
        const current = known.find((p) => p.id === m.provider_id);
        const outside = !usableFrom(current, owner);
        const errorId = `group-member-${i}-scope`;
        return (
          <React.Fragment key={i}>
            <div
              className="grid items-center gap-2"
              style={{ gridTemplateColumns: "1.4fr 1.4fr 64px 28px" }}
            >
              <Combobox
                value={m.provider_id}
                aria-label={t("common.provider")}
                aria-invalid={outside || undefined}
                aria-describedby={describedBy(outside && errorId)}
                onChange={(provider_id) => update(i, { provider_id })}
                options={(outside && current ? [...providers, current] : providers).map((p) => ({
                  value: p.id,
                  label: p.name,
                }))}
              />
              <Input
                aria-label={t("providerGroupSheet.members.upstreamModel")}
                value={m.upstream_model}
                onChange={(e) => update(i, { upstream_model: e.target.value })}
                placeholder={t("providerGroupSheet.members.upstreamModelPlaceholder")}
                className="font-mono"
              />
              {/* the grid's column captions above are not `<label>`s, so each cell
              names itself — a `title` alone is a hidden label and nothing a
              screen reader announces reliably */}
              <Input
                aria-label={t("providerGroupSheet.members.relativeWeight")}
                type="number"
                min={1}
                value={m.weight}
                onChange={(e) => update(i, { weight: e.target.value })}
                title={t("providerGroupSheet.members.relativeWeight")}
              />
              <button
                type="button"
                title={t("providerGroupSheet.members.remove")}
                aria-label={t("providerGroupSheet.members.remove")}
                onClick={() => remove(i)}
                className="flex flex-none items-center justify-center rounded-[6px] border border-[color:var(--border-subtle)] p-1.5 text-[color:var(--text-secondary)] transition-colors hover:border-[color:var(--status-danger)] hover:text-[color:var(--status-danger-text)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <X className="h-3.5 w-3.5" />
              </button>
            </div>
            <FieldError
              id={errorId}
              error={
                outside
                  ? t(
                      owner
                        ? "providerGroupSheet.members.outOfScopeProject"
                        : "providerGroupSheet.members.outOfScopeOrg",
                      { name: current?.name ?? "" },
                    )
                  : undefined
              }
            />
          </React.Fragment>
        );
      })}
    </div>
  );
}

export interface ProviderGroupSheetProps {
  open: boolean;
  mode: ProviderGroupSheetMode;
  onOpenChange: (open: boolean) => void;
  orgId: string | null;
  providers: ProviderRow[];
  group?: ProviderGroupRow | null;
  /**
   * The project the dashboard is open on. Someone who may not make a group
   * org-wide has to name a project, and this is the one they start on (#1919).
   */
  defaultProjectId?: string | null;
  onDone: () => void;
}

export function ProviderGroupSheet({
  open,
  mode,
  onOpenChange,
  orgId,
  providers,
  group,
  defaultProjectId,
  onDone,
}: ProviderGroupSheetProps) {
  const [draft, setDraft] = React.useState<GroupDraft>(() => blankDraft());
  const initialRef = React.useRef("");

  // seed the draft once per open
  const seededRef = React.useRef(false);
  React.useEffect(() => {
    if (!open) {
      seededRef.current = false;
      return;
    }
    if (seededRef.current) return;
    seededRef.current = true;
    const d = mode === "edit" && group ? fromGroup(group) : blankDraft();
    setDraft(d);
    initialRef.current = JSON.stringify(d);
  }, [open, mode, group]);

  // a caller who may not make a group org-wide can only scope it to a project,
  // so the picker has no org option for them and their current project stands
  // in until they pick another
  const mayWiden = useMayWiden("provider_group", mode);
  const scopeValue =
    draft.projectId || (mode === "add" && !mayWiden ? (defaultProjectId ?? "") : "");
  // the providers a group of this scope may hold: a scoped group takes its
  // project's and org-wide ones, an org-wide group only org-wide ones
  const usable = React.useMemo(
    () => providersUsableFrom(providers, scopeValue || null),
    [providers, scopeValue],
  );

  const set = (patch: Partial<GroupDraft>) => setDraft((d) => ({ ...d, ...patch }));

  const dirty = initialRef.current !== "" && JSON.stringify(draft) !== initialRef.current;
  const { t } = useTranslation();
  const toast = useToast();

  // form lifecycle for the UX stream (#805). the target names the form and the
  // mode; nothing derived from what was typed into it
  const ux = useFormTelemetry(
    mode === "add" ? "provider-group-create" : "provider-group-edit",
    open,
    { dirty },
  );

  const save = useMutation({
    mutationFn: () => {
      const members = toMemberInputs(draft.members);
      if (mode === "add") {
        return createProviderGroup(orgId as string, {
          name: draft.name,
          slug: draft.slug.trim() || undefined,
          strategy: draft.strategy,
          members,
          project_id: scopeValue || undefined,
        });
      }
      const g = group!;
      const slugChanged = draft.allowSlugChange && draft.slug.trim() !== g.slug;
      return updateProviderGroup(g.id, {
        name: draft.name !== g.name ? draft.name : undefined,
        strategy: draft.strategy !== g.strategy ? draft.strategy : undefined,
        slug: slugChanged ? draft.slug.trim() : undefined,
        allow_slug_change: slugChanged ? true : undefined,
        members,
        // sent only when it moved, `null` being the word for org-wide again
        project_id: scopeValue !== (g.project_id ?? "") ? scopeValue || null : undefined,
      });
    },
    onSuccess: () => {
      ux.saved();
      // the sheet closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push(
        mode === "add"
          ? { tone: "success", title: t("toast.created", { what: draft.name }) }
          : {
              tone: "success",
              title: t("toast.saved"),
              detail: t("toast.savedDetail", { what: draft.name }),
            },
      );
      onDone();
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

  const title =
    mode === "add"
      ? t("providerGroupSheet.titleAdd")
      : t("providerGroupSheet.titleEdit", { name: group?.name ?? "" });
  const subtitle =
    mode === "add"
      ? t("providerGroupSheet.subtitleAdd")
      : `${draft.slug || "—"}/model · ${draft.strategy}`;
  const cta = mode === "add" ? t("providerGroupSheet.create") : t("providerGroupSheet.save");
  // the sheet's own dismissal paths (Escape, scrim, close, Cancel) all run
  // through the shared discard prompt (#1463)
  const { guard, close, locked, prompt } = useDiscardGuard({
    dirty,
    saving: save.isPending,
    onOpenChange,
  });

  const canSave =
    !!draft.name.trim() &&
    !save.isPending &&
    (mode === "add" ? !!orgId : true) &&
    // no org-wide option to fall back on: a project has to be named
    (mayWiden || scopeValue !== "");

  return (
    <Sheet open={open} onOpenChange={onOpenChange} onDismiss={guard}>
      <SheetHeader title={title} subtitle={subtitle} onClose={close} closeDisabled={locked} />
      <SheetBody>
        <p className="text-xs leading-snug text-muted-foreground">
          <Trans
            i18nKey="providerGroupSheet.lead"
            components={[<span key="address" className="font-mono text-foreground" />]}
          />
        </p>

        <Field label={t("providerGroupSheet.fields.name")}>
          <Input
            value={draft.name}
            onChange={(e) => set({ name: e.target.value })}
            placeholder="vllm-cluster"
          />
        </Field>

        {mode === "add" ? (
          <Field
            label={t("providerGroupSheet.fields.slugOptional")}
            hint={t("providerGroupSheet.fields.slugHintAdd")}
          >
            <Input
              value={draft.slug}
              onChange={(e) => set({ slug: e.target.value })}
              placeholder="vllm-cluster"
              className="font-mono"
            />
          </Field>
        ) : (
          <Field
            label={t("providerGroupSheet.fields.slug")}
            hint={
              draft.allowSlugChange
                ? t("providerGroupSheet.fields.slugHintUnlocked")
                : t("providerGroupSheet.fields.slugHintLocked")
            }
            // the child here is a row, not the control, so Field cannot find
            // the input to hang the id on — say which one the label means
            htmlFor="provider-group-slug"
          >
            <div className="flex items-center gap-2">
              <Input
                id="provider-group-slug"
                value={draft.slug}
                onChange={(e) => set({ slug: e.target.value })}
                readOnly={!draft.allowSlugChange}
                disabled={!draft.allowSlugChange}
                className="font-mono"
              />
              {group && !draft.allowSlugChange && (
                <CopyButton
                  value={`${group.slug}/`}
                  label={t("providerGroupSheet.fields.copyPrefix")}
                />
              )}
            </div>
            <div className="flex items-center gap-2 pt-1.5">
              <Switch
                checked={draft.allowSlugChange}
                aria-labelledby="provider-group-slug-toggle"
                onCheckedChange={(v) => set({ allowSlugChange: v })}
              />
              <span id="provider-group-slug-toggle" className="text-xs text-muted-foreground">
                {t("providerGroupSheet.fields.allowSlugChange")}
              </span>
            </div>
          </Field>
        )}

        <Field
          label={t("providerGroupSheet.fields.strategy")}
          hint={t("providerGroupSheet.fields.strategyHint")}
          // two children, so Field cannot tell which one the label means — the
          // hint below the select is the other one
          htmlFor="provider-group-strategy"
        >
          <Combobox
            id="provider-group-strategy"
            value={draft.strategy}
            onChange={(strategy) => set({ strategy })}
            options={strategyOptions(draft.strategy).map((s) => ({ value: s, label: s }))}
          />
          <StrategyHint strategy={draft.strategy} />
        </Field>

        <ProjectScopeField
          resource="provider_group"
          mode={mode}
          orgId={orgId}
          id="provider-group-scope"
          value={scopeValue}
          onChange={(projectId) => set({ projectId })}
          mayWiden={mayWiden}
        />

        <MemberEditor
          providers={usable}
          known={providers}
          owner={scopeValue || null}
          members={draft.members}
          onChange={(members) => set({ members })}
        />
      </SheetBody>

      <SheetFooter>
        <SheetError message={save.isError ? (save.error as Error).message : undefined} />
        <SheetActions>
          <Button variant="ghost" disabled={locked} onClick={close}>
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
        </SheetActions>
      </SheetFooter>
      {prompt}
    </Sheet>
  );
}
