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
  createSkill,
  createSkillVersion,
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
import { useScope } from "@/lib/scope";
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

function parseMetadata(value: string): { value?: Record<string, unknown>; error?: string } {
  try {
    const parsed = JSON.parse(value) as unknown;
    if (!parsed || Array.isArray(parsed) || typeof parsed !== "object") {
      return { error: "Metadata must be a JSON object." };
    }
    return { value: parsed as Record<string, unknown> };
  } catch {
    return { error: "Metadata is not valid JSON." };
  }
}

function draftProblem(draft: Draft): string | undefined {
  if (draft.mode === "inline" && !draft.content.trim()) return "Inline SKILL.md content is required.";
  if (draft.mode === "reference") {
    if (!draft.contentRef.trim()) return "A content reference is required.";
    if (!/^(https|git\+https|oci|s3):\/\//.test(draft.contentRef.trim())) {
      return "Use an https, git+https, oci, or s3 reference.";
    }
    const authority = draft.contentRef.trim().split("://")[1]?.split("/")[0];
    if (!authority || authority.includes("@")) return "References cannot contain embedded credentials.";
  }
  return parseMetadata(draft.metadataText).error;
}

function formatDate(value: string) {
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(new Date(value));
}

export default function SkillsRepository() {
  const scope = useScope();
  const queryClient = useQueryClient();
  const [selectedId, setSelectedId] = React.useState<string>();
  const [selectedVersion, setSelectedVersion] = React.useState<number>();
  const [draft, setDraft] = React.useState<Draft>({ ...EMPTY_DRAFT });
  const [createOpen, setCreateOpen] = React.useState(false);
  const [settingsOpen, setSettingsOpen] = React.useState(false);
  const [rollbackVersion, setRollbackVersion] = React.useState<number>();
  const [notice, setNotice] = React.useState<string>();

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
      setNotice("Skill created. Add its first immutable version.");
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
      setNotice(`Version v${version.version} saved. Its content and metadata are now immutable.`);
    },
  });

  const publish = useMutation({
    mutationFn: (version: number) => publishSkillVersion(selectedId as string, version),
    onSuccess: (skill) => {
      replaceSkill(queryClient, scope.orgId, skill);
      setNotice(`v${skill.published_version} is now the resolved version.`);
    },
  });

  const rollback = useMutation({
    mutationFn: (version: number) => rollbackSkillVersion(selectedId as string, version),
    onSuccess: (skill) => {
      replaceSkill(queryClient, scope.orgId, skill);
      setRollbackVersion(undefined);
      setNotice(`Rolled back: v${skill.published_version} resolves again.`);
    },
  });

  const settings = useMutation({
    mutationFn: (input: UpdateSkillInput) => updateSkill(selectedId as string, input),
    onSuccess: (skill) => {
      replaceSkill(queryClient, scope.orgId, skill);
      setSettingsOpen(false);
      setNotice(skill.retired_at ? "Skill retired. It no longer resolves for callers." : "Skill settings updated.");
    },
  });

  if (scope.isLoading || skills.isLoading) return <LoadingState />;
  if (scope.error || skills.isError) {
    return (
      <EmptyState uxTarget="skill-list"
        icon={<BookOpen />}
        title="Skills repository unavailable"
        description={scope.error ?? (skills.error as Error).message}
        actions={<Button variant="outline" onClick={() => skills.refetch()}>Try again</Button>}
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
              title="Share your first skill"
              description="Create an org skill, define who can resolve it, then save inline content or a credential-free artifact reference."
              actions={<Button onClick={() => setCreateOpen(true)}>Create skill</Button>}
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
              title="Skill versions unavailable"
              description={(versions.error as Error).message}
              actions={<Button variant="outline" onClick={() => versions.refetch()}>Try again</Button>}
            />
          </main>
        ) : (
          <SkillWorkbench
            skill={selected}
            baseVersion={baseVersion}
            draft={draft}
            notice={notice}
            pending={save.isPending || publish.isPending}
            error={(save.error ?? publish.error) as Error | null}
            onDraftChange={(next) => {
              setDraft(next);
              setNotice(undefined);
            }}
            onSave={() => save.mutate()}
            onPublish={(version) => publish.mutate(version)}
            onSettings={() => setSettingsOpen(true)}
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
    </div>
  );
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
  return (
    <aside className="overflow-hidden rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]">
      <div className="flex items-center justify-between border-b border-[color:var(--border-subtle)] px-3 py-2.5">
        <div><p className="text-xs font-semibold uppercase tracking-[0.12em] text-[color:var(--text-subtle)]">Org skills</p><p className="mt-0.5 text-xs text-muted-foreground">{skills.length} visible to you</p></div>
        <Button variant="ghost" onClick={onCreate} aria-label="Create skill"><Plus className="h-4 w-4" /></Button>
      </div>
      <div className="max-h-[26rem] overflow-y-auto p-1.5 lg:max-h-[calc(100vh-14rem)]">
        {skills.length === 0 ? <p className="px-2 py-5 text-center text-xs text-muted-foreground">No skills yet</p> : skills.map((skill) => (
          <button key={skill.id} type="button" aria-current={selectedId === skill.id ? "page" : undefined} onClick={() => onSelect(skill.id)} className={cn("w-full rounded-lg px-2.5 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring", selectedId === skill.id ? "bg-[color:var(--surface-selected)]" : "text-[color:var(--text-secondary)] hover:bg-[color:var(--surface-hover)] hover:text-foreground")}>
            <span className="flex items-center gap-2"><span className="min-w-0 flex-1 truncate text-sm font-medium">{skill.name}</span>{skill.retired_at ? <Badge tone="neutral">Retired</Badge> : skill.published_version ? <span className="text-[0.6875rem] tabular-nums text-[color:var(--status-success)]">v{skill.published_version}</span> : <span className="text-[0.6875rem] text-[color:var(--text-subtle)]">draft</span>}</span>
            <span className="mt-0.5 block truncate font-mono text-[0.6875rem] text-[color:var(--text-subtle)]">{skill.slug}</span>
          </button>
        ))}
      </div>
    </aside>
  );
}

