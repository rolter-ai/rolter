import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Archive,
  ArchiveRestore,
  BookOpen,
  Check,
  ExternalLink,
  FileCode2,
  FilePlus2,
  Plus,
  RotateCcw,
  Settings2,
  ShieldCheck,
} from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { GatedButton } from "@/components/GatedButton";
import {
  ChangesPanel,
  CompareButton,
  DeleteBySlugDialog,
  MakeLiveDialog,
  MakeLiveLabel,
  RepositoryIndex,
  RepositoryLoadingState,
  RepositoryShell,
  SectionHeading,
  VersionBadge,
  VersionCompareDialog,
  VersionMeta,
  VersionRail,
  liveAction,
  useLatched,
  type LiveAction,
} from "@/components/versioned-repository";
import { Checkbox } from "@/components/ui/checkbox";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import {
  Dialog,
  DialogBody,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Combobox } from "@/components/ui/combobox";
import { EmptyState } from "@/components/ui/empty-state";
import { LoadError } from "@/components/LoadError";
import { LoadingRegion } from "@/components/LoadingState";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/components/ui/segmented";
import { Skeleton } from "@/components/ui/skeleton";
import { FieldDiff, TextDiff } from "@/components/ui/text-diff";
import { Textarea } from "@/components/ui/textarea";
import {
  createSkill,
  createSkillVersion,
  deleteSkill,
  fetchSkillVersions,
  fetchSkills,
  publishSkillVersion,
  rollbackSkillVersion,
  updateSkill,
  type CreateSkillInput,
  type SkillMinimumRole,
  type SkillRow,
  type SkillVersionRow,
  type TeamRow,
  type UpdateSkillInput,
} from "@/lib/api";
import { skillRoleLabel } from "@/lib/enum-labels";
import { errorText } from "@/lib/error-copy";
import { useScope } from "@/lib/scope";
import { useToast } from "@/lib/toast";
import { useDraftSwitch } from "@/lib/use-draft-switch";
import { useLeaveGuard } from "@/lib/use-leave-guard";
import { diffSkillVersions } from "@/lib/version-diff";
import { useScreenReady } from "@/lib/ux-react";

type SourceMode = "inline" | "reference";

interface Draft {
  mode: SourceMode;
  content: string;
  contentRef: string;
  metadataText: string;
}

const EMPTY_DRAFT: Draft = {
  mode: "inline",
  content: "---\nname: my-skill\ndescription:\n---\n",
  contentRef: "",
  metadataText: "{}",
};

function draftFromVersion(version?: SkillVersionRow): Draft {
  if (!version) return { ...EMPTY_DRAFT };
  return {
    mode: version.content_ref ? "reference" : "inline",
    content: version.content ?? "",
    contentRef: version.content_ref ?? "",
    metadataText: JSON.stringify(version.metadata, null, 2),
  };
}

/** the draft as a comparable string, for telling an edited workbench from a clean one */
function draftKey(draft: Draft): string {
  return JSON.stringify([draft.mode, draft.content, draft.contentRef, draft.metadataText]);
}

/** a catalog key rather than English, so validation can be translated where it
 * is rendered instead of shipping a hardcoded sentence out of a helper */
type ProblemKey =
  | "metadataNotObject"
  | "metadataInvalid"
  | "inlineRequired"
  | "refRequired"
  | "refScheme"
  | "refCredentials";

function parseMetadata(value: string): { value?: Record<string, unknown>; error?: ProblemKey } {
  try {
    const parsed = JSON.parse(value) as unknown;
    if (!parsed || Array.isArray(parsed) || typeof parsed !== "object") {
      return { error: "metadataNotObject" };
    }
    return { value: parsed as Record<string, unknown> };
  } catch {
    return { error: "metadataInvalid" };
  }
}

function draftProblem(draft: Draft): ProblemKey | undefined {
  if (draft.mode === "inline" && !draft.content.trim()) return "inlineRequired";
  if (draft.mode === "reference") {
    if (!draft.contentRef.trim()) return "refRequired";
    if (!/^(https|git\+https|oci|s3):\/\//.test(draft.contentRef.trim())) {
      return "refScheme";
    }
    const authority = draft.contentRef.trim().split("://")[1]?.split("/")[0];
    if (!authority || authority.includes("@")) return "refCredentials";
  }
  return parseMetadata(draft.metadataText).error;
}

