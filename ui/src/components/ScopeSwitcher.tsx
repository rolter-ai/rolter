import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Building2,
  ChevronsUpDown,
  Loader2,
  MoreHorizontal,
  Plus,
  Settings,
  Trash2,
} from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { AnchoredPanel } from "@/components/ui/anchored-panel";
import { Button } from "@/components/ui/button";
import { Combobox } from "@/components/ui/combobox";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { LoadError } from "@/components/LoadError";
import { FormSkeleton } from "@/components/LoadingState";
import { Field } from "@/components/ui/field";
import { FieldLabel } from "@/components/ui/field-label";
import { Input } from "@/components/ui/input";
import { Menu, MenuItem } from "@/components/ui/menu";
import { SwitchRow } from "@/components/ui/switch-row";
import {
  createOrg,
  createProject,
  createTeam,
  deleteOrg,
  deleteProject,
  deleteTeam,
  fetchProjectSettings,
  updateProjectSettings,
} from "@/lib/api";
import { useCreateProjectOpener, useScope } from "@/lib/scope";
import { slugify } from "@/lib/slug";
import { errorDetail, useToast } from "@/lib/toast";
import { cn } from "@/lib/utils";

type Level = "org" | "team" | "project";

// the scope the whole dashboard is looking at — org → team → project —
// persisted to localStorage through `useScope`, so every screen shares one
// selection. it lives at the top of the rail, under the brand (#2805), because
// it changes what every screen shows and is not part of anyone's account.
//
// the rail shows the path it is on and opens a popover of three labelled
// pickers. creating and deleting a level is one overflow button per row, not
// six bare icons on its surface.
export function ScopeSwitcher({ folded = false }: { folded?: boolean }) {
  const { t } = useTranslation();
  const scope = useScope();
  const queryClient = useQueryClient();
  const trigger = React.useRef<HTMLButtonElement>(null);
  const [open, setOpen] = React.useState(false);

  const [createLevel, setCreateLevel] = React.useState<Level | null>(null);
  const [deleteTarget, setDeleteTarget] = React.useState<{
    level: Level;
    id: string;
    name: string;
  } | null>(null);
  const [settingsTarget, setSettingsTarget] = React.useState<{ id: string; name: string } | null>(
    null,
  );

  const invalidateScope = () => {
    queryClient.invalidateQueries({ queryKey: ["scope"] });
  };

  const orgName = scope.orgs.find((o) => o.id === scope.orgId)?.name;
  const teamName = scope.teams.find((x) => x.id === scope.teamId)?.name;
  const projectName = scope.projects.find((p) => p.id === scope.projectId)?.name;

  // a level that is still arriving after a pick keeps the popover on its rows;
  // only the very first read, with nothing to show yet, stands in for them
  const firstRead = scope.isLoading && scope.orgs.length === 0;
  // the broadest level gives way first
  const levels: { name: string | undefined; shrink: string }[] = [
    { name: orgName, shrink: "shrink-[4]" },
    { name: teamName, shrink: "shrink-[2]" },
    { name: projectName, shrink: "shrink" },
  ];
  const segments = levels.flatMap(({ name, shrink }) => (name ? [{ name, shrink }] : []));
  const path = segments.map((segment) => segment.name).join(PATH_SEPARATOR);
  const shown = firstRead ? t("scope.loading") : path || t("scope.noOrg");
  const name = firstRead ? shown : t("scope.trigger", { path: shown });

  // a dialog is raised from a menu inside the popover, which goes with the pick:
  // focus is parked on the trigger first, so the dialog's own "return to the
  // opener" lands somewhere that survives the menu unmounting
  const raise = (action: () => void) => () => {
    setOpen(false);
    trigger.current?.focus();
    action();
  };

  // what each row's overflow button holds. written out as elements rather than
  // built from data, because a gated entry names itself for the UX stream with a
  // literal `control` slug that the source guard reads off the tag
  const menus: Record<Level, React.ReactNode> = {
    org: (
      <>
        <MenuItem
          icon={<Plus />}
          gate="org:create"
          control="scope-org-new"
          onSelect={raise(() => setCreateLevel("org"))}
        >
          {t("scope.newOrg")}
        </MenuItem>
        {scope.orgId && (
          <MenuItem
            icon={<Trash2 />}
            tone="danger"
            gate="org:delete"
            control="scope-org-delete"
            onSelect={raise(() =>
              setDeleteTarget({ level: "org", id: scope.orgId as string, name: orgName ?? "" }),
            )}
          >
            {t("scope.menu.deleteOrg")}
          </MenuItem>
        )}
      </>
    ),
    team: (
      <>
        <MenuItem
          icon={<Plus />}
          gate="team:create"
          control="scope-team-new"
          onSelect={raise(() => setCreateLevel("team"))}
        >
          {t("scope.newTeam")}
        </MenuItem>
        {scope.teamId && (
          <MenuItem
            icon={<Trash2 />}
            tone="danger"
            gate="team:delete"
            control="scope-team-delete"
            onSelect={raise(() =>
              setDeleteTarget({ level: "team", id: scope.teamId as string, name: teamName ?? "" }),
            )}
          >
            {t("scope.menu.deleteTeam")}
          </MenuItem>
        )}
      </>
    ),
    project: (
      <>
        <MenuItem
          icon={<Plus />}
          gate="project:create"
          control="scope-project-new"
          onSelect={raise(() => setCreateLevel("project"))}
        >
          {t("scope.newProject")}
        </MenuItem>
        {scope.projectId && (
          <>
            <MenuItem
              icon={<Settings />}
              onSelect={raise(() =>
                setSettingsTarget({ id: scope.projectId as string, name: projectName ?? "" }),
              )}
            >
              {t("scope.projectSettings")}
            </MenuItem>
            <MenuItem
              icon={<Trash2 />}
              tone="danger"
              gate="project:delete"
              control="scope-project-delete"
              onSelect={raise(() =>
                setDeleteTarget({
                  level: "project",
                  id: scope.projectId as string,
                  name: projectName ?? "",
                }),
              )}
            >
              {t("scope.menu.deleteProject")}
            </MenuItem>
          </>
        )}
      </>
    ),
  };

  // the reason a row is unavailable, or what is wrong with its list. one
  // sentence under the row it belongs to, rather than one under all three
  const failure = scope.errorKey ? t(scope.errorKey) : undefined;
  const failed = (...keys: string[]) =>
    failure && keys.includes(scope.errorKey ?? "") ? failure : undefined;
  // a level waiting on the one above it says so only once nothing is arriving:
  // right after a pick the level below is empty for a moment, and that is not
  // "waiting for a selection"
  const waiting = (key: string) => (scope.isLoading ? undefined : t(key));
  const hints: Record<Level, string | undefined> = {
    org: failed("scope.errors.orgsFailed", "scope.errors.noOrg"),
    team: scope.orgId
      ? failed("scope.errors.teamsFailed", "scope.errors.noTeam")
      : waiting("scope.needsOrg"),
    project: scope.teamId
      ? failed("scope.errors.projectsFailed", "scope.errors.noProject")
      : waiting("scope.needsTeam"),
  };

  return (
    <>
      <button
        ref={trigger}
        type="button"
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={name}
        title={folded ? name : shown}
        onClick={() => setOpen((v) => !v)}
        className={cn(
          "flex items-center rounded-md transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
          folded
            ? "w-full justify-center py-1.5 text-muted-foreground hover:bg-muted hover:text-foreground"
            : "w-full gap-2 border border-[color:var(--border-subtle)] bg-[color:var(--surface-base)] px-2 py-1.5 text-left hover:border-[color:var(--border-default)]",
          open && (folded ? "bg-muted text-foreground" : "border-[color:var(--border-default)]"),
        )}
      >
        {folded ? (
          <Building2 aria-hidden className="h-4 w-4 flex-none" />
        ) : (
          <>
            {/* each level truncates on its own, the broadest first, so a long
                org name does not push the project — the level the reader is
                working in — out of sight */}
            <span
              className={cn(
                "flex min-w-0 flex-1 items-center gap-0.5 text-xs text-foreground",
                segments.length > 0 && !firstRead && "font-mono",
              )}
            >
              {firstRead || segments.length === 0 ? (
                <span className="min-w-0 truncate">{shown}</span>
              ) : (
                segments.map((segment, i) => (
                  <React.Fragment key={segment.name + i}>
                    {i > 0 && (
                      <span
                        aria-hidden="true"
                        className="flex-none text-[color:var(--text-subtle)]"
                      >
                        /
                      </span>
                    )}
                    <span className={cn("min-w-0 truncate", segment.shrink)}>{segment.name}</span>
                  </React.Fragment>
                ))
              )}
            </span>
            <ChevronsUpDown
              aria-hidden
              className="h-3.5 w-3.5 flex-none text-[color:var(--text-subtle)]"
            />
          </>
        )}
      </button>

      {open && trigger.current && (
        <AnchoredPanel
          anchor={trigger.current}
          side={folded ? "right" : "below"}
          role="dialog"
          aria-label={t("shell.scope")}
          onClose={(restoreFocus) => {
            setOpen(false);
            if (restoreFocus) trigger.current?.focus();
          }}
          className="w-[min(20rem,calc(100vw-1rem))] p-3"
        >
          {firstRead ? (
            <FormSkeleton fields={3} className="gap-3" />
          ) : (
            <ScopeRows scope={scope} menus={menus} hints={hints} loading={scope.isLoading} />
          )}
        </AnchoredPanel>
      )}

      <CreateScopeDialog
        level={createLevel}
        orgId={scope.orgId}
        teamId={scope.teamId}
        onOpenChange={(next) => !next && setCreateLevel(null)}
        onCreated={(level, id) => {
          invalidateScope();
          if (level === "org") scope.setOrgId(id);
          else if (level === "team") scope.setTeamId(id);
          else scope.setProjectId(id);
          setCreateLevel(null);
        }}
      />

      <DeleteScopeDialog
        target={deleteTarget}
        onOpenChange={(next) => !next && setDeleteTarget(null)}
        onDeleted={() => {
          invalidateScope();
          setDeleteTarget(null);
        }}
      />

      <ProjectSettingsDialog
        target={settingsTarget}
        onOpenChange={(next) => !next && setSettingsTarget(null)}
      />
    </>
  );
}

