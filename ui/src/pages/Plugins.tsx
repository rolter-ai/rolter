import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowDown, Loader2, Plus, Puzzle, Webhook } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { GatedButton } from "@/components/GatedButton";
import { GatedSwitch } from "@/components/GatedSwitch";
import { LoadError } from "@/components/LoadError";
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
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import {
  createPlugin,
  deletePlugin,
  fetchPlugins,
  updatePlugin,
  type PluginInstanceInput,
  type PluginInstanceRow,
} from "@/lib/api";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

type Stage = PluginInstanceRow["stage"];

const STAGE_KEYS: Stage[] = ["pre_route", "pre_upstream", "post_response"];

// stage labels and descriptions are catalog-backed, so they are resolved
// through `t` rather than held in a module-level constant
type StageCopy = { key: Stage; label: string; description: string };

const asInput = (plugin: PluginInstanceRow): PluginInstanceInput => ({
  project_id: plugin.project_id,
  name: plugin.name,
  slug: plugin.slug,
  description: plugin.description,
  kind: plugin.kind,
  stage: plugin.stage,
  enabled: plugin.enabled,
  position: plugin.position,
  failure_mode: plugin.failure_mode,
  endpoint: plugin.endpoint,
  secret_env: plugin.secret_env,
  config: plugin.config,
});

function useStages(): StageCopy[] {
  const { t } = useTranslation();
  return React.useMemo(
    () =>
      STAGE_KEYS.map((key) => ({
        key,
        label: t(`pages.plugins.stages.${key}.label`),
        description: t(`pages.plugins.stages.${key}.description`),
      })),
    [t],
  );
}

