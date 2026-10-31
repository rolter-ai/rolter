import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import {
  ArrowLeftRight,
  Building2,
  Loader2,
  Mail,
  KeyRound,
  Pencil,
  Plus,
  Send,
  ShieldCheck,
  Trash2,
  UserCheck,
  UsersRound,
  UserX,
  X,
} from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";
import { useSearchParams } from "react-router";

import { ConfirmDialog } from "@/components/ConfirmDialog";
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
  primaryColumn,
} from "@/components/screen";
import {
  AssignSubjectProfileSheet,
  SubjectProfiles,
  UnassignSubjectProfileDialog,
  type UnassignTarget,
} from "@/components/SubjectProfiles";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Combobox, type ComboboxOption } from "@/components/ui/combobox";
import { Segmented } from "@/components/ui/segmented";
import { SecretRevealDialog } from "@/components/ui/secret-reveal";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { SwitchRow } from "@/components/ui/switch-row";
import {
  ApiError,
  checkMailRelay,
  createInvitation,
  createMembership,
  deleteMembership,
  deleteUser,
  fetchMemberships,
  fetchOrgMfaStatus,
  setDeploymentRole,
  type DeploymentRole,
  fetchUsers,
  inviteUser,
  issuePasswordReset,
  listInvitations,
  resendInvitation,
  revokeInvitation,
  ROLES,
  updateMembership,
  updateUser,
  type Invitation,
  type MembershipRow,
  type OrgMemberMfa,
  type Role,
  type UserRow,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { avatarColor } from "@/lib/avatar";
import { useOptionalAuth } from "@/lib/auth";
import { useCan, useCapabilities } from "@/lib/can";
import { useFormat } from "@/lib/i18n/format";
import { classifyLoadError } from "@/lib/load-error";
import {
  afterRevoke,
  grantScope,
  isLastOrgAdmin,
  membershipScope,
  sameScope,
} from "@/lib/role-grants";
import { roleLabel } from "@/lib/roles";
import { type ReadState } from "@/lib/read-state";
import { useScope } from "@/lib/scope";
import { useToast } from "@/lib/toast";
import {
  filterValue,
  parseFilterValue,
  readUserScopeFilter,
  scopeExists,
  usersInScope,
  writeUserScopeFilter,
  type UserScopeFilter,
} from "@/lib/user-scope-filter";
import { useScreenReady } from "@/lib/ux-react";

const DEPLOYMENT_ROLE_KEY: Record<DeploymentRole, string> = {
  operator: "pages.users.deploymentRole.operator",
  security_auditor: "pages.users.deploymentRole.securityAuditor",
};

const NO_DEPLOYMENT_ROLE = "none";

/**
 * Give an account a deployment role, or clear it (#3290). Superadmin only
 * (`deployment_role:update`): neither role can grant itself or the other. The
 * change reaches the account's next request, with no session to revoke.
 */
function DeploymentRoleSheet({
  target,
  onClose,
  onDone,
}: {
  target: UserRow | null;
  onClose: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const held = target?.deployment_role ?? NO_DEPLOYMENT_ROLE;
  const [role, setRole] = React.useState<string>(held);
  React.useEffect(() => setRole(held), [held, target?.id]);

  const save = useMutation({
    mutationFn: () =>
      setDeploymentRole(
        (target as UserRow).id,
        role === NO_DEPLOYMENT_ROLE ? null : (role as DeploymentRole),
      ),
    onSuccess: (user) => {
      onDone();
      toast.push({
        tone: "success",
        title: user.deployment_role
          ? t("pages.users.deploymentRole.toastSet", {
              email: user.email,
              role: t(DEPLOYMENT_ROLE_KEY[user.deployment_role]),
            })
          : t("pages.users.deploymentRole.toastCleared", { email: user.email }),
      });
      onClose();
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: target?.email ?? "" }),
        detail: errorText(error, t),
      });
    },
  });

  return (
    <EditorSheet
      name="user-deployment-role"
      open={!!target}
      onOpenChange={(open) => {
        if (open) return;
        save.reset();
        onClose();
      }}
      title={t("pages.users.deploymentRole.title")}
      subtitle={t("pages.users.deploymentRole.subtitle", { email: target?.email ?? "" })}
      dirty={role !== held}
      errorMessage={save.isError ? errorText(save.error, t) : undefined}
      saveLabel={t("common.save")}
      canSave={role !== held}
      saving={save.isPending}
      onSubmit={() => save.mutate()}
    >
      <Field
        label={t("pages.users.deploymentRole.label")}
        hint={t("pages.users.deploymentRole.hint")}
      >
        <Combobox
          value={role}
          onChange={setRole}
          options={[
            {
              value: NO_DEPLOYMENT_ROLE,
              label: t("pages.users.deploymentRole.none"),
              description: t("pages.users.deploymentRole.noneBody"),
            },
            {
              value: "operator",
              label: t("pages.users.deploymentRole.operator"),
              description: t("pages.users.deploymentRole.operatorBody"),
            },
            {
              value: "security_auditor",
              label: t("pages.users.deploymentRole.securityAuditor"),
              description: t("pages.users.deploymentRole.securityAuditorBody"),
            },
          ]}
        />
      </Field>
      <p className="text-sm text-muted-foreground">{t("pages.users.deploymentRole.scopeNote")}</p>
    </EditorSheet>
  );
}

/**
 * The Second factor cell (#3264): armed or not set up, from the org's
 * enrolment roster. While the roster loads it is a skeleton and when it failed
 * (the `LoadError` above says so) a dash; an account the roster does not list
 * (deactivated, or holding no grant in the org) is "Not listed" rather than
 * wrongly "Not set up".
 */
function SecondFactorCell({
  read,
  row,
}: {
  read: { isPending: boolean; isError: boolean };
  row: OrgMemberMfa | undefined;
}) {
  const { t } = useTranslation();
  if (read.isPending) return <Skeleton width={64} height={20} radius={6} />;
  if (read.isError) return <span className="text-xs text-muted-foreground">—</span>;
  if (!row) {
    return (
      <Badge tone="neutral" title={t("pages.users.factorNotListedHint")}>
        {t("pages.users.factorNotListed")}
      </Badge>
    );
  }
  return row.enabled ? (
    <Badge
      dot
      tone="success"
      title={t("pages.users.factorArmedHint", { count: row.recovery_codes_remaining })}
    >
      {t("pages.users.factorArmed")}
    </Badge>
  ) : (
    <Badge tone="warning" title={t("pages.users.factorNoneHint")}>
      {t("pages.users.factorNone")}
    </Badge>
  );
}

