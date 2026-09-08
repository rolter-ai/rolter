import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowDown,
  ArrowUp,
  Braces,
  Check,
  ChevronRight,
  Clock3,
  FilePlus2,
  GitBranch,
  Pencil,
  Plus,
  RotateCcw,
  Trash2,
} from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

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
import { EmptyState } from "@/components/ui/empty-state";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { Textarea } from "@/components/ui/textarea";
import {
  createPromptTemplate,
  createPromptTemplateVersion,
  deletePromptTemplate,
  fetchPromptTemplateScopes,
  fetchPromptTemplates,
  fetchPromptTemplateVersions,
  fetchRoutes,
  fetchVirtualKeys,
  publishPromptTemplateVersion,
  rollbackPromptTemplateVersion,
  setPromptTemplateScopes,
  updatePromptTemplate,
  type PromptTemplateDecorator,
  type PromptTemplateRow,
  type PromptTemplateScopeInput,
  type PromptTemplateVariable,
  type PromptTemplateVersionRow,
} from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

interface Draft {
  variables: PromptTemplateVariable[];
  decorators: PromptTemplateDecorator[];
  scopes: PromptTemplateScopeInput[];
}

const EMPTY_DRAFT: Draft = {
  variables: [],
  decorators: [{ role: "system", position: "prepend", content: "" }],
  scopes: [],
};

function copyDraft(version?: PromptTemplateVersionRow, scopes: PromptTemplateScopeInput[] = []): Draft {
  if (!version) return structuredClone(EMPTY_DRAFT);
  return {
    variables: version.variables.map((variable) => ({ ...variable })),
    decorators: version.decorators.map((decorator) => ({ ...decorator })),
    scopes: scopes.map((scope) => ({ ...scope })),
  };
}

/** the `{{ name }}` syntax the decorators use, kept out of the catalogs so
 * i18next does not read it as an interpolation placeholder of its own */
const VARIABLE_SAMPLE = "{{ variable_name }}";
const CUSTOMER_SAMPLE = "{{ customer_name }}";

/** a catalog key plus its interpolation, rather than an English sentence built
 * inside a helper that has no `t` (#1092) */
type Problem = { key: string; name?: string };

function draftProblem(draft: Draft): Problem | undefined {
  if (draft.decorators.length === 0) return { key: "problemNoDecorator" };
  const names = new Set<string>();
  for (const variable of draft.variables) {
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(variable.name)) {
      return { key: "problemInvalidName", name: variable.name };
    }
    if (names.has(variable.name)) return { key: "problemDuplicate", name: variable.name };
    if (variable.required && variable.default !== undefined) {
      return { key: "problemRequiredDefault", name: variable.name };
    }
    names.add(variable.name);
  }
  for (const decorator of draft.decorators) {
    if (!decorator.content.trim()) return { key: "problemEmptyDecorator" };
    for (const match of decorator.content.matchAll(/{{\s*([A-Za-z_][A-Za-z0-9_]*)\s*}}/g)) {
      if (!names.has(match[1])) return { key: "problemUndeclared", name: match[1] };
    }
  }
  return undefined;
}