function SkillWorkbench({ skill, baseVersion, draft, notice, pending, error, onDraftChange, onSave, onPublish, onSettings }: { skill: SkillRow; baseVersion?: SkillVersionRow; draft: Draft; notice?: string; pending: boolean; error: Error | null; onDraftChange: (draft: Draft) => void; onSave: () => void; onPublish: (version: number) => void; onSettings: () => void }) {
  const problem = draftProblem(draft);
  const selectedPublished = baseVersion?.version === skill.published_version;
  return (
    <main className="min-w-0 overflow-hidden rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]">
      <header className="border-b border-[color:var(--border-subtle)] px-4 py-3 sm:px-5">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0"><div className="flex flex-wrap items-center gap-2"><h2 className="truncate text-lg font-semibold tracking-[-0.02em]">{skill.name}</h2>{skill.retired_at ? <Badge tone="neutral">Retired</Badge> : skill.published_version ? <Badge tone="success" dot>v{skill.published_version} live</Badge> : <Badge tone="warning">Unpublished</Badge>}</div><p className="mt-1 max-w-2xl text-sm text-muted-foreground">{skill.description || "No description yet."}</p></div>
          <div className="flex flex-wrap items-center gap-2"><Button variant="ghost" onClick={onSettings}><Settings2 className="h-4 w-4" /> Settings</Button>{baseVersion && !selectedPublished && !skill.retired_at && <Button variant="outline" disabled={pending} onClick={() => onPublish(baseVersion.version)}><Check className="h-4 w-4" /> Publish v{baseVersion.version}</Button>}<Button disabled={pending || !!problem || !!skill.retired_at} onClick={onSave}><FilePlus2 className="h-4 w-4" />{pending ? "Saving…" : "Save new version"}</Button></div>
        </div>
        <div className="mt-3 flex min-h-5 flex-wrap items-center gap-x-3 gap-y-1 text-xs"><span className="font-mono text-[color:var(--text-subtle)]">{skill.slug}</span><span className="text-muted-foreground">minimum role: {skill.minimum_role}</span><span className="text-muted-foreground">{skill.allowed_team_ids.length ? `${skill.allowed_team_ids.length} teams` : "all org teams"}</span>{baseVersion && <span className="text-muted-foreground">editing from immutable v{baseVersion.version}</span>}{problem && <span role="alert" className="text-[color:var(--status-danger)]">{problem}</span>}{error && <span role="alert" className="text-[color:var(--status-danger)]">{error.message}</span>}{notice && <span role="status" className="text-[color:var(--status-success)]">{notice}</span>}</div>
      </header>

      <div className="space-y-7 p-4 sm:p-5">
        <section>
          <SectionHeading eyebrow="Artifact" title="Content source" description="Store a complete SKILL.md inline or point to an immutable artifact without credentials." />
          <div className="mb-3 grid gap-2 sm:grid-cols-2" role="radiogroup" aria-label="Skill content source">
            <SourceOption selected={draft.mode === "inline"} title="Inline SKILL.md" description="Version the instructions in Rolter" onSelect={() => onDraftChange({ ...draft, mode: "inline" })} />
            <SourceOption selected={draft.mode === "reference"} title="Artifact reference" description="HTTPS, Git, OCI, or S3" onSelect={() => onDraftChange({ ...draft, mode: "reference" })} />
          </div>
          {draft.mode === "inline" ? <label className="block text-xs font-medium">SKILL.md content<Textarea className="mt-1 font-mono" rows={18} value={draft.content} onChange={(event) => onDraftChange({ ...draft, content: event.target.value })} /></label> : <label className="block text-xs font-medium">Content reference<Input className="mt-1 font-mono" aria-label="Content reference" value={draft.contentRef} placeholder="oci://registry.example/skills/support@sha256:…" onChange={(event) => onDraftChange({ ...draft, contentRef: event.target.value })} /><span className="mt-1 block text-[0.6875rem] font-normal text-muted-foreground">Pin a digest or commit where the scheme supports it. Embedded credentials are rejected.</span></label>}
        </section>

        <section>
          <SectionHeading eyebrow="Manifest" title="Version metadata" description="Optional non-secret JSON for runtime requirements, capabilities, or provenance." />
          <label className="block text-xs font-medium">Metadata JSON<Textarea className="mt-1 font-mono" rows={7} value={draft.metadataText} onChange={(event) => onDraftChange({ ...draft, metadataText: event.target.value })} /></label>
          <p className="mt-2 flex items-center gap-1.5 text-[0.6875rem] text-muted-foreground"><ShieldCheck className="h-3.5 w-3.5" />Fields containing secret, password, token, or API key names are rejected by the control plane.</p>
        </section>
      </div>
    </main>
  );
}

