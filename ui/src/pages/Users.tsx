import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Ban, Pencil, Plus, Trash2 } from "lucide-react";
import * as React from "react";

import {
  ListHeader,
  ListRow,
  ListTable,
  PageBody,
  RowIconButton,
  SearchInput,
} from "@/components/screen";
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
import {
  createMembership,
  deleteUser,
  fetchMemberships,
  fetchUsers,
  inviteUser,
  MEMBERSHIP_SCOPE_TYPES,
  ROLES,
  updateUser,
  type MembershipRow,
  type TeamRow,
  type UserRow,
} from "@/lib/api";
import { useScope } from "@/lib/scope";

// admin surface for the user/team lifecycle (ROL-223): invite accounts into the
// current org, grant/revoke roles at org/team/project scope, and
// deactivate/delete accounts. everything is scoped to the org selected in the
// sidebar ScopeSwitcher; account edits (email/password/superadmin) require
// superadmin on the backend, org-scoped invite/role-grant require org admin.
export default function Users() {
  const scope = useScope();
  const orgId = scope.orgId;
  const queryClient = useQueryClient();

  const users = useQuery({
    queryKey: ["users", orgId],
    queryFn: () => fetchUsers(orgId as string),
    enabled: !!orgId,
  });

  const memberships = useQuery({
    queryKey: ["memberships", orgId],
    queryFn: () => fetchMemberships(orgId as string),
    enabled: !!orgId,
  });

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["users", orgId] });
    queryClient.invalidateQueries({ queryKey: ["memberships", orgId] });
  };

  const [inviteOpen, setInviteOpen] = React.useState(false);
  const [editUser, setEditUser] = React.useState<UserRow | null>(null);
  const [roleUser, setRoleUser] = React.useState<UserRow | null>(null);
  const [search, setSearch] = React.useState("");
  const [statusTab, setStatusTab] = React.useState<"all" | "active" | "deactivated">("all");

  // group role grants by user for per-row rendering
  const byUser = React.useMemo(() => {
    const map = new Map<string, MembershipRow[]>();
    for (const m of memberships.data ?? []) {
      const list = map.get(m.user_id) ?? [];
      list.push(m);
      map.set(m.user_id, list);
    }
    return map;
  }, [memberships.data]);

  const toggleActive = useMutation({
    mutationFn: (user: UserRow) =>
      updateUser(user.id, { deactivated: !user.deactivated_at ? true : false }),
    onSuccess: invalidate,
  });

  const q = search.trim().toLowerCase();
  const rows = (users.data ?? []).filter((u) => {
    const active = !u.deactivated_at;
    if (statusTab === "active" && !active) return false;
    if (statusTab === "deactivated" && active) return false;
    return !q || u.email.toLowerCase().includes(q);
  });

  const counts = {
    all: users.data?.length ?? 0,
    active: (users.data ?? []).filter((u) => !u.deactivated_at).length,
    deactivated: (users.data ?? []).filter((u) => !!u.deactivated_at).length,
  };

  const GRID = "1.7fr 1.4fr 110px 1fr 110px";
  const AVATARS = ["#c0392b", "#2e7d5b", "#3d6fb4", "#8e5aa8", "#b8860b", "#6b7280"];

  return (
    <PageBody>
      <div className="flex items-center gap-3">
        <SearchInput
          placeholder="Search users"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <div className="flex gap-0.5">
          {(["all", "active", "deactivated"] as const).map((t) => (
            <button
              key={t}
              type="button"
              onClick={() => setStatusTab(t)}
              className={
                "border-b-2 px-3 py-[7px] text-sm capitalize transition-colors " +
                (statusTab === t
                  ? "border-[color:var(--red-folk)] text-foreground"
                  : "border-transparent text-muted-foreground hover:text-foreground")
              }
            >
              {t}{" "}
              <span className="font-mono text-[11px] text-[color:var(--text-subtle)]">
                {counts[t]}
              </span>
            </button>
          ))}
        </div>
        <Button className="ml-auto" onClick={() => setInviteOpen(true)} disabled={!orgId}>
          <Plus className="h-4 w-4" />
          Invite user
        </Button>
      </div>

      {!orgId && (
        <p className="text-sm text-muted-foreground">
          {scope.error ?? "Select an org to manage its users."}
        </p>
      )}
      {(users.error || memberships.error) && (
        <p className="text-sm text-destructive">Failed to load users.</p>
      )}
      {orgId && users.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}

      <ListTable>
        <ListHeader grid={GRID}>
          <span>User</span>
          <span>Roles</span>
          <span>Status</span>
          <span>Created</span>
          <span />
        </ListHeader>
        {rows.map((user, i) => {
          const active = !user.deactivated_at;
          const grants = byUser.get(user.id) ?? [];
          const initials = user.email.slice(0, 2).toUpperCase();
          return (
            <ListRow key={user.id} grid={GRID} style={{ opacity: active ? 1 : 0.55 }}>
              <div className="flex min-w-0 items-center gap-2.5">
                <span
                  className="flex h-8 w-8 flex-none items-center justify-center rounded-full font-mono text-[11px] font-semibold text-white"
                  style={{ background: AVATARS[i % AVATARS.length] }}
                >
                  {initials}
                </span>
                <div className="min-w-0">
                  <div className="flex items-center gap-1.5">
                    <span className="truncate font-mono text-sm">{user.email}</span>
                    {user.is_superadmin && (
                      <span className="flex-none rounded-[3px] border border-[color:var(--red-folk)] px-1 text-[9px] uppercase tracking-[0.06em] text-[color:var(--red-folk)]">
                        super
                      </span>
                    )}
                  </div>
                </div>
              </div>
              <div className="min-w-0 truncate text-[11px] text-[color:var(--text-subtle)]">
                {grants.length === 0
                  ? "no roles"
                  : grants.map((g) => `${g.role}@${scopeLabel(g, scope.teams)}`).join(" · ")}
              </div>
              <div>
                <span
                  className="inline-flex items-center gap-[5px] rounded-full px-[9px] py-0.5 text-[11px] font-semibold capitalize"
                  style={{
                    color: active ? "var(--status-success)" : "var(--status-danger)",
                    background: active ? "rgba(22,163,74,.14)" : "rgba(229,57,53,.14)",
                  }}
                >
                  <span
                    className="h-1.5 w-1.5 rounded-full"
                    style={{ background: "currentColor" }}
                  />
                  {active ? "active" : "blocked"}
                </span>
              </div>
              <span className="font-mono text-xs text-muted-foreground">
                {user.created_at?.slice(0, 10)}
              </span>
              <div className="flex justify-end gap-[5px]">
                <RowIconButton title="Grant role" onClick={() => setRoleUser(user)}>
                  <Plus className="h-3.5 w-3.5" />
                </RowIconButton>
                <RowIconButton title="Edit user" onClick={() => setEditUser(user)}>
                  <Pencil className="h-3.5 w-3.5" />
                </RowIconButton>
                <RowIconButton
                  danger={active}
                  title={active ? "Deactivate user" : "Reactivate user"}
                  disabled={toggleActive.isPending}
                  onClick={() => toggleActive.mutate(user)}
                >
                  <Ban className="h-3.5 w-3.5" />
                </RowIconButton>
              </div>
            </ListRow>
          );
        })}
        {orgId && !users.isLoading && rows.length === 0 && (
          <p className="px-4 py-8 text-center text-sm text-muted-foreground">No users match.</p>
        )}
      </ListTable>

      {orgId && (
        <InviteUserDialog
          open={inviteOpen}
          onOpenChange={setInviteOpen}
          orgId={orgId}
          onDone={invalidate}
        />
      )}
      {editUser && (
        <EditUserDialog
          user={editUser}
          onOpenChange={(open) => !open && setEditUser(null)}
          onDone={invalidate}
        />
      )}
      {roleUser && orgId && (
        <AddRoleDialog
          user={roleUser}
          orgId={orgId}
          teams={scope.teams}
          defaultProjectId={scope.projectId}
          onOpenChange={(open) => !open && setRoleUser(null)}
          onDone={invalidate}
        />
      )}
    </PageBody>
  );
}