export default function PromptRepository() {
  const scope = useScope();
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  const toast = useToast();
  // the scope hook names a catalog key rather than carrying english copy
  const scopeMessage = scope.errorKey ? t(scope.errorKey) : undefined;
  const [selectedId, setSelectedId] = React.useState<string>();
  const [selectedVersion, setSelectedVersion] = React.useState<number>();
  const [draft, setDraft] = React.useState<Draft>(() => copyDraft());
  const [samples, setSamples] = React.useState<Record<string, string>>({});
  const [createOpen, setCreateOpen] = React.useState(false);
  const [renameOpen, setRenameOpen] = React.useState(false);
  const [deleteOpen, setDeleteOpen] = React.useState(false);
  const [rollbackVersion, setRollbackVersion] = React.useState<number>();

  const templates = useQuery({
    queryKey: ["prompt-templates", scope.orgId],
    queryFn: () => fetchPromptTemplates(scope.orgId as string),
    enabled: !!scope.orgId,
  });


  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;

  // `templates` is the query the user is actually waiting on for this screen

  useScreenReady(!templates.isLoading);

  useErrorState(!!templates.error, "prompt-repository");
  const selected = templates.data?.find((template) => template.id === selectedId);

  React.useEffect(() => {
    if (!templates.data?.length) {
      setSelectedId(undefined);
      return;
    }
    if (!selectedId || !templates.data.some((template) => template.id === selectedId)) {
      setSelectedId(templates.data[0].id);
    }
  }, [selectedId, templates.data]);

  const versions = useQuery({
    queryKey: ["prompt-template-versions", selectedId],
    queryFn: () => fetchPromptTemplateVersions(selectedId as string),
    enabled: !!selectedId,
  });
  const orderedVersions = React.useMemo(
    () => [...(versions.data ?? [])].sort((a, b) => b.version - a.version),
    [versions.data],
  );

  React.useEffect(() => {
    if (!orderedVersions.length) {
      setSelectedVersion(undefined);
      return;
    }
    if (!selectedVersion || !orderedVersions.some((version) => version.version === selectedVersion)) {
      setSelectedVersion(orderedVersions[0].version);
    }
  }, [orderedVersions, selectedVersion]);

  const baseVersion = orderedVersions.find((version) => version.version === selectedVersion);
  const scopes = useQuery({
    queryKey: ["prompt-template-scopes", selectedId, selectedVersion],
    queryFn: () => fetchPromptTemplateScopes(selectedId as string, selectedVersion as number),
    enabled: !!selectedId && selectedVersion !== undefined,
  });

  React.useEffect(() => {
    if (versions.isSuccess && (!selectedVersion || baseVersion) && (!selectedVersion || scopes.isSuccess)) {
      setDraft(copyDraft(baseVersion, scopes.data ?? []));
      setSamples({});
    }
  }, [baseVersion, scopes.data, scopes.isSuccess, selectedId, selectedVersion, versions.isSuccess]);

  const routes = useQuery({
    queryKey: ["routes", scope.projectId],
    queryFn: () => fetchRoutes(scope.projectId as string),
    enabled: !!scope.projectId,
  });
  const keys = useQuery({
    queryKey: ["virtual-keys", scope.projectId],
    queryFn: () => fetchVirtualKeys(scope.projectId as string),
    enabled: !!scope.projectId,
  });

  const create = useMutation({
    mutationFn: (input: { name: string; slug?: string; description?: string }) =>
      createPromptTemplate(scope.orgId as string, input),
    onSuccess: (template) => {
      queryClient.setQueryData<PromptTemplateRow[]>(
        ["prompt-templates", scope.orgId],
        (current = []) => [...current, template],
      );
      setSelectedId(template.id);
      setCreateOpen(false);
      toast.push({ tone: "success", title: t("pages.promptRepo.created") });
    },
    onError: (error, input) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: input.name }),
        detail: errorDetail(error),
      });
    },
  });

  const saveDraft = useMutation({
    mutationFn: async () => {
      const version = await createPromptTemplateVersion(selectedId as string, {
        variables: draft.variables.map(({ default: defaultValue, ...variable }) => ({
          ...variable,
          ...(defaultValue === undefined || defaultValue === "" ? {} : { default: defaultValue }),
        })),
        decorators: draft.decorators,
      });
      try {
        await setPromptTemplateScopes(selectedId as string, version.version, draft.scopes);
        return { version };
      } catch (scopeError) {
        // version creation is intentionally append-only and cannot be rolled
        // back; surface the partial result so a retry never creates a duplicate
        return { version, scopeError: scopeError as Error };
      }
    },
    onSuccess: async ({ version, scopeError }) => {
      await queryClient.invalidateQueries({ queryKey: ["prompt-template-versions", selectedId] });
      await queryClient.invalidateQueries({
        queryKey: ["prompt-template-scopes", selectedId, version.version],
      });
      setSelectedVersion(version.version);
      // a version that saved but could not take its scopes is not a clean
      // success: it is announced assertively so it is read, not glanced at
      toast.push(
        scopeError
          ? {
              tone: "error",
              title: t("pages.promptRepo.draftSavedScopeError", {
                version: version.version,
                error: scopeError.message,
              }),
            }
          : {
              tone: "success",
              title: t("pages.promptRepo.draftSaved", { version: version.version }),
            },
      );
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: selected?.name ?? "" }),
        detail: errorDetail(error),
      });
    },
  });

  const rename = useMutation({
    mutationFn: (input: { name?: string; description?: string }) =>
      updatePromptTemplate(selectedId as string, input),
    onSuccess: (template) => {
      queryClient.setQueryData<PromptTemplateRow[]>(
        ["prompt-templates", scope.orgId],
        (current = []) => current.map((item) => (item.id === template.id ? template : item)),
      );
      setRenameOpen(false);
      toast.push({ tone: "success", title: t("pages.promptRepo.renamed") });
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: selected?.name ?? "" }),
        detail: errorDetail(error),
      });
    },
  });

  const remove = useMutation({
    mutationFn: () => deletePromptTemplate(selectedId as string),
    onSuccess: async () => {
      const removedId = selectedId;
      const what = selected?.name ?? "";
      // drop the selection before the list refetches so the workbench never
      // renders against a template the control plane no longer has
      setSelectedId(undefined);
      setSelectedVersion(undefined);
      setDeleteOpen(false);
      queryClient.setQueryData<PromptTemplateRow[]>(
        ["prompt-templates", scope.orgId],
        (current = []) => current.filter((item) => item.id !== removedId),
      );
      await queryClient.invalidateQueries({ queryKey: ["prompt-templates", scope.orgId] });
      toast.push({ tone: "success", title: t("toast.deleted", { what }) });
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.deleteFailed", { what: selected?.name ?? "" }),
        detail: errorDetail(error),
      });
    },
  });

  const publish = useMutation({
    mutationFn: (version: number) => publishPromptTemplateVersion(selectedId as string, version),
    onSuccess: (template) => {
      queryClient.setQueryData<PromptTemplateRow[]>(
        ["prompt-templates", scope.orgId],
        (current = []) => current.map((item) => (item.id === template.id ? template : item)),
      );
      toast.push({
        tone: "success",
        title: t("pages.promptRepo.publishedNotice", { version: template.published_version }),
      });
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: selected?.name ?? "" }),
        detail: errorDetail(error),
      });
    },
  });

  const rollback = useMutation({
    mutationFn: (version: number) => rollbackPromptTemplateVersion(selectedId as string, version),
    onSuccess: (template) => {
      queryClient.setQueryData<PromptTemplateRow[]>(
        ["prompt-templates", scope.orgId],
        (current = []) => current.map((item) => (item.id === template.id ? template : item)),
      );
      setRollbackVersion(undefined);
      toast.push({
        tone: "success",
        title: t("pages.promptRepo.rolledBack", { version: template.published_version }),
      });
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: selected?.name ?? "" }),
        detail: errorDetail(error),
      });
    },
  });

  if (scope.isLoading || templates.isLoading) return <LoadingState />;
  if (scope.errorKey || templates.isError) {
    return (
      <EmptyState uxTarget="prompt-list"
        icon={<GitBranch />}
        title={t("pages.promptRepo.unavailableTitle")}
        description={scopeMessage ?? (templates.error as Error).message}
        actions={<Button variant="outline" onClick={() => templates.refetch()}>{t("pages.promptRepo.tryAgain")}</Button>}
      />
    );
  }

  return (
    <div className="h-full min-h-0 overflow-y-auto p-4 sm:p-5">
      <div className="mx-auto grid min-h-full max-w-[1500px] gap-4 lg:grid-cols-[15rem_minmax(0,1fr)] xl:grid-cols-[14rem_minmax(0,1fr)_15rem] 2xl:grid-cols-[15rem_minmax(0,1fr)_17rem]">
        <TemplateIndex
          templates={templates.data ?? []}
          selectedId={selectedId}
          onSelect={(id) => {
            setSelectedId(id);
            setSelectedVersion(undefined);
          }}
          onCreate={() => setCreateOpen(true)}
        />

        {!selected ? (
          <main className="flex min-h-[32rem] items-center justify-center rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]">
            <EmptyState uxTarget="prompt-versions"
              icon={<FilePlus2 />}
              title={t("pages.promptRepo.emptyTitle")}
              description={t("pages.promptRepo.emptyDescription")}
              actions={<GatedButton gate="prompt_template:create" onClick={() => setCreateOpen(true)}>{t("pages.promptRepo.createTemplate")}</GatedButton>}
            />
          </main>
        ) : (
          <PromptWorkbench
            template={selected}
            baseVersion={baseVersion}
            draft={draft}
            samples={samples}
            routes={routes.data ?? []}
            virtualKeys={keys.data ?? []}
            orgId={scope.orgId}
            projectId={scope.projectId}
            pending={saveDraft.isPending || publish.isPending}
            error={(saveDraft.error ?? publish.error) as Error | null}
            onDraftChange={setDraft}
            onSamplesChange={setSamples}
            onSave={() => saveDraft.mutate()}
            onPublish={(version) => publish.mutate(version)}
            onRename={() => setRenameOpen(true)}
            onDelete={() => setDeleteOpen(true)}
          />
        )}

        {selected && (
          <VersionRail
            className="lg:col-start-2 xl:col-start-3 xl:row-start-1"
            template={selected}
            versions={orderedVersions}
            selectedVersion={selectedVersion}
            loading={versions.isLoading}
            onSelect={setSelectedVersion}
            onRollback={setRollbackVersion}
          />
        )}
      </div>

      <CreateTemplateDialog
        open={createOpen}
        pending={create.isPending}
        error={create.error as Error | null}
        onOpenChange={setCreateOpen}
        onSubmit={(input) => create.mutate(input)}
      />
      <RollbackDialog
        version={rollbackVersion}
        publishedVersion={selected?.published_version}
        pending={rollback.isPending}
        error={rollback.error as Error | null}
        onClose={() => setRollbackVersion(undefined)}
        onConfirm={() => rollbackVersion && rollback.mutate(rollbackVersion)}
      />
      <RenameTemplateDialog
        open={renameOpen && !!selected}
        template={selected}
        pending={rename.isPending}
        error={rename.error as Error | null}
        onOpenChange={setRenameOpen}
        onSubmit={(input) => rename.mutate(input)}
      />
      <DeleteTemplateDialog
        open={deleteOpen && !!selected}
        template={selected}
        pending={remove.isPending}
        error={remove.error as Error | null}
        onOpenChange={setDeleteOpen}
        onConfirm={() => remove.mutate()}
      />
    </div>
  );
}