// admin surface for the user/team lifecycle (ROL-223): invite people into the
// current org, a team or a project and withdraw invitations still pending,
// grant/revoke roles at org/team/project scope, and deactivate/delete accounts.
//
// everything is scoped to the org selected in the sidebar ScopeSwitcher;
// account edits (email/password/superadmin) require superadmin on the backend,
// and an invite or a role grant takes admin at the scope it reaches. what
// cuts a person off (deactivate, delete) or raises them to superadmin asks
// first, through ConfirmDialog (#2055); reactivating only gives access back
// and is one click.
export default function Users() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const scope = useScope();
  // the scope hook names a catalog key rather than carrying english copy
  const scopeMessage = scope.errorKey ? t(scope.errorKey) : undefined;
  const orgId = scope.orgId;
  const queryClient = useQueryClient();
  const toast = useToast();

  // a superadmin also lists the accounts that hold no role anywhere, such as
  // the first administrator `rolter-seed --admin-email` creates: they are in no
  // org's membership list, so without the flag their own row is nowhere to edit
  // (#2804). the control plane ignores it for anyone else. the flag is in the
  // key because AuditLog and the model sheet cache the plain list under
  // `["users", orgId]`, and the two answers must not stand in for each other
  const self = useOptionalAuth()?.user?.id;
  const callerIsSuperadmin = !!useOptionalAuth()?.user?.is_superadmin;
  const users = useQuery({
    queryKey: ["users", orgId, { includeUnassigned: callerIsSuperadmin }],
    queryFn: () => fetchUsers(orgId as string, { includeUnassigned: callerIsSuperadmin }),
    enabled: !!orgId,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;

  // `users` is the query the user is actually waiting on for this screen

  useScreenReady(!users.isLoading);

  const memberships = useQuery({
    queryKey: ["memberships", orgId],
    queryFn: () => fetchMemberships(orgId as string),
    enabled: !!orgId,
  });

  // who has an armed second factor (#3264), read with the same gate as the
  // endpoint (`org_auth_policy:read`). a "no" omits the column rather than
  // showing it refused on every row, and an answer still in flight holds it
  // back so it is not shown and then retracted; outside a provider (a story, a
  // test) no answer is coming, so there is nothing to wait for
  const can = useCan();
  const capabilities = useCapabilities();
  const awaitingGate = capabilities !== null && !capabilities.resolved;
  const factorReadable = !!orgId && !awaitingGate && can("org_auth_policy", "read") !== false;
  const factors = useQuery({
    queryKey: ["mfa-status", orgId],
    queryFn: () => fetchOrgMfaStatus(orgId as string),
    enabled: factorReadable,
    retry: false,
  });
  const factorByUser = React.useMemo(
    () => new Map((factors.data ?? []).map((row) => [row.user_id, row])),
    [factors.data],
  );

  // every team and project in the org by name, so a grant reads as the scope
  // it is and never as the head of a uuid (#2053)
  const orgScope = useOrgScope(orgId);

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["users", orgId] });
    queryClient.invalidateQueries({ queryKey: ["memberships", orgId] });
    queryClient.invalidateQueries({ queryKey: ["invitations", orgId] });
  };

  const [inviteOpen, setInviteOpen] = React.useState(false);

  // the relay is the deployment's, so the answer is a toast like a resend's:
  // which stage failed with the reason the API returns, and a control plane
  // with no relay (409) is stated as such and not as an error (#3377)
  const mailCheck = useMutation({
    mutationFn: () => checkMailRelay(orgId as string),
    onSuccess: (result) =>
      toast.push(
        result.ok
          ? { tone: "success", title: t("pages.users.mailCheck.ok") }
          : {
              tone: "error",
              title: t("pages.users.mailCheck.failed", {
                stage: t(`pages.users.mailCheck.stage.${result.stage ?? "connect"}`),
              }),
              detail: result.reason ?? t("pages.users.mailCheck.noReason"),
            },
      ),
    onError: (error) => {
      if (error instanceof ApiError && error.status === 409) {
        toast.push({ tone: "info", title: t("pages.users.mailCheck.notConfigured") });
      } else if (error instanceof ApiError && error.status === 429) {
        toast.push({ tone: "info", title: t("pages.users.mailCheck.tooSoon") });
      } else {
        toast.push({
          tone: "error",
          title: t("pages.users.mailCheck.action"),
          detail: errorText(error, t),
        });
      }
    },
  });
  const [editUser, setEditUser] = React.useState<UserRow | null>(null);
  const [deploymentRoleUser, setDeploymentRoleUser] = React.useState<UserRow | null>(null);
  const [roleUser, setRoleUser] = React.useState<UserRow | null>(null);
  const [profileUser, setProfileUser] = React.useState<UserRow | null>(null);
  const [unassignProfile, setUnassignProfile] = React.useState<UnassignTarget | null>(null);
  const [revokeTarget, setRevokeTarget] = React.useState<GrantTarget | null>(null);
  const [changeTarget, setChangeTarget] = React.useState<GrantTarget | null>(null);
  const [deactivateTarget, setDeactivateTarget] = React.useState<UserRow | null>(null);
  const [deleteTarget, setDeleteTarget] = React.useState<UserRow | null>(null);
  const [resetTarget, setResetTarget] = React.useState<UserRow | null>(null);
  const [search, setSearch] = React.useState("");
  // the team or project the list is narrowed to lives in the URL, so a link
  // (the team sheet's) can land on a team's people and a reload keeps it (#3127)
  const [params, setParams] = useSearchParams();
  const scopeFilter = readUserScopeFilter(params);
  const setScopeFilter = (next: UserScopeFilter | null) =>
    setParams((current) => writeUserScopeFilter(current, next), { replace: true });
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

  // the org's last admin grant, which the control plane refuses to revoke for
  // anyone but a superadmin, who is not offered the warning
  const isLastAdmin = (grant: MembershipRow) =>
    !callerIsSuperadmin &&
    users.data !== undefined &&
    memberships.data !== undefined &&
    isLastOrgAdmin(grant, memberships.data, users.data);

  // giving access back is the one account change that needs no question: it
  // undoes a deactivation and a misfire is one click to reverse. the other
  // direction goes through `DeactivateUserDialog`
  const reactivate = useMutation({
    mutationFn: (user: UserRow) => updateUser(user.id, { deactivated: false }),
    onSuccess: (_result, user) => {
      invalidate();
      toast.push({
        tone: "success",
        title: t("pages.users.toastReactivated", { email: user.email }),
      });
    },
    onError: (error, user) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: user.email }),
        detail: errorText(error, t),
      });
    },
  });

  const orgTeams = orgScope.byTeam.map((entry) => entry.team);
  const orgProjects = orgScope.byTeam.flatMap((entry) => entry.projects);
  // the scope is known once both org-scope reads and the memberships answered;
  // before that the list holds back rather than showing everyone as a match
  const scopeReady = !scopeFilter || (!!memberships.data && !orgScope.isLoading && !orgScope.error);
  const scopeMissing =
    !!scopeFilter && scopeReady && !scopeExists(scopeFilter, orgTeams, orgProjects);
  const inScope =
    scopeFilter && scopeReady && !scopeMissing && memberships.data
      ? usersInScope(scopeFilter, memberships.data, orgProjects)
      : undefined;
  const scopeOptions: ComboboxOption[] = [
    ...orgTeams.map((team) => ({
      value: filterValue({ kind: "team", id: team.id }),
      label: team.name,
      group: t("pages.users.scopeFilterTeams"),
    })),
    ...orgProjects.map((project) => ({
      value: filterValue({ kind: "project", id: project.id }),
      label: project.name,
      description: project.team_name,
      group: t("pages.users.scopeFilterProjects"),
    })),
  ];
  // the list read the table states: the users, plus what the scope filter needs
  const listRead: ReadState = {
    isPending:
      users.isPending || (!!scopeFilter && !scopeReady && !orgScope.error && !memberships.error),
    isSuccess: users.isSuccess && scopeReady,
    fetchStatus: users.fetchStatus,
  };

  const q = search.trim().toLowerCase();
  const rows = (users.data ?? []).filter((u) => {
    if (scopeFilter && (!inScope || !inScope.has(u.id))) return false;
    const active = !u.deactivated_at;
    if (statusTab === "active" && !active) return false;
    if (statusTab === "deactivated" && active) return false;
    return !q || u.email.toLowerCase().includes(q) || !!u.display_name?.toLowerCase().includes(q);
  });

  // no counts until the list is held: a tab reading "All 0" while the read is
  // in flight or has failed states an outage as an org with no users (#2211)
  const counts = users.data && {
    all: users.data.length,
    active: users.data.filter((u) => !u.deactivated_at).length,
    deactivated: users.data.filter((u) => !!u.deactivated_at).length,
  };

  const filtersActive = !!q || statusTab !== "all" || !!scopeFilter;
  const clearFilters = () => {
    setSearch("");
    setStatusTab("all");
    setScopeFilter(null);
  };

  const statusLabels = {
    all: t("pages.users.statusAll"),
    active: t("pages.users.statusActive"),
    deactivated: t("pages.users.statusDeactivated"),
  };

  // the roles column holds one line per grant, each with its own two controls,
  // so it takes the widest share and the table a floor that fits a grant line
  const GRID = factorReadable
    ? `${primaryColumn(1.5)} 2.2fr 1.6fr 110px 120px 0.9fr 110px`
    : `${primaryColumn(1.5)} 2.2fr 1.6fr 110px 0.9fr 110px`;

  return (
    <PageBody>
      <Toolbar>
        {/* flex-1 alone lets the field shrink to its icon beside the status
            filter at phone width; a full-width floor sends the filter to its
            own line instead */}
        <SearchInput
          className="min-w-full max-w-full sm:min-w-0 sm:max-w-[320px]"
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
        <Combobox
          aria-label={t("pages.users.scopeFilterAria")}
          className="w-full sm:w-[240px]"
          options={scopeOptions}
          value={scopeMissing ? "" : filterValue(scopeFilter)}
          onChange={(picked) => setScopeFilter(parseFilterValue(picked))}
          placeholder={t("pages.users.scopeFilterAll")}
          clearable
          disabled={!orgId}
        />
        <GatedButton
          gate="invitation:create"
          control="user-mail-check"
          className="ml-auto"
          variant="outline"
          onClick={() => mailCheck.mutate()}
          disabled={!orgId || mailCheck.isPending}
        >
          {mailCheck.isPending ? (
            <Loader2 className="h-4 w-4 motion-safe:animate-spin" aria-hidden />
          ) : (
            <Mail className="h-4 w-4" aria-hidden />
          )}
          {mailCheck.isPending
            ? t("pages.users.mailCheck.checking")
            : t("pages.users.mailCheck.action")}
        </GatedButton>
        <GatedButton
          gate="invitation:create"
          control="user-invite"
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
      {factors.error && (
        // load-error-allow: the roster only adds a column to the users list, whose own read pairs as users
        <LoadError
          error={factors.error}
          resource={t("errors.resources.mfaStatus")}
          onRetry={() => factors.refetch()}
          target="users-mfa"
        />
      )}
      {!!(users.error || memberships.error || (scopeFilter && orgScope.error)) && (
        <LoadError
          error={users.error ?? memberships.error ?? orgScope.error}
          resource={t("errors.resources.users")}
          onRetry={() => {
            users.refetch();
            memberships.refetch();
          }}
          target="users"
        />
      )}

      <ListTable label={t("screens.gov-users.title")} minWidth={factorReadable ? 1300 : 1180}>
        <ListHeader grid={GRID}>
          <ListHeaderCell>{t("pages.users.colUser")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.users.colRoles")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.users.colProfiles")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.users.colStatus")}</ListHeaderCell>
          {factorReadable && <ListHeaderCell>{t("pages.users.colSecondFactor")}</ListHeaderCell>}
          <ListHeaderCell>{t("pages.users.colCreated")}</ListHeaderCell>
          <ListActionsHeader />
        </ListHeader>
        <ListLoadingRow read={listRead}>
          <ListSkeleton rows={4} className="p-3" />
        </ListLoadingRow>
        {rows.map((user) => {
          const active = !user.deactivated_at;
          const grants = byUser.get(user.id) ?? [];
          const initials = (user.display_name || user.email).slice(0, 2).toUpperCase();
          return (
            <ListRow
              key={user.id}
              grid={GRID}
              // a deactivated account reads as a quieter band, not as faded text
              // — container opacity takes every glyph under 4.5:1 (#1181)
              className={active ? undefined : "bg-[color:var(--surface-subtle)]/60"}
            >
              <ListCell className="flex min-w-0 items-center gap-2.5">
                <span
                  className="flex h-8 w-8 flex-none items-center justify-center rounded-full font-mono text-2xs font-semibold text-white"
                  style={{ background: avatarColor(user.id) }}
                >
                  {initials}
                </span>
                <div className="min-w-0">
                  <div className="flex items-center gap-1.5">
                    {/* the name leads when the account has one, with the email
                        beneath it; without one the email stays the headline (#2434) */}
                    {user.display_name ? (
                      <span className="truncate text-sm">{user.display_name}</span>
                    ) : (
                      <span className="truncate font-mono text-sm">{user.email}</span>
                    )}
                    {user.is_superadmin && (
                      <Badge tone="neutral" className="flex-none">
                        {t("pages.users.superBadge")}
                      </Badge>
                    )}
                    {/* the deployment role (#3290): shown to anyone who can read
                        the account, changed by a superadmin alone */}
                    {user.deployment_role && (
                      <Badge
                        tone="neutral"
                        className="flex-none"
                        title={t("pages.users.deploymentRole.badgeHint")}
                      >
                        {t(DEPLOYMENT_ROLE_KEY[user.deployment_role])}
                      </Badge>
                    )}
                  </div>
                  {user.display_name && (
                    <p className="truncate font-mono text-xs text-muted-foreground">{user.email}</p>
                  )}
                  {/* the bio says who to ask about what, so others can read it (V2.1) */}
                  {user.bio && (
                    <p className="truncate text-xs text-muted-foreground" title={user.bio}>
                      {user.bio}
                    </p>
                  )}
                </div>
              </ListCell>
              <ListCell className="min-w-0">
                <RoleGrants
                  user={user}
                  grants={grants}
                  read={memberships}
                  orgScope={orgScope}
                  isLastAdmin={isLastAdmin}
                  onChange={(grant) => setChangeTarget({ grant, user })}
                  onRevoke={(grant) => setRevokeTarget({ grant, user })}
                />
              </ListCell>
              <ListCell className="min-w-0">
                {orgId && (
                  <SubjectProfiles
                    orgId={orgId}
                    subject={{ user_id: user.id }}
                    name={user.display_name || user.email}
                    onAssign={() => setProfileUser(user)}
                    onRemove={setUnassignProfile}
                  />
                )}
              </ListCell>
              <ListCell>
                <Badge dot tone={active ? "success" : "danger"}>
                  {active
                    ? t("pages.users.rowStatusActive")
                    : t("pages.users.rowStatusDeactivated")}
                </Badge>
              </ListCell>
              {factorReadable && (
                <ListCell>
                  <SecondFactorCell read={factors} row={factorByUser.get(user.id)} />
                </ListCell>
              )}
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
                {active && user.id !== self && (
                  <RowIconButton
                    gate="password_reset:create"
                    control="user-password-reset"
                    title={t("pages.users.resetPassword", { email: user.email })}
                    aria-label={t("pages.users.resetPassword", { email: user.email })}
                    onClick={() => setResetTarget(user)}
                  >
                    <KeyRound className="h-3.5 w-3.5" />
                  </RowIconButton>
                )}
                <RowIconButton
                  gate="deployment_role:update"
                  control="user-deployment-role"
                  title={t("pages.users.deploymentRole.set", { email: user.email })}
                  aria-label={t("pages.users.deploymentRole.set", { email: user.email })}
                  onClick={() => setDeploymentRoleUser(user)}
                >
                  <ShieldCheck className="h-3.5 w-3.5" />
                </RowIconButton>
                <RowIconButton
                  gate="user:update"
                  control="user-deactivate"
                  title={t(active ? "pages.users.deactivate" : "pages.users.reactivate", {
                    email: user.email,
                  })}
                  aria-label={t(active ? "pages.users.deactivate" : "pages.users.reactivate", {
                    email: user.email,
                  })}
                  disabled={reactivate.isPending && reactivate.variables?.id === user.id}
                  onClick={() => (active ? setDeactivateTarget(user) : reactivate.mutate(user))}
                >
                  {reactivate.isPending && reactivate.variables?.id === user.id ? (
                    <Loader2 className="h-3.5 w-3.5 motion-safe:animate-spin" />
                  ) : active ? (
                    <UserX className="h-3.5 w-3.5" />
                  ) : (
                    <UserCheck className="h-3.5 w-3.5" />
                  )}
                </RowIconButton>
              </ListCell>
            </ListRow>
          );
        })}
        <ListEmptyRow read={listRead} rows={rows.length}>
          <EmptyState
            thread
            uxTarget="users"
            icon={<UsersRound />}
            title={
              scopeMissing
                ? t("pages.users.scopeUnknownTitle")
                : filtersActive
                  ? t("pages.users.noMatchTitle")
                  : t("pages.users.emptyTitle")
            }
            description={
              scopeMissing
                ? t("pages.users.scopeUnknownBody", { id: scopeFilter?.id ?? "" })
                : filtersActive
                  ? t("pages.users.noMatchBody")
                  : t("pages.users.emptyBody")
            }
            actions={
              filtersActive ? (
                <Button variant="outline" onClick={clearFilters}>
                  {t("common.clearFilters")}
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
      {/* mounted whether or not a sheet is open, so the confirmations it raises
          over the sheet see their own landing (#2055) */}
      <EditUserDialog
        user={editUser}
        onClose={() => setEditUser(null)}
        onDone={invalidate}
        onDelete={setDeleteTarget}
      />
      <DeploymentRoleSheet
        target={deploymentRoleUser}
        onClose={() => setDeploymentRoleUser(null)}
        onDone={invalidate}
      />
      {profileUser && orgId && (
        <AssignSubjectProfileSheet
          orgId={orgId}
          subject={{ user_id: profileUser.id }}
          name={profileUser.display_name || profileUser.email}
          onClose={() => setProfileUser(null)}
        />
      )}
      <UnassignSubjectProfileDialog
        target={unassignProfile}
        onClose={() => setUnassignProfile(null)}
      />
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
      {orgId && (
        <ResetPasswordDialog
          target={resetTarget}
          orgId={orgId}
          onClose={() => setResetTarget(null)}
          onDone={invalidate}
        />
      )}
      {/* each stays mounted and opens on a target, so each sees its own
          landing and reports it (docs/dev-docs/development/destructive-actions.md) */}
      <DeactivateUserDialog
        target={deactivateTarget}
        onClose={() => setDeactivateTarget(null)}
        onDone={invalidate}
      />
      {/* a delete raised from the edit sheet, which stays open behind it so a
          cancel returns to the form; a landed delete takes the sheet with it */}
      <DeleteUserDialog
        target={deleteTarget}
        onClose={() => setDeleteTarget(null)}
        onDone={() => {
          invalidate();
          setEditUser(null);
        }}
      />
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
  // how emailing that link went, kept beside it so the dialog can say so (#1828)
  const [mail, setMail] = React.useState<Invitation | null>(null);

  React.useEffect(() => {
    if (open) {
      setEmail("");
      setPassword("");
      setRole("member");
      setTarget(ORG_TARGET);
      setMethod("link");
      setLink(null);
      setMail(null);
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

  // the same read the pending list makes, so it is one request: creating an
  // invitation for an address that already has a live one replaces it (#2324)
  const pendingInvitations = useQuery({
    queryKey: ["invitations", orgId],
    queryFn: () => listInvitations(orgId),
    enabled: open && method === "link",
    retry: false,
  });
  const typed = email.trim().toLowerCase();
  const replaces =
    method === "link" &&
    typed !== "" &&
    (pendingInvitations.data ?? []).some(
      (invitation) =>
        invitation.email.toLowerCase() === typed &&
        !invitation.accepted_at &&
        !invitation.revoked_at &&
        !isExpired(invitation),
    );

  const create = useMutation({
    mutationFn: async () => {
      if (method === "link") {
        const created = await createInvitation(orgId, {
          email: email.trim(),
          role: role as Role,
          ...scope,
        });
        return created;
      }
      await inviteUser(orgId, {
        email: email.trim(),
        password: password.trim() ? password : undefined,
        role,
      });
      return null;
    },
    onSuccess: (created) => {
      onDone();
      toast.push({ tone: "success", title: t("toast.created", { what: email.trim() }) });
      // the link is shown once and never recoverable, so the dialog stays open
      // until it has been copied somewhere
      if (created) {
        setLink(created.accept_url);
        setMail(created.invitation);
      } else onOpenChange(false);
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: email.trim() }),
        detail: errorText(error, t),
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

  // the one-time link keeps its own center dialog rather than the editor sheet:
  // it is a reveal-once secret, not a form. the shared reveal asks before it
  // closes over a link nobody copied, and says so when the clipboard is
  // withheld, which a plain-http dashboard on a LAN always does
  if (link) {
    return (
      <SecretRevealDialog
        name="user-invite-link"
        open={open}
        onOpenChange={onOpenChange}
        title={t("pages.users.linkTitle")}
        description={t("pages.users.linkBody")}
        secret={link}
        copyLabel={t("pages.users.copyLink")}
        doneLabel={t("pages.users.done")}
      >
        <div className="space-y-2">
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
          {/* the link is the fallback whatever happened to the email, and the
              dialog says which of the three it was (#1828) */}
          {mail && (
            <p className="text-sm text-muted-foreground">
              {mail.mail_status === "sent"
                ? t("pages.users.linkMailSent", { email: email.trim() })
                : mail.mail_status === "failed"
                  ? t("pages.users.linkMailFailed", { reason: mail.mail_error ?? "" })
                  : t("pages.users.linkMailNotConfigured")}
            </p>
          )}
        </div>
      </SecretRevealDialog>
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
      errorMessage={create.isError ? errorText(create.error, t) : undefined}
      saveLabel={t("pages.users.inviteSave")}
      canSave={Boolean(email.trim())}
      saving={create.isPending}
      onSubmit={() => create.mutate()}
    >
      <div className="space-y-3">
        <Field
          label={t("pages.users.email")}
          required
          hint={replaces ? t("pages.users.inviteReplaces", { email: email.trim() }) : undefined}
        >
          <Input
            type="email"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            placeholder={t("pages.users.emailPlaceholder")}
          />
        </Field>
        <Field label={t("pages.users.method")} info={t("pages.users.methodInfo")}>
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
          info={t("pages.users.scopeInfo")}
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
        <Field label={roleFieldLabel} info={t("pages.users.roleInfo")}>
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

/** the fields the edit sheet holds, which one save sends */
interface AccountDraft {
  email: string;
  password: string;
  isSuperadmin: boolean;
}

type PrivilegeChange = "grant" | "removeOwn";

/**
 * The change a save makes to who may do what, when it asks first (#2055).
 *
 * Turning superadmin on hands over every organization and every deployment-wide
 * setting, so it is confirmed. Turning it off saves directly, except on the
 * caller's own account: the control plane does not refuse a superadmin
 * dropping their own flag, and the very next request is then made without it.
 */
function privilegeChange(user: UserRow, draft: AccountDraft, own: boolean): PrivilegeChange | null {
  if (draft.isSuperadmin && !user.is_superadmin) return "grant";
  if (!draft.isSuperadmin && user.is_superadmin && own) return "removeOwn";
  return null;
}

/**
 * What the control plane's refusal of an account change means here, under the
 * message it sent. A 403 is a caller who is not a superadmin, which a gate
 * that answered for another scope can still let through; a 404 is an account
 * that was deleted in the meantime; a 409 `last_superadmin` reads from the shared
 * catalog sentence the dialog already shows.
 */
function AccountErrorHint({ error }: { error: unknown }) {
  const { t } = useTranslation();
  if (!(error instanceof ApiError)) return null;
  if (error.status !== 403 && error.status !== 404) return null;
  return (
    <p className="text-xs text-muted-foreground">
      {error.status === 403
        ? t("pages.users.confirm.accountRefused")
        : t("pages.users.confirm.accountGone")}
    </p>
  );
}

/**
 * Editing an account: the email, a new password and the superadmin flag, with
 * a delete at the foot (#2055).
 *
 * The component stays mounted whether or not a sheet is open, because the
 * confirmations it raises sit over the sheet and must outlive it: a save that
 * lands closes the sheet in the same commit, and a dialog mounted inside it
 * would go with it before it could report the landing.
 *
 * A save asks first when it grants superadmin, or takes it off the account
 * the caller is signed in with (`privilegeChange`). Every other save goes
 * straight out. Cancelling the question returns to the sheet with the draft
 * as it was.
 */
function EditUserDialog({
  user,
  onClose,
  onDone,
  onDelete,
}: {
  user: UserRow | null;
  onClose: () => void;
  onDone: () => void;
  onDelete: (user: UserRow) => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const self = useOptionalAuth()?.user?.id;
  // the save waiting on an answer, and the account it is for: the account
  // rides along so the title keeps its name as the sheet closes under it
  const [asking, setAsking] = React.useState<{
    kind: PrivilegeChange;
    user: UserRow;
    draft: AccountDraft;
  } | null>(null);

  const save = useMutation({
    mutationFn: ({ user, draft }: { user: UserRow; draft: AccountDraft }) =>
      updateUser(user.id, {
        email: draft.email.trim() !== user.email ? draft.email.trim() : undefined,
        password: draft.password.trim() ? draft.password : undefined,
        is_superadmin: draft.isSuperadmin !== user.is_superadmin ? draft.isSuperadmin : undefined,
      }),
    onSuccess: (_result, { user }) => {
      // the sheet closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: user.email }),
      });
      onDone();
      onClose();
      setAsking(null);
    },
    onError: (error, { user }) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: user.email }),
        detail: errorText(error, t),
      });
      // deleted elsewhere: the list is stale, so it is read again
      if (error instanceof ApiError && error.status === 404) onDone();
    },
  });

  const requestSave = (draft: AccountDraft) => {
    if (!user) return;
    const kind = privilegeChange(user, draft, !!self && self === user.id);
    if (kind) setAsking({ kind, user, draft });
    else save.mutate({ user, draft });
  };

  const stopAsking = (open: boolean) => {
    if (open) return;
    setAsking(null);
    // a refusal of this save would otherwise greet the next one
    save.reset();
  };
  const confirmSave = () => asking && save.mutate({ user: asking.user, draft: asking.draft });

  return (
    <>
      {user && (
        <EditUserSheet
          key={user.id}
          user={user}
          saving={save.isPending}
          errorMessage={save.isError ? errorText(save.error, t) : undefined}
          onSave={requestSave}
          onClose={onClose}
          onDelete={onDelete}
        />
      )}
      {/* raised over the sheet, which stays open behind it: a cancel goes back
          to the form with every field as it was. granting superadmin is not a
          removal and one flip undoes it, so its button is not red */}
      <ConfirmDialog
        name="user-superadmin-grant"
        open={asking?.kind === "grant"}
        onOpenChange={stopAsking}
        tone="default"
        title={asking ? t("pages.users.confirm.superadminTitle", { email: asking.user.email }) : ""}
        description={t("pages.users.confirm.superadminBody")}
        confirmLabel={t("pages.users.confirm.superadminConfirm")}
        pending={save.isPending}
        error={save.error}
        onConfirm={confirmSave}
      >
        <AccountErrorHint error={save.error} />
      </ConfirmDialog>
      <ConfirmDialog
        name="user-superadmin-remove"
        open={asking?.kind === "removeOwn"}
        onOpenChange={stopAsking}
        title={
          asking ? t("pages.users.confirm.superadminRemoveTitle", { email: asking.user.email }) : ""
        }
        description={t("pages.users.confirm.superadminRemoveSelf")}
        confirmLabel={t("pages.users.confirm.superadminRemoveConfirm")}
        pending={save.isPending}
        error={save.error}
        onConfirm={confirmSave}
      >
        <AccountErrorHint error={save.error} />
      </ConfirmDialog>
    </>
  );
}

function EditUserSheet({
  user,
  saving,
  errorMessage,
  onSave,
  onClose,
  onDelete,
}: {
  user: UserRow;
  saving: boolean;
  errorMessage?: string;
  onSave: (draft: AccountDraft) => void;
  onClose: () => void;
  onDelete: (user: UserRow) => void;
}) {
  const { t } = useTranslation();
  const [email, setEmail] = React.useState(user.email);
  const [password, setPassword] = React.useState("");
  const [isSuperadmin, setIsSuperadmin] = React.useState(user.is_superadmin);

  return (
    <EditorSheet
      name="user-edit"
      open
      onOpenChange={(open) => !open && onClose()}
      title={t("pages.users.editTitle")}
      subtitle={t("pages.users.editSubtitle")}
      dirty={email.trim() !== user.email || password !== "" || isSuperadmin !== user.is_superadmin}
      errorMessage={errorMessage}
      saveLabel={t("common.save")}
      canSave
      saving={saving}
      onSubmit={() => onSave({ email, password, isSuperadmin })}
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
        <SwitchRow
          title={t("pages.users.superadmin")}
          checked={isSuperadmin}
          onChange={setIsSuperadmin}
          disabled={saving}
        />

        <div className="rounded-md border border-destructive/30 bg-destructive/5 p-3">
          <div className="flex items-center justify-between gap-2">
            <span className="text-xs text-muted-foreground">{t("pages.users.deleteHint")}</span>
            <Button
              size="sm"
              variant="destructive"
              disabled={saving}
              onClick={() => onDelete(user)}
            >
              <Trash2 className="h-3.5 w-3.5" aria-hidden />
              {t("common.delete")}
            </Button>
          </div>
        </div>
      </div>
    </EditorSheet>
  );
}

/**
 * Deactivating an account (#2055).
 *
 * The body says what the control plane does with it: sign-in is blocked, every
 * live session of the account ends, and the virtual keys it minted for itself
 * stop working at the gateway until it is reactivated. Nothing is deleted, so
 * the dialog says the way back too. Nothing in the control plane refuses a
 * superadmin deactivating their own account, so that case says plainly what it
 * costs.
 */
function DeactivateUserDialog({
  target,
  onClose,
  onDone,
}: {
  target: UserRow | null;
  onClose: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const self = useOptionalAuth()?.user?.id;

  const deactivate = useMutation({
    mutationFn: (user: UserRow) => updateUser(user.id, { deactivated: true }),
    onSuccess: (_result, user) => {
      onDone();
      toast.push({
        tone: "success",
        title: t("pages.users.toastDeactivated", { email: user.email }),
      });
      onClose();
    },
    onError: (error) => {
      // deleted elsewhere: the list is stale, so it is read again
      if (error instanceof ApiError && error.status === 404) onDone();
    },
  });

  const body = target ? [t("pages.users.confirm.deactivateBody")] : [];
  if (target && self && self === target.id) body.push(t("pages.users.confirm.deactivateSelf"));

  return (
    <ConfirmDialog
      name="user-deactivate"
      open={!!target}
      onOpenChange={(open) => {
        if (open) return;
        onClose();
        // a refusal for one account would otherwise greet the next one opened
        deactivate.reset();
      }}
      title={target ? t("pages.users.confirm.deactivateTitle", { email: target.email }) : ""}
      description={body.join(" ")}
      confirmLabel={t("pages.users.confirm.deactivateConfirm")}
      pending={deactivate.isPending}
      error={deactivate.error}
      onConfirm={() => target && deactivate.mutate(target)}
    >
      <AccountErrorHint error={deactivate.error} />
    </ConfirmDialog>
  );
}

/**
 * Resetting a member's password (#2240).
 *
 * An administrator never sees or picks a password. The confirmation says what
 * the reset does (ends the member's sessions now, leaves the old password valid
 * until the link is used, leaves any second factor in place), and a confirm
 * returns a one-time link in the shared reveal dialog, which asks before it
 * closes over a link nobody copied.
 */
function ResetPasswordDialog({
  target,
  orgId,
  onClose,
  onDone,
}: {
  target: UserRow | null;
  orgId: string;
  onClose: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  // kept apart from `target` so the reveal outlives the row that raised it
  const [issued, setIssued] = React.useState<{
    email: string;
    link: string;
    revoked: number;
  } | null>(null);

  const reset = useMutation({
    mutationFn: (user: UserRow) => issuePasswordReset(orgId, user.id),
    onSuccess: (created, user) => {
      onDone();
      toast.push({
        tone: "success",
        title: t("pages.users.toastResetIssued", { email: user.email }),
      });
      setIssued({ email: user.email, link: created.reset_url, revoked: created.sessions_revoked });
      onClose();
    },
    onError: (error) => {
      // deleted elsewhere: the list is stale, so it is read again
      if (error instanceof ApiError && error.status === 404) onDone();
    },
  });

  return (
    <>
      <ConfirmDialog
        name="user-password-reset"
        open={!!target}
        onOpenChange={(open) => {
          if (open) return;
          onClose();
          reset.reset();
        }}
        title={target ? t("pages.users.confirm.resetTitle", { email: target.email }) : ""}
        description={target ? t("pages.users.confirm.resetBody", { email: target.email }) : ""}
        confirmLabel={t("pages.users.confirm.resetConfirm")}
        tone="default"
        pending={reset.isPending}
        error={reset.error}
        onConfirm={() => target && reset.mutate(target)}
      >
        <AccountErrorHint error={reset.error} />
      </ConfirmDialog>
      <SecretRevealDialog
        name="user-password-reset-link"
        open={!!issued}
        onOpenChange={(open) => !open && setIssued(null)}
        title={t("pages.users.resetLinkTitle")}
        description={t("pages.users.resetLinkBody")}
        secret={issued?.link ?? ""}
        copyLabel={t("pages.users.copyLink")}
        doneLabel={t("pages.users.done")}
      >
        {issued && (
          <div className="space-y-2">
            <p className="text-sm text-muted-foreground">
              <Trans
                i18nKey="pages.users.resetLinkSendTo"
                values={{ email: issued.email }}
                components={[<strong key="email" />]}
              />
            </p>
            <p className="text-sm text-muted-foreground">
              {t("pages.users.resetLinkSessions", { count: issued.revoked })}
            </p>
          </div>
        )}
      </SecretRevealDialog>
    </>
  );
}

/**
 * Deleting an account (#2055), raised from the edit sheet.
 *
 * An account is deployment-wide, so the body says it leaves every
 * organization and not only the one on screen, and what goes with it: its
 * roles and sessions, and the keys it minted for itself, which are disabled
 * rather than deleted. It points at deactivating, the reversible way to block
 * a person, as `CostAttribution` does for retiring. Nothing in the control
 * plane refuses a superadmin deleting their own account, so that case says
 * plainly what it costs.
 */
function DeleteUserDialog({
  target,
  onClose,
  onDone,
}: {
  target: UserRow | null;
  onClose: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const self = useOptionalAuth()?.user?.id;

  const remove = useMutation({
    mutationFn: (user: UserRow) => deleteUser(user.id),
    onSuccess: (_result, user) => {
      onDone();
      toast.push({ tone: "success", title: t("toast.deleted", { what: user.email }) });
      onClose();
    },
    onError: (error) => {
      // deleted elsewhere: the list is stale, so it is read again
      if (error instanceof ApiError && error.status === 404) onDone();
    },
  });

  const body = target ? [t("pages.users.confirm.deleteBody")] : [];
  if (target && self && self === target.id) body.push(t("pages.users.confirm.deleteSelf"));

  return (
    <ConfirmDialog
      name="user-delete"
      open={!!target}
      onOpenChange={(open) => {
        if (open) return;
        onClose();
        // a refusal for one account would otherwise greet the next one opened
        remove.reset();
      }}
      title={target ? t("pages.users.confirm.deleteTitle", { email: target.email }) : ""}
      description={body.join(" ")}
      confirmLabel={t("pages.users.confirm.deleteConfirm")}
      pending={remove.isPending}
      error={remove.error}
      onConfirm={() => target && remove.mutate(target)}
    >
      <AccountErrorHint error={remove.error} />
    </ConfirmDialog>
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
        detail: errorText(error, t),
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
      errorMessage={create.isError ? errorText(create.error, t) : undefined}
      saveLabel={t("pages.users.grantSave")}
      canSave={!held}
      saving={create.isPending}
      onSubmit={() => create.mutate()}
    >
      <div className="space-y-3">
        <Field label={t("pages.users.scope")} info={t("pages.users.scopeInfo")}>
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
          info={t("pages.users.roleInfo")}
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
  isLastAdmin,
  onChange,
  onRevoke,
}: {
  user: UserRow;
  grants: MembershipRow[];
  read: { data?: MembershipRow[]; isError: boolean };
  orgScope: OrgScope;
  /** the grant the control plane would refuse to revoke, so neither action is offered */
  isLastAdmin: (grant: MembershipRow) => boolean;
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
        const last = isLastAdmin(grant);
        const fromIdp =
          grant.source === "sso" || grant.source === "scim" || grant.source === "ldap";
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
                <span className="text-2xs text-[color:var(--text-subtle)]">
                  {t(
                    grant.source === "sso"
                      ? "pages.users.viaSso"
                      : grant.source === "ldap"
                        ? "pages.users.viaLdap"
                        : "pages.users.viaScim",
                  )}
                </span>
              )}
            </span>
            {/* each control names the grant it acts on, so a screen reader
                tabbing a row of several grants can tell them apart (#1214) */}
            <RowIconButton
              gate="membership:create"
              control="user-role-change"
              disabled={last}
              title={
                last ? t("pages.users.lastAdminHint", names) : t("pages.users.changeRole", names)
              }
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
              disabled={last}
              title={
                last ? t("pages.users.lastAdminHint", names) : t("pages.users.revokeRole", names)
              }
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
        // a coded refusal (`last_org_admin`) is translated by the dialog
        // through the catalog, like every other code (#3522)
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
    if (target.grant.source === "ldap") body.push(t("pages.users.confirm.revokeFromLdap"));
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
 * Changing the role of one grant in one confirmed step (#2053, #2310).
 *
 * The change is one `PUT /api/v1/memberships/{id}`: the control plane updates
 * the row in a single statement and writes one audit row with the old and new
 * role, so the person is never left holding two roles and there is no partial
 * state to retry. When the person already holds the picked role at that scope
 * (a second grant there), the grant being changed is revoked instead, since
 * updating it would only duplicate the other.
 *
 * Demoting the org's last admin is refused with `last_org_admin`, which the
 * dialog says through the catalog's sentence for the code (#3484, #3522).
 */
function ChangeRoleDialog({
  target,
  grants,
  orgScope,
  onClose,
  onDone,
}: {
  target: GrantTarget | null;
  grants: MembershipRow[];
  orgScope: OrgScope;
  onClose: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const self = useOptionalAuth()?.user?.id;
  const [role, setRole] = React.useState("");

  const change = useMutation({
    mutationFn: async (vars: { target: GrantTarget; role: string; held: boolean }) => {
      const { grant } = vars.target;
      if (!vars.held) {
        await updateMembership(grant.id, vars.role);
        return;
      }
      try {
        await deleteMembership(grant.id);
      } catch (error) {
        // already gone is what the revoke was for
        if (error instanceof ApiError && error.status === 404) return;
        throw error;
      }
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
      setRole("");
    },
    onError: (error) => {
      // gone elsewhere: the list is stale, so it is read again
      if (error instanceof ApiError && error.status === 404) onDone();
    },
  });

  const close = () => {
    onClose();
    setRole("");
    change.reset();
  };

  const names = target && {
    role: roleLabel(t, target.grant.role),
    scope: scopePhrase(t, orgScope, target.grant),
    email: target.user.email,
  };

  const body: string[] = [];
  if (target && names) {
    body.push(t("pages.users.confirm.changeBody", names));
    if (target.grant.source === "sso") body.push(t("pages.users.confirm.changeFromSso", names));
    if (target.grant.source === "scim") body.push(t("pages.users.confirm.changeFromScim", names));
    if (target.grant.source === "ldap") body.push(t("pages.users.confirm.changeFromLdap", names));
    if (self && self === target.user.id) body.push(t("pages.users.confirm.changeSelf"));
  }

  const confirm = () => {
    if (!target) return;
    const held = grants.some(
      (other) =>
        other.id !== target.grant.id && sameScope(other, target.grant) && other.role === role,
    );
    change.mutate({ target, role, held });
  };

  return (
    <ConfirmDialog
      name="user-role-change"
      open={!!target}
      onOpenChange={(open) => !open && close()}
      title={names ? t("pages.users.confirm.changeTitle", names) : ""}
      description={body.join(" ")}
      confirmLabel={t("pages.users.confirm.changeConfirm")}
      tone="default"
      pending={change.isPending}
      error={change.error}
      confirmDisabled={!role}
      onConfirm={confirm}
    >
      {target && (
        <Field label={t("pages.users.newRole")} info={t("pages.users.roleInfo")}>
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
const PENDING_GRID = `${primaryColumn(1.5)} 0.8fr 1fr 1.3fr 1fr 56px`;

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
  onInvite,
}: {
  orgId: string;
  orgScope: OrgScope;
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

  // the same link goes out again, so the invitee's first message still works;
  // the answer says whether the relay took it this time (#3304)
  const resend = useMutation({
    mutationFn: (invitation: Invitation) => resendInvitation(invitation.id),
    onSuccess: (resent, invitation) => {
      refresh();
      toast.push(
        resent.mail_status === "sent"
          ? {
              tone: "success",
              title: t("pages.users.invitations.toastResent", { email: invitation.email }),
            }
          : {
              tone: "error",
              title: t("pages.users.invitations.toastResendFailed", { email: invitation.email }),
              detail: resent.mail_error ?? undefined,
            },
      );
    },
    onError: (error, invitation) => {
      toast.push({
        tone: "error",
        title: t("pages.users.invitations.toastResendFailed", { email: invitation.email }),
        detail: errorText(error, t),
      });
      // accepted or revoked in the meantime: the list is stale
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
          target="invitations"
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
                <ListCell className="flex min-w-0 items-center gap-2">
                  <span className="min-w-0 truncate font-mono text-sm">{invitation.email}</span>
                  {invitation.mail_status === "sent" && (
                    <Badge tone="success">{t("pages.users.invitations.mailSent")}</Badge>
                  )}
                  {invitation.mail_status === "failed" && (
                    <span title={invitation.mail_error ?? undefined}>
                      <Badge tone="warning">{t("pages.users.invitations.mailFailed")}</Badge>
                    </span>
                  )}
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
                  <InvitationSender invitation={invitation} />
                </ListCell>
                <ListCell className="flex min-w-0 flex-wrap items-center gap-1.5 font-mono text-xs text-muted-foreground">
                  {expired && <Badge tone="warning">{t("pages.users.invitations.expired")}</Badge>}
                  {fmt.date(invitation.expires_at)}
                </ListCell>
                <ListCell className="flex justify-end gap-1">
                  {/* only a link that was mailed once can be mailed again: a row
                      with no relay behind it has no kept link to send */}
                  {!expired && invitation.mail_status !== "not_configured" && (
                    <RowIconButton
                      gate="invitation:create"
                      control="invitation-resend"
                      title={t("pages.users.invitations.resend", { email: invitation.email })}
                      aria-label={t("pages.users.invitations.resend", {
                        email: invitation.email,
                      })}
                      disabled={resend.isPending}
                      onClick={() => resend.mutate(invitation)}
                    >
                      {resend.isPending && resend.variables?.id === invitation.id ? (
                        <Loader2 className="h-3.5 w-3.5 motion-safe:animate-spin" />
                      ) : (
                        <Send className="h-3.5 w-3.5" />
                      )}
                    </RowIconButton>
                  )}
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
 * The control plane resolves the address on each list row (`invited_by_email`),
 * whatever users the caller may list. No address means the invitation was sent
 * with the admin token or by an account since deleted; it is never drawn as a
 * uuid.
 */
function InvitationSender({ invitation }: { invitation: Invitation }) {
  const { t } = useTranslation();
  if (!invitation.invited_by_email) {
    return (
      <span className="text-[color:var(--text-subtle)]">
        {t("pages.users.invitations.senderNotRecorded")}
      </span>
    );
  }
  return <span className="font-mono">{invitation.invited_by_email}</span>;
}
