import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2, X } from "lucide-react";
import * as React from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
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
  deleteMembership,
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

  // group role grants by user for per-card rendering
  const byUser = React.useMemo(() => {
    const map = new Map<string, MembershipRow[]>();
    for (const m of memberships.data ?? []) {
      const list = map.get(m.user_id) ?? [];
      list.push(m);
      map.set(m.user_id, list);
    }
    return map;
  }, [memberships.data]);

  return (
    <div className="space-y-6">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold">Users &amp; teams</h1>
          <p className="text-sm text-muted-foreground">
            Accounts and role assignments for the current org. Invite users,
            grant roles at org/team/project scope, and deactivate or remove
            accounts.
          </p>
        </div>
        <Button size="sm" onClick={() => setInviteOpen(true)} disabled={!orgId}>
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
      {orgId && users.isLoading && (
        <p className="text-sm text-muted-foreground">Loading…</p>
      )}
      {orgId && !users.isLoading && users.data?.length === 0 && (
        <p className="text-sm text-muted-foreground">
          No users in this org yet. Invite one to get started.
        </p>
      )}

      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
        {users.data?.map((user) => (
          <UserCard
            key={user.id}
            user={user}
            grants={byUser.get(user.id) ?? []}
            teams={scope.teams}
            onEdit={() => setEditUser(user)}
            onAddRole={() => setRoleUser(user)}
            onChanged={invalidate}
          />
        ))}
      </div>

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
    </div>
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

function UserCard({
  user,
  grants,
  teams,
  onEdit,
  onAddRole,
  onChanged,
}: {
  user: UserRow;
  grants: MembershipRow[];
  teams: TeamRow[];
  onEdit: () => void;
  onAddRole: () => void;
  onChanged: () => void;
}) {
  const active = !user.deactivated_at;

  const toggleActive = useMutation({
    mutationFn: () => updateUser(user.id, { deactivated: active }),
    onSuccess: onChanged,
  });

  const revokeRole = useMutation({
    mutationFn: (id: string) => deleteMembership(id),
    onSuccess: onChanged,
  });

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center justify-between gap-2">
          <span className="truncate font-mono text-sm" title={user.email}>
            {user.email}
          </span>
          <div className="flex shrink-0 items-center gap-1">
            {user.is_superadmin && <Badge tone="accent">superadmin</Badge>}
            <Badge tone={active ? "success" : "danger"}>
              {active ? "active" : "deactivated"}
            </Badge>
          </div>
        </CardTitle>
        <CardDescription>
          {grants.length === 0
            ? "No roles in this org."
            : `${grants.length} role${grants.length === 1 ? "" : "s"} in this org`}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        {grants.length > 0 && (
          <div className="flex flex-wrap gap-1.5">
            {grants.map((m) => (
              <span
                key={m.id}
                className="inline-flex items-center gap-1 rounded-md border border-border bg-muted px-1.5 py-0.5 text-xs"
              >
                <span className="font-medium">{m.role}</span>
                <span className="text-muted-foreground">
                  @{scopeLabel(m, teams)}
                </span>
                <button
                  type="button"
                  aria-label={`Revoke ${m.role} at ${scopeLabel(m, teams)}`}
                  className="text-muted-foreground transition-colors hover:text-destructive"
                  disabled={revokeRole.isPending}
                  onClick={() => revokeRole.mutate(m.id)}
                >
                  <X className="h-3 w-3" />
                </button>
              </span>
            ))}
          </div>
        )}
        <div className="flex items-center justify-between gap-2">
          <label className="flex items-center gap-2 text-xs text-muted-foreground">
            <Switch
              checked={active}
              disabled={toggleActive.isPending}
              onCheckedChange={() => toggleActive.mutate()}
            />
            Active
          </label>
          <div className="flex items-center gap-2">
            <Button size="sm" variant="outline" onClick={onAddRole}>
              <Plus className="h-3.5 w-3.5" />
              Role
            </Button>
            <Button size="sm" variant="outline" onClick={onEdit}>
              Edit
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>
  );
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