/** what the trigger puts between the levels of the path it shows */
const PATH_SEPARATOR = " / ";

/** the three pickers, each named, with the first field taking focus on open */
function ScopeRows({
  scope,
  menus,
  hints,
  loading,
}: {
  scope: ReturnType<typeof useScope>;
  menus: Record<Level, React.ReactNode>;
  hints: Record<Level, string | undefined>;
  loading: boolean;
}) {
  const box = React.useRef<HTMLDivElement>(null);
  // a popover of form controls opens onto its first one, so the keyboard does
  // not have to find the way in
  React.useEffect(() => {
    box.current?.querySelector<HTMLElement>("input:not(:disabled)")?.focus({ preventScroll: true });
  }, []);
  return (
    <div ref={box} className="flex flex-col gap-3">
      <ScopeRow
        level="org"
        value={scope.orgId ?? ""}
        options={scope.orgs.map((o) => ({ id: o.id, name: o.name }))}
        onChange={scope.setOrgId}
        hint={hints.org}
        loading={loading}
        menu={menus.org}
      />
      <ScopeRow
        level="team"
        value={scope.teamId ?? ""}
        options={scope.teams.map((x) => ({ id: x.id, name: x.name }))}
        onChange={scope.setTeamId}
        disabled={!scope.orgId}
        hint={hints.team}
        loading={loading}
        menu={menus.team}
      />
      <ScopeRow
        level="project"
        value={scope.projectId ?? ""}
        options={scope.projects.map((p) => ({ id: p.id, name: p.name }))}
        onChange={scope.setProjectId}
        disabled={!scope.teamId}
        hint={hints.project}
        loading={loading}
        menu={menus.project}
      />
    </div>
  );
}