function LoadingState() {
  return (
    <div className="grid gap-4 p-5 lg:grid-cols-[15rem_minmax(0,1fr)_17rem]">
      <Skeleton width="100%" height={460} radius={12} />
      <Skeleton width="100%" height={620} radius={12} />
      <Skeleton width="100%" height={460} radius={12} />
    </div>
  );
}

function TemplateIndex({
  templates,
  selectedId,
  onSelect,
  onCreate,
}: {
  templates: PromptTemplateRow[];
  selectedId?: string;
  onSelect: (id: string) => void;
  onCreate: () => void;
}) {
  const { t } = useTranslation();
  return (
    <aside aria-label={t("pages.promptRepo.templates")} className="overflow-hidden rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]">
      <div className="flex items-center justify-between border-b border-[color:var(--border-subtle)] px-3 py-2.5">
        <div>
          <p className="text-xs font-semibold uppercase tracking-[0.12em] text-[color:var(--text-subtle)]">{t("pages.promptRepo.templates")}</p>
          <p className="mt-0.5 text-xs text-muted-foreground">{t("pages.promptRepo.inThisOrg", { count: templates.length })}</p>
        </div>
        <GatedButton gate="prompt_template:create" variant="ghost" onClick={onCreate} aria-label={t("pages.promptRepo.createTemplate")}>
          <Plus className="h-4 w-4" />
        </GatedButton>
      </div>
      <div className="max-h-[26rem] overflow-y-auto p-1.5 lg:max-h-[calc(100vh-14rem)]">
        {templates.length === 0 ? (
          <p className="px-2 py-5 text-center text-xs text-muted-foreground">{t("pages.promptRepo.noTemplates")}</p>
        ) : (
          templates.map((template) => (
            <button
              key={template.id}
              type="button"
              aria-current={selectedId === template.id ? "page" : undefined}
              onClick={() => onSelect(template.id)}
              className={cn(
                "group flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
                selectedId === template.id
                  ? "bg-[color:var(--surface-selected)] text-foreground"
                  : "text-[color:var(--text-secondary)] hover:bg-[color:var(--surface-hover)] hover:text-foreground",
              )}
            >
              <span className="min-w-0 flex-1">
                <span className="block truncate text-sm font-medium">{template.name}</span>
                <span className="mt-0.5 block truncate font-mono text-[0.6875rem] text-[color:var(--text-subtle)]">{template.slug}</span>
              </span>
              {template.published_version ? (
                <span className="text-[0.6875rem] tabular-nums text-[color:var(--status-success-text)]">v{template.published_version}</span>
              ) : (
                <span className="text-[0.6875rem] text-[color:var(--text-subtle)]">{t("pages.promptRepo.draftBadge")}</span>
              )}
              <ChevronRight className="h-3.5 w-3.5 opacity-0 transition-opacity group-hover:opacity-100" />
            </button>
          ))
        )}
      </div>
    </aside>
  );
}