export default function SkillsRepository() {
  const { t } = useTranslation();
  const scope = useScope();
  // the scope hook names a catalog key rather than carrying english copy
  const scopeMessage = scope.errorKey ? t(scope.errorKey) : undefined;
  const queryClient = useQueryClient();
  const toast = useToast();
  const [selectedId, setSelectedId] = React.useState<string>();
  const [selectedVersion, setSelectedVersion] = React.useState<number>();
  const [draft, setDraft] = React.useState<Draft>({ ...EMPTY_DRAFT });
  // what the draft was last seeded with: the draft differs from it exactly
  // when there is something to lose (#2115)
  const [seeded, setSeeded] = React.useState<Draft>();
  const [createOpen, setCreateOpen] = React.useState(false);
  const [settingsOpen, setSettingsOpen] = React.useState(false);
  const [deleteOpen, setDeleteOpen] = React.useState(false);
  const [retireOpen, setRetireOpen] = React.useState(false);
  const [compareOpen, setCompareOpen] = React.useState(false);
  // the version waiting on the publish / roll back confirmation, with the
  // direction fixed when the button was pressed: once a publish lands the
  // version *is* the live one, and a direction read again would call the
  // landing a roll back
  const [liveTarget, setLiveTarget] = React.useState<{ version: number; action: LiveAction }>();

  const skills = useQuery({
    queryKey: ["skills", scope.orgId],
    queryFn: () => fetchSkills(scope.orgId as string),
    enabled: !!scope.orgId,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;

  // `skills` is the query the user is actually waiting on for this screen

  useScreenReady(!skills.isLoading);

  const selected = skills.data?.find((skill) => skill.id === selectedId);

  React.useEffect(() => {
    if (!skills.data?.length) {
      setSelectedId(undefined);
      return;
    }
    if (!selectedId || !skills.data.some((skill) => skill.id === selectedId)) {
      setSelectedId(skills.data[0].id);
    }
  }, [selectedId, skills.data]);

  const versions = useQuery({
    queryKey: ["skill-versions", selectedId],
    queryFn: () => fetchSkillVersions(selectedId as string),
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
    if (
      !selectedVersion ||
      !orderedVersions.some((version) => version.version === selectedVersion)
    ) {
      setSelectedVersion(orderedVersions[0].version);
    }
  }, [orderedVersions, selectedVersion]);

  const baseVersion = orderedVersions.find((version) => version.version === selectedVersion);
  React.useEffect(() => {
    if (!versions.isSuccess) return;
    const next = draftFromVersion(baseVersion);
    setDraft(next);
    setSeeded(next);
  }, [baseVersion, selectedId, versions.isSuccess]);
  const dirty = !!seeded && draftKey(draft) !== draftKey(seeded);
  const leavePrompt = useLeaveGuard(dirty);
  const switchGuard = useDraftSwitch(dirty);

  const create = useMutation({
    mutationFn: (input: CreateSkillInput) => createSkill(scope.orgId as string, input),
    onSuccess: (skill) => {
      queryClient.setQueryData<SkillRow[]>(["skills", scope.orgId], (current = []) => [
        ...current,
        skill,
      ]);
      setCreateOpen(false);
      // the new row is selected through the same guard as any other switch: the
      // open draft's unsaved edits are not thrown away without a word (#3118).
      // cancelling leaves the skill created and the edits where they were
      switchGuard.request(() => {
        setSelectedId(skill.id);
        setSelectedVersion(undefined);
      });
      // the workbench's notice strip vanished the moment the draft was
      // touched; the outcome belongs in the toast queue instead (#1197)
      toast.push({ tone: "success", title: t("pages.skillsRepo.created") });
    },
    onError: (error, input) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: input.name }),
        detail: errorText(error, t),
      });
    },
  });

  const save = useMutation({
    mutationFn: () => {
      const metadata = parseMetadata(draft.metadataText).value as Record<string, unknown>;
      return createSkillVersion(
        selectedId as string,
        draft.mode === "inline"
          ? { content: draft.content.trim(), metadata }
          : { content_ref: draft.contentRef.trim(), metadata },
      );
    },
    onSuccess: async (version) => {
      await queryClient.invalidateQueries({ queryKey: ["skill-versions", selectedId] });
      setSelectedVersion(version.version);
      toast.push({
        tone: "success",
        title: t("pages.skillsRepo.versionSaved", { version: version.version }),
      });
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: selected?.name ?? "" }),
        detail: errorText(error, t),
      });
    },
  });

  // one mutation for both directions, so both go through one confirmation;
  // the endpoint still follows the direction, since it is what the audit log
  // records the change as
  const makeLive = useMutation({
    mutationFn: ({ version, action }: { version: number; action: LiveAction }) =>
      action === "publish"
        ? publishSkillVersion(selectedId as string, version)
        : rollbackSkillVersion(selectedId as string, version),
    onSuccess: (skill, { action }) => {
      replaceSkill(queryClient, scope.orgId, skill);
      setLiveTarget(undefined);
      toast.push({
        tone: "success",
        title:
          action === "publish"
            ? t("pages.skillsRepo.publishedNotice", { version: skill.published_version })
            : t("pages.skillsRepo.rolledBack", { version: skill.published_version }),
      });
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: selected?.name ?? "" }),
        detail: errorText(error, t),
      });
    },
  });
  const liveVersion = orderedVersions.find(
    (version) => version.version === selected?.published_version,
  );
  // the request is latched through the closing render, see `useLatched`
  const shownTarget = useLatched(liveTarget);
  const targetVersion = orderedVersions.find((version) => version.version === shownTarget?.version);
  const confirmMakeLive = (version: number) =>
    setLiveTarget({ version, action: liveAction(version, selected?.published_version) });

  const remove = useMutation({
    mutationFn: () => deleteSkill(selectedId as string),
    onSuccess: async () => {
      const removedId = selectedId;
      const what = selected?.name ?? "";
      // clear the selection first so the workbench never renders against a
      // skill the control plane has already dropped
      setSelectedId(undefined);
      setSelectedVersion(undefined);
      setDeleteOpen(false);
      queryClient.setQueryData<SkillRow[]>(["skills", scope.orgId], (current = []) =>
        current.filter((item) => item.id !== removedId),
      );
      await queryClient.invalidateQueries({ queryKey: ["skills", scope.orgId] });
      toast.push({ tone: "success", title: t("toast.deleted", { what }) });
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.deleteFailed", { what: selected?.name ?? "" }),
        detail: errorText(error, t),
      });
    },
  });

  const settings = useMutation({
    mutationFn: (input: UpdateSkillInput) => updateSkill(selectedId as string, input),
    onSuccess: (skill) => {
      replaceSkill(queryClient, scope.orgId, skill);
      setSettingsOpen(false);
      toast.push({ tone: "success", title: t("pages.skillsRepo.settingsUpdated") });
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: selected?.name ?? "" }),
        detail: errorText(error, t),
      });
    },
  });

  // retiring is a lifecycle move of its own, not a field of the settings form:
  // it hides the skill from every team at once
  const retire = useMutation({
    mutationFn: (retired: boolean) => updateSkill(selectedId as string, { retired }),
    onSuccess: (skill) => {
      replaceSkill(queryClient, scope.orgId, skill);
      setRetireOpen(false);
      toast.push({
        tone: "success",
        title: skill.retired_at
          ? t("pages.skillsRepo.retiredNotice")
          : t("pages.skillsRepo.restoredNotice"),
      });
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: selected?.name ?? "" }),
        detail: errorText(error, t),
      });
    },
  });

  if (scope.isLoading || skills.isLoading) return <RepositoryLoadingState />;
  // never hand-rolled: the bespoke "repository unavailable / Try again" panel
  // offered the same retry for every cause, including the 403 a viewer gets
  // here, where retrying cannot ever work. LoadError names the cause and only
  // offers the action that fits it (#1605, the bug #1259 fixed on the
  // guardrail screens)
  if (skills.isError) {
    return (
      <div className="p-4 sm:p-5">
        <LoadError
          error={skills.error}
          resource={t("errors.resources.skills")}
          onRetry={() => void skills.refetch()}
          target="skill-list"
        />
      </div>
    );
  }
  // a scope that is not chosen yet is not a failed load: nothing was asked for
  // and nothing went wrong, so this stays a placeholder with no retry
  if (scope.errorKey) {
    return (
      <EmptyState
        uxTarget="skill-list"
        icon={<BookOpen />}
        title={t("pages.skillsRepo.unavailableTitle")}
        description={scopeMessage}
      />
    );
  }

  return (
    <RepositoryShell
      wide={!selected}
      index={
        <RepositoryIndex
          label={t("pages.skillsRepo.orgSkills")}
          count={t("pages.skillsRepo.visibleCount", { count: skills.data?.length ?? 0 })}
          empty={t("pages.skillsRepo.noSkills")}
          create={
            <GatedButton
              gate="skill:create"
              control="skill-new"
              variant="ghost"
              onClick={() => setCreateOpen(true)}
              aria-label={t("pages.skillsRepo.createSkill")}
            >
              <Plus className="h-4 w-4" />
            </GatedButton>
          }
          items={(skills.data ?? []).map((skill) => ({
            id: skill.id,
            name: skill.name,
            slug: skill.slug,
            badge: skill.retired_at ? (
              <Badge tone="neutral">{t("pages.skillsRepo.retired")}</Badge>
            ) : (
              <VersionBadge
                version={skill.published_version}
                draft={t("pages.skillsRepo.draftBadge")}
              />
            ),
          }))}
          selectedId={selectedId}
          onSelect={(id) =>
            id !== selectedId &&
            switchGuard.request(() => {
              setSelectedId(id);
              setSelectedVersion(undefined);
            })
          }
        />
      }
      workbench={
        !selected ? (
          <main className="flex min-h-[32rem] items-center justify-center rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]">
            <EmptyState
              thread
              uxTarget="skill-versions"
              icon={<FilePlus2 />}
              title={t("pages.skillsRepo.emptyTitle")}
              description={t("pages.skillsRepo.emptyDescription")}
              actions={
                <GatedButton
                  gate="skill:create"
                  control="skill-new-empty"
                  onClick={() => setCreateOpen(true)}
                >
                  {t("pages.skillsRepo.createSkill")}
                </GatedButton>
              }
            />
          </main>
        ) : versions.isLoading ? (
          <LoadingRegion className="min-h-[32rem] rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)] p-5">
            <Skeleton width="45%" height={24} radius={6} data-testid="skill-workbench-loading" />
            <div className="mt-5 space-y-3">
              <Skeleton width="100%" height={74} radius={8} />
              <Skeleton width="100%" height={360} radius={8} />
              <Skeleton width="100%" height={160} radius={8} />
            </div>
          </LoadingRegion>
        ) : versions.isError ? (
          /* a 403 on the versions endpoint is not retryable, and the hand-rolled
             panel offered a "Try again" that could only fail again (#1259, #1618) */
          <main className="flex min-h-[32rem] items-center justify-center rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]">
            <LoadError
              error={versions.error as Error}
              resource={t("errors.resources.skillVersions")}
              onRetry={() => versions.refetch()}
              target="skill-versions"
            />
          </main>
        ) : (
          <SkillWorkbench
            skill={selected}
            baseVersion={baseVersion}
            liveVersion={liveVersion}
            draft={draft}
            teams={scope.teams}
            dirty={dirty}
            pending={save.isPending || makeLive.isPending}
            error={save.error as Error | null}
            onDraftChange={setDraft}
            onSave={() => save.mutate()}
            onMakeLive={confirmMakeLive}
            onCompare={() => setCompareOpen(true)}
            onSettings={() => setSettingsOpen(true)}
            onRetire={() => setRetireOpen(true)}
            onDelete={() => setDeleteOpen(true)}
          />
        )
      }
      rail={
        selected && (
          <VersionRail
            label={t("repository.history")}
            hint={t("pages.skillsRepo.versionHistoryHint")}
            versions={orderedVersions}
            publishedVersion={selected.published_version}
            selectedVersion={selectedVersion}
            loading={versions.isLoading}
            loadingTestId="skill-versions-loading"
            emptyText={t("repository.noSavedVersions")}
            renderMakeLive={(version, action) =>
              selected.retired_at ? null : action === "publish" ? (
                <GatedButton
                  gate="skill:update"
                  control="skill-rail-publish"
                  variant="ghost"
                  onClick={() => confirmMakeLive(version)}
                >
                  <MakeLiveLabel action={action} version={version} size="sm" />
                </GatedButton>
              ) : (
                <GatedButton
                  gate="skill:update"
                  control="skill-rollback"
                  variant="ghost"
                  onClick={() => confirmMakeLive(version)}
                >
                  <MakeLiveLabel action={action} version={version} size="sm" />
                </GatedButton>
              )
            }
            renderMeta={(version) => (
              <VersionMeta>
                {version.content_ref ? (
                  <>
                    <ExternalLink className="h-3 w-3" />
                    {t("pages.skillsRepo.artifactReferenceMeta")}
                  </>
                ) : (
                  <>
                    <FileCode2 className="h-3 w-3" />
                    {t("pages.skillsRepo.inlineContentMeta")}
                  </>
                )}
              </VersionMeta>
            )}
            onSelect={(version) =>
              version !== selectedVersion && switchGuard.request(() => setSelectedVersion(version))
            }
          />
        )
      }
    >
      <CreateSkillDialog
        open={createOpen}
        teams={scope.teams}
        pending={create.isPending}
        error={create.error as Error | null}
        onOpenChange={setCreateOpen}
        onSubmit={(input) => create.mutate(input)}
      />
      {selected && (
        <SkillSettingsDialog
          open={settingsOpen}
          skill={selected}
          teams={scope.teams}
          pending={settings.isPending}
          error={settings.error as Error | null}
          onOpenChange={setSettingsOpen}
          onSubmit={(input) => settings.mutate(input)}
        />
      )}
      <SkillMakeLiveDialog
        skill={selected}
        open={!!liveTarget}
        request={shownTarget}
        target={targetVersion}
        live={liveVersion}
        pending={makeLive.isPending}
        error={makeLive.error}
        onOpenChange={(open) => {
          if (open) return;
          setLiveTarget(undefined);
          // a refusal for one version must not greet the next one opened
          makeLive.reset();
        }}
        onConfirm={() => liveTarget && makeLive.mutate(liveTarget)}
      />
      <VersionCompareDialog
        open={compareOpen && !!liveVersion && !!baseVersion}
        live={liveVersion?.version}
        target={baseVersion?.version}
        onOpenChange={setCompareOpen}
      >
        {liveVersion && baseVersion && <SkillChanges live={liveVersion} target={baseVersion} />}
      </VersionCompareDialog>
      <RetireDialog
        open={retireOpen && !!selected}
        skill={selected}
        pending={retire.isPending}
        error={retire.error}
        onOpenChange={(next) => {
          setRetireOpen(next);
          if (!next) retire.reset();
        }}
        onConfirm={() => selected && retire.mutate(!selected.retired_at)}
      />
      {switchGuard.prompt}
      {leavePrompt}
      <DeleteSkillDialog
        open={deleteOpen && !!selected}
        skill={selected}
        pending={remove.isPending}
        error={remove.error as Error | null}
        onOpenChange={(next) => {
          setDeleteOpen(next);
          // a refusal for this skill must not greet the next one opened
          if (!next) remove.reset();
        }}
        onConfirm={() => remove.mutate()}
      />
    </RepositoryShell>
  );
}