/**
 * The create-project dialog other screens open through `openCreateProject()`
 * (#2611), such as the Getting started card when no project exists yet.
 *
 * Mounted once in the shell rather than inside `ScopeSwitcher`: the switcher
 * lives in the rail, which below `md` is a drawer that is out of the document
 * until it is opened (and was once an account menu that was only in it while
 * open), so an opener registered there was gone by the time any screen could
 * call it. It is the same dialog **New project** raises from the switcher,
 * under the same team, and it selects the project it creates the way the
 * switcher does.
 */
export function CreateProjectHost() {
  const scope = useScope();
  const queryClient = useQueryClient();
  const [open, setOpen] = React.useState(false);

  // a project is created under the team in scope, so with no team there is
  // nothing to open — the same condition that disables the Project row
  useCreateProjectOpener(() => {
    if (scope.teamId) setOpen(true);
  });

  return (
    <CreateScopeDialog
      level={open ? "project" : null}
      orgId={scope.orgId}
      teamId={scope.teamId}
      onOpenChange={(next) => !next && setOpen(false)}
      onCreated={(_, id) => {
        queryClient.invalidateQueries({ queryKey: ["scope"] });
        scope.setProjectId(id);
        setOpen(false);
      }}
    />
  );
}

// every label is looked up by an explicit per-level key: interpolating a noun
// into "no {{level}}" / "Delete {{level}}" cannot be declined correctly in
// russian (and most inflected languages), so the catalog spells each one out
const ROW_KEYS: Record<Level, { label: string; empty: string; actions: string; remove: string }> = {
  org: {
    label: "scope.rows.org",
    empty: "scope.noOrg",
    actions: "scope.actions.org",
    remove: "scope.deleteOrg",
  },
  team: {
    label: "scope.rows.team",
    empty: "scope.noTeam",
    actions: "scope.actions.team",
    remove: "scope.deleteTeam",
  },
  project: {
    label: "scope.rows.project",
    empty: "scope.noProject",
    actions: "scope.actions.project",
    remove: "scope.deleteProject",
  },
};