function SourceOption({ selected, title, description, onSelect }: { selected: boolean; title: string; description: string; onSelect: () => void }) {
  return <button type="button" role="radio" aria-checked={selected} onClick={onSelect} className={cn("rounded-lg border p-3 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring", selected ? "border-[color:var(--red-folk)] bg-[color:var(--surface-selected)]" : "border-[color:var(--border-subtle)] hover:bg-[color:var(--surface-hover)]")}><span className="flex items-center gap-2 text-sm font-medium"><span className={cn("flex h-4 w-4 items-center justify-center rounded-full border", selected ? "border-[color:var(--red-folk)]" : "border-[color:var(--border-default)]")}>{selected && <span className="h-2 w-2 rounded-full bg-[color:var(--red-folk)]" />}</span>{title}</span><span className="ml-6 mt-1 block text-xs text-muted-foreground">{description}</span></button>;
}

function SectionHeading({ eyebrow, title, description }: { eyebrow: string; title: string; description: string }) {
  return <div className="mb-3"><p className="text-[0.6875rem] font-semibold uppercase tracking-[0.14em] text-[color:var(--red-folk)]">{eyebrow}</p><h3 className="mt-1 text-sm font-semibold">{title}</h3><p className="mt-0.5 text-xs leading-5 text-muted-foreground">{description}</p></div>;
}