// render a membership's scope compactly, resolving team names where the scope
// is a team in the current org; projects fall back to a short id
function scopeLabel(m: MembershipRow, teams: TeamRow[]): string {
  if (m.project_id) return `project:${m.project_id.slice(0, 8)}`;
  if (m.team_id) {
    const team = teams.find((t) => t.id === m.team_id);
    return team ? `team:${team.name}` : `team:${m.team_id.slice(0, 8)}`;
  }
  return "org";
}

function InviteUserDialog({
  open,
  onOpenChange,
  orgId,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
  onDone: () => void;
}) {
  const [email, setEmail] = React.useState("");
  const [password, setPassword] = React.useState("");
  const [role, setRole] = React.useState<string>("member");

  React.useEffect(() => {
    if (open) {
      setEmail("");
      setPassword("");
      setRole("member");
    }
  }, [open]);

  const create = useMutation({
    mutationFn: () =>
      inviteUser(orgId, {
        email: email.trim(),
        password: password.trim() ? password : undefined,
        role,
      }),
    onSuccess: () => {
      onDone();
      onOpenChange(false);
    },
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>Invite user</DialogTitle>
        <DialogDescription>
          Create an account and grant it a role in this org. Leave the password
          blank for an SSO-only account that can't sign in locally yet.
        </DialogDescription>
      </DialogHeader>
      <div className="space-y-3">
        <Field label="Email">
          <Input
            type="email"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            placeholder="dev@example.com"
          />
        </Field>
        <Field label="Password (optional)" hint="at least 8 characters if set">
          <Input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            placeholder="leave blank for SSO-only"
          />
        </Field>
        <Field label="Org role">
          <Select value={role} onChange={(e) => setRole(e.target.value)}>
            {ROLES.map((r) => (
              <option key={r} value={r}>
                {r}
              </option>
            ))}
          </Select>
        </Field>
        {create.isError && (
          <p className="text-xs text-destructive">
            {(create.error as Error).message}
          </p>
        )}
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button
          disabled={!email.trim() || create.isPending}
          onClick={() => create.mutate()}
        >
          Invite
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

function EditUserDialog({
  user,
  onOpenChange,
  onDone,
}: {
  user: UserRow;
  onOpenChange: (open: boolean) => void;
  onDone: () => void;
}) {
  const [email, setEmail] = React.useState(user.email);
  const [password, setPassword] = React.useState("");
  const [isSuperadmin, setIsSuperadmin] = React.useState(user.is_superadmin);
  const [confirmDelete, setConfirmDelete] = React.useState(false);

  const save = useMutation({
    mutationFn: () =>
      updateUser(user.id, {
        email: email.trim() !== user.email ? email.trim() : undefined,
        password: password.trim() ? password : undefined,
        is_superadmin:
          isSuperadmin !== user.is_superadmin ? isSuperadmin : undefined,
      }),
    onSuccess: () => {
      onDone();
      onOpenChange(false);
    },
  });

  const remove = useMutation({
    mutationFn: () => deleteUser(user.id),
    onSuccess: () => {
      onDone();
      onOpenChange(false);
    },
  });

  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>Edit user</DialogTitle>
        <DialogDescription>
          Update account details. Editing email/password or the superadmin flag
          requires superadmin privileges.
        </DialogDescription>
      </DialogHeader>
      <div className="space-y-3">
        <Field label="Email">
          <Input
            type="email"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
          />
        </Field>
        <Field label="New password" hint="leave blank to keep the current one">
          <Input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            placeholder="unchanged"
          />
        </Field>
        <label className="flex items-center gap-2 text-sm">
          <Switch checked={isSuperadmin} onCheckedChange={setIsSuperadmin} />
          Superadmin (full cross-org access)
        </label>
        {save.isError && (
          <p className="text-xs text-destructive">
            {(save.error as Error).message}
          </p>
        )}

        <div className="rounded-md border border-destructive/30 bg-destructive/5 p-3">
          {!confirmDelete ? (
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs text-muted-foreground">
                Permanently delete this account and its memberships.
              </span>
              <Button
                size="sm"
                variant="destructive"
                onClick={() => setConfirmDelete(true)}
              >
                <Trash2 className="h-3.5 w-3.5" />
                Delete
              </Button>
            </div>
          ) : (
            <div className="space-y-2">
              <p className="text-xs text-destructive">
                Delete <span className="font-mono">{user.email}</span>? This
                can't be undone.
              </p>
              {remove.isError && (
                <p className="text-xs text-destructive">
                  {(remove.error as Error).message}
                </p>
              )}
              <div className="flex justify-end gap-2">
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => setConfirmDelete(false)}
                >
                  Cancel
                </Button>
                <Button
                  size="sm"
                  variant="destructive"
                  disabled={remove.isPending}
                  onClick={() => remove.mutate()}
                >
                  Confirm delete
                </Button>
              </div>
            </div>
          )}
        </div>
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button disabled={save.isPending} onClick={() => save.mutate()}>
          Save
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

function AddRoleDialog({
  user,
  orgId,
  teams,
  defaultProjectId,
  onOpenChange,
  onDone,
}: {
  user: UserRow;
  orgId: string;
  teams: TeamRow[];
  defaultProjectId?: string;
  onOpenChange: (open: boolean) => void;
  onDone: () => void;
}) {
  const [scopeType, setScopeType] =
    React.useState<(typeof MEMBERSHIP_SCOPE_TYPES)[number]>("org");
  const [teamId, setTeamId] = React.useState<string>(teams[0]?.id ?? "");
  const [projectId, setProjectId] = React.useState<string>(
    defaultProjectId ?? "",
  );
  const [role, setRole] = React.useState<string>("member");

  const scopeId =
    scopeType === "org" ? orgId : scopeType === "team" ? teamId : projectId;

  const create = useMutation({
    mutationFn: () =>
      createMembership(orgId, {
        user_id: user.id,
        scope_type: scopeType,
        scope_id: scopeId,
        role,
      }),
    onSuccess: () => {
      onDone();
      onOpenChange(false);
    },
  });

  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>Grant role</DialogTitle>
        <DialogDescription>
          Grant <span className="font-mono">{user.email}</span> a role at a
          scope within this org.
        </DialogDescription>
      </DialogHeader>
      <div className="space-y-3">
        <Field label="Scope">
          <Select
            value={scopeType}
            onChange={(e) =>
              setScopeType(
                e.target.value as (typeof MEMBERSHIP_SCOPE_TYPES)[number],
              )
            }
          >
            {MEMBERSHIP_SCOPE_TYPES.map((t) => (
              <option key={t} value={t}>
                {t}
              </option>
            ))}
          </Select>
        </Field>
        {scopeType === "team" && (
          <Field label="Team">
            <Select value={teamId} onChange={(e) => setTeamId(e.target.value)}>
              {teams.length === 0 && <option value="">no teams in org</option>}
              {teams.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name}
                </option>
              ))}
            </Select>
          </Field>
        )}
        {scopeType === "project" && (
          <Field
            label="Project id"
            hint="uuid of a project in this org (from the Providers/Keys scope)"
          >
            <Input
              value={projectId}
              onChange={(e) => setProjectId(e.target.value)}
              placeholder="00000000-0000-0000-0000-000000000000"
              className="font-mono text-xs"
            />
          </Field>
        )}
        <Field label="Role">
          <Select value={role} onChange={(e) => setRole(e.target.value)}>
            {ROLES.map((r) => (
              <option key={r} value={r}>
                {r}
              </option>
            ))}
          </Select>
        </Field>
        {create.isError && (
          <p className="text-xs text-destructive">
            {(create.error as Error).message}
          </p>
        )}
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button
          disabled={!scopeId.trim() || create.isPending}
          onClick={() => create.mutate()}
        >
          Grant
        </Button>
      </DialogFooter>
    </Dialog>
  );
}