function PromptWorkbench({
  template,
  baseVersion,
  draft,
  samples,
  routes,
  virtualKeys,
  orgId,
  projectId,
  pending,
  error,
  onDraftChange,
  onSamplesChange,
  onSave,
  onPublish,
  onRename,
  onDelete,
}: {
  template: PromptTemplateRow;
  baseVersion?: PromptTemplateVersionRow;
  draft: Draft;
  samples: Record<string, string>;
  routes: { id: string; model: string }[];
  virtualKeys: { id: string; name?: string | null; key_prefix: string }[];
  orgId?: string;
  projectId?: string;
  pending: boolean;
  error: Error | null;
  onDraftChange: (draft: Draft) => void;
  onSamplesChange: (samples: Record<string, string>) => void;
  onSave: () => void;
  onPublish: (version: number) => void;
  onRename: () => void;
  onDelete: () => void;
}) {
  const { t } = useTranslation();
  const problem = draftProblem(draft);
  const problemText = problem && t(`pages.promptRepo.${problem.key}`, { name: problem.name || t("pages.promptRepo.problemUnnamed") });
  const selectedPublished = baseVersion?.version === template.published_version;
  // publishing, rolling back, saving a version and renaming are one guard in
  // crates/rolter-control/src/crud.rs — `prompt_template:update` — so the
  // header gates on that and reserves `prompt_template:delete` for the delete
  return (
    <main className="min-w-0 overflow-hidden rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]">
      <header className="border-b border-[color:var(--border-subtle)] px-4 py-3 sm:px-5">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <h2 className="truncate text-lg font-semibold tracking-[-0.02em]">{template.name}</h2>
              {template.published_version ? <Badge tone="success" dot>{t("pages.promptRepo.liveBadge", { version: template.published_version })}</Badge> : <Badge tone="warning">{t("pages.promptRepo.unpublished")}</Badge>}
            </div>
            <p className="mt-1 max-w-2xl text-sm text-muted-foreground">{template.description || t("pages.promptRepo.noDescription")}</p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            {baseVersion && !selectedPublished && (
              <GatedButton gate="prompt_template:update" variant="outline" disabled={pending} onClick={() => onPublish(baseVersion.version)}>
                <Check className="h-4 w-4" /> {t("pages.promptRepo.publishVersion", { version: baseVersion.version })}
              </GatedButton>
            )}
            <GatedButton gate="prompt_template:update" disabled={pending || !!problem} onClick={onSave}>
              <FilePlus2 className="h-4 w-4" /> {pending ? t("pages.promptRepo.saving") : t("pages.promptRepo.saveNewDraft")}
            </GatedButton>
            <GatedButton gate="prompt_template:update" variant="ghost" aria-label={t("pages.promptRepo.renameAction", { name: template.name })} onClick={onRename}>
              <Pencil className="h-4 w-4" />
            </GatedButton>
            <GatedButton gate="prompt_template:delete" variant="ghost" aria-label={t("pages.promptRepo.deleteAction", { name: template.name })} onClick={onDelete}>
              <Trash2 className="h-4 w-4" />
            </GatedButton>
          </div>
        </div>
        <div className="mt-3 flex min-h-5 flex-wrap items-center gap-x-3 gap-y-1 text-xs">
          <span className="font-mono text-[color:var(--text-subtle)]">{template.slug}</span>
          {baseVersion && <span className="text-muted-foreground">{t("pages.promptRepo.editingFrom", { version: baseVersion.version })}</span>}
          {problemText && <span role="alert" className="text-[color:var(--status-danger-text)]">{problemText}</span>}
          {error && <span role="alert" className="text-[color:var(--status-danger-text)]">{error.message}</span>}
        </div>
      </header>

      <div className="grid min-w-0 gap-0 2xl:grid-cols-[minmax(0,1fr)_minmax(18rem,0.78fr)]">
        <div className="min-w-0 space-y-7 p-4 sm:p-5">
          <VariableEditor
            variables={draft.variables}
            onChange={(variables) => onDraftChange({ ...draft, variables })}
          />
          <DecoratorEditor
            decorators={draft.decorators}
            onChange={(decorators) => onDraftChange({ ...draft, decorators })}
          />
          <ScopeEditor
            scopes={draft.scopes}
            orgId={orgId}
            projectId={projectId}
            routes={routes}
            virtualKeys={virtualKeys}
            onChange={(scopes) => onDraftChange({ ...draft, scopes })}
          />
        </div>
        <PreviewPanel
          variables={draft.variables}
          decorators={draft.decorators}
          samples={samples}
          onSamplesChange={onSamplesChange}
        />
      </div>
    </main>
  );
}

function SectionHeading({ eyebrow, title, description, action }: { eyebrow: string; title: string; description: string; action?: React.ReactNode }) {
  return (
    <div className="mb-3 flex items-start justify-between gap-3">
      <div>
        <p className="text-[0.6875rem] font-semibold uppercase tracking-[0.14em] text-[color:var(--red-folk-text)]">{eyebrow}</p>
        <h3 className="mt-1 text-sm font-semibold">{title}</h3>
        <p className="mt-0.5 text-xs leading-5 text-muted-foreground">{description}</p>
      </div>
      {action}
    </div>
  );
}