function DeleteSkillDialog({
  open,
  skill,
  pending,
  error,
  onOpenChange,
  onConfirm,
}: {
  open: boolean;
  skill?: SkillRow;
  pending: boolean;
  error: Error | null;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}) {
  const { t } = useTranslation();
  // retiring hides a skill but keeps its history; deleting drops every
  // immutable version, so the operator retypes the slug first
  return (
    <DeleteBySlugDialog
      uxName="skill-delete"
      open={open}
      onOpenChange={onOpenChange}
      slug={skill?.slug}
      title={t("pages.skillsRepo.confirm.deleteTitle", { name: skill?.name ?? "" })}
      description={
        <>
          {skill?.published_version
            ? t("pages.skillsRepo.confirm.deletePublished", { version: skill.published_version })
            : t("pages.skillsRepo.confirm.deleteUnpublished")}{" "}
          {t("pages.skillsRepo.confirm.deleteConsequence")}
        </>
      }
      slugLabel={t("pages.skillsRepo.confirm.deleteSlugLabel", { slug: skill?.slug ?? "" })}
      confirmLabel={t("pages.skillsRepo.confirm.deleteConfirm")}
      pending={pending}
      error={error}
      onConfirm={onConfirm}
    />
  );
}

function replaceSkill(
  queryClient: ReturnType<typeof useQueryClient>,
  orgId: string | undefined,
  skill: SkillRow,
) {
  queryClient.setQueryData<SkillRow[]>(["skills", orgId], (current = []) =>
    current.map((item) => (item.id === skill.id ? skill : item)),
  );
}

