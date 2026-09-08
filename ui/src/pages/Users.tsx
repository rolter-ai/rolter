import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Ban, Building2, Loader2, Pencil, Plus, Trash2, UsersRound } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { EditorSheet } from "@/components/EditorSheet";
import {
  ListHeader,
  ListRow,
  ListTable,
  PageBody,
  RowIconButton,
  SearchInput,
  Toolbar,
} from "@/components/screen";
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
import { Switch } from "@/components/ui/switch";
import {
  createInvitation,
  createMembership,
  deleteUser,
  fetchMemberships,
  fetchUsers,
  inviteUser,
  MEMBERSHIP_SCOPE_TYPES,
  ROLES,
  updateUser,
  type MembershipRow,
  type Role,
  type TeamRow,
  type UserRow,
} from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

// admin surface for the user/team lifecycle (ROL-223): invite accounts into the
// current org, grant/revoke roles at org/team/project scope, and
// deactivate/delete accounts. everything is scoped to the org selected in the
// sidebar ScopeSwitcher; account edits (email/password/superadmin) require
// superadmin on the backend, org-scoped invite/role-grant require org admin.
export default function Users() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const scope = useScope();
  // the scope hook names a catalog key rather than carrying english copy
  const scopeMessage = scope.errorKey ? t(scope.errorKey) : undefined;
  const orgId = scope.orgId;
  const queryClient = useQueryClient();
  const toast = useToast();

  const users = useQuery({
    queryKey: ["users", orgId],
    queryFn: () => fetchUsers(orgId as string),
    enabled: !!orgId,
  });


  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;

  // `users` is the query the user is actually waiting on for this screen

  useScreenReady(!users.isLoading);

  useErrorState(!!users.error, "users");

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
    onSuccess: (_result, user) => {
      invalidate();
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: user.email }),
      });
    },
    onError: (error, user) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: user.email }),
        detail: errorDetail(error),
      });
    },
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

  const filtersActive = !!q || statusTab !== "all";
  const clearFilters = () => {
    setSearch("");
    setStatusTab("all");
  };

  const GRID = "1.7fr 1.4fr 110px 1fr 110px";
  // the categorical chip palette, one token per entry: a raw hex here is not
  // retunable and is contrast-checked by nobody, which is how the gold entry
  // reached white initials at 3.25:1 (#1181, #1245). the ratios are recorded
  // beside the tokens in index.css
  const AVATARS = [
    "var(--avatar-1)",
    "var(--avatar-2)",
    "var(--avatar-3)",
    "var(--avatar-4)",
    "var(--avatar-5)",
    "var(--avatar-6)",
  ];

  return (
    <PageBody>
      <Toolbar>
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
                "border-b-2 px-3 py-[7px] text-sm capitalize transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring " +
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
        <GatedButton gate="invitation:create" className="ml-auto" onClick={() => setInviteOpen(true)} disabled={!orgId}>
          <Plus className="h-4 w-4" />
          Invite user
        </GatedButton>
      </Toolbar>

      {!orgId && (
        <EmptyState
          uxTarget="users-no-org"
          icon={<Building2 />}
          title={t("pages.users.noOrgTitle")}
          description={scopeMessage ?? t("pages.users.noOrgBody")}
        />
      )}
      {(users.error || memberships.error) && (
        <LoadError
          error={users.error ?? memberships.error}
          resource={t("errors.resources.users")}
          onRetry={() => {
            users.refetch();
            memberships.refetch();
          }}
        />
      )}

      <ListTable>
        <ListHeader grid={GRID}>
          <span>User</span>
          <span>Roles</span>
          <span>Status</span>
          <span>Created</span>
          <span />
        </ListHeader>
        {orgId && users.isLoading && <ListSkeleton rows={4} className="p-3" />}
        {rows.map((user, i) => {
          const active = !user.deactivated_at;
          const grants = byUser.get(user.id) ?? [];
          const initials = user.email.slice(0, 2).toUpperCase();
          return (
            <ListRow
              key={user.id}
              grid={GRID}
              // a blocked account reads as a quieter band, not as faded text
              // — container opacity takes every glyph under 4.5:1 (#1181)
              className={active ? undefined : "bg-[color:var(--surface-subtle)]/60"}
            >
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
                      <span className="flex-none rounded-[3px] border border-[color:var(--red-folk)] px-1 text-[9px] uppercase tracking-[0.06em] text-[color:var(--red-folk-text)]">
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
                    color: active ? "var(--status-success-text)" : "var(--status-danger-text)",
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
                {fmt.date(user.created_at ?? "")}
              </span>
              <div className="flex justify-end gap-[5px]">
                {/* an icon button's accessible name is the whole of what a
                    screen reader gets, so it names the account (#1214) */}
                <RowIconButton
                  gate="membership:create"
                  title={t("pages.users.grantRole", { email: user.email })}
                  aria-label={t("pages.users.grantRole", { email: user.email })}
                  onClick={() => setRoleUser(user)}
                >
                  <Plus className="h-3.5 w-3.5" />
                </RowIconButton>
                <RowIconButton
                  gate="user:update"
                  title={t("pages.users.editUser", { email: user.email })}
                  aria-label={t("pages.users.editUser", { email: user.email })}
                  onClick={() => setEditUser(user)}
                >
                  <Pencil className="h-3.5 w-3.5" />
                </RowIconButton>
                <RowIconButton
                  gate="user:update"
                  danger={active}
                  title={t(
                    active ? "pages.users.deactivate" : "pages.users.reactivate",
                    { email: user.email },
                  )}
                  aria-label={t(
                    active ? "pages.users.deactivate" : "pages.users.reactivate",
                    { email: user.email },
                  )}
                  disabled={toggleActive.isPending && toggleActive.variables?.id === user.id}
                  onClick={() => toggleActive.mutate(user)}
                >
                  {toggleActive.isPending && toggleActive.variables?.id === user.id ? (
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  ) : (
                    <Ban className="h-3.5 w-3.5" />
                  )}
                </RowIconButton>
              </div>
            </ListRow>
          );
        })}
        {orgId && !users.isLoading && rows.length === 0 && (
          <EmptyState
            uxTarget="users"
            icon={<UsersRound />}
            title={filtersActive ? t("pages.users.noMatchTitle") : t("pages.users.emptyTitle")}
            description={filtersActive ? t("pages.users.noMatchBody") : t("pages.users.emptyBody")}
            actions={
              filtersActive ? (
                <Button variant="outline" onClick={clearFilters}>
                  {t("common.clearSearch")}
                </Button>
              ) : (
                <GatedButton gate="invitation:create" disabled={!orgId} onClick={() => setInviteOpen(true)}>
                  {t("pages.users.emptyAction")}
                </GatedButton>
              )
            }
          />
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
  const { t } = useTranslation();
  const toast = useToast();
  const [email, setEmail] = React.useState("");
  const [password, setPassword] = React.useState("");
  const [role, setRole] = React.useState<string>("member");
  // default to a link: it is the only method where nobody but the invitee ever
  // knows their password
  const [method, setMethod] = React.useState<"link" | "password">("link");
  const [link, setLink] = React.useState<string | null>(null);
  const [copied, setCopied] = React.useState(false);

  React.useEffect(() => {
    if (open) {
      setEmail("");
      setPassword("");
      setRole("member");
      setMethod("link");
      setLink(null);
      setCopied(false);
    }
  }, [open]);

  const create = useMutation({
    mutationFn: async () => {
      if (method === "link") {
        const created = await createInvitation(orgId, {
          email: email.trim(),
          role: role as Role,
        });
        return created.accept_url;
      }
      await inviteUser(orgId, {
        email: email.trim(),
        password: password.trim() ? password : undefined,
        role,
      });
      return null;
    },
    onSuccess: (accept_url) => {
      onDone();
      toast.push({ tone: "success", title: t("toast.created", { what: email.trim() }) });
      // the link is shown once and never recoverable, so the dialog stays open
      // until it has been copied somewhere
      if (accept_url) setLink(accept_url);
      else onOpenChange(false);
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: email.trim() }),
        detail: errorDetail(error),
      });
    },
  });

  // the one-time link keeps its own center Dialog rather than the editor sheet:
  // it is a reveal-once secret with a copy/done footer, not a form — the same
  // split Keys and Account already use for a freshly minted key
  if (link) {
    return (
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogHeader>
          <DialogTitle>Invitation link</DialogTitle>
          <DialogDescription>
            Shown once and not recoverable after you close this dialog.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-3">
          <p className="text-sm text-muted-foreground">
            Send this link to <strong>{email.trim()}</strong>. It works once and
            expires in seven days.
          </p>
          <code className="block break-all rounded-md border bg-muted/40 p-2 text-xs">
            {link}
          </code>
        </div>
        <DialogFooter>
          <Button
            variant="outline"
            onClick={() => {
              void navigator.clipboard?.writeText(link);
              setCopied(true);
            }}
          >
            {copied ? "Copied" : "Copy link"}
          </Button>
          <Button onClick={() => onOpenChange(false)}>Done</Button>
        </DialogFooter>
      </Dialog>
    );
  }

  return (
    <EditorSheet
      open={open}
      onOpenChange={onOpenChange}
      title="Invite user"
      subtitle="Send a one-time link, or create the account with a password you choose"
      dirty={Boolean(email || password) || role !== "member" || method !== "link"}
      errorMessage={create.isError ? (create.error as Error).message : undefined}
      saveLabel="Invite"
      canSave={Boolean(email.trim())}
      saving={create.isPending}
      onSave={() => create.mutate()}
    >
        <div className="space-y-3">
          <Field label="Email">
            <Input
              type="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              placeholder="dev@example.com"
            />
          </Field>
          <Field label="Method">
            <Select
              value={method}
              onChange={(e) =>
                setMethod(e.target.value as "link" | "password")
              }
            >
              <option value="link">Invitation link (they pick a password)</option>
              <option value="password">Set a password now</option>
            </Select>
          </Field>
          {method === "password" && (
            <Field
              label="Password (optional)"
              hint="at least 8 characters if set; blank leaves an SSO-only account"
            >
              <Input
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                placeholder="leave blank for SSO-only"
              />
            </Field>
          )}
          <Field label="Org role">
            <Select value={role} onChange={(e) => setRole(e.target.value)}>
              {ROLES.map((r) => (
                <option key={r} value={r}>
                  {r}
                </option>
              ))}
            </Select>
          </Field>
        </div>
    </EditorSheet>
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
  const { t } = useTranslation();
  const toast = useToast();
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
      // the sheet closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: user.email }),
      });
      onDone();
      onOpenChange(false);
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: user.email }),
        detail: errorDetail(error),
      });
    },
  });

  const remove = useMutation({
    mutationFn: () => deleteUser(user.id),
    onSuccess: () => {
      toast.push({ tone: "success", title: t("toast.deleted", { what: user.email }) });
      onDone();
      onOpenChange(false);
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.deleteFailed", { what: user.email }),
        detail: errorDetail(error),
      });
    },
  });

  return (
    <EditorSheet
      open
      onOpenChange={onOpenChange}
      title="Edit user"
      subtitle="Email, password and the superadmin flag require superadmin privileges"
      dirty={
        email.trim() !== user.email ||
        password !== "" ||
        isSuperadmin !== user.is_superadmin
      }
      errorMessage={save.isError ? (save.error as Error).message : undefined}
      saveLabel="Save"
      canSave
      saving={save.isPending}
      onSave={() => save.mutate()}
    >
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
          <Switch
            checked={isSuperadmin}
            aria-labelledby="user-superadmin-label"
            onCheckedChange={setIsSuperadmin}
          />
          <span id="user-superadmin-label">{t("pages.users.superadmin")}</span>
        </label>

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
              <p className="text-xs text-[color:var(--status-danger-text)]">
                Delete <span className="font-mono">{user.email}</span>? This
                can't be undone.
              </p>
              {remove.isError && (
                <p className="text-xs text-[color:var(--status-danger-text)]">
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
    </EditorSheet>
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
  const { t } = useTranslation();
  const toast = useToast();
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
      // the sheet closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: user.email }),
      });
      onDone();
      onOpenChange(false);
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: user.email }),
        detail: errorDetail(error),
      });
    },
  });

  return (
    <EditorSheet
      open
      onOpenChange={onOpenChange}
      title="Grant role"
      subtitle={`Grant ${user.email} a role at a scope within this org`}
      dirty={
        scopeType !== "org" ||
        teamId !== (teams[0]?.id ?? "") ||
        projectId !== (defaultProjectId ?? "") ||
        role !== "member"
      }
      errorMessage={create.isError ? (create.error as Error).message : undefined}
      saveLabel="Grant"
      canSave={Boolean(scopeId.trim())}
      saving={create.isPending}
      onSave={() => create.mutate()}
    >
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
      </div>
    </EditorSheet>
  );
}