function VersionRail({ className, skill, versions, selectedVersion, loading, onSelect, onRollback }: { className?: string; skill: SkillRow; versions: SkillVersionRow[]; selectedVersion?: number; loading: boolean; onSelect: (version: number) => void; onRollback: (version: number) => void }) {
  return (
    <aside className={cn("overflow-hidden rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]", className)}>
      <div className="border-b border-[color:var(--border-subtle)] px-4 py-3"><p className="text-xs font-semibold uppercase tracking-[0.12em] text-[color:var(--text-subtle)]">Version history</p><p className="mt-1 text-xs text-muted-foreground">Content and metadata never change.</p></div>
      <div className="max-h-[30rem] space-y-1 overflow-y-auto p-2 xl:max-h-[calc(100vh-14rem)]">{loading ? <Skeleton width="100%" height={180} radius={8} /> : versions.length === 0 ? <p className="px-2 py-5 text-center text-xs text-muted-foreground">No saved versions</p> : versions.map((version) => { const published = skill.published_version === version.version; return <div key={version.version} className={cn("rounded-lg border p-2.5", selectedVersion === version.version ? "border-[color:var(--red-folk)] bg-[color:var(--surface-selected)]" : "border-transparent hover:bg-[color:var(--surface-hover)]")}><button type="button" className="w-full text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded-md" aria-pressed={selectedVersion === version.version} onClick={() => onSelect(version.version)}><div className="flex items-center justify-between gap-2"><span className="text-sm font-semibold tabular-nums">v{version.version}</span><Badge tone={published ? "success" : "neutral"}>{published ? "Published" : "Immutable"}</Badge></div><p className="mt-1.5 flex items-center gap-1 text-[0.6875rem] text-muted-foreground"><Clock3 className="h-3 w-3" />{formatDate(version.created_at)}</p><p className="mt-1 flex items-center gap-1 text-[0.6875rem] text-[color:var(--text-subtle)]">{version.content_ref ? <><ExternalLink className="h-3 w-3" />artifact reference</> : <><FileCode2 className="h-3 w-3" />inline content</>}</p></button>{!published && skill.published_version && !skill.retired_at && <Button variant="ghost" onClick={() => onRollback(version.version)}><RotateCcw className="h-3.5 w-3.5" /> Roll back to v{version.version}</Button>}</div>; })}</div>
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
  const toggleTeam = (id: string) => onChange({ ...value, allowed_team_ids: value.allowed_team_ids.includes(id) ? value.allowed_team_ids.filter((teamId) => teamId !== id) : [...value.allowed_team_ids, id] });
  return <div className="space-y-3"><label className="block text-xs font-medium">Name<Input className="mt-1" value={value.name} onChange={(event) => onChange({ ...value, name: event.target.value })} /></label>{includeSlug && <label className="block text-xs font-medium">Slug <span className="font-normal text-muted-foreground">optional</span><Input className="mt-1" value={value.slug ?? ""} placeholder="support-triage" onChange={(event) => onChange({ ...value, slug: event.target.value })} /></label>}<label className="block text-xs font-medium">Description<Textarea className="mt-1" rows={3} value={value.description} onChange={(event) => onChange({ ...value, description: event.target.value })} /></label><label className="block text-xs font-medium">Minimum role<Select className="mt-1" value={value.minimum_role} onChange={(event) => onChange({ ...value, minimum_role: event.target.value as SkillMinimumRole })}><option value="viewer">Viewer</option><option value="member">Member</option><option value="admin">Admin</option></Select></label><fieldset><legend className="text-xs font-medium">Allowed teams</legend><p className="mt-0.5 text-[0.6875rem] text-muted-foreground">No selection shares the skill with every team in the organization.</p><div className="mt-2 grid gap-2 sm:grid-cols-2">{teams.map((team) => <label key={team.id} className="flex cursor-pointer items-center gap-2 rounded-lg border border-[color:var(--border-subtle)] p-2.5 text-sm hover:bg-[color:var(--surface-hover)]"><input type="checkbox" className="h-4 w-4 accent-[color:var(--red-folk)]" checked={value.allowed_team_ids.includes(team.id)} onChange={() => toggleTeam(team.id)} />{team.name}</label>)}</div></fieldset>{includeRetired && <label className="flex cursor-pointer items-start gap-2.5 rounded-lg border border-[color:var(--border-subtle)] p-3"><input type="checkbox" className="mt-0.5 h-4 w-4 accent-[color:var(--red-folk)]" checked={value.retired ?? false} onChange={(event) => onChange({ ...value, retired: event.target.checked })} /><span><span className="block text-sm font-medium">Retire this skill</span><span className="text-xs text-muted-foreground">Callers can no longer resolve it. Versions remain in history.</span></span></label>}</div>;
}

function CreateSkillDialog({ open, teams, pending, error, onOpenChange, onSubmit }: { open: boolean; teams: TeamRow[]; pending: boolean; error: Error | null; onOpenChange: (open: boolean) => void; onSubmit: (input: CreateSkillInput) => void }) {
  const [value, setValue] = React.useState<SkillFormValue>({ name: "", slug: "", description: "", minimum_role: "viewer", allowed_team_ids: [] });
  React.useEffect(() => { if (!open) setValue({ name: "", slug: "", description: "", minimum_role: "viewer", allowed_team_ids: [] }); }, [open]);
  return <Dialog open={open} onOpenChange={onOpenChange}><DialogHeader><DialogTitle>Create organization skill</DialogTitle><DialogDescription>Define its identity and access policy. Content is saved as a separate immutable version.</DialogDescription></DialogHeader><form onSubmit={(event) => { event.preventDefault(); onSubmit({ name: value.name.trim(), ...(value.slug?.trim() ? { slug: value.slug.trim() } : {}), ...(value.description.trim() ? { description: value.description.trim() } : {}), allowed_team_ids: value.allowed_team_ids, minimum_role: value.minimum_role }); }}><AccessFields value={value} teams={teams} includeSlug onChange={setValue} />{error && <p role="alert" className="mt-3 text-xs text-[color:var(--status-danger)]">{error.message}</p>}<DialogFooter><Button type="button" variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button><Button type="submit" disabled={pending || !value.name.trim()}>{pending ? "Creating…" : "Create skill"}</Button></DialogFooter></form></Dialog>;
}

function SkillSettingsDialog({ open, skill, teams, pending, error, onOpenChange, onSubmit }: { open: boolean; skill: SkillRow; teams: TeamRow[]; pending: boolean; error: Error | null; onOpenChange: (open: boolean) => void; onSubmit: (input: UpdateSkillInput) => void }) {
  const fromSkill = React.useCallback((): SkillFormValue => ({ name: skill.name, description: skill.description, minimum_role: skill.minimum_role, allowed_team_ids: skill.allowed_team_ids, retired: !!skill.retired_at }), [skill]);
  const [value, setValue] = React.useState<SkillFormValue>(fromSkill);
  React.useEffect(() => { if (open) setValue(fromSkill()); }, [fromSkill, open]);
  return <Dialog open={open} onOpenChange={onOpenChange}><DialogHeader><DialogTitle>Skill settings</DialogTitle><DialogDescription>Change the human-facing metadata and who can resolve this skill. The slug and versions stay immutable.</DialogDescription></DialogHeader><form onSubmit={(event) => { event.preventDefault(); onSubmit({ name: value.name.trim(), description: value.description.trim(), minimum_role: value.minimum_role, allowed_team_ids: value.allowed_team_ids, retired: value.retired }); }}><AccessFields value={value} teams={teams} includeRetired onChange={setValue} />{error && <p role="alert" className="mt-3 text-xs text-[color:var(--status-danger)]">{error.message}</p>}<DialogFooter><Button type="button" variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button><Button type="submit" disabled={pending || !value.name.trim()}>{pending ? "Saving…" : "Save settings"}</Button></DialogFooter></form></Dialog>;
}

function RollbackDialog({ version, publishedVersion, pending, error, onClose, onConfirm }: { version?: number; publishedVersion?: number | null; pending: boolean; error: Error | null; onClose: () => void; onConfirm: () => void }) {
  return <Dialog open={version !== undefined} onOpenChange={(open) => !open && onClose()}><DialogHeader><DialogTitle>Roll back to v{version}</DialogTitle><DialogDescription>This changes the resolved version from v{publishedVersion} to immutable v{version}. Newer versions remain in history.</DialogDescription></DialogHeader>{error && <p role="alert" className="text-xs text-[color:var(--status-danger)]">{error.message}</p>}<DialogFooter><Button variant="outline" onClick={onClose}>Keep v{publishedVersion} live</Button><Button disabled={pending} onClick={onConfirm}><RotateCcw className="h-4 w-4" />{pending ? "Rolling back…" : `Roll back to v${version}`}</Button></DialogFooter></Dialog>;
}
