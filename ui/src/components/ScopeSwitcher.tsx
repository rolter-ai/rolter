import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2 } from "lucide-react";
import * as React from "react";

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
import {
  createOrg,
  createProject,
  createTeam,
  deleteOrg,
  deleteProject,
  deleteTeam,
} from "@/lib/api";
import { useScope } from "@/lib/scope";

type Level = "org" | "team" | "project";

// org → team → project switcher, persisted to localStorage via useScope.
// mounted in the app shell sidebar so every page shares one selection.
export function ScopeSwitcher() {
  const scope = useScope();
  const queryClient = useQueryClient();

  const [createLevel, setCreateLevel] = React.useState<Level | null>(null);
  const [deleteTarget, setDeleteTarget] = React.useState<{
    level: Level;
    id: string;
    name: string;
  } | null>(null);

  const invalidateScope = () => {
    queryClient.invalidateQueries({ queryKey: ["scope"] });
  };

  if (scope.isLoading) {
    return (
      <div className="px-3 py-1 text-xs text-muted-foreground">
        Loading scope…
      </div>
    );
  }

  return (
    <div className="space-y-1.5 px-2">
      <ScopeRow
        label="Org"
        value={scope.orgId ?? ""}
        options={scope.orgs.map((o) => ({ id: o.id, name: o.name }))}
        onChange={scope.setOrgId}
        onAdd={() => setCreateLevel("org")}
        onDelete={
          scope.orgId
            ? () =>
                setDeleteTarget({
                  level: "org",
                  id: scope.orgId as string,
                  name: scope.orgs.find((o) => o.id === scope.orgId)?.name ?? "",
                })
            : undefined
        }
      />
      <ScopeRow
        label="Team"
        value={scope.teamId ?? ""}
        options={scope.teams.map((t) => ({ id: t.id, name: t.name }))}
        onChange={scope.setTeamId}
        onAdd={scope.orgId ? () => setCreateLevel("team") : undefined}
        onDelete={
          scope.teamId
            ? () =>
                setDeleteTarget({
                  level: "team",
                  id: scope.teamId as string,
                  name: scope.teams.find((t) => t.id === scope.teamId)?.name ?? "",
                })
            : undefined
        }
        disabled={!scope.orgId}
      />
      <ScopeRow
        label="Project"
        value={scope.projectId ?? ""}
        options={scope.projects.map((p) => ({ id: p.id, name: p.name }))}
        onChange={scope.setProjectId}
        onAdd={scope.teamId ? () => setCreateLevel("project") : undefined}
        onDelete={
          scope.projectId
            ? () =>
                setDeleteTarget({
                  level: "project",
                  id: scope.projectId as string,
                  name:
                    scope.projects.find((p) => p.id === scope.projectId)?.name ?? "",
                })
            : undefined
        }
        disabled={!scope.teamId}
      />
      {scope.error && (
        <p className="px-1 text-xs text-muted-foreground">{scope.error}</p>
      )}

      <CreateScopeDialog
        level={createLevel}
        orgId={scope.orgId}
        teamId={scope.teamId}
        onOpenChange={(open) => !open && setCreateLevel(null)}
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
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        onDeleted={() => {
          invalidateScope();
          setDeleteTarget(null);
        }}
      />
    </div>
  );
}

function ScopeRow({
  label,
  value,
  options,
  onChange,
  onAdd,
  onDelete,
  disabled,
}: {
  label: string;
  value: string;
  options: { id: string; name: string }[];
  onChange: (id: string) => void;
  onAdd?: () => void;
  onDelete?: () => void;
  disabled?: boolean;
}) {
  return (
    <div className="flex items-center gap-1">
      <Select
        aria-label={label}
        value={value}
        disabled={disabled || options.length === 0}
        onChange={(e) => onChange(e.target.value)}
        className="h-7 text-xs"
      >
        {options.length === 0 && <option value="">no {label.toLowerCase()}</option>}
        {options.map((o) => (
          <option key={o.id} value={o.id}>
            {o.name}
          </option>
        ))}
      </Select>
      {onAdd && (
        <button
          type="button"
          aria-label={`Add ${label.toLowerCase()}`}
          onClick={onAdd}
          className="shrink-0 rounded p-1 text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
        >
          <Plus className="h-3.5 w-3.5" />
        </button>
      )}
      {onDelete && (
        <button
          type="button"
          aria-label={`Delete ${label.toLowerCase()}`}
          onClick={onDelete}
          className="shrink-0 rounded p-1 text-muted-foreground transition-colors hover:bg-secondary hover:text-destructive"
        >
          <Trash2 className="h-3.5 w-3.5" />
        </button>
      )}
    </div>
  );
}

function slugify(name: string): string {
  return name
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/(^-|-$)/g, "");
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
      if (level) onCreated(level, row.id);
    },
  });

  const title =
    level === "org" ? "New org" : level === "team" ? "New team" : "New project";

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>
          {level === "org"
            ? "Creates an org with a URL-safe slug derived from the name."
            : `Creates a ${level} under the currently selected ${level === "team" ? "org" : "team"}.`}
        </DialogDescription>
      </DialogHeader>
      <div className="space-y-3">
        <Field label="Name">
          <Input
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={level === "org" ? "Acme Inc" : level === "team" ? "Platform" : "prod"}
          />
        </Field>
        {create.isError && (
          <p className="text-xs text-destructive">{(create.error as Error).message}</p>
        )}
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button disabled={!name.trim() || create.isPending} onClick={() => create.mutate()}>
          Create
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
  const remove = useMutation({
    mutationFn: async () => {
      if (!target) return;
      if (target.level === "org") return deleteOrg(target.id);
      if (target.level === "team") return deleteTeam(target.id);
      return deleteProject(target.id);
    },
    onSuccess: onDeleted,
  });

  return (
    <Dialog open={!!target} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>Delete {target?.level}</DialogTitle>
        <DialogDescription>
          This removes <span className="font-mono">{target?.name}</span>
          {target?.level !== "project" ? " and everything under it" : ""}. This
          cannot be undone.
        </DialogDescription>
      </DialogHeader>
      {remove.isError && (
        <p className="text-xs text-destructive">{(remove.error as Error).message}</p>
      )}
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button
          variant="destructive"
          disabled={remove.isPending}
          onClick={() => remove.mutate()}
        >
          Delete
        </Button>
      </DialogFooter>
    </Dialog>
  );
}