function VariableEditor({ variables, onChange }: { variables: PromptTemplateVariable[]; onChange: (variables: PromptTemplateVariable[]) => void }) {
  const { t } = useTranslation();
  const update = (index: number, patch: Partial<PromptTemplateVariable>) => onChange(variables.map((variable, i) => (i === index ? { ...variable, ...patch } : variable)));
  return (
    <section aria-labelledby="variables-heading">
      <SectionHeading eyebrow={t("pages.promptRepo.inputsEyebrow")} title={t("pages.promptRepo.variablesTitle")} description={t("pages.promptRepo.variablesDescription", { sample: VARIABLE_SAMPLE })} action={<Button variant="outline" onClick={() => onChange([...variables, { name: "", required: true }])}><Plus className="h-4 w-4" /> {t("pages.promptRepo.addVariable")}</Button>} />
      <h3 id="variables-heading" className="sr-only">{t("pages.promptRepo.variablesTitle")}</h3>
      <div className="space-y-2">
        {variables.length === 0 ? <p className="rounded-lg border border-dashed border-[color:var(--border-default)] p-3 text-xs text-muted-foreground">{t("pages.promptRepo.noVariables")}</p> : variables.map((variable, index) => (
          <div key={index} className="grid gap-2 rounded-lg border border-[color:var(--border-subtle)] p-3 sm:grid-cols-[minmax(8rem,1fr)_8rem_minmax(8rem,1fr)_auto]">
            <label className="text-xs font-medium">{t("pages.promptRepo.fieldName")}<Input className="mt-1" aria-label={t("pages.promptRepo.variableNameAria", { index: index + 1 })} value={variable.name} placeholder="customer_name" onChange={(event) => update(index, { name: event.target.value })} /></label>
            <label className="text-xs font-medium">{t("pages.promptRepo.fieldMode")}<Select className="mt-1" aria-label={t("pages.promptRepo.variableModeAria", { index: index + 1 })} value={variable.required ? "required" : "default"} onChange={(event) => update(index, event.target.value === "required" ? { required: true, default: undefined } : { required: false, default: variable.default ?? "" })}><option value="required">{t("pages.promptRepo.modeRequired")}</option><option value="default">{t("pages.promptRepo.modeHasDefault")}</option></Select></label>
            <label className="text-xs font-medium">{t("pages.promptRepo.fieldDefault")}<Input className="mt-1" aria-label={t("pages.promptRepo.variableDefaultAria", { index: index + 1 })} disabled={variable.required} value={variable.default ?? ""} placeholder={variable.required ? t("pages.promptRepo.defaultNotAvailable") : t("pages.promptRepo.defaultFallback")} onChange={(event) => update(index, { default: event.target.value })} /></label>
            <Button variant="ghost" aria-label={t("pages.promptRepo.removeVariableAria", { name: variable.name || index + 1 })} onClick={() => onChange(variables.filter((_, i) => i !== index))}><Trash2 className="h-4 w-4" /></Button>
          </div>
        ))}
      </div>
    </section>
  );
}

function DecoratorEditor({ decorators, onChange }: { decorators: PromptTemplateDecorator[]; onChange: (decorators: PromptTemplateDecorator[]) => void }) {
  const { t } = useTranslation();
  const update = (index: number, patch: Partial<PromptTemplateDecorator>) => onChange(decorators.map((decorator, i) => (i === index ? { ...decorator, ...patch } : decorator)));
  const move = (from: number, to: number) => {
    if (to < 0 || to >= decorators.length) return;
    const next = [...decorators];
    const [item] = next.splice(from, 1);
    next.splice(to, 0, item);
    onChange(next);
  };
  return (
    <section>
      <SectionHeading eyebrow={t("pages.promptRepo.compositionEyebrow")} title={t("pages.promptRepo.decoratorsTitle")} description={t("pages.promptRepo.decoratorsDescription")} action={<Button variant="outline" onClick={() => onChange([...decorators, { role: "system", position: "prepend", content: "" }])}><Plus className="h-4 w-4" /> {t("pages.promptRepo.addDecorator")}</Button>} />
      <div className="space-y-2">
        {decorators.map((decorator, index) => (
          <article key={index} className="rounded-lg border border-[color:var(--border-subtle)] p-3">
            <div className="mb-2 flex flex-wrap items-center gap-2">
              <span className="font-mono text-[0.6875rem] text-[color:var(--text-subtle)]">{String(index + 1).padStart(2, "0")}</span>
              <Select aria-label={t("pages.promptRepo.decoratorRoleAria", { index: index + 1 })} value={decorator.role} onChange={(event) => update(index, { role: event.target.value as PromptTemplateDecorator["role"] })}><option value="system">{t("pages.promptRepo.roleSystem")}</option><option value="assistant">{t("pages.promptRepo.roleAssistant")}</option><option value="user">{t("pages.promptRepo.roleUser")}</option></Select>
              <Select aria-label={t("pages.promptRepo.decoratorPositionAria", { index: index + 1 })} value={decorator.position} onChange={(event) => update(index, { position: event.target.value as PromptTemplateDecorator["position"] })}><option value="prepend">{t("pages.promptRepo.positionPrepend")}</option><option value="append">{t("pages.promptRepo.positionAppend")}</option></Select>
              <span className="flex-1" />
              <Button variant="ghost" aria-label={t("pages.promptRepo.moveDecoratorUpAria", { index: index + 1 })} disabled={index === 0} onClick={() => move(index, index - 1)}><ArrowUp className="h-4 w-4" /></Button>
              <Button variant="ghost" aria-label={t("pages.promptRepo.moveDecoratorDownAria", { index: index + 1 })} disabled={index === decorators.length - 1} onClick={() => move(index, index + 1)}><ArrowDown className="h-4 w-4" /></Button>
              <Button variant="ghost" aria-label={t("pages.promptRepo.removeDecoratorAria", { index: index + 1 })} onClick={() => onChange(decorators.filter((_, i) => i !== index))}><Trash2 className="h-4 w-4" /></Button>
            </div>
            <Textarea aria-label={t("pages.promptRepo.decoratorContentAria", { index: index + 1 })} rows={4} value={decorator.content} placeholder={t("pages.promptRepo.decoratorPlaceholder", { sample: CUSTOMER_SAMPLE })} onChange={(event) => update(index, { content: event.target.value })} />
          </article>
        ))}
      </div>
    </section>
  );
}