const CREATE_KEYS: Record<Level, { title: string; hint: string }> = {
  org: { title: "scope.newOrg", hint: "scope.newOrgHint" },
  team: { title: "scope.newTeam", hint: "scope.newTeamHint" },
  project: { title: "scope.newProject", hint: "scope.newProjectHint" },
};

// the title names the row, so each level carries its own noun for the same
// reason the row labels above do
const DELETE_KEYS: Record<Level, string> = {
  org: "scope.confirm.orgTitle",
  team: "scope.confirm.teamTitle",
  project: "scope.confirm.projectTitle",
};

/**
 * One level: its name above, the picker, and the one button that holds what
 * can be done to the level (create, settings, delete).
 *
 * A level with nothing above it to belong to is disabled, picker and button
 * both, and says why underneath. A level with nothing in it is disabled too —
 * there is nothing to pick — but keeps its button, since creating the first one
 * is the way out.
 */
function ScopeRow({
  level,
  value,
  options,
  onChange,
  disabled,
  hint,
  loading,
  menu,
}: {
  level: Level;
  value: string;
  options: { id: string; name: string }[];
  onChange: (id: string) => void;
  disabled?: boolean;
  hint?: string;
  loading: boolean;
  /** the entries of the row's overflow menu */
  menu: React.ReactNode;
}) {
  const { t } = useTranslation();
  const keys = ROW_KEYS[level];
  const id = React.useId();
  const hintId = React.useId();
  const button = React.useRef<HTMLButtonElement>(null);
  const [menuOpen, setMenuOpen] = React.useState(false);
  const empty = options.length === 0;
  return (
    <div className="space-y-1.5">
      <FieldLabel label={t(keys.label)} htmlFor={id} />
      <div className="flex items-center gap-1.5">
        <Combobox
          id={id}
          aria-describedby={hint ? hintId : undefined}
          value={value}
          disabled={disabled || empty}
          onChange={onChange}
          size="sm"
          className="min-w-0 flex-1"
          placeholder={empty ? t(loading ? "scope.loading" : keys.empty) : undefined}
          options={options.map((o) => ({ value: o.id, label: o.name }))}
        />
        <button
          ref={button}
          type="button"
          aria-haspopup="menu"
          aria-expanded={menuOpen}
          aria-label={t(keys.actions)}
          title={t(keys.actions)}
          disabled={disabled}
          onClick={() => setMenuOpen((v) => !v)}
          className={cn(
            "flex h-8 w-8 flex-none items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 disabled:hover:bg-transparent",
            menuOpen && "bg-muted text-foreground",
          )}
        >
          <MoreHorizontal aria-hidden className="h-4 w-4" />
        </button>
        {menuOpen && button.current && (
          <Menu
            anchor={button.current}
            label={t(keys.actions)}
            side="below"
            align="end"
            onClose={(restoreFocus) => {
              setMenuOpen(false);
              if (restoreFocus) button.current?.focus();
            }}
          >
            {menu}
          </Menu>
        )}
      </div>
      {hint && (
        <p id={hintId} className="text-xs text-muted-foreground">
          {hint}
        </p>
      )}
    </div>
  );
}