function SkillWorkbench({
  skill,
  baseVersion,
  liveVersion,
  draft,
  teams,
  dirty,
  pending,
  error,
  onDraftChange,
  onSave,
  onMakeLive,
  onCompare,
  onSettings,
  onRetire,
  onDelete,
}: {
  skill: SkillRow;
  baseVersion?: SkillVersionRow;
  /** the version callers resolve now, to compare the selected one with */
  liveVersion?: SkillVersionRow;
  draft: Draft;
  teams: TeamRow[];
  /** the draft differs from the version it was seeded with */
  dirty: boolean;
  pending: boolean;
  error: Error | null;
  onDraftChange: (draft: Draft) => void;
  onSave: () => void;
  onMakeLive: (version: number) => void;
  onCompare: () => void;
  onSettings: () => void;
  onRetire: () => void;
  onDelete: () => void;
}) {
  const { t } = useTranslation();
  const sourceHintId = React.useId();
  const action = baseVersion && liveAction(baseVersion.version, skill.published_version);
  // the allowed teams by name; a team outside the loaded list is still named,
  // by the start of its id
  const teamNames = skill.allowed_team_ids.map(
    (id) => teams.find((team) => team.id === id)?.name ?? id.slice(0, 8),
  );
  const problemKey = draftProblem(draft);
  const problem = problemKey && t(`pages.skillsRepo.${problemKey}`);
  const selectedPublished = baseVersion?.version === skill.published_version;
  // settings, publish and save all land on a `skill:update` guard in
  // crates/rolter-control/src/crud/skills.rs — publishing is a version pointer move,
  // not a capability of its own — and only the delete asks for `skill:delete`
  return (
    <main className="min-w-0 overflow-hidden rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]">
      <header className="border-b border-[color:var(--border-subtle)] px-4 py-3 sm:px-5">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <h2 className="truncate text-lg font-semibold tracking-[-0.02em]">{skill.name}</h2>
              {skill.retired_at ? (
                <Badge tone="neutral">{t("pages.skillsRepo.retired")}</Badge>
              ) : skill.published_version ? (
                <Badge tone="success" dot>
                  {t("pages.skillsRepo.liveBadge", { version: skill.published_version })}
                </Badge>
              ) : (
                <Badge tone="warning">{t("pages.skillsRepo.unpublished")}</Badge>
              )}
            </div>
            <p className="mt-1 max-w-2xl text-sm text-muted-foreground">
              {skill.description || t("pages.skillsRepo.noDescription")}
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <GatedButton
              gate="skill:update"
              control="skill-settings"
              variant="ghost"
              onClick={onSettings}
            >
              <Settings2 className="h-4 w-4" /> {t("pages.skillsRepo.settings")}
            </GatedButton>
            <GatedButton
              gate="skill:update"
              control="skill-retire"
              variant="ghost"
              onClick={onRetire}
            >
              {skill.retired_at ? (
                <ArchiveRestore className="h-4 w-4" />
              ) : (
                <Archive className="h-4 w-4" />
              )}{" "}
              {skill.retired_at ? t("pages.skillsRepo.restore") : t("pages.skillsRepo.retire")}
            </GatedButton>
            <DeleteIconButton
              gate="skill:delete"
              control="skill-delete"
              label={t("pages.skillsRepo.deleteAction", { name: skill.name })}
              onClick={onDelete}
            />
            {baseVersion && liveVersion && !selectedPublished && (
              <CompareButton live={liveVersion.version} onClick={onCompare} />
            )}
            {baseVersion && !selectedPublished && !skill.retired_at && action === "publish" && (
              <GatedButton
                gate="skill:update"
                control="skill-publish"
                variant="outline"
                disabled={pending}
                onClick={() => onMakeLive(baseVersion.version)}
              >
                <Check className="h-4 w-4" />{" "}
                {t("repository.publishVersion", { version: baseVersion.version })}
              </GatedButton>
            )}
            {baseVersion && !selectedPublished && !skill.retired_at && action === "rollback" && (
              <GatedButton
                gate="skill:update"
                control="skill-header-rollback"
                variant="outline"
                disabled={pending}
                onClick={() => onMakeLive(baseVersion.version)}
              >
                <RotateCcw className="h-4 w-4" />{" "}
                {t("repository.rollbackTo", { version: baseVersion.version })}
              </GatedButton>
            )}
            <GatedButton
              gate="skill:update"
              control="skill-save-version"
              disabled={pending || !!problem || !!skill.retired_at}
              onClick={onSave}
            >
              <FilePlus2 className="h-4 w-4" />
              {pending ? t("pages.skillsRepo.saving") : t("pages.skillsRepo.saveNewVersion")}
            </GatedButton>
          </div>
        </div>
        <div className="mt-3 flex min-h-5 flex-wrap items-center gap-x-3 gap-y-1 text-xs">
          <span className="font-mono text-[color:var(--text-subtle)]">{skill.slug}</span>
          <span className="text-muted-foreground">
            {t("pages.skillsRepo.minimumRoleMeta", { role: skillRoleLabel(skill.minimum_role, t) })}
          </span>
          <span
            className="max-w-full truncate text-muted-foreground"
            title={teamNames.length ? teamNames.join(", ") : undefined}
          >
            {teamNames.length
              ? t("pages.skillsRepo.teamsMeta", { names: teamNames.join(", ") })
              : t("pages.skillsRepo.allOrgTeams")}
          </span>
          {baseVersion && (
            <span className="text-muted-foreground">
              {t("pages.skillsRepo.editingFrom", { version: baseVersion.version })}
            </span>
          )}
          {dirty && (
            <span className="font-medium text-[color:var(--status-warning-text)]">
              {t("pages.skillsRepo.unsavedChanges")}
            </span>
          )}
          {problem && (
            <span role="alert" className="text-[color:var(--status-danger-text)]">
              {problem}
            </span>
          )}
          {error && (
            <span role="alert" className="text-[color:var(--status-danger-text)]">
              {errorText(error, t)}
            </span>
          )}
        </div>
      </header>

      <div className="space-y-7 p-4 sm:p-5">
        <section>
          <SectionHeading
            title={t("pages.skillsRepo.contentSourceTitle")}
            description={t("pages.skillsRepo.contentSourceDescription")}
          />
          <Segmented
            ariaLabel={t("pages.skillsRepo.sourceRadioGroup")}
            describedBy={sourceHintId}
            value={draft.mode}
            options={[
              { value: "inline", label: t("pages.skillsRepo.inlineTitle") },
              { value: "reference", label: t("pages.skillsRepo.referenceTitle") },
            ]}
            onChange={(mode) => onDraftChange({ ...draft, mode })}
          />
          <p id={sourceHintId} className="mb-3 mt-2 text-xs text-muted-foreground">
            {draft.mode === "inline"
              ? t("pages.skillsRepo.inlineDescription")
              : t("pages.skillsRepo.referenceDescription")}
          </p>
          {draft.mode === "inline" ? (
            <label className="block text-xs font-medium">
              {t("pages.skillsRepo.skillMdContent")}
              <Textarea
                className="mt-1 font-mono"
                rows={18}
                value={draft.content}
                onChange={(event) => onDraftChange({ ...draft, content: event.target.value })}
              />
            </label>
          ) : (
            <label className="block text-xs font-medium">
              {t("pages.skillsRepo.contentReference")}
              <Input
                className="mt-1 font-mono"
                aria-label={t("pages.skillsRepo.contentReference")}
                value={draft.contentRef}
                placeholder="oci://registry.example/skills/support@sha256:…"
                onChange={(event) => onDraftChange({ ...draft, contentRef: event.target.value })}
              />
              <span className="mt-1 block text-2xs font-normal text-muted-foreground">
                {t("pages.skillsRepo.contentRefHint")}
              </span>
            </label>
          )}
        </section>

        <section>
          <SectionHeading
            title={t("pages.skillsRepo.versionMetadataTitle")}
            description={t("pages.skillsRepo.versionMetadataDescription")}
          />
          <label className="block text-xs font-medium">
            {t("pages.skillsRepo.metadataJson")}
            <Textarea
              className="mt-1 font-mono"
              rows={7}
              value={draft.metadataText}
              onChange={(event) => onDraftChange({ ...draft, metadataText: event.target.value })}
            />
          </label>
          <p className="mt-2 flex items-center gap-1.5 text-2xs text-muted-foreground">
            <ShieldCheck className="h-3.5 w-3.5" />
            {t("pages.skillsRepo.secretFieldsRejected")}
          </p>
        </section>
      </div>
    </main>
  );
}