function ScopeEditor({ scopes, orgId, projectId, routes, virtualKeys, onChange }: { scopes: PromptTemplateScopeInput[]; orgId?: string; projectId?: string; routes: { id: string; model: string }[]; virtualKeys: { id: string; name?: string | null; key_prefix: string }[]; onChange: (scopes: PromptTemplateScopeInput[]) => void }) {
  const { t } = useTranslation();
  const options = [
    ...(orgId ? [{ scope_type: "org" as const, scope_id: orgId, label: t("pages.promptRepo.scopeOrg"), detail: t("pages.promptRepo.scopeOrgDetail") }] : []),
    ...(projectId ? [{ scope_type: "project" as const, scope_id: projectId, label: t("pages.promptRepo.scopeProject"), detail: t("pages.promptRepo.scopeProjectDetail") }] : []),
    ...routes.map((route) => ({ scope_type: "route" as const, scope_id: route.id, label: route.model, detail: t("pages.promptRepo.scopeRouteDetail") })),
    ...virtualKeys.map((key) => ({ scope_type: "virtual_key" as const, scope_id: key.id, label: key.name || key.key_prefix, detail: t("pages.promptRepo.scopeVirtualKeyDetail") })),
  ];
  const checked = (option: PromptTemplateScopeInput) => scopes.some((scope) => scope.scope_type === option.scope_type && scope.scope_id === option.scope_id);
  const toggle = (option: PromptTemplateScopeInput) => onChange(checked(option) ? scopes.filter((scope) => !(scope.scope_type === option.scope_type && scope.scope_id === option.scope_id)) : [...scopes, option]);
  return (
    <section>
      <SectionHeading eyebrow={t("pages.promptRepo.deploymentEyebrow")} title={t("pages.promptRepo.scopesTitle")} description={t("pages.promptRepo.scopesDescription")} />
      <div className="grid gap-2 sm:grid-cols-2">
        {options.map((option) => (
          <label key={`${option.scope_type}:${option.scope_id}`} className="flex cursor-pointer items-start gap-2.5 rounded-lg border border-[color:var(--border-subtle)] p-3 hover:bg-[color:var(--surface-hover)]">
            <input type="checkbox" className="mt-0.5 h-4 w-4 accent-[color:var(--red-folk)]" checked={checked(option)} onChange={() => toggle(option)} />
            <span className="min-w-0"><span className="block truncate text-sm font-medium">{option.label}</span><span className="text-xs text-muted-foreground">{option.detail}</span></span>
          </label>
        ))}
      </div>
    </section>
  );
}

function PreviewPanel({ variables, decorators, samples, onSamplesChange }: { variables: PromptTemplateVariable[]; decorators: PromptTemplateDecorator[]; samples: Record<string, string>; onSamplesChange: (samples: Record<string, string>) => void }) {
  const { t } = useTranslation();
  const resolved = (content: string) => content.replace(/{{\s*([A-Za-z_][A-Za-z0-9_]*)\s*}}/g, (_match, name: string) => samples[name] || variables.find((variable) => variable.name === name)?.default || `{{ ${name} }}`);
  const missing = variables.filter((variable) => variable.required && !samples[variable.name]);
  return (
    <aside aria-label={t("pages.promptRepo.previewTitle")} className="border-t border-[color:var(--border-subtle)] bg-[color:var(--surface-app)] p-4 sm:p-5 2xl:border-l 2xl:border-t-0">
      <SectionHeading eyebrow={t("pages.promptRepo.liveRenderEyebrow")} title={t("pages.promptRepo.previewTitle")} description={t("pages.promptRepo.previewDescription")} />
      {variables.length > 0 && <div className="mb-5 space-y-2">{variables.map((variable) => <label key={variable.name} className="block text-xs font-medium">{variable.name || t("pages.promptRepo.unnamedVariable")}{variable.required && <span className="ml-1 text-[color:var(--red-folk-text)]">{t("pages.promptRepo.requiredMark")}</span>}<Input className="mt-1" aria-label={t("pages.promptRepo.sampleValueAria", { name: variable.name || t("pages.promptRepo.unnamedVariableLower") })} value={samples[variable.name] ?? ""} placeholder={variable.default ? t("pages.promptRepo.samplePlaceholderDefault", { value: variable.default }) : t("pages.promptRepo.samplePlaceholder")} onChange={(event) => onSamplesChange({ ...samples, [variable.name]: event.target.value })} /></label>)}</div>}
      {missing.length > 0 && <p role="status" className="mb-3 rounded-lg border border-[color:var(--status-warning)]/40 bg-[color:var(--status-warning)]/5 p-2.5 text-xs text-[color:var(--text-secondary)]">{t("pages.promptRepo.missingSamples", { names: missing.map((variable) => variable.name).join(", ") })}</p>}
      <div className="space-y-2" aria-label={t("pages.promptRepo.renderedPreviewAria")}>
        {decorators.length === 0 ? <p className="text-xs text-muted-foreground">{t("pages.promptRepo.addDecoratorHint")}</p> : <>
          {decorators.filter((decorator) => decorator.position === "prepend").map((decorator, index) => <PreviewMessage key={`prepend-${index}`} decorator={decorator} content={resolved(decorator.content)} />)}
          <div className="flex items-center gap-2 py-1 text-[0.6875rem] uppercase tracking-[0.12em] text-[color:var(--text-subtle)]"><span className="h-px flex-1 bg-[color:var(--border-subtle)]" /><Braces className="h-3.5 w-3.5" />{t("pages.promptRepo.callerMessages")}<span className="h-px flex-1 bg-[color:var(--border-subtle)]" /></div>
          {decorators.filter((decorator) => decorator.position === "append").map((decorator, index) => <PreviewMessage key={`append-${index}`} decorator={decorator} content={resolved(decorator.content)} />)}
        </>}
      </div>
    </aside>
  );
}