/**
 * A project's own settings (#1820), opened from the project row.
 *
 * Today that is one switch: whether the project's viewers may read the request
 * and response bodies payload capture stored for its traffic. Members and
 * admins always may; viewers only once a project admin turns this on, because
 * a prompt is the most sensitive thing the request log holds. Anyone on the
 * project can open the dialog and see where it stands — only the switch is
 * gated, on the same `project_settings:update` the server enforces.
 */
function ProjectSettingsDialog({
  target,
  onOpenChange,
}: {
  target: { id: string; name: string } | null;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const queryClient = useQueryClient();
  const id = target?.id;
  const settings = useQuery({
    queryKey: ["project-settings", id],
    queryFn: () => fetchProjectSettings(id as string),
    enabled: !!id,
  });
  const save = useMutation({
    mutationFn: (viewers: boolean) =>
      updateProjectSettings(id as string, { payload_min_role: viewers ? "viewer" : "member" }),
    onSuccess: (next) => {
      queryClient.setQueryData(["project-settings", id], next);
      // an open request log re-reads with the new floor rather than showing
      // bodies it would no longer be sent, or hiding ones it now would
      queryClient.invalidateQueries({ queryKey: ["invocations"] });
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: target?.name ?? "" }),
      });
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: target?.name ?? "" }),
        detail: errorDetail(error),
      });
    },
  });
  // the switch follows the click while the save is in flight, then the answer
  const viewers = save.isPending
    ? save.variables === true
    : settings.data?.payload_min_role === "viewer";

  return (
    <Dialog open={!!target} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>{t("scope.settings.title", { name: target?.name ?? "" })}</DialogTitle>
        <DialogDescription>{t("scope.settings.hint")}</DialogDescription>
      </DialogHeader>
      <div className="space-y-3">
        {settings.isError ? (
          <LoadError
            error={settings.error}
            resource={t("errors.resources.projectSettings")}
            onRetry={() => settings.refetch()}
            target="project-settings"
          />
        ) : settings.isPending ? (
          <FormSkeleton fields={1} />
        ) : (
          <SwitchRow
            title={t("scope.settings.viewersSeePayloads")}
            hint={t("scope.settings.viewersSeePayloadsHint")}
            checked={viewers}
            onChange={(next) => save.mutate(next)}
            disabled={save.isPending}
            gate="project_settings:update"
            control="project-payload-visibility"
          />
        )}
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          {t("common.close")}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

function CreateScopeDialog({
  level,
  orgId,
  teamId,
  onOpenChange,
  onCreated,
}: {
  level: Level | null;
  orgId?: string;
  teamId?: string;
  onOpenChange: (open: boolean) => void;
  onCreated: (level: Level, id: string) => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [name, setName] = React.useState("");
  const open = !!level;

  React.useEffect(() => {
    if (open) setName("");
  }, [open, level]);

  const create = useMutation({
    mutationFn: async () => {
      if (level === "org") return createOrg({ name, slug: slugify(name) });
      if (level === "team") return createTeam(orgId as string, { name });
      return createProject(teamId as string, { name });
    },
    onSuccess: (row) => {
      // the dialog closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push({ tone: "success", title: t("toast.created", { what: row.name }) });
      if (level) onCreated(level, row.id);
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: name }),
        detail: errorDetail(error),
      });
    },
  });

  const title = level ? t(CREATE_KEYS[level].title) : "";

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{level ? t(CREATE_KEYS[level].hint) : ""}</DialogDescription>
      </DialogHeader>
      <div className="space-y-3">
        {/* no `autoFocus`: the dialog already puts focus on its first field,
            and focusing it during the commit made the field the "opener" the
            dialog hands focus back to on close, so focus fell to the page */}
        <Field label={t("scope.name")}>
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={t(
              level === "org"
                ? "scope.namePlaceholder.org"
                : level === "team"
                  ? "scope.namePlaceholder.team"
                  : "scope.namePlaceholder.project",
            )}
          />
        </Field>
        {create.isError && (
          <p className="text-xs text-[color:var(--status-danger-text)]">
            {(create.error as Error).message}
          </p>
        )}
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          {t("common.cancel")}
        </Button>
        <Button disabled={!name.trim() || create.isPending} onClick={() => create.mutate()}>
          {create.isPending && <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" />}
          {t("common.create")}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

function DeleteScopeDialog({
  target,
  onOpenChange,
  onDeleted,
}: {
  target: { level: Level; id: string; name: string } | null;
  onOpenChange: (open: boolean) => void;
  onDeleted: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const remove = useMutation({
    mutationFn: async () => {
      if (!target) return;
      if (target.level === "org") return deleteOrg(target.id);
      if (target.level === "team") return deleteTeam(target.id);
      return deleteProject(target.id);
    },
    onSuccess: () => {
      toast.push({
        tone: "success",
        title: t("toast.deleted", { what: target?.name ?? "" }),
      });
      onDeleted();
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.deleteFailed", { what: target?.name ?? "" }),
        detail: errorDetail(error),
      });
    },
  });

  // the dialog's UX key is read once more on its closing edge, where `target`
  // is already gone, so the level outlives the target by that one close. a key
  // that changed with it would file the abandon under another form
  const [level, setLevel] = React.useState<Level>(target?.level ?? "project");
  if (target && target.level !== level) setLevel(target.level);

  return (
    <ConfirmDialog
      name={`${level}-delete`}
      open={!!target}
      onOpenChange={(open) => {
        onOpenChange(open);
        // a refusal for this row must not greet the next one opened
        if (!open) remove.reset();
      }}
      title={t(DELETE_KEYS[level], { name: target?.name ?? "" })}
      description={t(level === "project" ? "scope.confirm.body" : "scope.confirm.cascadeBody")}
      confirmLabel={t(ROW_KEYS[level].remove)}
      pending={remove.isPending}
      error={remove.error}
      onConfirm={() => remove.mutate()}
    />
  );
}
