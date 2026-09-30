import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import {
  ArrowLeftRight,
  Ban,
  Building2,
  Loader2,
  Mail,
  Pencil,
  Plus,
  Trash2,
  UsersRound,
  X,
} from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { CopyButton } from "@/components/CopyButton";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { EditorSheet } from "@/components/EditorSheet";
import {
  ORG_TARGET,
  orgScopeText,
  OrgScopePicker,
  OrgScopePill,
  scopeTargetIds,
  useOrgScope,
  type OrgScope,
  type ScopeTarget,
} from "@/components/OrgScopePicker";
import {
  ListActionsHeader,
  ListCell,
  ListEmptyRow,
  ListHeader,
  ListHeaderCell,
  ListLoadingRow,
  ListRow,
  ListSummary,
  ListTable,
  PageBody,
  RowIconButton,
  SearchInput,
  Toolbar,
} from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Combobox } from "@/components/ui/combobox";
import { Segmented } from "@/components/ui/segmented";
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
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import {
  ApiError,
  createInvitation,
  createMembership,
  deleteMembership,
  deleteUser,
  fetchMemberships,
  fetchUsers,
  inviteUser,
  listInvitations,
  revokeInvitation,
  ROLES,
  updateUser,
  type Invitation,
  type MembershipRow,
  type Role,
  type UserRow,
} from "@/lib/api";
import { useOptionalAuth } from "@/lib/auth";
import { useCan, useCapabilities } from "@/lib/can";
import { useFormat } from "@/lib/i18n/format";
import { classifyLoadError } from "@/lib/load-error";
import { afterRevoke, grantScope, higherRole, membershipScope, sameScope } from "@/lib/role-grants";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