function PreviewMessage({ decorator, content }: { decorator: PromptTemplateDecorator; content: string }) {
  const { t } = useTranslation();
  return <article className="overflow-hidden rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]"><div className="flex items-center justify-between border-b border-[color:var(--border-subtle)] px-3 py-1.5"><span className="text-[0.6875rem] font-semibold uppercase tracking-[0.12em]">{decorator.role}</span><Badge tone="outline">{decorator.position}</Badge></div><p className="whitespace-pre-wrap break-words px-3 py-2.5 text-sm leading-6">{content || <span className="text-muted-foreground">{t("pages.promptRepo.emptyDecorator")}</span>}</p></article>;
}

export function VersionRail({ className, template, versions, selectedVersion, loading, onSelect, onRollback }: { className?: string; template: PromptTemplateRow; versions: PromptTemplateVersionRow[]; selectedVersion?: number; loading: boolean; onSelect: (version: number) => void; onRollback: (version: number) => void }) {
  const { t } = useTranslation();
  const format = useFormat();
  return (
    <aside aria-label={t("pages.promptRepo.versionHistory")} className={cn("overflow-hidden rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]", className)}>
      <div className="border-b border-[color:var(--border-subtle)] px-4 py-3"><p className="text-xs font-semibold uppercase tracking-[0.12em] text-[color:var(--text-subtle)]">{t("pages.promptRepo.versionHistory")}</p><p className="mt-1 text-xs text-muted-foreground">{t("pages.promptRepo.versionHistoryHint")}</p></div>
      <div className="max-h-[30rem] space-y-1 overflow-y-auto p-2 2xl:max-h-[calc(100vh-14rem)]">
        {loading ? <Skeleton width="100%" height={180} radius={8} /> : versions.length === 0 ? <p className="px-2 py-5 text-center text-xs text-muted-foreground">{t("pages.promptRepo.noSavedVersions")}</p> : versions.map((version) => {
          const published = template.published_version === version.version;
          return <div key={version.version} className={cn("rounded-lg border p-2.5", selectedVersion === version.version ? "border-[color:var(--red-folk)] bg-[color:var(--surface-selected)]" : "border-transparent hover:bg-[color:var(--surface-hover)]")}>
            <button type="button" className="w-full text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded-md" aria-pressed={selectedVersion === version.version} onClick={() => onSelect(version.version)}>
              <div className="flex items-center justify-between gap-2"><span className="text-sm font-semibold tabular-nums">v{version.version}</span><Badge tone={published ? "success" : "neutral"}>{published ? t("pages.promptRepo.published") : t("pages.promptRepo.immutable")}</Badge></div>
              <p className="mt-1.5 flex items-center gap-1 text-[0.6875rem] text-muted-foreground"><Clock3 className="h-3 w-3" />{format.date(version.created_at, { dateStyle: "medium", timeStyle: "short" })}</p>
              <p className="mt-1 text-[0.6875rem] text-[color:var(--text-subtle)]">{t("pages.promptRepo.versionCounts", { variables: version.variables.length, decorators: version.decorators.length })}</p>
            </button>
            {!published && template.published_version && <GatedButton gate="prompt_template:update" variant="ghost" onClick={() => onRollback(version.version)}><RotateCcw className="h-3.5 w-3.5" /> {t("pages.promptRepo.rollbackTo", { version: version.version })}</GatedButton>}
          </div>;
        })}
      </div>
    </aside>
  );
}