export default function Plugins() {
  const { t } = useTranslation();
  const stages = useStages();
  const scope = useScope();
  // the scope hook names a catalog key rather than carrying english copy
  const scopeMessage = scope.errorKey ? t(scope.errorKey) : undefined;
  const client = useQueryClient();
  const toast = useToast();
  const query = useQuery({
    queryKey: ["plugins", scope.orgId],
    queryFn: () => fetchPlugins(scope.orgId as string),
    enabled: Boolean(scope.orgId),
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `query` is the query the user is actually waiting on for this screen
  useScreenReady(!query.isLoading);
  useErrorState(!!query.error, "plugins");
  const [editing, setEditing] = React.useState<
    PluginInstanceRow | null | undefined
  >();
  const invalidate = () =>
    client.invalidateQueries({ queryKey: ["plugins", scope.orgId] });
  const toggle = useMutation({
    mutationFn: (plugin: PluginInstanceRow) =>
      updatePlugin(plugin.id, { ...asInput(plugin), enabled: !plugin.enabled }),
    onSuccess: invalidate,
    // a switch that bounced back reads as nothing having happened (#1197)
    onError: (error, plugin) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: plugin.name }),
        detail: errorDetail(error),
      });
    },
  });
  const remove = useMutation({
    mutationFn: deletePlugin,
    onSuccess: invalidate,
  });
  // was a bare window.confirm, which carried the copy but none of the styling,
  // and had no pending state to show while the delete was in flight (#1179)
  const [deleteTarget, setDeleteTarget] =
    React.useState<PluginInstanceRow | null>(null);
  const startDelete = (plugin: PluginInstanceRow) => {
    remove.reset();
    setDeleteTarget(plugin);
  };

  if (scope.isLoading) {
    return <PluginLoading />;
  }
  if (scope.errorKey || !scope.orgId) {
    return (
      <p className="p-[22px] text-sm text-muted-foreground">
        {scopeMessage ?? t("pages.plugins.noOrg")}
      </p>
    );
  }

  const plugins = query.data ?? [];
  return (
    <div className="mx-auto flex max-w-[1180px] flex-col gap-5 p-[22px]">
      <header className="flex flex-col gap-3 border-b border-[color:var(--border-subtle)] pb-5 sm:flex-row sm:items-end sm:justify-between">
        <div>
          <div className="flex flex-wrap items-center gap-2">
            <p className="font-mono text-[0.6875rem] uppercase tracking-[0.16em] text-[color:var(--status-info-text)]">
              {t("pages.plugins.eyebrow")}
            </p>
          </div>
          <h1 className="mt-1 text-xl font-semibold tracking-tight">
            {t("pages.plugins.heading")}
          </h1>
          <p className="mt-1 max-w-2xl text-sm text-muted-foreground">
            {t("pages.plugins.intro")}
          </p>
        </div>
        <GatedButton gate="plugin:create" onClick={() => setEditing(null)}>
          <Plus className="h-4 w-4" aria-hidden /> {t("pages.plugins.install")}
        </GatedButton>
      </header>

      {query.isLoading ? (
        <PluginLoading />
      ) : query.isError ? (
        <LoadError
          error={query.error}
          resource={t("errors.resources.plugins")}
          onRetry={() => void query.refetch()}
        />
      ) : plugins.length === 0 ? (
        <EmptyState
          uxTarget="plugin-list"
          icon={<Puzzle />}
          title={t("pages.plugins.emptyTitle")}
          description={t("pages.plugins.emptyDescription")}
          actions={
            <GatedButton gate="plugin:create" onClick={() => setEditing(null)}>
              {t("pages.plugins.installFirst")}
            </GatedButton>
          }
        />
      ) : (
        <div className="space-y-3">
          {stages.map((stage, index) => (
            <React.Fragment key={stage.key}>
              <StageLane
                stage={stage}
                plugins={plugins.filter((plugin) => plugin.stage === stage.key)}
                projectNames={Object.fromEntries(
                  scope.projects.map((project) => [project.id, project.name]),
                )}
                togglingId={toggle.isPending ? toggle.variables?.id : undefined}
                removingId={remove.isPending ? remove.variables : undefined}
                onToggle={(plugin) => toggle.mutate(plugin)}
                onEdit={setEditing}
                onDelete={startDelete}
              />
              {index < stages.length - 1 && (
                <ArrowDown
                  className="mx-auto h-4 w-4 text-[color:var(--text-subtle)]"
                  aria-hidden
                />
              )}
            </React.Fragment>
          ))}
        </div>
      )}

      <ConfirmDialog
        open={!!deleteTarget}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title={t("pages.plugins.confirm.title", { name: deleteTarget?.name })}
        description={t("pages.plugins.confirm.body")}
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

      <PluginDialog
        key={editing?.id ?? (editing === null ? "new" : "closed")}
        open={editing !== undefined}
        initial={editing ?? null}
        orgId={scope.orgId}
        projects={scope.projects}
        stages={stages}
        onClose={() => setEditing(undefined)}
        onDone={() => {
          setEditing(undefined);
          void invalidate();
        }}
      />
    </div>
  );
}

function PluginLoading() {
  return (
    <div className="mx-auto grid w-full max-w-[1180px] gap-3 p-[22px] md:grid-cols-3">
      {[0, 1, 2].map((item) => (
        <Skeleton key={item} width="100%" height={220} radius={10} />
      ))}
    </div>
  );
}

