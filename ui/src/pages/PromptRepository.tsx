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
  Plus,
  RotateCcw,
  Trash2,
} from "lucide-react";
import * as React from "react";

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
  fetchPromptTemplateScopes,
  fetchPromptTemplates,
  fetchPromptTemplateVersions,
  fetchRoutes,
  fetchVirtualKeys,
  publishPromptTemplateVersion,
  rollbackPromptTemplateVersion,
  setPromptTemplateScopes,
  type PromptTemplateDecorator,
  type PromptTemplateRow,
  type PromptTemplateScopeInput,
  type PromptTemplateVariable,
  type PromptTemplateVersionRow,
} from "@/lib/api";
import { useScope } from "@/lib/scope";
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

function formatDate(value: string) {
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(value));
}

function versionLabel(template: PromptTemplateRow, version: number) {
  return template.published_version === version ? "Published" : "Immutable";
}

function draftProblem(draft: Draft): string | undefined {
  if (draft.decorators.length === 0) return "Add at least one decorator.";
  const names = new Set<string>();
  for (const variable of draft.variables) {
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(variable.name)) {
      return `“${variable.name || "unnamed"}” is not a valid variable name.`;
    }
    if (names.has(variable.name)) return `Variable “${variable.name}” is duplicated.`;
    if (variable.required && variable.default !== undefined) {
      return `Required variable “${variable.name}” cannot also have a default.`;
    }
    names.add(variable.name);
  }
  for (const decorator of draft.decorators) {
    if (!decorator.content.trim()) return "Every decorator needs content.";
    for (const match of decorator.content.matchAll(/{{\s*([A-Za-z_][A-Za-z0-9_]*)\s*}}/g)) {
      if (!names.has(match[1])) return `Decorator references undeclared variable “${match[1]}”.`;
    }
  }
  return undefined;
}