// admin surface for the user/team lifecycle (ROL-223): invite people into the
// current org, a team or a project and withdraw invitations still pending,
// grant/revoke roles at org/team/project scope, and deactivate/delete accounts.
// everything is scoped to the org selected in the sidebar ScopeSwitcher;
// account edits (email/password/superadmin) require superadmin on the backend,
// and an invite or a role grant takes admin at the scope it reaches.
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

  // every team and project in the org by name, so a grant reads as the scope
  // it is and never as the head of a uuid (#2053)
  const orgScope = useOrgScope(orgId);

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["users", orgId] });
    queryClient.invalidateQueries({ queryKey: ["memberships", orgId] });
    queryClient.invalidateQueries({ queryKey: ["invitations", orgId] });
  };

  const [inviteOpen, setInviteOpen] = React.useState(false);
  const [editUser, setEditUser] = React.useState<UserRow | null>(null);
  const [roleUser, setRoleUser] = React.useState<UserRow | null>(null);
  const [revokeTarget, setRevokeTarget] = React.useState<GrantTarget | null>(null);
  const [changeTarget, setChangeTarget] = React.useState<GrantTarget | null>(null);
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

  // no counts until the list is held: a tab reading "All 0" while the read is
  // in flight or has failed states an outage as an org with no users (#2211)
  const counts = users.data && {
    all: users.data.length,
    active: users.data.filter((u) => !u.deactivated_at).length,
    deactivated: users.data.filter((u) => !!u.deactivated_at).length,
  };

  const filtersActive = !!q || statusTab !== "all";
  const clearFilters = () => {
    setSearch("");
    setStatusTab("all");
  };

  const statusLabels = {
    all: t("pages.users.statusAll"),
    active: t("pages.users.statusActive"),
    deactivated: t("pages.users.statusDeactivated"),
  };

  // the roles column holds one line per grant, each with its own two controls,
  // so it takes the widest share and the table a floor that fits a grant line
  const GRID = "1.5fr 2.2fr 110px 0.9fr 110px";
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
          placeholder={t("pages.users.searchPlaceholder")}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <Segmented
          ariaLabel={t("pages.users.statusFilterAria")}
          value={statusTab}
          options={(["all", "active", "deactivated"] as const).map((tab) => ({
            value: tab,
            label: counts ? `${statusLabels[tab]} ${counts[tab]}` : statusLabels[tab],
          }))}
          onChange={setStatusTab}
        />
        <GatedButton
          gate="invitation:create"
          control="user-invite"
          className="ml-auto"
          onClick={() => setInviteOpen(true)}
          disabled={!orgId}
        >
          <Plus className="h-4 w-4" />
          {t("pages.users.inviteAction")}
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

      <ListTable label={t("screens.gov-users.title")} minWidth={960}>
        <ListHeader grid={GRID}>
          <ListHeaderCell>{t("pages.users.colUser")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.users.colRoles")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.users.colStatus")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.users.colCreated")}</ListHeaderCell>
          <ListActionsHeader />
        </ListHeader>
        <ListLoadingRow read={users}>
          <ListSkeleton rows={4} className="p-3" />
        </ListLoadingRow>
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
              <ListCell className="flex min-w-0 items-center gap-2.5">
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
                        {t("pages.users.superBadge")}
                      </span>
                    )}
                  </div>
                </div>
              </ListCell>
              <ListCell className="min-w-0">
                <RoleGrants
                  user={user}
                  grants={grants}
                  read={memberships}
                  orgScope={orgScope}
                  onChange={(grant) => setChangeTarget({ grant, user })}
                  onRevoke={(grant) => setRevokeTarget({ grant, user })}
                />
              </ListCell>
              <ListCell>
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
                  {active ? t("pages.users.statusActive") : t("pages.users.statusBlocked")}
                </span>
              </ListCell>
              <ListCell className="font-mono text-xs text-muted-foreground">
                {fmt.date(user.created_at ?? "")}
              </ListCell>
              <ListCell className="flex justify-end gap-[5px]">
                {/* an icon button's accessible name is the whole of what a
                    screen reader gets, so it names the account (#1214) */}
                <RowIconButton
                  gate="membership:create"
                  control="user-role-grant"
                  title={t("pages.users.grantRole", { email: user.email })}
                  aria-label={t("pages.users.grantRole", { email: user.email })}
                  onClick={() => setRoleUser(user)}
                >
                  <Plus className="h-3.5 w-3.5" />
                </RowIconButton>
                <RowIconButton
                  gate="user:update"
                  control="user-edit"
                  title={t("pages.users.editUser", { email: user.email })}
                  aria-label={t("pages.users.editUser", { email: user.email })}
                  onClick={() => setEditUser(user)}
                >
                  <Pencil className="h-3.5 w-3.5" />
                </RowIconButton>
                <RowIconButton
                  gate="user:update"
                  control="user-deactivate"
                  danger={active}
                  title={t(active ? "pages.users.deactivate" : "pages.users.reactivate", {
                    email: user.email,
                  })}
                  aria-label={t(active ? "pages.users.deactivate" : "pages.users.reactivate", {
                    email: user.email,
                  })}
                  disabled={toggleActive.isPending && toggleActive.variables?.id === user.id}
                  onClick={() => toggleActive.mutate(user)}
                >
                  {toggleActive.isPending && toggleActive.variables?.id === user.id ? (
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  ) : (
                    <Ban className="h-3.5 w-3.5" />
                  )}
                </RowIconButton>
              </ListCell>
            </ListRow>
          );
        })}
        <ListEmptyRow read={users} rows={rows.length}>
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
                <GatedButton
                  gate="invitation:create"
                  control="user-invite-empty"
                  disabled={!orgId}
                  onClick={() => setInviteOpen(true)}
                >
                  {t("pages.users.emptyAction")}
                </GatedButton>
              )
            }
          />
        </ListEmptyRow>
      </ListTable>

      {orgId && (
        <PendingInvitations
          orgId={orgId}
          orgScope={orgScope}
          users={users}
          onInvite={() => setInviteOpen(true)}
        />
      )}

      {orgId && (
        <InviteUserDialog
          open={inviteOpen}
          onOpenChange={setInviteOpen}
          orgId={orgId}
          orgScope={orgScope}
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
          grants={byUser.get(roleUser.id) ?? []}
          orgScope={orgScope}
          onOpenChange={(open) => !open && setRoleUser(null)}
          onDone={invalidate}
        />
      )}
      {/* both stay mounted and open on a target, so each sees its own
          landing and reports it (docs/dev-docs/development/destructive-actions.md) */}
      {orgId && (
        <RevokeRoleDialog
          target={revokeTarget}
          grants={revokeTarget ? (byUser.get(revokeTarget.user.id) ?? []) : []}
          orgId={orgId}
          orgScope={orgScope}
          onClose={() => setRevokeTarget(null)}
          onDone={invalidate}
        />
      )}
      {orgId && (
        <ChangeRoleDialog
          target={changeTarget}
          grants={changeTarget ? (byUser.get(changeTarget.user.id) ?? []) : []}
          orgId={orgId}
          orgScope={orgScope}
          onClose={() => setChangeTarget(null)}
          onDone={invalidate}
        />
      )}
    </PageBody>
  );
}

/**
 * Inviting a person, at the org, a team or a project (#2054).
 *
 * The scope is the shared `OrgScopePicker`, and the role field is named for it
 * ("Org role", "Team role", "Project role"). An invitation link is authorized
 * at the scope it grants, so a team admin invites into their own team without
 * an org admin. An account created with a password is the exception: its
 * endpoint takes no scope and grants the role on the org, which the sheet says
 * rather than offering a scope it would ignore.
 */