function CreateTemplateDialog({ open, pending, error, onOpenChange, onSubmit }: { open: boolean; pending: boolean; error: Error | null; onOpenChange: (open: boolean) => void; onSubmit: (input: { name: string; slug?: string; description?: string }) => void }) {
  const { t } = useTranslation();
  const [name, setName] = React.useState("");
  const [slug, setSlug] = React.useState("");
  const [description, setDescription] = React.useState("");
  React.useEffect(() => { if (!open) { setName(""); setSlug(""); setDescription(""); } }, [open]);
  return <Dialog open={open} onOpenChange={onOpenChange}><DialogHeader><DialogTitle>{t("pages.promptRepo.createTitle")}</DialogTitle><DialogDescription>{t("pages.promptRepo.createDescription")}</DialogDescription></DialogHeader><form className="space-y-3" onSubmit={(event) => { event.preventDefault(); onSubmit({ name: name.trim(), ...(slug.trim() ? { slug: slug.trim() } : {}), ...(description.trim() ? { description: description.trim() } : {}) }); }}><label className="block text-xs font-medium">{t("pages.promptRepo.fieldName")}<Input className="mt-1" autoFocus value={name} onChange={(event) => setName(event.target.value)} placeholder={t("pages.promptRepo.namePlaceholder")} /></label><label className="block text-xs font-medium">{t("pages.promptRepo.fieldSlug")} <span className="font-normal text-muted-foreground">{t("pages.promptRepo.fieldOptional")}</span><Input className="mt-1" value={slug} onChange={(event) => setSlug(event.target.value)} placeholder="support-concierge" /></label><label className="block text-xs font-medium">{t("pages.promptRepo.fieldDescription")} <span className="font-normal text-muted-foreground">{t("pages.promptRepo.fieldOptional")}</span><Textarea className="mt-1" rows={3} value={description} onChange={(event) => setDescription(event.target.value)} /></label>{error && <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">{error.message}</p>}<DialogFooter><Button type="button" variant="outline" onClick={() => onOpenChange(false)}>{t("pages.promptRepo.cancel")}</Button><Button type="submit" disabled={pending || !name.trim()}>{pending ? t("pages.promptRepo.creating") : t("pages.promptRepo.createTemplate")}</Button></DialogFooter></form></Dialog>;
}

function RenameTemplateDialog({ open, template, pending, error, onOpenChange, onSubmit }: { open: boolean; template?: PromptTemplateRow; pending: boolean; error: Error | null; onOpenChange: (open: boolean) => void; onSubmit: (input: { name?: string; description?: string }) => void }) {
  const [name, setName] = React.useState("");
  const [description, setDescription] = React.useState("");
  const { t } = useTranslation();
  React.useEffect(() => { if (open && template) { setName(template.name); setDescription(template.description ?? ""); } }, [open, template]);
  const trimmed = name.trim();
  // the slug is the stable identity and stays put, so only these two move
  const unchanged = trimmed === template?.name && description.trim() === (template?.description ?? "");
  return <Dialog open={open} onOpenChange={onOpenChange}><DialogHeader><DialogTitle>{t("pages.promptRepo.renameTitle")}</DialogTitle><DialogDescription>{t("pages.promptRepo.renameDescription", { slug: template?.slug ?? "" })}</DialogDescription></DialogHeader><form className="space-y-3" onSubmit={(event) => { event.preventDefault(); onSubmit({ name: trimmed, description: description.trim() }); }}><label className="block text-xs font-medium">{t("pages.promptRepo.fieldName")}<Input className="mt-1" autoFocus value={name} onChange={(event) => setName(event.target.value)} /></label><label className="block text-xs font-medium">{t("pages.promptRepo.fieldDescription")} <span className="font-normal text-muted-foreground">{t("pages.promptRepo.fieldOptional")}</span><Textarea className="mt-1" rows={3} value={description} onChange={(event) => setDescription(event.target.value)} /></label>{error && <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">{error.message}</p>}<DialogFooter><Button type="button" variant="outline" onClick={() => onOpenChange(false)}>{t("pages.promptRepo.cancel")}</Button><Button type="submit" disabled={pending || !trimmed || unchanged}>{pending ? t("pages.promptRepo.renameSaving") : t("pages.promptRepo.renameSubmit")}</Button></DialogFooter></form></Dialog>;
}

function DeleteTemplateDialog({ open, template, pending, error, onOpenChange, onConfirm }: { open: boolean; template?: PromptTemplateRow; pending: boolean; error: Error | null; onOpenChange: (open: boolean) => void; onConfirm: () => void }) {
  const { t } = useTranslation();
  const [confirmation, setConfirmation] = React.useState("");
  React.useEffect(() => { if (!open) setConfirmation(""); }, [open]);
  // deleting takes every immutable version with it, so make the operator
  // retype the slug rather than let one stray click drop live prompt content
  const matches = confirmation.trim() === template?.slug;
  return <Dialog open={open} onOpenChange={onOpenChange}><DialogHeader><DialogTitle>{t("pages.promptRepo.deleteTitle", { name: template?.name ?? "" })}</DialogTitle><DialogDescription>{template?.published_version ? t("pages.promptRepo.deleteLive", { version: template.published_version }) : t("pages.promptRepo.deleteUnpublished")} {t("pages.promptRepo.deleteConsequence")}</DialogDescription></DialogHeader><label className="block text-xs font-medium">{t("pages.promptRepo.deleteConfirmLabel", { slug: template?.slug ?? "" })}<Input className="mt-1" autoFocus value={confirmation} onChange={(event) => setConfirmation(event.target.value)} placeholder={template?.slug} /></label>{error && <p role="alert" className="mt-2 text-xs text-[color:var(--status-danger-text)]">{error.message}</p>}<DialogFooter><Button variant="outline" onClick={() => onOpenChange(false)}>{t("pages.promptRepo.cancel")}</Button><Button variant="destructive" disabled={pending || !matches} onClick={onConfirm}><Trash2 className="h-4 w-4" />{pending ? t("pages.promptRepo.deleting") : t("pages.promptRepo.deleteSubmit")}</Button></DialogFooter></Dialog>;
}

function RollbackDialog({ version, publishedVersion, pending, error, onClose, onConfirm }: { version?: number; publishedVersion?: number | null; pending: boolean; error: Error | null; onClose: () => void; onConfirm: () => void }) {
  const { t } = useTranslation();
  return <Dialog open={version !== undefined} onOpenChange={(open) => !open && onClose()}><DialogHeader><DialogTitle>{t("pages.promptRepo.rollbackTo", { version })}</DialogTitle><DialogDescription>{t("pages.promptRepo.rollbackDescription", { from: publishedVersion, to: version })}</DialogDescription></DialogHeader>{error && <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">{error.message}</p>}<DialogFooter><Button variant="outline" onClick={onClose}>{t("pages.promptRepo.keepLive", { version: publishedVersion })}</Button><Button disabled={pending} onClick={onConfirm}><RotateCcw className="h-4 w-4" />{pending ? t("pages.promptRepo.rollingBack") : t("pages.promptRepo.rollbackTo", { version })}</Button></DialogFooter></Dialog>;
}