export default function PromptRepository() {
  const scope = useScope();
  const queryClient = useQueryClient();
  const [selectedId, setSelectedId] = React.useState<string>();
  const [selectedVersion, setSelectedVersion] = React.useState<number>();
  const [draft, setDraft] = React.useState<Draft>(() => copyDraft());
  const [samples, setSamples] = React.useState<Record<string, string>>({});
  const [createOpen, setCreateOpen] = React.useState(false);
  const [rollbackVersion, setRollbackVersion] = React.useState<number>();
  const [notice, setNotice] = React.useState<string>();

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
      setNotice("Template created. Add its first immutable draft.");
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
      setNotice(
        scopeError
          ? `Draft v${version.version} was saved, but its scopes were not: ${scopeError.message}`
          : `Draft v${version.version} saved. Its content and scopes are now immutable.`,
      );
    },
  });

  const publish = useMutation({
    mutationFn: (version: number) => publishPromptTemplateVersion(selectedId as string, version),
    onSuccess: (template) => {
      queryClient.setQueryData<PromptTemplateRow[]>(
        ["prompt-templates", scope.orgId],
        (current = []) => current.map((item) => (item.id === template.id ? template : item)),
      );
      setNotice(`v${template.published_version} is now live.`);
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
      setNotice(`Rolled back: v${template.published_version} is live again.`);
    },
  });

  if (scope.isLoading || templates.isLoading) return <LoadingState />;
  if (scope.error || templates.isError) {
    return (
      <EmptyState uxTarget="prompt-list"
        icon={<GitBranch />}
        title="Prompt repository unavailable"
        description={scope.error ?? (templates.error as Error).message}
        actions={<Button variant="outline" onClick={() => templates.refetch()}>Try again</Button>}
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
              title="Start with a prompt template"
              description="Create a named template, then add variables, ordered decorators, and its first deployment scope."
              actions={<Button onClick={() => setCreateOpen(true)}>Create template</Button>}
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
            notice={notice}
            pending={saveDraft.isPending || publish.isPending}
            error={(saveDraft.error ?? publish.error) as Error | null}
            onDraftChange={(next) => {
              setDraft(next);
              setNotice(undefined);
            }}
            onSamplesChange={setSamples}
            onSave={() => saveDraft.mutate()}
            onPublish={(version) => publish.mutate(version)}
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
  return (
    <aside className="overflow-hidden rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]">
      <div className="flex items-center justify-between border-b border-[color:var(--border-subtle)] px-3 py-2.5">
        <div>
          <p className="text-xs font-semibold uppercase tracking-[0.12em] text-[color:var(--text-subtle)]">Templates</p>
          <p className="mt-0.5 text-xs text-muted-foreground">{templates.length} in this org</p>
        </div>
        <Button variant="ghost" onClick={onCreate} aria-label="Create template">
          <Plus className="h-4 w-4" />
        </Button>
      </div>
      <div className="max-h-[26rem] overflow-y-auto p-1.5 lg:max-h-[calc(100vh-14rem)]">
        {templates.length === 0 ? (
          <p className="px-2 py-5 text-center text-xs text-muted-foreground">No templates yet</p>
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
                <span className="text-[0.6875rem] tabular-nums text-[color:var(--status-success)]">v{template.published_version}</span>
              ) : (
                <span className="text-[0.6875rem] text-[color:var(--text-subtle)]">draft</span>
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
  notice,
  pending,
  error,
  onDraftChange,
  onSamplesChange,
  onSave,
  onPublish,
}: {
  template: PromptTemplateRow;
  baseVersion?: PromptTemplateVersionRow;
  draft: Draft;
  samples: Record<string, string>;
  routes: { id: string; model: string }[];
  virtualKeys: { id: string; name?: string | null; key_prefix: string }[];
  orgId?: string;
  projectId?: string;
  notice?: string;
  pending: boolean;
  error: Error | null;
  onDraftChange: (draft: Draft) => void;
  onSamplesChange: (samples: Record<string, string>) => void;
  onSave: () => void;
  onPublish: (version: number) => void;
}) {
  const problem = draftProblem(draft);
  const selectedPublished = baseVersion?.version === template.published_version;
  return (
    <main className="min-w-0 overflow-hidden rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]">
      <header className="border-b border-[color:var(--border-subtle)] px-4 py-3 sm:px-5">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <h2 className="truncate text-lg font-semibold tracking-[-0.02em]">{template.name}</h2>
              {template.published_version ? <Badge tone="success" dot>v{template.published_version} live</Badge> : <Badge tone="warning">Unpublished</Badge>}
            </div>
            <p className="mt-1 max-w-2xl text-sm text-muted-foreground">{template.description || "No description yet."}</p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            {baseVersion && !selectedPublished && (
              <Button variant="outline" disabled={pending} onClick={() => onPublish(baseVersion.version)}>
                <Check className="h-4 w-4" /> Publish v{baseVersion.version}
              </Button>
            )}
            <Button disabled={pending || !!problem} onClick={onSave}>
              <FilePlus2 className="h-4 w-4" /> {pending ? "Saving…" : "Save as new draft"}
            </Button>
          </div>
        </div>
        <div className="mt-3 flex min-h-5 flex-wrap items-center gap-x-3 gap-y-1 text-xs">
          <span className="font-mono text-[color:var(--text-subtle)]">{template.slug}</span>
          {baseVersion && <span className="text-muted-foreground">editing from immutable v{baseVersion.version}</span>}
          {problem && <span role="alert" className="text-[color:var(--status-danger)]">{problem}</span>}
          {error && <span role="alert" className="text-[color:var(--status-danger)]">{error.message}</span>}
          {notice && <span role="status" className={notice.includes("but its scopes were not") ? "text-[color:var(--status-warning)]" : "text-[color:var(--status-success)]"}>{notice}</span>}
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
        <p className="text-[0.6875rem] font-semibold uppercase tracking-[0.14em] text-[color:var(--red-folk)]">{eyebrow}</p>
        <h3 className="mt-1 text-sm font-semibold">{title}</h3>
        <p className="mt-0.5 text-xs leading-5 text-muted-foreground">{description}</p>
      </div>
      {action}
    </div>
  );
}

function VariableEditor({ variables, onChange }: { variables: PromptTemplateVariable[]; onChange: (variables: PromptTemplateVariable[]) => void }) {
  const update = (index: number, patch: Partial<PromptTemplateVariable>) => onChange(variables.map((variable, i) => (i === index ? { ...variable, ...patch } : variable)));
  return (
    <section aria-labelledby="variables-heading">
      <SectionHeading eyebrow="Inputs" title="Variables" description="Declare placeholders used as {{ variable_name }} inside decorators." action={<Button variant="outline" onClick={() => onChange([...variables, { name: "", required: true }])}><Plus className="h-4 w-4" /> Add variable</Button>} />
      <h3 id="variables-heading" className="sr-only">Variables</h3>
      <div className="space-y-2">
        {variables.length === 0 ? <p className="rounded-lg border border-dashed border-[color:var(--border-default)] p-3 text-xs text-muted-foreground">No variables. This template renders the same content for every request.</p> : variables.map((variable, index) => (
          <div key={index} className="grid gap-2 rounded-lg border border-[color:var(--border-subtle)] p-3 sm:grid-cols-[minmax(8rem,1fr)_8rem_minmax(8rem,1fr)_auto]">
            <label className="text-xs font-medium">Name<Input className="mt-1" aria-label={`Variable ${index + 1} name`} value={variable.name} placeholder="customer_name" onChange={(event) => update(index, { name: event.target.value })} /></label>
            <label className="text-xs font-medium">Mode<Select className="mt-1" aria-label={`Variable ${index + 1} mode`} value={variable.required ? "required" : "default"} onChange={(event) => update(index, event.target.value === "required" ? { required: true, default: undefined } : { required: false, default: variable.default ?? "" })}><option value="required">Required</option><option value="default">Has default</option></Select></label>
            <label className="text-xs font-medium">Default<Input className="mt-1" aria-label={`Variable ${index + 1} default`} disabled={variable.required} value={variable.default ?? ""} placeholder={variable.required ? "Not available" : "Fallback value"} onChange={(event) => update(index, { default: event.target.value })} /></label>
            <Button variant="ghost" aria-label={`Remove variable ${variable.name || index + 1}`} onClick={() => onChange(variables.filter((_, i) => i !== index))}><Trash2 className="h-4 w-4" /></Button>
          </div>
        ))}
      </div>
    </section>
  );
}

function DecoratorEditor({ decorators, onChange }: { decorators: PromptTemplateDecorator[]; onChange: (decorators: PromptTemplateDecorator[]) => void }) {
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
      <SectionHeading eyebrow="Composition" title="Decorators" description="Build ordered messages around the caller's original conversation." action={<Button variant="outline" onClick={() => onChange([...decorators, { role: "system", position: "prepend", content: "" }])}><Plus className="h-4 w-4" /> Add decorator</Button>} />
      <div className="space-y-2">
        {decorators.map((decorator, index) => (
          <article key={index} className="rounded-lg border border-[color:var(--border-subtle)] p-3">
            <div className="mb-2 flex flex-wrap items-center gap-2">
              <span className="font-mono text-[0.6875rem] text-[color:var(--text-subtle)]">{String(index + 1).padStart(2, "0")}</span>
              <Select aria-label={`Decorator ${index + 1} role`} value={decorator.role} onChange={(event) => update(index, { role: event.target.value as PromptTemplateDecorator["role"] })}><option value="system">System</option><option value="assistant">Assistant</option><option value="user">User</option></Select>
              <Select aria-label={`Decorator ${index + 1} position`} value={decorator.position} onChange={(event) => update(index, { position: event.target.value as PromptTemplateDecorator["position"] })}><option value="prepend">Prepend</option><option value="append">Append</option></Select>
              <span className="flex-1" />
              <Button variant="ghost" aria-label={`Move decorator ${index + 1} up`} disabled={index === 0} onClick={() => move(index, index - 1)}><ArrowUp className="h-4 w-4" /></Button>
              <Button variant="ghost" aria-label={`Move decorator ${index + 1} down`} disabled={index === decorators.length - 1} onClick={() => move(index, index + 1)}><ArrowDown className="h-4 w-4" /></Button>
              <Button variant="ghost" aria-label={`Remove decorator ${index + 1}`} onClick={() => onChange(decorators.filter((_, i) => i !== index))}><Trash2 className="h-4 w-4" /></Button>
            </div>
            <Textarea aria-label={`Decorator ${index + 1} content`} rows={4} value={decorator.content} placeholder="You are assisting {{ customer_name }}…" onChange={(event) => update(index, { content: event.target.value })} />
          </article>
        ))}
      </div>
    </section>
  );
}

function ScopeEditor({ scopes, orgId, projectId, routes, virtualKeys, onChange }: { scopes: PromptTemplateScopeInput[]; orgId?: string; projectId?: string; routes: { id: string; model: string }[]; virtualKeys: { id: string; name?: string | null; key_prefix: string }[]; onChange: (scopes: PromptTemplateScopeInput[]) => void }) {
  const options = [
    ...(orgId ? [{ scope_type: "org" as const, scope_id: orgId, label: "Current organization", detail: "All projects in this org" }] : []),
    ...(projectId ? [{ scope_type: "project" as const, scope_id: projectId, label: "Current project", detail: "Every route in this project" }] : []),
    ...routes.map((route) => ({ scope_type: "route" as const, scope_id: route.id, label: route.model, detail: "Route" })),
    ...virtualKeys.map((key) => ({ scope_type: "virtual_key" as const, scope_id: key.id, label: key.name || key.key_prefix, detail: "Virtual key" })),
  ];
  const checked = (option: PromptTemplateScopeInput) => scopes.some((scope) => scope.scope_type === option.scope_type && scope.scope_id === option.scope_id);
  const toggle = (option: PromptTemplateScopeInput) => onChange(checked(option) ? scopes.filter((scope) => !(scope.scope_type === option.scope_type && scope.scope_id === option.scope_id)) : [...scopes, option]);
  return (
    <section>
      <SectionHeading eyebrow="Deployment" title="Assignment scopes" description="Choose where this exact version can be resolved. Empty means it is not assigned." />
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
  const resolved = (content: string) => content.replace(/{{\s*([A-Za-z_][A-Za-z0-9_]*)\s*}}/g, (_match, name: string) => samples[name] || variables.find((variable) => variable.name === name)?.default || `{{ ${name} }}`);
  const missing = variables.filter((variable) => variable.required && !samples[variable.name]);
  return (
    <aside className="border-t border-[color:var(--border-subtle)] bg-[color:var(--surface-app)] p-4 sm:p-5 2xl:border-l 2xl:border-t-0">
      <SectionHeading eyebrow="Live render" title="Preview" description="Sample values stay in your browser and are never saved." />
      {variables.length > 0 && <div className="mb-5 space-y-2">{variables.map((variable) => <label key={variable.name} className="block text-xs font-medium">{variable.name || "Unnamed variable"}{variable.required && <span className="ml-1 text-[color:var(--red-folk)]">required</span>}<Input className="mt-1" aria-label={`Sample value for ${variable.name || "unnamed variable"}`} value={samples[variable.name] ?? ""} placeholder={variable.default ? `Default: ${variable.default}` : "Sample value"} onChange={(event) => onSamplesChange({ ...samples, [variable.name]: event.target.value })} /></label>)}</div>}
      {missing.length > 0 && <p role="status" className="mb-3 rounded-lg border border-[color:var(--status-warning)]/40 bg-[color:var(--status-warning)]/5 p-2.5 text-xs text-[color:var(--text-secondary)]">Add sample values for {missing.map((variable) => variable.name).join(", ")} to complete the preview.</p>}
      <div className="space-y-2" aria-label="Rendered prompt preview">
        {decorators.length === 0 ? <p className="text-xs text-muted-foreground">Add a decorator to see the rendered prompt.</p> : <>
          {decorators.filter((decorator) => decorator.position === "prepend").map((decorator, index) => <PreviewMessage key={`prepend-${index}`} decorator={decorator} content={resolved(decorator.content)} />)}
          <div className="flex items-center gap-2 py-1 text-[0.6875rem] uppercase tracking-[0.12em] text-[color:var(--text-subtle)]"><span className="h-px flex-1 bg-[color:var(--border-subtle)]" /><Braces className="h-3.5 w-3.5" />caller messages<span className="h-px flex-1 bg-[color:var(--border-subtle)]" /></div>
          {decorators.filter((decorator) => decorator.position === "append").map((decorator, index) => <PreviewMessage key={`append-${index}`} decorator={decorator} content={resolved(decorator.content)} />)}
        </>}
      </div>
    </aside>
  );
}

function PreviewMessage({ decorator, content }: { decorator: PromptTemplateDecorator; content: string }) {
  return <article className="overflow-hidden rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]"><div className="flex items-center justify-between border-b border-[color:var(--border-subtle)] px-3 py-1.5"><span className="text-[0.6875rem] font-semibold uppercase tracking-[0.12em]">{decorator.role}</span><Badge tone="outline">{decorator.position}</Badge></div><p className="whitespace-pre-wrap break-words px-3 py-2.5 text-sm leading-6">{content || <span className="text-muted-foreground">Empty decorator</span>}</p></article>;
}

export function VersionRail({ className, template, versions, selectedVersion, loading, onSelect, onRollback }: { className?: string; template: PromptTemplateRow; versions: PromptTemplateVersionRow[]; selectedVersion?: number; loading: boolean; onSelect: (version: number) => void; onRollback: (version: number) => void }) {
  return (
    <aside className={cn("overflow-hidden rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]", className)}>
      <div className="border-b border-[color:var(--border-subtle)] px-4 py-3"><p className="text-xs font-semibold uppercase tracking-[0.12em] text-[color:var(--text-subtle)]">Version history</p><p className="mt-1 text-xs text-muted-foreground">Saved versions cannot be edited.</p></div>
      <div className="max-h-[30rem] space-y-1 overflow-y-auto p-2 2xl:max-h-[calc(100vh-14rem)]">
        {loading ? <Skeleton width="100%" height={180} radius={8} /> : versions.length === 0 ? <p className="px-2 py-5 text-center text-xs text-muted-foreground">No saved versions</p> : versions.map((version) => {
          const published = template.published_version === version.version;
          return <div key={version.version} className={cn("rounded-lg border p-2.5", selectedVersion === version.version ? "border-[color:var(--red-folk)] bg-[color:var(--surface-selected)]" : "border-transparent hover:bg-[color:var(--surface-hover)]")}>
            <button type="button" className="w-full text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded-md" aria-pressed={selectedVersion === version.version} onClick={() => onSelect(version.version)}>
              <div className="flex items-center justify-between gap-2"><span className="text-sm font-semibold tabular-nums">v{version.version}</span><Badge tone={published ? "success" : "neutral"}>{versionLabel(template, version.version)}</Badge></div>
              <p className="mt-1.5 flex items-center gap-1 text-[0.6875rem] text-muted-foreground"><Clock3 className="h-3 w-3" />{formatDate(version.created_at)}</p>
              <p className="mt-1 text-[0.6875rem] text-[color:var(--text-subtle)]">{version.variables.length} variables · {version.decorators.length} decorators</p>
            </button>
            {!published && template.published_version && <Button variant="ghost" onClick={() => onRollback(version.version)}><RotateCcw className="h-3.5 w-3.5" /> Roll back to v{version.version}</Button>}
          </div>;
        })}
      </div>
    </aside>
  );
}

function CreateTemplateDialog({ open, pending, error, onOpenChange, onSubmit }: { open: boolean; pending: boolean; error: Error | null; onOpenChange: (open: boolean) => void; onSubmit: (input: { name: string; slug?: string; description?: string }) => void }) {
  const [name, setName] = React.useState("");
  const [slug, setSlug] = React.useState("");
  const [description, setDescription] = React.useState("");
  React.useEffect(() => { if (!open) { setName(""); setSlug(""); setDescription(""); } }, [open]);
  return <Dialog open={open} onOpenChange={onOpenChange}><DialogHeader><DialogTitle>Create prompt template</DialogTitle><DialogDescription>The slug is a stable identity. Content is added as an immutable version next.</DialogDescription></DialogHeader><form className="space-y-3" onSubmit={(event) => { event.preventDefault(); onSubmit({ name: name.trim(), ...(slug.trim() ? { slug: slug.trim() } : {}), ...(description.trim() ? { description: description.trim() } : {}) }); }}><label className="block text-xs font-medium">Name<Input className="mt-1" autoFocus value={name} onChange={(event) => setName(event.target.value)} placeholder="Support concierge" /></label><label className="block text-xs font-medium">Slug <span className="font-normal text-muted-foreground">optional</span><Input className="mt-1" value={slug} onChange={(event) => setSlug(event.target.value)} placeholder="support-concierge" /></label><label className="block text-xs font-medium">Description <span className="font-normal text-muted-foreground">optional</span><Textarea className="mt-1" rows={3} value={description} onChange={(event) => setDescription(event.target.value)} /></label>{error && <p role="alert" className="text-xs text-[color:var(--status-danger)]">{error.message}</p>}<DialogFooter><Button type="button" variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button><Button type="submit" disabled={pending || !name.trim()}>{pending ? "Creating…" : "Create template"}</Button></DialogFooter></form></Dialog>;
}

function RollbackDialog({ version, publishedVersion, pending, error, onClose, onConfirm }: { version?: number; publishedVersion?: number | null; pending: boolean; error: Error | null; onClose: () => void; onConfirm: () => void }) {
  return <Dialog open={version !== undefined} onOpenChange={(open) => !open && onClose()}><DialogHeader><DialogTitle>Roll back to v{version}</DialogTitle><DialogDescription>This changes the live version from v{publishedVersion} to immutable v{version}. The newer versions remain in history.</DialogDescription></DialogHeader>{error && <p role="alert" className="text-xs text-[color:var(--status-danger)]">{error.message}</p>}<DialogFooter><Button variant="outline" onClick={onClose}>Keep v{publishedVersion} live</Button><Button disabled={pending} onClick={onConfirm}><RotateCcw className="h-4 w-4" />{pending ? "Rolling back…" : `Roll back to v${version}`}</Button></DialogFooter></Dialog>;
}