function InviteUserDialog({
  open,
  onOpenChange,
  orgId,
  orgScope,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
  orgScope: OrgScope;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [email, setEmail] = React.useState("");
  const [password, setPassword] = React.useState("");
  const [role, setRole] = React.useState<string>("member");
  const [target, setTarget] = React.useState<ScopeTarget>(ORG_TARGET);
  // default to a link: it is the only method where nobody but the invitee ever
  // knows their password
  const [method, setMethod] = React.useState<"link" | "password">("link");
  const [link, setLink] = React.useState<string | null>(null);

  React.useEffect(() => {
    if (open) {
      setEmail("");
      setPassword("");
      setRole("member");
      setTarget(ORG_TARGET);
      setMethod("link");
      setLink(null);
    }
  }, [open]);

  const orgOnly = method === "password";
  const picked = scopeTargetIds(orgOnly ? ORG_TARGET : target);
  const scope = membershipScope(picked, orgId);
  const roleFieldLabel =
    scope.scope_type === "project"
      ? t("pages.users.projectRole")
      : scope.scope_type === "team"
        ? t("pages.users.teamRole")
        : t("pages.users.orgRole");

  const create = useMutation({
    mutationFn: async () => {
      if (method === "link") {
        const created = await createInvitation(orgId, {
          email: email.trim(),
          role: role as Role,
          ...scope,
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

  // a gate that allowed the click answered for the scope switcher's scope, not
  // the one picked here, so a refusal says what inviting at this scope takes
  const refused = create.error instanceof ApiError && create.error.status === 403;
  const pickedScope = scopePhrase(t, orgScope, {
    team_id: picked.team_id ?? null,
    project_id: picked.project_id ?? null,
  });

  // the one-time link keeps its own center Dialog rather than the editor sheet:
  // it is a reveal-once secret with a copy/done footer, not a form — the same
  // split Keys and Account already use for a freshly minted key
  if (link) {
    return (
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogHeader>
          <DialogTitle>{t("pages.users.linkTitle")}</DialogTitle>
          <DialogDescription>{t("pages.users.linkBody")}</DialogDescription>
        </DialogHeader>
        <div className="space-y-3">
          <p className="text-sm text-muted-foreground">
            <Trans
              i18nKey="pages.users.linkSendTo"
              values={{ email: email.trim() }}
              components={[<strong key="email" />]}
            />
          </p>
          <p className="text-sm text-muted-foreground">
            {t("pages.users.linkGrants", { role: roleLabel(t, role), scope: pickedScope })}
          </p>
          {/* the link stays on screen whether or not the copy worked: the
              clipboard is withheld on a plain-http dashboard, which is common
              on a LAN, and `CopyButton` says so when it is */}
          <div className="flex items-start gap-2 rounded-md border bg-muted/40 p-2">
            <code className="min-w-0 flex-1 break-all text-xs">{link}</code>
            <CopyButton value={link} label={t("pages.users.copyLink")} />
          </div>
        </div>
        <DialogFooter>
          <Button onClick={() => onOpenChange(false)}>{t("pages.users.done")}</Button>
        </DialogFooter>
      </Dialog>
    );
  }

  return (
    <EditorSheet
      name="user-invite"
      open={open}
      onOpenChange={onOpenChange}
      title={t("pages.users.inviteTitle")}
      subtitle={t("pages.users.inviteSubtitle")}
      dirty={
        Boolean(email || password) ||
        role !== "member" ||
        method !== "link" ||
        target !== ORG_TARGET
      }
      errorMessage={create.isError ? (create.error as Error).message : undefined}
      saveLabel={t("pages.users.inviteSave")}
      canSave={Boolean(email.trim())}
      saving={create.isPending}
      onSave={() => create.mutate()}
    >
      <div className="space-y-3">
        <Field label={t("pages.users.email")}>
          <Input
            type="email"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            placeholder={t("pages.users.emailPlaceholder")}
          />
        </Field>
        <Field label={t("pages.users.method")}>
          <Combobox
            value={method}
            onChange={(picked) => setMethod(picked as "link" | "password")}
            options={[
              { value: "link", label: t("pages.users.methodLink") },
              { value: "password", label: t("pages.users.methodPassword") },
            ]}
          />
        </Field>
        {method === "password" && (
          <Field
            label={t("pages.users.passwordOptional")}
            hint={t("pages.users.passwordOptionalHint")}
          >
            <Input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder={t("pages.users.passwordOptionalPlaceholder")}
            />
          </Field>
        )}
        <Field
          label={t("pages.users.scope")}
          hint={
            orgOnly
              ? t("pages.users.scopeOrgOnly")
              : refused
                ? t("pages.users.inviteRefused", { scope: pickedScope })
                : undefined
          }
        >
          <OrgScopePicker
            orgId={orgId}
            value={orgOnly ? ORG_TARGET : target}
            onChange={setTarget}
            label={t("pages.users.scope")}
            size="default"
            className="w-full"
            disabled={orgOnly}
          />
        </Field>
        <Field label={roleFieldLabel}>
          <Combobox
            value={role}
            onChange={setRole}
            options={ROLES.map((r) => ({ value: r, label: roleLabel(t, r) }))}
          />
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
        is_superadmin: isSuperadmin !== user.is_superadmin ? isSuperadmin : undefined,
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
      name="user-edit"
      open
      onOpenChange={onOpenChange}
      title={t("pages.users.editTitle")}
      subtitle={t("pages.users.editSubtitle")}
      dirty={email.trim() !== user.email || password !== "" || isSuperadmin !== user.is_superadmin}
      errorMessage={save.isError ? (save.error as Error).message : undefined}
      saveLabel={t("common.save")}
      canSave
      saving={save.isPending}
      onSave={() => save.mutate()}
    >
      <div className="space-y-3">
        <Field label={t("pages.users.email")}>
          <Input type="email" value={email} onChange={(e) => setEmail(e.target.value)} />
        </Field>
        <Field label={t("pages.users.newPassword")} hint={t("pages.users.newPasswordHint")}>
          <Input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            placeholder={t("pages.users.newPasswordPlaceholder")}
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
              <span className="text-xs text-muted-foreground">{t("pages.users.deleteHint")}</span>
              <Button size="sm" variant="destructive" onClick={() => setConfirmDelete(true)}>
                <Trash2 className="h-3.5 w-3.5" />
                {t("common.delete")}
              </Button>
            </div>
          ) : (
            <div className="space-y-2">
              <p className="text-xs text-[color:var(--status-danger-text)]">
                <Trans
                  i18nKey="pages.users.deleteConfirmBody"
                  values={{ email: user.email }}
                  components={[<span key="email" className="font-mono" />]}
                />
              </p>
              {remove.isError && (
                <p className="text-xs text-[color:var(--status-danger-text)]">
                  {(remove.error as Error).message}
                </p>
              )}
              <div className="flex justify-end gap-2">
                <Button size="sm" variant="outline" onClick={() => setConfirmDelete(false)}>
                  {t("common.cancel")}
                </Button>
                <Button
                  size="sm"
                  variant="destructive"
                  disabled={remove.isPending}
                  onClick={() => remove.mutate()}
                >
                  {t("pages.users.deleteConfirm")}
                </Button>
              </div>
            </div>
          )}
        </div>
      </div>
    </EditorSheet>
  );
}

/**
 * Granting one more role (#2053).
 *
 * The scope is the shared `OrgScopePicker`: the org, every team in it and every
 * project under those teams, by name. It used to ask for a pasted project uuid.
 * A role the person already holds at the picked scope is refused before the
 * round trip, since the control plane would store it twice.
 */
function AddRoleDialog({
  user,
  orgId,
  grants,
  orgScope,
  onOpenChange,
  onDone,
}: {
  user: UserRow;
  orgId: string;
  grants: MembershipRow[];
  orgScope: OrgScope;
  onOpenChange: (open: boolean) => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [target, setTarget] = React.useState<ScopeTarget>(ORG_TARGET);
  const [role, setRole] = React.useState<string>("member");

  const picked = scopeTargetIds(target);
  const body = membershipScope(picked, orgId);
  const pickedGrant = {
    org_id: body.scope_type === "org" ? orgId : null,
    team_id: picked.team_id ?? null,
    project_id: picked.project_id ?? null,
  };
  const held = grants.some((grant) => grant.role === role && sameScope(grant, pickedGrant));

  const create = useMutation({
    mutationFn: () => createMembership(orgId, { user_id: user.id, ...body, role }),
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
      name="user-grant"
      open
      onOpenChange={onOpenChange}
      title={t("pages.users.grantTitle")}
      subtitle={t("pages.users.grantSubtitle", { email: user.email })}
      dirty={target !== ORG_TARGET || role !== "member"}
      errorMessage={create.isError ? (create.error as Error).message : undefined}
      saveLabel={t("pages.users.grantSave")}
      canSave={!held}
      saving={create.isPending}
      onSave={() => create.mutate()}
    >
      <div className="space-y-3">
        <Field label={t("pages.users.scope")}>
          <OrgScopePicker
            orgId={orgId}
            value={target}
            onChange={setTarget}
            label={t("pages.users.scope")}
            size="default"
            className="w-full"
          />
        </Field>
        <Field
          label={t("pages.users.role")}
          hint={
            held
              ? t("pages.users.alreadyHeld", {
                  email: user.email,
                  role: roleLabel(t, role),
                  scope: scopePhrase(t, orgScope, pickedGrant),
                })
              : undefined
          }
        >
          <Combobox
            value={role}
            onChange={setRole}
            options={ROLES.map((r) => ({ value: r, label: roleLabel(t, r) }))}
          />
        </Field>
      </div>
    </EditorSheet>
  );
}

/** one grant on one person's row, which the revoke and change dialogs act on */
interface GrantTarget {
  grant: MembershipRow;
  user: UserRow;
}

function roleLabel(t: TFunction, role: string): string {
  return t(`shell.roles.${role}`, { defaultValue: role });
}

/**
 * A grant's scope as it reads inside a sentence: "the Platform team", "the
 * whole organization". The chip on the row names the scope alone; a sentence
 * says which kind it is too, since a team and a project may share a name.
 */
function scopePhrase(
  t: TFunction,
  orgScope: OrgScope,
  grant: Pick<MembershipRow, "team_id" | "project_id">,
): string {
  const resolved = orgScope.resolve(grant);
  if (resolved.kind === "unresolved") return orgScopeText(t, orgScope, grant);
  if (resolved.kind === "org") return t("pages.users.scopePhrase.org");
  return t(grant.project_id ? "pages.users.scopePhrase.project" : "pages.users.scopePhrase.team", {
    name: resolved.name,
  });
}

/**
 * The roles column: each grant on its own line, role then scope, with the two
 * things an admin does to one: change it or revoke it (#2053).
 *
 * The grants are a read of their own, so a list still loading or one that
 * failed is never drawn as "no roles" (#2211), and a team or project whose
 * name is still coming is a placeholder rather than an "unresolved" warning
 * that retracts itself a moment later.
 */
function RoleGrants({
  user,
  grants,
  read,
  orgScope,
  onChange,
  onRevoke,
}: {
  user: UserRow;
  grants: MembershipRow[];
  read: { data?: MembershipRow[]; isError: boolean };
  orgScope: OrgScope;
  onChange: (grant: MembershipRow) => void;
  onRevoke: (grant: MembershipRow) => void;
}) {
  const { t } = useTranslation();
  if (read.data === undefined) {
    return read.isError ? (
      <span className="text-xs text-[color:var(--text-subtle)]">
        {t("pages.users.rolesUnread")}
      </span>
    ) : (
      <Skeleton width={140} height={20} radius={6} />
    );
  }
  if (grants.length === 0) {
    return (
      <span className="text-xs text-[color:var(--text-subtle)]">{t("pages.users.noRoles")}</span>
    );
  }
  return (
    <ul className="flex min-w-0 flex-col gap-1">
      {grants.map((grant) => {
        const names = {
          role: roleLabel(t, grant.role),
          scope: scopePhrase(t, orgScope, grant),
          email: user.email,
        };
        const fromIdp = grant.source === "sso" || grant.source === "scim";
        return (
          <li key={grant.id} className="flex min-w-0 items-center gap-1.5">
            <span className="flex min-w-0 flex-wrap items-center gap-1.5">
              <Badge tone="neutral">{names.role}</Badge>
              {orgScope.isLoading && grantScope(grant).type !== "org" ? (
                <Skeleton width={72} height={20} radius={6} className="inline-block" />
              ) : (
                <OrgScopePill scope={orgScope} value={grant} />
              )}
              {fromIdp && (
                <span className="text-[11px] text-[color:var(--text-subtle)]">
                  {t(grant.source === "sso" ? "pages.users.viaSso" : "pages.users.viaScim")}
                </span>
              )}
            </span>
            {/* each control names the grant it acts on, so a screen reader
                tabbing a row of several grants can tell them apart (#1214) */}
            <RowIconButton
              gate="membership:create"
              control="user-role-change"
              title={t("pages.users.changeRole", names)}
              aria-label={t("pages.users.changeRole", names)}
              onClick={() => onChange(grant)}
            >
              <ArrowLeftRight className="h-3.5 w-3.5" />
            </RowIconButton>
            {/* quiet until reached for: a row of several grants would
                otherwise carry a red mark per grant beside the row's own */}
            <RowIconButton
              className="hover:border-[color:var(--status-danger)] hover:text-[color:var(--status-danger-text)]"
              gate="membership:delete"
              control="user-role-revoke"
              title={t("pages.users.revokeRole", names)}
              aria-label={t("pages.users.revokeRole", names)}
              onClick={() => onRevoke(grant)}
            >
              <X className="h-3.5 w-3.5" />
            </RowIconButton>
          </li>
        );
      })}
    </ul>
  );
}

/** the project → team map `afterRevoke` walks, from the org-wide lists */
function teamOfProject(orgScope: OrgScope): (projectId: string) => string | undefined {
  return (projectId) =>
    orgScope.byTeam.find((entry) => entry.projects.some((p) => p.id === projectId))?.team.id;
}

/**
 * Revoking one grant (#2053).
 *
 * The body says what the person loses, worked out from their other grants the
 * way the control plane resolves a role: nothing else reaches the scope, or
 * which grant decides their role there once this one is gone. It also says
 * when this is their last role in the org (they leave this list), when the
 * grant came from the identity provider (it comes back), and when it is the
 * caller's own.
 *
 * A 404 means the grant is already gone, revoked elsewhere or reconciled away
 * by the IdP, so what the operator asked for holds and it lands as a success.
 * A 403 keeps the dialog open with the control plane's message and a line on
 * what revoking at that scope takes.
 */
function RevokeRoleDialog({
  target,
  grants,
  orgId,
  orgScope,
  onClose,
  onDone,
}: {
  target: GrantTarget | null;
  grants: MembershipRow[];
  orgId: string;
  orgScope: OrgScope;
  onClose: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const self = useOptionalAuth()?.user?.id;

  const revoke = useMutation({
    mutationFn: async ({ grant }: GrantTarget) => {
      try {
        await deleteMembership(grant.id);
        return "revoked" as const;
      } catch (error) {
        if (error instanceof ApiError && error.status === 404) return "gone" as const;
        throw error;
      }
    },
    onSuccess: (outcome, { grant, user }) => {
      onDone();
      const names = {
        role: roleLabel(t, grant.role),
        scope: scopePhrase(t, orgScope, grant),
        email: user.email,
      };
      toast.push({
        tone: "success",
        title: t(
          outcome === "gone" ? "pages.users.toastAlreadyRevoked" : "pages.users.toastRevoked",
          names,
        ),
      });
      onClose();
    },
  });

  const names = target && {
    role: roleLabel(t, target.grant.role),
    scope: scopePhrase(t, orgScope, target.grant),
    email: target.user.email,
  };

  const body: string[] = [];
  if (target && names) {
    const after = afterRevoke(target.grant, grants, orgId, teamOfProject(orgScope));
    if (after.kind === "unknown") {
      body.push(t("pages.users.confirm.revokeUnknown", names));
    } else if (after.kind === "none") {
      body.push(t("pages.users.confirm.revokeNone", names));
    } else if (sameScope(after.grant, target.grant) && after.grant.role === target.grant.role) {
      body.push(t("pages.users.confirm.revokeDuplicate", names));
    } else {
      body.push(
        t("pages.users.confirm.revokeFallback", {
          ...names,
          fallback: roleLabel(t, after.grant.role),
          fallbackScope: scopePhrase(t, orgScope, after.grant),
        }),
      );
    }
    if (grants.length === 1) body.push(t("pages.users.confirm.revokeLast"));
    if (target.grant.source === "sso") body.push(t("pages.users.confirm.revokeFromSso"));
    if (target.grant.source === "scim") body.push(t("pages.users.confirm.revokeFromScim"));
    if (self && self === target.user.id) body.push(t("pages.users.confirm.revokeSelf"));
  }

  const refused = revoke.error instanceof ApiError && revoke.error.status === 403;

  return (
    <ConfirmDialog
      name="user-role-revoke"
      open={!!target}
      onOpenChange={(open) => {
        if (open) return;
        onClose();
        // a refusal from one grant would otherwise greet the next one opened
        revoke.reset();
      }}
      title={names ? t("pages.users.confirm.revokeTitle", names) : ""}
      description={body.join(" ")}
      confirmLabel={t("pages.users.confirm.revokeConfirm")}
      pending={revoke.isPending}
      error={revoke.error}
      onConfirm={() => target && revoke.mutate(target)}
    >
      {/* the control plane's message stays verbatim below; this says what it
          means for this grant, since a gate that allowed the click was
          answering for the scope switcher's scope, not the grant's */}
      {refused && names && (
        <p className="text-xs text-muted-foreground">
          {t("pages.users.confirm.revokeRefused", names)}
        </p>
      )}
    </ConfirmDialog>
  );
}

/**
 * The revoke half of a role change failed after the grant half landed. The
 * message is the control plane's own, so the dialog can print it verbatim.
 */
class RevokeAfterGrantFailed extends Error {
  constructor(cause: unknown) {
    super(cause instanceof Error ? cause.message : String(cause));
    this.name = "RevokeAfterGrantFailed";
  }
}

/**
 * Changing the role of one grant in one confirmed step (#2053).
 *
 * The control plane has no update for a membership, so a change is a grant
 * and a revoke, in that order: at one scope the higher of two roles applies,
 * so the moment both exist grants nothing beyond the old role or the new one,
 * while the other order would leave the person with no role at all in
 * between. When the person already holds the picked role at that scope, the
 * grant is skipped and only the revoke goes out.
 *
 * A grant that fails changes nothing and is reported as it is. A revoke that
 * fails after the grant landed leaves the person holding both, and the dialog
 * says exactly that, with the role that applies meanwhile, and turns its
 * confirm into a retry of the revoke alone.
 */
function ChangeRoleDialog({
  target,
  grants,
  orgId,
  orgScope,
  onClose,
  onDone,
}: {
  target: GrantTarget | null;
  grants: MembershipRow[];
  orgId: string;
  orgScope: OrgScope;
  onClose: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const self = useOptionalAuth()?.user?.id;
  const [role, setRole] = React.useState("");
  // the role whose grant landed while the revoke did not; set, the dialog is
  // a retry of the revoke
  const [granted, setGranted] = React.useState<string | null>(null);

  const change = useMutation({
    mutationFn: async (vars: { target: GrantTarget; role: string; grantFirst: boolean }) => {
      const { grant, user } = vars.target;
      if (vars.grantFirst) {
        const scope = grantScope(grant);
        await createMembership(orgId, {
          user_id: user.id,
          scope_type: scope.type,
          scope_id: scope.id || orgId,
          role: vars.role,
        });
      }
      try {
        await deleteMembership(grant.id);
      } catch (error) {
        // already gone is what the revoke was for
        if (error instanceof ApiError && error.status === 404) return;
        throw new RevokeAfterGrantFailed(error);
      }
    },
    onError: (error, vars) => {
      if (!(error instanceof RevokeAfterGrantFailed)) return;
      setGranted(vars.role);
      // the list shows both grants, which is the state the person is in
      onDone();
    },
    onSuccess: (_void, vars) => {
      onDone();
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("pages.users.toastChanged", {
          email: vars.target.user.email,
          role: roleLabel(t, vars.role),
          scope: scopePhrase(t, orgScope, vars.target.grant),
        }),
      });
      onClose();
      clear();
    },
  });

  const clear = () => {
    setRole("");
    setGranted(null);
  };
  const close = () => {
    onClose();
    clear();
    change.reset();
  };

  const names = target && {
    role: roleLabel(t, target.grant.role),
    scope: scopePhrase(t, orgScope, target.grant),
    email: target.user.email,
  };

  const body: string[] = [];
  if (target && names) {
    if (granted) {
      body.push(
        t("pages.users.confirm.changePartial", {
          ...names,
          next: roleLabel(t, granted),
          applies: roleLabel(t, higherRole(target.grant.role, granted)),
        }),
      );
    } else {
      body.push(t("pages.users.confirm.changeBody", names));
      if (target.grant.source === "sso") body.push(t("pages.users.confirm.changeFromSso", names));
      if (target.grant.source === "scim") body.push(t("pages.users.confirm.changeFromScim", names));
      if (self && self === target.user.id) body.push(t("pages.users.confirm.changeSelf"));
    }
  }

  const confirm = () => {
    if (!target) return;
    if (granted) {
      change.mutate({ target, role: granted, grantFirst: false });
      return;
    }
    const held = grants.some(
      (other) =>
        other.id !== target.grant.id && sameScope(other, target.grant) && other.role === role,
    );
    change.mutate({ target, role, grantFirst: !held });
  };

  return (
    <ConfirmDialog
      name="user-role-change"
      open={!!target}
      onOpenChange={(open) => !open && close()}
      title={names ? t("pages.users.confirm.changeTitle", names) : ""}
      description={body.join(" ")}
      confirmLabel={
        granted && names
          ? t("pages.users.confirm.changeRetry", names)
          : t("pages.users.confirm.changeConfirm")
      }
      tone="default"
      pending={change.isPending}
      error={change.error}
      confirmDisabled={!granted && !role}
      onConfirm={confirm}
    >
      {!granted && target && (
        <Field label={t("pages.users.newRole")}>
          <Combobox
            value={role}
            onChange={setRole}
            placeholder={t("pages.users.newRolePlaceholder")}
            disabled={change.isPending}
            options={ROLES.filter((r) => r !== target.grant.role).map((r) => ({
              value: r,
              label: roleLabel(t, r),
            }))}
          />
        </Field>
      )}
    </ConfirmDialog>
  );
}

// columns of the pending table: invitee, role, scope, who sent it, expiry, revoke
const PENDING_GRID = "1.5fr 0.8fr 1fr 1.3fr 1fr 56px";

/** the link no longer works, but the invitation still holds its address until it is revoked */
function isExpired(invitation: Invitation): boolean {
  return new Date(invitation.expires_at).getTime() <= Date.now();
}

/**
 * The invitations still waiting on an answer, and the revoke for each (#2054).
 *
 * An invitation is a live credential for seven days, and the link is shown
 * once, so this list is the only place a link sent to the wrong address can be
 * withdrawn. The endpoint answers every invitation of the org, accepted and
 * revoked ones included; the list keeps the ones not yet accepted or revoked.
 * An expired one stays, marked: its link is dead but it still holds its
 * address (one unaccepted, unrevoked invitation per address per org), so
 * revoking it is what lets the address be invited again.
 *
 * Who may see the list is the control plane's call: an org admin sees every
 * invitation, a team admin the ones into the teams and projects they
 * administer, and a caller with neither is refused. The section follows the
 * way `GettingStarted` does: it waits for the capability answer rather than
 * showing and retracting, and a 403 from the list is an answer, not an outage,
 * so it is never a `forbidden` `LoadError` on a screen a viewer may open.
 */
function PendingInvitations({
  orgId,
  orgScope,
  users,
  onInvite,
}: {
  orgId: string;
  orgScope: OrgScope;
  /** the screen's own read, whose rows name who sent each invitation */
  users: { data?: UserRow[]; isError: boolean };
  onInvite: () => void;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const toast = useToast();
  const queryClient = useQueryClient();
  const can = useCan();
  const capabilities = useCapabilities();
  const headingId = React.useId();
  const [target, setTarget] = React.useState<Invitation | null>(null);

  // an explicit "no" hides the section, and an answer still in flight holds it
  // back: shown and then retracted reads as broken. outside a provider (a story,
  // a test) no answer is coming, so there is nothing to wait for
  const awaitingGate = capabilities !== null && !capabilities.resolved;
  const readable = !awaitingGate && can("invitation", "read") !== false;

  const invitations = useQuery({
    queryKey: ["invitations", orgId],
    queryFn: () => listInvitations(orgId),
    enabled: readable,
    retry: false,
  });

  const unreadable =
    invitations.error != null && classifyLoadError(invitations.error) === "forbidden";
  useErrorState(invitations.error != null && !unreadable, "invitations");

  const refresh = () => queryClient.invalidateQueries({ queryKey: ["invitations", orgId] });

  const revoke = useMutation({
    mutationFn: (invitation: Invitation) => revokeInvitation(invitation.id),
    onSuccess: (_revoked, invitation) => {
      refresh();
      toast.push({
        tone: "success",
        title: t("pages.users.invitations.toastRevoked", { email: invitation.email }),
      });
      setTarget(null);
    },
    onError: (error) => {
      // 404 is an invitation that was accepted in the meantime (the revoke only
      // touches unaccepted ones): the list is stale, so it is read again
      if (error instanceof ApiError && error.status === 404) refresh();
    },
  });

  if (!readable || unreadable) return null;

  const pending = invitations.data?.filter(
    (invitation) => !invitation.accepted_at && !invitation.revoked_at,
  );

  const names = target && {
    email: target.email,
    role: roleLabel(t, target.role),
    scope: scopePhrase(t, orgScope, target),
  };
  // a dead link is revoked only to free the address, which the body says
  const description =
    names && target
      ? isExpired(target)
        ? t("pages.users.confirm.invitationExpired", names)
        : t("pages.users.confirm.invitationBody", names)
      : "";
  const refused = revoke.error instanceof ApiError && revoke.error.status === 403;
  const gone = revoke.error instanceof ApiError && revoke.error.status === 404;
  // a failed refetch keeps the rows it already holds, so only a read that never
  // answered loses the table
  const failed = invitations.isError && invitations.data === undefined;

  return (
    <section aria-labelledby={headingId} className="flex flex-col gap-3">
      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <h2 id={headingId} className="text-sm font-medium text-foreground">
          {t("pages.users.invitations.title")}
        </h2>
        <ListSummary data={pending} className="text-xs">
          {(rows) =>
            rows.length > 0 ? t("pages.users.invitations.summary", { count: rows.length }) : null
          }
        </ListSummary>
      </div>

      {invitations.error && (
        <LoadError
          error={invitations.error}
          resource={t("errors.resources.invitations")}
          onRetry={() => invitations.refetch()}
        />
      )}

      {/* a header over nothing, under the failure that says why, is noise */}
      {!failed && (
        <ListTable label={t("pages.users.invitations.title")} minWidth={880}>
          <ListHeader grid={PENDING_GRID}>
            <ListHeaderCell>{t("pages.users.invitations.colInvitee")}</ListHeaderCell>
            <ListHeaderCell>{t("pages.users.invitations.colRole")}</ListHeaderCell>
            <ListHeaderCell>{t("pages.users.invitations.colScope")}</ListHeaderCell>
            <ListHeaderCell>{t("pages.users.invitations.colSentBy")}</ListHeaderCell>
            <ListHeaderCell>{t("pages.users.invitations.colExpires")}</ListHeaderCell>
            <ListActionsHeader />
          </ListHeader>
          <ListLoadingRow read={invitations}>
            <ListSkeleton rows={2} className="p-3" />
          </ListLoadingRow>
          {(pending ?? []).map((invitation) => {
            const expired = isExpired(invitation);
            return (
              <ListRow key={invitation.id} grid={PENDING_GRID}>
                <ListCell className="min-w-0 truncate font-mono text-sm">
                  {invitation.email}
                </ListCell>
                <ListCell>
                  <Badge tone="neutral">{roleLabel(t, invitation.role)}</Badge>
                </ListCell>
                <ListCell className="min-w-0">
                  {orgScope.isLoading && grantScope(invitation).type !== "org" ? (
                    <Skeleton width={72} height={20} radius={6} className="inline-block" />
                  ) : (
                    <OrgScopePill scope={orgScope} value={invitation} />
                  )}
                </ListCell>
                <ListCell className="min-w-0 truncate text-xs">
                  <InvitationSender invitation={invitation} users={users} />
                </ListCell>
                <ListCell className="flex min-w-0 flex-wrap items-center gap-1.5 font-mono text-xs text-muted-foreground">
                  {expired && <Badge tone="warning">{t("pages.users.invitations.expired")}</Badge>}
                  {fmt.date(invitation.expires_at)}
                </ListCell>
                <ListCell className="flex justify-end">
                  <RowIconButton
                    className="hover:border-[color:var(--status-danger)] hover:text-[color:var(--status-danger-text)]"
                    gate="invitation:delete"
                    control="invitation-revoke"
                    title={t("pages.users.invitations.revoke", { email: invitation.email })}
                    aria-label={t("pages.users.invitations.revoke", { email: invitation.email })}
                    onClick={() => setTarget(invitation)}
                  >
                    <X className="h-3.5 w-3.5" />
                  </RowIconButton>
                </ListCell>
              </ListRow>
            );
          })}
          <ListEmptyRow read={invitations} rows={pending?.length ?? 0}>
            <EmptyState
              uxTarget="invitations"
              className="py-6"
              thread={false}
              icon={<Mail />}
              title={t("pages.users.invitations.emptyTitle")}
              description={t("pages.users.invitations.emptyBody")}
              actions={
                <GatedButton
                  gate="invitation:create"
                  control="invitation-invite-empty"
                  onClick={onInvite}
                >
                  {t("pages.users.emptyAction")}
                </GatedButton>
              }
            />
          </ListEmptyRow>
        </ListTable>
      )}

      <ConfirmDialog
        name="invitation-revoke"
        open={!!target}
        onOpenChange={(open) => {
          if (open) return;
          setTarget(null);
          // a refusal from one invitation would otherwise greet the next one opened
          revoke.reset();
        }}
        title={names ? t("pages.users.confirm.invitationTitle", names) : ""}
        description={description}
        confirmLabel={t("pages.users.confirm.invitationConfirm")}
        pending={revoke.isPending}
        error={revoke.error}
        onConfirm={() => target && revoke.mutate(target)}
      >
        {/* the control plane's message stays verbatim below; this says what it
            means here, since a gate that allowed the click was answering for
            the scope switcher's scope, not the invitation's */}
        {refused && names && (
          <p className="text-xs text-muted-foreground">
            {t("pages.users.confirm.invitationRefused", names)}
          </p>
        )}
        {gone && (
          <p className="text-xs text-muted-foreground">{t("pages.users.confirm.invitationGone")}</p>
        )}
      </ConfirmDialog>
    </section>
  );
}

/**
 * Who sent an invitation, by address.
 *
 * The list row carries the sender's id only, so the address comes from the
 * org's users list the screen already reads. No id means the invitation was
 * sent with the admin token or by an account since deleted; an id this account
 * cannot see is unknown. Neither is drawn as a uuid.
 */
function InvitationSender({
  invitation,
  users,
}: {
  invitation: Invitation;
  users: { data?: UserRow[]; isError: boolean };
}) {
  const { t } = useTranslation();
  const quiet = "text-[color:var(--text-subtle)]";
  if (!invitation.invited_by) {
    return <span className={quiet}>{t("pages.users.invitations.senderNotRecorded")}</span>;
  }
  if (users.data === undefined && !users.isError) {
    return <Skeleton width={120} height={16} radius={4} />;
  }
  const sender = users.data?.find((user) => user.id === invitation.invited_by);
  return sender ? (
    <span className="font-mono">{sender.email}</span>
  ) : (
    <span className={quiet}>{t("pages.users.invitations.senderUnknown")}</span>
  );
}