interface SkillFormValue {
  name: string;
  slug?: string;
  description: string;
  minimum_role: SkillMinimumRole;
  allowed_team_ids: string[];
}

function AccessFields({
  value,
  teams,
  includeSlug,
  onChange,
}: {
  value: SkillFormValue;
  teams: TeamRow[];
  includeSlug?: boolean;
  onChange: (value: SkillFormValue) => void;
}) {
  const { t } = useTranslation();
  const toggleTeam = (id: string) =>
    onChange({
      ...value,
      allowed_team_ids: value.allowed_team_ids.includes(id)
        ? value.allowed_team_ids.filter((teamId) => teamId !== id)
        : [...value.allowed_team_ids, id],
    });
  return (
    <div className="space-y-3">
      <Field label={t("pages.skillsRepo.fieldName")} required>
        <Input
          value={value.name}
          onChange={(event) => onChange({ ...value, name: event.target.value })}
        />
      </Field>
      {includeSlug && (
        <label className="block text-xs font-medium">
          {t("pages.skillsRepo.fieldSlug")}{" "}
          <span className="font-normal text-muted-foreground">
            {t("pages.skillsRepo.fieldOptional")}
          </span>
          <Input
            className="mt-1"
            value={value.slug ?? ""}
            placeholder="support-triage"
            onChange={(event) => onChange({ ...value, slug: event.target.value })}
          />
        </label>
      )}
      <label className="block text-xs font-medium">
        {t("pages.skillsRepo.fieldDescription")}
        <Textarea
          className="mt-1"
          rows={3}
          value={value.description}
          onChange={(event) => onChange({ ...value, description: event.target.value })}
        />
      </label>
      <label className="block text-xs font-medium">
        {t("pages.skillsRepo.fieldMinimumRole")}
        <Combobox
          className="mt-1"
          value={value.minimum_role}
          onChange={(picked) => onChange({ ...value, minimum_role: picked as SkillMinimumRole })}
          options={[
            { value: "viewer", label: t("pages.skillsRepo.roleViewer") },
            { value: "member", label: t("pages.skillsRepo.roleMember") },
            { value: "admin", label: t("pages.skillsRepo.roleAdmin") },
          ]}
        />
      </label>
      <fieldset>
        <legend className="text-xs font-medium">{t("pages.skillsRepo.allowedTeams")}</legend>
        <p className="mt-0.5 text-2xs text-muted-foreground">
          {t("pages.skillsRepo.allowedTeamsHint")}
        </p>
        <div className="mt-2 grid gap-2 sm:grid-cols-2">
          {teams.map((team) => (
            <label
              key={team.id}
              className="flex cursor-pointer items-center gap-2 rounded-lg border border-[color:var(--border-subtle)] p-2.5 text-sm hover:bg-[color:var(--surface-hover)]"
            >
              <Checkbox
                checked={value.allowed_team_ids.includes(team.id)}
                onChange={() => toggleTeam(team.id)}
              />
              {team.name}
            </label>
          ))}
        </div>
      </fieldset>
    </div>
  );
}