function StageLane({
  stage,
  plugins,
  projectNames,
  togglingId,
  removingId,
  onToggle,
  onEdit,
  onDelete,
}: {
  stage: StageCopy;
  plugins: PluginInstanceRow[];
  projectNames: Record<string, string>;
  togglingId?: string;
  removingId?: string;
  onToggle: (plugin: PluginInstanceRow) => void;
  onEdit: (plugin: PluginInstanceRow) => void;
  onDelete: (plugin: PluginInstanceRow) => void;
}) {
  const { t } = useTranslation();
  return (
    <section className="grid gap-3 rounded-[12px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)]/30 p-4 lg:grid-cols-[210px_1fr]">
      <div>
        <div className="flex items-center gap-2">
          <span className="flex h-8 w-8 items-center justify-center rounded-lg border border-[color:var(--border-default)] bg-background">
            <Webhook className="h-4 w-4" aria-hidden />
          </span>
          <h2 className="text-sm font-semibold">{stage.label}</h2>
        </div>
        <p className="mt-2 text-xs leading-relaxed text-muted-foreground">
          {stage.description}
        </p>
      </div>
      {plugins.length === 0 ? (
        <div className="flex min-h-[116px] items-center justify-center rounded-[10px] border border-dashed border-[color:var(--border-default)] text-xs text-[color:var(--text-subtle)]">
          {t("pages.plugins.stageEmpty")}
        </div>
      ) : (
        <div className="min-w-0 grid gap-3 md:grid-cols-2">
          {plugins.map((plugin) => (
            <article
              key={plugin.id}
              className="min-w-0 rounded-[10px] border border-[color:var(--border-default)] bg-background p-4"
            >
              <div className="flex items-start gap-3">
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <h3 className="truncate text-sm font-semibold">
                      {plugin.position.toString().padStart(2, "0")} ·{" "}
                      {plugin.name}
                    </h3>
                    <Badge tone={plugin.enabled ? "accent" : "neutral"} dot>
                      {plugin.enabled
                        ? t("pages.plugins.stateEnabled")
                        : t("pages.plugins.statePaused")}
                    </Badge>
                  </div>
                  <p className="mt-1 truncate font-mono text-xs text-muted-foreground">
                    {plugin.endpoint}
                  </p>
                </div>
                <GatedSwitch
                  gate="plugin:update"
                  checked={plugin.enabled}
                  disabled={
                    togglingId === plugin.id || removingId === plugin.id
                  }
                  aria-label={t("pages.plugins.toggleAria", {
                    name: plugin.name,
                  })}
                  onCheckedChange={() => onToggle(plugin)}
                />
              </div>
              <p className="mt-3 line-clamp-2 text-xs text-muted-foreground">
                {plugin.description || t("pages.plugins.noDescription")}
              </p>
              <div className="mt-3 flex flex-wrap gap-1.5">
                <Badge tone="outline">
                  {plugin.project_id
                    ? (projectNames[plugin.project_id] ??
                      t("pages.plugins.scopeProject"))
                    : t("pages.plugins.scopeOrg")}
                </Badge>
                <Badge
                  tone={
                    plugin.failure_mode === "fail_closed" ? "danger" : "warning"
                  }
                >
                  {plugin.failure_mode === "fail_closed"
                    ? t("pages.plugins.failClosed")
                    : t("pages.plugins.failOpen")}
                </Badge>
                {plugin.secret_env && (
                  <Badge tone="info">{t("pages.plugins.secretRef")}</Badge>
                )}
              </div>
              <div className="mt-4 flex justify-end gap-2 border-t border-[color:var(--border-subtle)] pt-3">
                <GatedButton
                  gate="plugin:delete"
                  variant="ghost"
                  aria-label={t("pages.plugins.deleteAria", { name: plugin.name })}
                  disabled={removingId === plugin.id}
                  onClick={() => onDelete(plugin)}
                >
                  {removingId === plugin.id && (
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  )}
                  {t("pages.plugins.delete")}
                </GatedButton>
                <GatedButton
                  gate="plugin:update"
                  variant="outline"
                  aria-label={t("pages.plugins.configureAria", { name: plugin.name })}
                  onClick={() => onEdit(plugin)}
                >
                  {t("pages.plugins.configure")}
                </GatedButton>
              </div>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}

function PluginDialog({
  open,
  initial,
  orgId,
  projects,
  stages,
  onClose,
  onDone,
}: {
  open: boolean;
  initial: PluginInstanceRow | null;
  orgId: string;
  projects: Array<{ id: string; name: string }>;
  stages: StageCopy[];
  onClose: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [form, setForm] = React.useState({
    project_id: initial?.project_id ?? "",
    name: initial?.name ?? "",
    slug: initial?.slug ?? "",
    description: initial?.description ?? "",
    stage: initial?.stage ?? ("pre_route" as Stage),
    enabled: initial?.enabled ?? false,
    position: String(initial?.position ?? 0),
    failure_mode:
      initial?.failure_mode ??
      ("fail_open" as PluginInstanceRow["failure_mode"]),
    endpoint: initial?.endpoint ?? "https://plugins.internal/hook",
    secret_env: initial?.secret_env ?? "",
    config: JSON.stringify(initial?.config ?? {}, null, 2),
  });
  const [localError, setLocalError] = React.useState<string | null>(null);
  const mutation = useMutation({
    mutationFn: (body: PluginInstanceInput) =>
      initial ? updatePlugin(initial.id, body) : createPlugin(orgId, body),
    onSuccess: (_result, body) => {
      // the dialog closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push(
        initial
          ? {
              tone: "success",
              title: t("toast.saved"),
              detail: t("toast.savedDetail", { what: body.name }),
            }
          : { tone: "success", title: t("toast.created", { what: body.name }) },
      );
      onDone();
    },
    onError: (error, body) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: body.name }),
        detail: errorDetail(error),
      });
    },
  });
  const set = (patch: Partial<typeof form>) => {
    setForm((value) => ({ ...value, ...patch }));
    setLocalError(null);
  };
  const save = () => {
    let config: unknown;
    try {
      config = JSON.parse(form.config);
    } catch {
      setLocalError(t("pages.plugins.errorConfigJson"));
      return;
    }
    if (!config || Array.isArray(config) || typeof config !== "object") {
      setLocalError(t("pages.plugins.errorConfigObject"));
      return;
    }
    if (!form.name.trim() || !/^https?:\/\//.test(form.endpoint)) {
      setLocalError(t("pages.plugins.errorNameEndpoint"));
      return;
    }
    mutation.mutate({
      project_id: form.project_id || null,
      name: form.name,
      ...(initial
        ? { slug: initial.slug }
        : form.slug
          ? { slug: form.slug }
          : {}),
      description: form.description,
      kind: "webhook",
      stage: form.stage,
      enabled: form.enabled,
      position: Number(form.position),
      failure_mode: form.failure_mode,
      endpoint: form.endpoint,
      secret_env: form.secret_env || null,
      config: config as Record<string, unknown>,
    });
  };
  return (
    <Dialog open={open} onOpenChange={(value) => !value && onClose()}>
      <DialogHeader>
        <DialogTitle>
          {initial
            ? t("pages.plugins.dialogTitleEdit")
            : t("pages.plugins.dialogTitleNew")}
        </DialogTitle>
        <DialogDescription>
          {t("pages.plugins.dialogDescription")}
        </DialogDescription>
      </DialogHeader>
      <div className="max-h-[65vh] space-y-4 overflow-y-auto pr-1">
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label={t("pages.plugins.fieldName")} htmlFor="plugin-name">
            <Input
              id="plugin-name"
              value={form.name}
              onChange={(event) => set({ name: event.target.value })}
            />
          </Field>
          <Field
            label={t("pages.plugins.fieldSlug")}
            htmlFor="plugin-slug"
            hint={
              initial
                ? t("pages.plugins.hintSlugLocked")
                : t("pages.plugins.hintSlugDerived")
            }
          >
            <Input
              id="plugin-slug"
              disabled={Boolean(initial)}
              value={form.slug}
              onChange={(event) => set({ slug: event.target.value })}
            />
          </Field>
        </div>
        <Field
          label={t("pages.plugins.fieldDescription")}
          htmlFor="plugin-description"
        >
          <Input
            id="plugin-description"
            value={form.description}
            onChange={(event) => set({ description: event.target.value })}
          />
        </Field>
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label={t("pages.plugins.fieldScope")} htmlFor="plugin-scope">
            <Select
              id="plugin-scope"
              value={form.project_id}
              onChange={(event) => set({ project_id: event.target.value })}
            >
              <option value="">{t("pages.plugins.scopeOrgWide")}</option>
              {projects.map((project) => (
                <option key={project.id} value={project.id}>
                  {project.name}
                </option>
              ))}
            </Select>
          </Field>
          <Field label={t("pages.plugins.fieldStage")} htmlFor="plugin-stage">
            <Select
              id="plugin-stage"
              value={form.stage}
              onChange={(event) => set({ stage: event.target.value as Stage })}
            >
              {stages.map((stage) => (
                <option key={stage.key} value={stage.key}>
                  {stage.label}
                </option>
              ))}
            </Select>
          </Field>
        </div>
        <Field
          label={t("pages.plugins.fieldEndpoint")}
          htmlFor="plugin-endpoint"
        >
          <Input
            id="plugin-endpoint"
            type="url"
            value={form.endpoint}
            onChange={(event) => set({ endpoint: event.target.value })}
          />
        </Field>
        <div className="grid gap-3 sm:grid-cols-2">
          <Field
            label={t("pages.plugins.fieldPosition")}
            htmlFor="plugin-position"
            hint={t("pages.plugins.hintPosition")}
          >
            <Input
              id="plugin-position"
              type="number"
              min={0}
              value={form.position}
              onChange={(event) => set({ position: event.target.value })}
            />
          </Field>
          <Field
            label={t("pages.plugins.fieldFailure")}
            htmlFor="plugin-failure"
          >
            <Select
              id="plugin-failure"
              value={form.failure_mode}
              onChange={(event) =>
                set({
                  failure_mode: event.target
                    .value as PluginInstanceRow["failure_mode"],
                })
              }
            >
              <option value="fail_open">
                {t("pages.plugins.failOpenOption")}
              </option>
              <option value="fail_closed">
                {t("pages.plugins.failClosedOption")}
              </option>
            </Select>
          </Field>
        </div>
        <Field
          label={t("pages.plugins.fieldSecret")}
          htmlFor="plugin-secret"
          hint={t("pages.plugins.hintSecret")}
        >
          <Input
            id="plugin-secret"
            className="font-mono"
            placeholder="ROLTER_PLUGIN_TOKEN"
            value={form.secret_env}
            onChange={(event) => set({ secret_env: event.target.value })}
          />
        </Field>
        <Field
          label={t("pages.plugins.fieldConfig")}
          htmlFor="plugin-config"
          hint={t("pages.plugins.hintConfig")}
        >
          <Textarea
            id="plugin-config"
            className="font-mono text-xs"
            rows={6}
            value={form.config}
            onChange={(event) => set({ config: event.target.value })}
          />
        </Field>
        <div className="flex items-start justify-between gap-4 rounded-lg border border-[color:var(--border-subtle)] p-3">
          <div>
            <p className="text-sm font-medium">
              {t("pages.plugins.enabledTitle")}
            </p>
            <p className="mt-0.5 text-xs text-muted-foreground">
              {t("pages.plugins.enabledHint")}
            </p>
          </div>
          <Switch
            checked={form.enabled}
            aria-label={t("pages.plugins.enabledTitle")}
            onCheckedChange={(enabled) => set({ enabled })}
          />
        </div>
        {(localError || mutation.isError) && (
          <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">
            {localError ?? (mutation.error as Error).message}
          </p>
        )}
      </div>
      <DialogFooter>
        <Button variant="ghost" onClick={onClose}>
          {t("pages.plugins.cancel")}
        </Button>
        <Button disabled={mutation.isPending} onClick={save}>
          {mutation.isPending
            ? t("pages.plugins.saving")
            : initial
              ? t("pages.plugins.save")
              : t("pages.plugins.installConfirm")}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}
