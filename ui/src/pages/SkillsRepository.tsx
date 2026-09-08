import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  BookOpen,
  Check,
  Clock3,
  ExternalLink,
  FileCode2,
  FilePlus2,
  Plus,
  RotateCcw,
  Settings2,
  ShieldCheck,
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
import { useFormat } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

type SourceMode = "inline" | "reference";

interface Draft {
  mode: SourceMode;
  content: string;
  contentRef: string;
  metadataText: string;
}

const EMPTY_DRAFT: Draft = {
  mode: "inline",
  content: "---\nname: my-skill\ndescription: Explain when this skill should run\n---\n\n# Instructions\n\nDescribe the workflow here.\n",
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
  const [createOpen, setCreateOpen] = React.useState(false);
  const [settingsOpen, setSettingsOpen] = React.useState(false);
  const [deleteOpen, setDeleteOpen] = React.useState(false);
  const [rollbackVersion, setRollbackVersion] = React.useState<number>();

  const skills = useQuery({
    queryKey: ["skills", scope.orgId],
    queryFn: () => fetchSkills(scope.orgId as string),
    enabled: !!scope.orgId,
  });


  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;

  // `skills` is the query the user is actually waiting on for this screen

  useScreenReady(!skills.isLoading);

  useErrorState(!!skills.error, "skills-repository");
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
    if (!selectedVersion || !orderedVersions.some((version) => version.version === selectedVersion)) {
      setSelectedVersion(orderedVersions[0].version);
    }
  }, [orderedVersions, selectedVersion]);

  const baseVersion = orderedVersions.find((version) => version.version === selectedVersion);
  React.useEffect(() => {
    if (versions.isSuccess) setDraft(draftFromVersion(baseVersion));
  }, [baseVersion, selectedId, versions.isSuccess]);

  const create = useMutation({
    mutationFn: (input: CreateSkillInput) => createSkill(scope.orgId as string, input),
    onSuccess: (skill) => {
      queryClient.setQueryData<SkillRow[]>(["skills", scope.orgId], (current = []) => [...current, skill]);
      setSelectedId(skill.id);
      setCreateOpen(false);
      // the workbench's notice strip vanished the moment the draft was
      // touched; the outcome belongs in the toast queue instead (#1197)
      toast.push({ tone: "success", title: t("pages.skillsRepo.created") });
    },
    onError: (error, input) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: input.name }),
        detail: errorDetail(error),
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
        detail: errorDetail(error),
      });
    },
  });

  const publish = useMutation({
    mutationFn: (version: number) => publishSkillVersion(selectedId as string, version),
    onSuccess: (skill) => {
      replaceSkill(queryClient, scope.orgId, skill);
      toast.push({
        tone: "success",
        title: t("pages.skillsRepo.publishedNotice", { version: skill.published_version }),
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
    mutationFn: (version: number) => rollbackSkillVersion(selectedId as string, version),
    onSuccess: (skill) => {
      replaceSkill(queryClient, scope.orgId, skill);
      setRollbackVersion(undefined);
      toast.push({
        tone: "success",
        title: t("pages.skillsRepo.rolledBack", { version: skill.published_version }),
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
        detail: errorDetail(error),
      });
    },
  });

  const settings = useMutation({
    mutationFn: (input: UpdateSkillInput) => updateSkill(selectedId as string, input),
    onSuccess: (skill) => {
      replaceSkill(queryClient, scope.orgId, skill);
      setSettingsOpen(false);
      toast.push({
        tone: "success",
        title: skill.retired_at
          ? t("pages.skillsRepo.retiredNotice")
          : t("pages.skillsRepo.settingsUpdated"),
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

  if (scope.isLoading || skills.isLoading) return <LoadingState />;
  if (scope.errorKey || skills.isError) {
    return (
      <EmptyState uxTarget="skill-list"
        icon={<BookOpen />}
        title={t("pages.skillsRepo.unavailableTitle")}
        description={scopeMessage ?? (skills.error as Error).message}
        actions={<Button variant="outline" onClick={() => skills.refetch()}>{t("pages.skillsRepo.tryAgain")}</Button>}
      />
    );
  }

  return (
    <div className="h-full min-h-0 overflow-y-auto p-4 sm:p-5">
      <div className="mx-auto grid min-h-full max-w-[1500px] gap-4 lg:grid-cols-[15rem_minmax(0,1fr)] xl:grid-cols-[14rem_minmax(0,1fr)_15rem] 2xl:grid-cols-[15rem_minmax(0,1fr)_17rem]">
        <SkillIndex
          skills={skills.data ?? []}
          selectedId={selectedId}
          onSelect={(id) => {
            setSelectedId(id);
            setSelectedVersion(undefined);
          }}
          onCreate={() => setCreateOpen(true)}
        />

        {!selected ? (
          <main className="flex min-h-[32rem] items-center justify-center rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)] xl:col-span-2">
            <EmptyState uxTarget="skill-versions"
              icon={<FilePlus2 />}
              title={t("pages.skillsRepo.emptyTitle")}
              description={t("pages.skillsRepo.emptyDescription")}
              actions={<GatedButton gate="skill:create" onClick={() => setCreateOpen(true)}>{t("pages.skillsRepo.createSkill")}</GatedButton>}
            />
          </main>
        ) : versions.isLoading ? (
          <main className="min-h-[32rem] rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)] p-5">
            <Skeleton width="45%" height={24} radius={6} />
            <div className="mt-5 space-y-3">
              <Skeleton width="100%" height={74} radius={8} />
              <Skeleton width="100%" height={360} radius={8} />
              <Skeleton width="100%" height={160} radius={8} />
            </div>
          </main>
        ) : versions.isError ? (
          <main className="flex min-h-[32rem] items-center justify-center rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]">
            <EmptyState uxTarget="skill-files"
              icon={<FileCode2 />}
              title={t("pages.skillsRepo.versionsUnavailable")}
              description={(versions.error as Error).message}
              actions={<Button variant="outline" onClick={() => versions.refetch()}>{t("pages.skillsRepo.tryAgain")}</Button>}
            />
          </main>
        ) : (
          <SkillWorkbench
            skill={selected}
            baseVersion={baseVersion}
            draft={draft}
            pending={save.isPending || publish.isPending}
            error={(save.error ?? publish.error) as Error | null}
            onDraftChange={setDraft}
            onSave={() => save.mutate()}
            onPublish={(version) => publish.mutate(version)}
            onSettings={() => setSettingsOpen(true)}
            onDelete={() => setDeleteOpen(true)}
          />
        )}

        {selected && (
          <VersionRail
            className="lg:col-start-2 xl:col-start-3 xl:row-start-1"
            skill={selected}
            versions={orderedVersions}
            selectedVersion={selectedVersion}
            loading={versions.isLoading}
            onSelect={setSelectedVersion}
            onRollback={setRollbackVersion}
          />
        )}
      </div>

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
      <RollbackDialog
        version={rollbackVersion}
        publishedVersion={selected?.published_version}
        pending={rollback.isPending}
        error={rollback.error as Error | null}
        onClose={() => setRollbackVersion(undefined)}
        onConfirm={() => rollbackVersion && rollback.mutate(rollbackVersion)}
      />
      <DeleteSkillDialog
        open={deleteOpen && !!selected}
        skill={selected}
        pending={remove.isPending}
        error={remove.error as Error | null}
        onOpenChange={setDeleteOpen}
        onConfirm={() => remove.mutate()}
      />
    </div>
  );
}

function DeleteSkillDialog({ open, skill, pending, error, onOpenChange, onConfirm }: { open: boolean; skill?: SkillRow; pending: boolean; error: Error | null; onOpenChange: (open: boolean) => void; onConfirm: () => void }) {
  const { t } = useTranslation();
  const [confirmation, setConfirmation] = React.useState("");
  React.useEffect(() => { if (!open) setConfirmation(""); }, [open]);
  // retiring hides a skill but keeps its history; deleting drops every
  // immutable version, so make the operator retype the slug first
  const matches = confirmation.trim() === skill?.slug;
  return <Dialog open={open} onOpenChange={onOpenChange}><DialogHeader><DialogTitle>{t("pages.skillsRepo.deleteTitle", { name: skill?.name ?? "" })}</DialogTitle><DialogDescription>{skill?.published_version ? t("pages.skillsRepo.deletePublished", { version: skill.published_version }) : t("pages.skillsRepo.deleteUnpublished")} {t("pages.skillsRepo.deleteConsequence")}</DialogDescription></DialogHeader><label className="block text-xs font-medium">{t("pages.skillsRepo.deleteConfirmLabel", { slug: skill?.slug ?? "" })}<Input className="mt-1" autoFocus value={confirmation} onChange={(event) => setConfirmation(event.target.value)} placeholder={skill?.slug} /></label>{error && <p role="alert" className="mt-2 text-xs text-[color:var(--status-danger-text)]">{error.message}</p>}<DialogFooter><Button variant="outline" onClick={() => onOpenChange(false)}>{t("pages.skillsRepo.cancel")}</Button><Button variant="destructive" disabled={pending || !matches} onClick={onConfirm}><Trash2 className="h-4 w-4" />{pending ? t("pages.skillsRepo.deleting") : t("pages.skillsRepo.deleteSubmit")}</Button></DialogFooter></Dialog>;
}

function replaceSkill(queryClient: ReturnType<typeof useQueryClient>, orgId: string | undefined, skill: SkillRow) {
  queryClient.setQueryData<SkillRow[]>(["skills", orgId], (current = []) =>
    current.map((item) => (item.id === skill.id ? skill : item)),
  );
}

function LoadingState() {
  return <div className="grid gap-4 p-5 lg:grid-cols-[15rem_minmax(0,1fr)_17rem]"><Skeleton width="100%" height={460} radius={12} /><Skeleton width="100%" height={620} radius={12} /><Skeleton width="100%" height={460} radius={12} /></div>;
}

function SkillIndex({ skills, selectedId, onSelect, onCreate }: { skills: SkillRow[]; selectedId?: string; onSelect: (id: string) => void; onCreate: () => void }) {
  const { t } = useTranslation();
  return (
    <aside className="overflow-hidden rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]">
      <div className="flex items-center justify-between border-b border-[color:var(--border-subtle)] px-3 py-2.5">
        <div><p className="text-xs font-semibold uppercase tracking-[0.12em] text-[color:var(--text-subtle)]">{t("pages.skillsRepo.orgSkills")}</p><p className="mt-0.5 text-xs text-muted-foreground">{t("pages.skillsRepo.visibleCount", { count: skills.length })}</p></div>
        <GatedButton gate="skill:create" variant="ghost" onClick={onCreate} aria-label={t("pages.skillsRepo.createSkill")}><Plus className="h-4 w-4" /></GatedButton>
      </div>
      <div className="max-h-[26rem] overflow-y-auto p-1.5 lg:max-h-[calc(100vh-14rem)]">
        {skills.length === 0 ? <p className="px-2 py-5 text-center text-xs text-muted-foreground">{t("pages.skillsRepo.noSkills")}</p> : skills.map((skill) => (
          <button key={skill.id} type="button" aria-current={selectedId === skill.id ? "page" : undefined} onClick={() => onSelect(skill.id)} className={cn("w-full rounded-lg px-2.5 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring", selectedId === skill.id ? "bg-[color:var(--surface-selected)]" : "text-[color:var(--text-secondary)] hover:bg-[color:var(--surface-hover)] hover:text-foreground")}>
            <span className="flex items-center gap-2"><span className="min-w-0 flex-1 truncate text-sm font-medium">{skill.name}</span>{skill.retired_at ? <Badge tone="neutral">{t("pages.skillsRepo.retired")}</Badge> : skill.published_version ? <span className="text-[0.6875rem] tabular-nums text-[color:var(--status-success-text)]">v{skill.published_version}</span> : <span className="text-[0.6875rem] text-[color:var(--text-subtle)]">{t("pages.skillsRepo.draftBadge")}</span>}</span>
            <span className="mt-0.5 block truncate font-mono text-[0.6875rem] text-[color:var(--text-subtle)]">{skill.slug}</span>
          </button>
        ))}
      </div>
    </aside>
  );
}

function SkillWorkbench({ skill, baseVersion, draft, pending, error, onDraftChange, onSave, onPublish, onSettings, onDelete }: { skill: SkillRow; baseVersion?: SkillVersionRow; draft: Draft; pending: boolean; error: Error | null; onDraftChange: (draft: Draft) => void; onSave: () => void; onPublish: (version: number) => void; onSettings: () => void; onDelete: () => void }) {
  const { t } = useTranslation();
  const problemKey = draftProblem(draft);
  const problem = problemKey && t(`pages.skillsRepo.${problemKey}`);
  const selectedPublished = baseVersion?.version === skill.published_version;
  // settings, publish and save all land on a `skill:update` guard in
  // crates/rolter-control/src/crud.rs — publishing is a version pointer move,
  // not a capability of its own — and only the delete asks for `skill:delete`
  return (
    <main className="min-w-0 overflow-hidden rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]">
      <header className="border-b border-[color:var(--border-subtle)] px-4 py-3 sm:px-5">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0"><div className="flex flex-wrap items-center gap-2"><h2 className="truncate text-lg font-semibold tracking-[-0.02em]">{skill.name}</h2>{skill.retired_at ? <Badge tone="neutral">{t("pages.skillsRepo.retired")}</Badge> : skill.published_version ? <Badge tone="success" dot>{t("pages.skillsRepo.liveBadge", { version: skill.published_version })}</Badge> : <Badge tone="warning">{t("pages.skillsRepo.unpublished")}</Badge>}</div><p className="mt-1 max-w-2xl text-sm text-muted-foreground">{skill.description || t("pages.skillsRepo.noDescription")}</p></div>
          <div className="flex flex-wrap items-center gap-2"><GatedButton gate="skill:update" variant="ghost" onClick={onSettings}><Settings2 className="h-4 w-4" /> {t("pages.skillsRepo.settings")}</GatedButton><GatedButton gate="skill:delete" variant="ghost" aria-label={t("pages.skillsRepo.deleteAction", { name: skill.name })} onClick={onDelete}><Trash2 className="h-4 w-4" /></GatedButton>{baseVersion && !selectedPublished && !skill.retired_at && <GatedButton gate="skill:update" variant="outline" disabled={pending} onClick={() => onPublish(baseVersion.version)}><Check className="h-4 w-4" /> {t("pages.skillsRepo.publishVersion", { version: baseVersion.version })}</GatedButton>}<GatedButton gate="skill:update" disabled={pending || !!problem || !!skill.retired_at} onClick={onSave}><FilePlus2 className="h-4 w-4" />{pending ? t("pages.skillsRepo.saving") : t("pages.skillsRepo.saveNewVersion")}</GatedButton></div>
        </div>
        <div className="mt-3 flex min-h-5 flex-wrap items-center gap-x-3 gap-y-1 text-xs"><span className="font-mono text-[color:var(--text-subtle)]">{skill.slug}</span><span className="text-muted-foreground">{t("pages.skillsRepo.minimumRoleMeta", { role: skill.minimum_role })}</span><span className="text-muted-foreground">{skill.allowed_team_ids.length ? t("pages.skillsRepo.teamsCount", { count: skill.allowed_team_ids.length }) : t("pages.skillsRepo.allOrgTeams")}</span>{baseVersion && <span className="text-muted-foreground">{t("pages.skillsRepo.editingFrom", { version: baseVersion.version })}</span>}{problem && <span role="alert" className="text-[color:var(--status-danger-text)]">{problem}</span>}{error && <span role="alert" className="text-[color:var(--status-danger-text)]">{error.message}</span>}</div>
      </header>

      <div className="space-y-7 p-4 sm:p-5">
        <section>
          <SectionHeading eyebrow={t("pages.skillsRepo.artifactEyebrow")} title={t("pages.skillsRepo.contentSourceTitle")} description={t("pages.skillsRepo.contentSourceDescription")} />
          <div className="mb-3 grid gap-2 sm:grid-cols-2" role="radiogroup" aria-label={t("pages.skillsRepo.sourceRadioGroup")}>
            <SourceOption selected={draft.mode === "inline"} title={t("pages.skillsRepo.inlineTitle")} description={t("pages.skillsRepo.inlineDescription")} onSelect={() => onDraftChange({ ...draft, mode: "inline" })} />
            <SourceOption selected={draft.mode === "reference"} title={t("pages.skillsRepo.referenceTitle")} description={t("pages.skillsRepo.referenceDescription")} onSelect={() => onDraftChange({ ...draft, mode: "reference" })} />
          </div>
          {draft.mode === "inline" ? <label className="block text-xs font-medium">{t("pages.skillsRepo.skillMdContent")}<Textarea className="mt-1 font-mono" rows={18} value={draft.content} onChange={(event) => onDraftChange({ ...draft, content: event.target.value })} /></label> : <label className="block text-xs font-medium">{t("pages.skillsRepo.contentReference")}<Input className="mt-1 font-mono" aria-label={t("pages.skillsRepo.contentReference")} value={draft.contentRef} placeholder="oci://registry.example/skills/support@sha256:…" onChange={(event) => onDraftChange({ ...draft, contentRef: event.target.value })} /><span className="mt-1 block text-[0.6875rem] font-normal text-muted-foreground">{t("pages.skillsRepo.contentRefHint")}</span></label>}
        </section>

        <section>
          <SectionHeading eyebrow={t("pages.skillsRepo.manifestEyebrow")} title={t("pages.skillsRepo.versionMetadataTitle")} description={t("pages.skillsRepo.versionMetadataDescription")} />
          <label className="block text-xs font-medium">{t("pages.skillsRepo.metadataJson")}<Textarea className="mt-1 font-mono" rows={7} value={draft.metadataText} onChange={(event) => onDraftChange({ ...draft, metadataText: event.target.value })} /></label>
          <p className="mt-2 flex items-center gap-1.5 text-[0.6875rem] text-muted-foreground"><ShieldCheck className="h-3.5 w-3.5" />{t("pages.skillsRepo.secretFieldsRejected")}</p>
        </section>
      </div>
    </main>
  );
}

function SourceOption({ selected, title, description, onSelect }: { selected: boolean; title: string; description: string; onSelect: () => void }) {
  return <button type="button" role="radio" aria-checked={selected} onClick={onSelect} className={cn("rounded-lg border p-3 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring", selected ? "border-[color:var(--red-folk)] bg-[color:var(--surface-selected)]" : "border-[color:var(--border-subtle)] hover:bg-[color:var(--surface-hover)]")}><span className="flex items-center gap-2 text-sm font-medium"><span className={cn("flex h-4 w-4 items-center justify-center rounded-full border", selected ? "border-[color:var(--red-folk)]" : "border-[color:var(--border-default)]")}>{selected && <span className="h-2 w-2 rounded-full bg-[color:var(--red-folk)]" />}</span>{title}</span><span className="ml-6 mt-1 block text-xs text-muted-foreground">{description}</span></button>;
}

function SectionHeading({ eyebrow, title, description }: { eyebrow: string; title: string; description: string }) {
  return <div className="mb-3"><p className="text-[0.6875rem] font-semibold uppercase tracking-[0.14em] text-[color:var(--red-folk-text)]">{eyebrow}</p><h3 className="mt-1 text-sm font-semibold">{title}</h3><p className="mt-0.5 text-xs leading-5 text-muted-foreground">{description}</p></div>;
}

function VersionRail({ className, skill, versions, selectedVersion, loading, onSelect, onRollback }: { className?: string; skill: SkillRow; versions: SkillVersionRow[]; selectedVersion?: number; loading: boolean; onSelect: (version: number) => void; onRollback: (version: number) => void }) {
  const { t } = useTranslation();
  // the dashboard locale, not the browser's, which is what a bare
  // Intl.DateTimeFormat(undefined, …) silently followed before (#1092)
  const format = useFormat();
  return (
    <aside className={cn("overflow-hidden rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]", className)}>
      <div className="border-b border-[color:var(--border-subtle)] px-4 py-3"><p className="text-xs font-semibold uppercase tracking-[0.12em] text-[color:var(--text-subtle)]">{t("pages.skillsRepo.versionHistory")}</p><p className="mt-1 text-xs text-muted-foreground">{t("pages.skillsRepo.versionHistoryHint")}</p></div>
      <div className="max-h-[30rem] space-y-1 overflow-y-auto p-2 xl:max-h-[calc(100vh-14rem)]">{loading ? <Skeleton width="100%" height={180} radius={8} /> : versions.length === 0 ? <p className="px-2 py-5 text-center text-xs text-muted-foreground">{t("pages.skillsRepo.noSavedVersions")}</p> : versions.map((version) => { const published = skill.published_version === version.version; return <div key={version.version} className={cn("rounded-lg border p-2.5", selectedVersion === version.version ? "border-[color:var(--red-folk)] bg-[color:var(--surface-selected)]" : "border-transparent hover:bg-[color:var(--surface-hover)]")}><button type="button" className="w-full text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded-md" aria-pressed={selectedVersion === version.version} onClick={() => onSelect(version.version)}><div className="flex items-center justify-between gap-2"><span className="text-sm font-semibold tabular-nums">v{version.version}</span><Badge tone={published ? "success" : "neutral"}>{published ? t("pages.skillsRepo.published") : t("pages.skillsRepo.immutable")}</Badge></div><p className="mt-1.5 flex items-center gap-1 text-[0.6875rem] text-muted-foreground"><Clock3 className="h-3 w-3" />{format.date(version.created_at, { dateStyle: "medium", timeStyle: "short" })}</p><p className="mt-1 flex items-center gap-1 text-[0.6875rem] text-[color:var(--text-subtle)]">{version.content_ref ? <><ExternalLink className="h-3 w-3" />{t("pages.skillsRepo.artifactReferenceMeta")}</> : <><FileCode2 className="h-3 w-3" />{t("pages.skillsRepo.inlineContentMeta")}</>}</p></button>{!published && skill.published_version && !skill.retired_at && <GatedButton gate="skill:update" variant="ghost" onClick={() => onRollback(version.version)}><RotateCcw className="h-3.5 w-3.5" /> {t("pages.skillsRepo.rollbackTo", { version: version.version })}</GatedButton>}</div>; })}</div>
    </aside>
  );
}

interface SkillFormValue {
  name: string;
  slug?: string;
  description: string;
  minimum_role: SkillMinimumRole;
  allowed_team_ids: string[];
  retired?: boolean;
}

function AccessFields({ value, teams, includeSlug, includeRetired, onChange }: { value: SkillFormValue; teams: TeamRow[]; includeSlug?: boolean; includeRetired?: boolean; onChange: (value: SkillFormValue) => void }) {
  const { t } = useTranslation();
  const toggleTeam = (id: string) => onChange({ ...value, allowed_team_ids: value.allowed_team_ids.includes(id) ? value.allowed_team_ids.filter((teamId) => teamId !== id) : [...value.allowed_team_ids, id] });
  return <div className="space-y-3"><label className="block text-xs font-medium">{t("pages.skillsRepo.fieldName")}<Input className="mt-1" value={value.name} onChange={(event) => onChange({ ...value, name: event.target.value })} /></label>{includeSlug && <label className="block text-xs font-medium">{t("pages.skillsRepo.fieldSlug")} <span className="font-normal text-muted-foreground">{t("pages.skillsRepo.fieldOptional")}</span><Input className="mt-1" value={value.slug ?? ""} placeholder="support-triage" onChange={(event) => onChange({ ...value, slug: event.target.value })} /></label>}<label className="block text-xs font-medium">{t("pages.skillsRepo.fieldDescription")}<Textarea className="mt-1" rows={3} value={value.description} onChange={(event) => onChange({ ...value, description: event.target.value })} /></label><label className="block text-xs font-medium">{t("pages.skillsRepo.fieldMinimumRole")}<Select className="mt-1" value={value.minimum_role} onChange={(event) => onChange({ ...value, minimum_role: event.target.value as SkillMinimumRole })}><option value="viewer">{t("pages.skillsRepo.roleViewer")}</option><option value="member">{t("pages.skillsRepo.roleMember")}</option><option value="admin">{t("pages.skillsRepo.roleAdmin")}</option></Select></label><fieldset><legend className="text-xs font-medium">{t("pages.skillsRepo.allowedTeams")}</legend><p className="mt-0.5 text-[0.6875rem] text-muted-foreground">{t("pages.skillsRepo.allowedTeamsHint")}</p><div className="mt-2 grid gap-2 sm:grid-cols-2">{teams.map((team) => <label key={team.id} className="flex cursor-pointer items-center gap-2 rounded-lg border border-[color:var(--border-subtle)] p-2.5 text-sm hover:bg-[color:var(--surface-hover)]"><input type="checkbox" className="h-4 w-4 accent-[color:var(--red-folk)]" checked={value.allowed_team_ids.includes(team.id)} onChange={() => toggleTeam(team.id)} />{team.name}</label>)}</div></fieldset>{includeRetired && <label className="flex cursor-pointer items-start gap-2.5 rounded-lg border border-[color:var(--border-subtle)] p-3"><input type="checkbox" className="mt-0.5 h-4 w-4 accent-[color:var(--red-folk)]" checked={value.retired ?? false} onChange={(event) => onChange({ ...value, retired: event.target.checked })} /><span><span className="block text-sm font-medium">{t("pages.skillsRepo.retireLabel")}</span><span className="text-xs text-muted-foreground">{t("pages.skillsRepo.retireHint")}</span></span></label>}</div>;
}

function CreateSkillDialog({ open, teams, pending, error, onOpenChange, onSubmit }: { open: boolean; teams: TeamRow[]; pending: boolean; error: Error | null; onOpenChange: (open: boolean) => void; onSubmit: (input: CreateSkillInput) => void }) {
  const { t } = useTranslation();
  const [value, setValue] = React.useState<SkillFormValue>({ name: "", slug: "", description: "", minimum_role: "viewer", allowed_team_ids: [] });
  React.useEffect(() => { if (!open) setValue({ name: "", slug: "", description: "", minimum_role: "viewer", allowed_team_ids: [] }); }, [open]);
  return <Dialog open={open} onOpenChange={onOpenChange}><DialogHeader><DialogTitle>{t("pages.skillsRepo.createTitle")}</DialogTitle><DialogDescription>{t("pages.skillsRepo.createDescription")}</DialogDescription></DialogHeader><form onSubmit={(event) => { event.preventDefault(); onSubmit({ name: value.name.trim(), ...(value.slug?.trim() ? { slug: value.slug.trim() } : {}), ...(value.description.trim() ? { description: value.description.trim() } : {}), allowed_team_ids: value.allowed_team_ids, minimum_role: value.minimum_role }); }}><AccessFields value={value} teams={teams} includeSlug onChange={setValue} />{error && <p role="alert" className="mt-3 text-xs text-[color:var(--status-danger-text)]">{error.message}</p>}<DialogFooter><Button type="button" variant="outline" onClick={() => onOpenChange(false)}>{t("pages.skillsRepo.cancel")}</Button><Button type="submit" disabled={pending || !value.name.trim()}>{pending ? t("pages.skillsRepo.creating") : t("pages.skillsRepo.createSkill")}</Button></DialogFooter></form></Dialog>;
}

function SkillSettingsDialog({ open, skill, teams, pending, error, onOpenChange, onSubmit }: { open: boolean; skill: SkillRow; teams: TeamRow[]; pending: boolean; error: Error | null; onOpenChange: (open: boolean) => void; onSubmit: (input: UpdateSkillInput) => void }) {
  const { t } = useTranslation();
  const fromSkill = React.useCallback((): SkillFormValue => ({ name: skill.name, description: skill.description, minimum_role: skill.minimum_role, allowed_team_ids: skill.allowed_team_ids, retired: !!skill.retired_at }), [skill]);
  const [value, setValue] = React.useState<SkillFormValue>(fromSkill);
  React.useEffect(() => { if (open) setValue(fromSkill()); }, [fromSkill, open]);
  return <Dialog open={open} onOpenChange={onOpenChange}><DialogHeader><DialogTitle>{t("pages.skillsRepo.settingsTitle")}</DialogTitle><DialogDescription>{t("pages.skillsRepo.settingsDescription")}</DialogDescription></DialogHeader><form onSubmit={(event) => { event.preventDefault(); onSubmit({ name: value.name.trim(), description: value.description.trim(), minimum_role: value.minimum_role, allowed_team_ids: value.allowed_team_ids, retired: value.retired }); }}><AccessFields value={value} teams={teams} includeRetired onChange={setValue} />{error && <p role="alert" className="mt-3 text-xs text-[color:var(--status-danger-text)]">{error.message}</p>}<DialogFooter><Button type="button" variant="outline" onClick={() => onOpenChange(false)}>{t("pages.skillsRepo.cancel")}</Button><Button type="submit" disabled={pending || !value.name.trim()}>{pending ? t("pages.skillsRepo.saving") : t("pages.skillsRepo.saveSettings")}</Button></DialogFooter></form></Dialog>;
}

function RollbackDialog({ version, publishedVersion, pending, error, onClose, onConfirm }: { version?: number; publishedVersion?: number | null; pending: boolean; error: Error | null; onClose: () => void; onConfirm: () => void }) {
  const { t } = useTranslation();
  return <Dialog open={version !== undefined} onOpenChange={(open) => !open && onClose()}><DialogHeader><DialogTitle>{t("pages.skillsRepo.rollbackTo", { version })}</DialogTitle><DialogDescription>{t("pages.skillsRepo.rollbackDescription", { from: publishedVersion, to: version })}</DialogDescription></DialogHeader>{error && <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">{error.message}</p>}<DialogFooter><Button variant="outline" onClick={onClose}>{t("pages.skillsRepo.keepLive", { version: publishedVersion })}</Button><Button disabled={pending} onClick={onConfirm}><RotateCcw className="h-4 w-4" />{pending ? t("pages.skillsRepo.rollingBack") : t("pages.skillsRepo.rollbackTo", { version })}</Button></DialogFooter></Dialog>;
}