function CreateSkillDialog({
  open,
  teams,
  pending,
  error,
  onOpenChange,
  onSubmit,
}: {
  open: boolean;
  teams: TeamRow[];
  pending: boolean;
  error: Error | null;
  onOpenChange: (open: boolean) => void;
  onSubmit: (input: CreateSkillInput) => void;
}) {
  const { t } = useTranslation();
  const [value, setValue] = React.useState<SkillFormValue>({
    name: "",
    slug: "",
    description: "",
    minimum_role: "viewer",
    allowed_team_ids: [],
  });
  React.useEffect(() => {
    if (!open)
      setValue({
        name: "",
        slug: "",
        description: "",
        minimum_role: "viewer",
        allowed_team_ids: [],
      });
  }, [open]);
  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      onSubmit={() =>
        onSubmit({
          name: value.name.trim(),
          ...(value.slug?.trim() ? { slug: value.slug.trim() } : {}),
          ...(value.description.trim() ? { description: value.description.trim() } : {}),
          allowed_team_ids: value.allowed_team_ids,
          minimum_role: value.minimum_role,
        })
      }
      canSubmit={!!value.name.trim()}
      busy={pending}
    >
      <DialogHeader>
        <DialogTitle>{t("pages.skillsRepo.createTitle")}</DialogTitle>
        <DialogDescription>{t("pages.skillsRepo.createDescription")}</DialogDescription>
      </DialogHeader>
      <DialogBody>
        <AccessFields value={value} teams={teams} includeSlug onChange={setValue} />
        {error && (
          <p role="alert" className="mt-3 text-xs text-[color:var(--status-danger-text)]">
            {errorText(error, t)}
          </p>
        )}
      </DialogBody>
      <DialogFooter>
        <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
          {t("pages.skillsRepo.cancel")}
        </Button>
        <Button
          type="submit"
          disabled={pending}
          aria-disabled={!value.name.trim() || undefined}
          className="aria-disabled:opacity-50"
        >
          {pending ? t("pages.skillsRepo.creating") : t("pages.skillsRepo.createSkill")}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

function SkillSettingsDialog({
  open,
  skill,
  teams,
  pending,
  error,
  onOpenChange,
  onSubmit,
}: {
  open: boolean;
  skill: SkillRow;
  teams: TeamRow[];
  pending: boolean;
  error: Error | null;
  onOpenChange: (open: boolean) => void;
  onSubmit: (input: UpdateSkillInput) => void;
}) {
  const { t } = useTranslation();
  const fromSkill = React.useCallback(
    (): SkillFormValue => ({
      name: skill.name,
      description: skill.description,
      minimum_role: skill.minimum_role,
      allowed_team_ids: skill.allowed_team_ids,
    }),
    [skill],
  );
  const [value, setValue] = React.useState<SkillFormValue>(fromSkill);
  React.useEffect(() => {
    if (open) setValue(fromSkill());
  }, [fromSkill, open]);
  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      onSubmit={() =>
        onSubmit({
          name: value.name.trim(),
          description: value.description.trim(),
          minimum_role: value.minimum_role,
          allowed_team_ids: value.allowed_team_ids,
        })
      }
      canSubmit={!!value.name.trim()}
      busy={pending}
    >
      <DialogHeader>
        <DialogTitle>{t("pages.skillsRepo.settingsTitle")}</DialogTitle>
        <DialogDescription>{t("pages.skillsRepo.settingsDescription")}</DialogDescription>
      </DialogHeader>
      <DialogBody>
        <AccessFields value={value} teams={teams} onChange={setValue} />
        {error && (
          <p role="alert" className="mt-3 text-xs text-[color:var(--status-danger-text)]">
            {errorText(error, t)}
          </p>
        )}
      </DialogBody>
      <DialogFooter>
        <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
          {t("pages.skillsRepo.cancel")}
        </Button>
        <Button
          type="submit"
          disabled={pending}
          aria-disabled={!value.name.trim() || undefined}
          className="aria-disabled:opacity-50"
        >
          {pending ? t("pages.skillsRepo.saving") : t("pages.skillsRepo.saveSettings")}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

/**
 * What this screen says about moving the live pointer, forwards or back. The
 * dialog itself is the kit's.
 *
 * Skills are not consumed through the gateway yet (the `skills_repository`
 * stability note), so there is no scope or variable impact to show as the
 * prompt repository does; the dialog states which version callers resolve
 * before and after, how to undo it, and what changed between the two.
 */
function SkillMakeLiveDialog({
  skill,
  open,
  request,
  target,
  live,
  pending,
  error,
  onOpenChange,
  onConfirm,
}: {
  skill?: SkillRow;
  open: boolean;
  /** the move, kept through the closing render */
  request?: { version: number; action: LiveAction };
  target?: SkillVersionRow;
  live?: SkillVersionRow;
  pending: boolean;
  error: unknown;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}) {
  const { t } = useTranslation();
  const version = request?.version;
  const publishing = request?.action !== "rollback";
  const liveNumber = skill?.published_version;
  return (
    <MakeLiveDialog
      uxName="skill"
      open={open}
      action={request?.action}
      version={version}
      onOpenChange={onOpenChange}
      title={
        publishing
          ? t("pages.skillsRepo.confirm.publishTitle", { version, name: skill?.name ?? "" })
          : t("repository.rollbackTo", { version })
      }
      description={
        !publishing
          ? t("pages.skillsRepo.rollbackDescription", { from: liveNumber, to: version })
          : liveNumber
            ? t("pages.skillsRepo.confirm.publishBody", { from: liveNumber, to: version })
            : t("pages.skillsRepo.confirm.publishFirst", { version })
      }
      pending={pending}
      error={error}
      onConfirm={onConfirm}
    >
      {live && target && live.version !== target.version && (
        <SkillChanges live={live} target={target} collapsible />
      )}
    </MakeLiveDialog>
  );
}

/** what differs between two skill versions: the source, the content as text, the metadata by key (#2112) */
function SkillChanges({
  live,
  target,
  collapsible = false,
}: {
  live: SkillVersionRow;
  target: SkillVersionRow;
  collapsible?: boolean;
}) {
  const { t } = useTranslation();
  const diff = React.useMemo(
    () =>
      diffSkillVersions(live, target, {
        source: t("repository.diff.source"),
        reference: t("pages.skillsRepo.contentReference"),
        inline: t("pages.skillsRepo.inlineContentMeta"),
        artifact: t("pages.skillsRepo.artifactReferenceMeta"),
      }),
    [live, target, t],
  );
  const sourceChanged = diff.source.some((row) => row.kind !== "same");
  const contentChanged = diff.content.some((line) => line.kind !== "same");
  const metadataChanged = diff.metadata.some((row) => row.kind !== "same");
  return (
    <ChangesPanel
      live={live.version}
      target={target.version}
      changed={diff.changed}
      collapsible={collapsible}
    >
      {sourceChanged && (
        <FieldDiff changes={diff.source} label={t("pages.skillsRepo.contentSourceTitle")} />
      )}
      {contentChanged && (
        <TextDiff lines={diff.content} label={t("pages.skillsRepo.skillMdContent")} />
      )}
      {metadataChanged && (
        <FieldDiff changes={diff.metadata} label={t("pages.skillsRepo.versionMetadataTitle")} />
      )}
    </ChangesPanel>
  );
}

/** retiring and restoring share one confirmation, worded for the move being made */
function RetireDialog({
  open,
  skill,
  pending,
  error,
  onOpenChange,
  onConfirm,
}: {
  open: boolean;
  skill?: SkillRow;
  pending: boolean;
  error: unknown;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}) {
  const { t } = useTranslation();
  // the move is latched while the dialog is open: a retire that lands flips
  // `retired_at` on the render that closes it, and the landing would otherwise
  // be reported (and worded) as the opposite move
  const [latched, setLatched] = React.useState(!!skill?.retired_at);
  if (open && latched !== !!skill?.retired_at) setLatched(!!skill?.retired_at);
  const restoring = latched;
  const name = skill?.name ?? "";
  return (
    <ConfirmDialog
      name={restoring ? "skill-restore" : "skill-retire"}
      open={open}
      onOpenChange={onOpenChange}
      title={
        restoring
          ? t("pages.skillsRepo.confirm.restoreTitle", { name })
          : t("pages.skillsRepo.confirm.retireTitle", { name })
      }
      description={
        restoring
          ? t("pages.skillsRepo.confirm.restoreBody")
          : t("pages.skillsRepo.confirm.retireBody")
      }
      confirmLabel={
        restoring
          ? t("pages.skillsRepo.confirm.restoreConfirm")
          : t("pages.skillsRepo.confirm.retireConfirm")
      }
      tone="default"
      pending={pending}
      error={error}
      onConfirm={onConfirm}
    />
  );
}
