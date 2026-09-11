import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { KeySquare, Pencil, Trash2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { EditorSheet } from "@/components/EditorSheet";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { PageBody, Pill, RowIconButton } from "@/components/screen";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs } from "@/components/ui/tabs";
import {
  createCustomRole,
  deleteCustomRole,
  fetchAccessProfile,
  fetchAccessProfiles,
  fetchCustomRoles,
  fetchMemberships,
  fetchRbacMatrix,
  updateCustomRole,
  type CustomRoleGrantInput,
  type CustomRoleRow,
  type RbacAction,
  type RbacCustomRoleView,
  type RbacMatrix,
  type RbacResourceView,
  type Role,
} from "@/lib/api";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

// Roles & Permissions, rendered from `GET /api/v1/rbac/matrix` (#1178).
//
// It used to draw a hardcoded 3-role x 7-resource table out of lib/mock.ts,
// which had drifted well away from the ~53-resource CAPABILITIES table the
// control plane actually guards with. That table is the same one the guard
// consults, so a cell here is the rule, not a description of it.
//
// The second tab edits the org-defined half (#1184). Both tabs read the same
// matrix, because the grant grid's rows and columns *are* the matrix's
// resources: a role must not be able to name a pair the deployment does not
// define, and a grid built from a hardcoded list would do exactly that the
// first time a release added a resource.

const ACTIONS: RbacAction[] = ["read", "create", "update", "delete"];

// the order CAPABILITIES presents scopes in, widest first. a resource whose
// scope this build does not know about is still rendered, in its own group
const SCOPE_ORDER = ["deployment", "org", "team", "project"];

const BASE_ROLES: Role[] = ["viewer", "member", "admin"];

/** What a single (role, resource, action) cell says. */
type CellState =
  // the action does not exist for this resource at all — an audit log is
  // append-only, orgs have no update route. never "denied": nobody can do it,
  // including a superadmin, because there is no route to call
  | "na"
  // reserved for the deployment admin token or a superadmin account. no org
  // role reaches it, and a custom grant deliberately cannot either
  | "superadmin"
  // the role's rank clears the minimum, or the action needs no membership
  | "allowed"
  // an org-defined role names this exact pair on top of its base role
  | "granted"
  | "denied";

/** A matrix column: one built-in role, or one org-defined role. */
interface RoleColumn {
  key: string;
  label: string;
  rank: number;
  custom?: RbacCustomRoleView;
}

// a cell holds a letter, so `color` is always the -text half of the hue and the
// border keeps the fill. the quiet states carry no `opacity` either: --text-subtle
// at 0.45 is about 2.3:1, and an empty tint already reads as "not granted" (#1181)
const CELL_STYLE: Record<CellState, React.CSSProperties> = {
  allowed: {
    color: "var(--red-folk-text)",
    background: "var(--red-tint)",
    borderColor: "color-mix(in srgb, var(--red-folk) 30%, transparent)",
  },
  granted: {
    color: "var(--status-info-text)",
    background: "rgba(59, 130, 246, .14)",
    borderColor: "color-mix(in srgb, var(--status-info) 34%, transparent)",
  },
  superadmin: {
    color: "var(--status-warning-text)",
    background: "rgba(245, 158, 11, .12)",
    borderColor: "color-mix(in srgb, var(--status-warning) 32%, transparent)",
  },
  denied: {
    color: "var(--text-subtle)",
    background: "transparent",
    borderColor: "var(--border-subtle)",
  },
  na: {
    color: "var(--text-subtle)",
    background: "transparent",
    borderColor: "transparent",
  },
};

const RANK: Record<string, number> = { viewer: 0, member: 1, admin: 2 };

function cellState(
  resource: RbacResourceView,
  action: RbacAction,
  column: RoleColumn,
): CellState {
  const view = resource.actions.find((a) => a.action === action);
  // absent from `actions` means the backend's authority was None
  if (!view) return "na";
  if (view.superadmin_only) return "superadmin";
  if (view.authenticated_only) return "allowed";
  if (view.minimum_role !== null && column.rank >= RANK[view.minimum_role]) {
    return "allowed";
  }
  const granted = column.custom?.grants.some(
    (g) => g.resource === resource.resource && g.action === action,
  );
  return granted ? "granted" : "denied";
}

/** Group resources by scope, widest scope first, keeping table order within. */
function byScope(resources: RbacResourceView[]): [string, RbacResourceView[]][] {
  const groups = new Map<string, RbacResourceView[]>();
  for (const resource of resources) {
    const list = groups.get(resource.scope);
    if (list) list.push(resource);
    else groups.set(resource.scope, [resource]);
  }
  return [...groups.entries()].sort(
    ([a], [b]) =>
      (SCOPE_ORDER.indexOf(a) + 1 || SCOPE_ORDER.length + 1) -
      (SCOPE_ORDER.indexOf(b) + 1 || SCOPE_ORDER.length + 1),
  );
}

function columns(matrix: RbacMatrix): RoleColumn[] {
  const builtins = [...matrix.roles]
    .sort((a, b) => a.rank - b.rank)
    .map((r) => ({ key: r.role, label: r.role, rank: r.rank }));
  const custom = [...matrix.custom_roles]
    .sort((a, b) => a.base_rank - b.base_rank || a.name.localeCompare(b.name))
    .map((r) => ({ key: r.id, label: r.name, rank: r.base_rank, custom: r }));
  return [...builtins, ...custom];
}

export default function Rbac() {
  const { t } = useTranslation();
  const scope = useScope();
  const [tab, setTab] = React.useState("matrix");

  const matrix = useQuery({
    queryKey: ["rbac-matrix", scope.orgId],
    queryFn: () => fetchRbacMatrix(scope.orgId),
    // the built-in half is a compiled-in table and the custom half only moves
    // when someone edits a role, so it never goes stale within a session
    staleTime: Infinity,
    retry: false,
  });

  // membership counts are live and independent: the matrix is still the answer
  // to "what can a role do" even when the user may not list the org's members
  const memberships = useQuery({
    queryKey: ["memberships", scope.orgId],
    queryFn: () => fetchMemberships(scope.orgId as string),
    enabled: !!scope.orgId,
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `matrix` is the query the user is actually waiting on for this screen
  useScreenReady(!matrix.isLoading);
  useErrorState(!!matrix.error, "rbac");

  if (matrix.isLoading) return <MatrixSkeleton />;

  if (matrix.error) {
    return (
      <PageBody>
        <LoadError
          error={matrix.error}
          resource={t("errors.resources.rbacMatrix")}
          onRetry={() => matrix.refetch()}
        />
      </PageBody>
    );
  }

  if (!matrix.data) return null;

  return (
    <PageBody>
      <Tabs
        value={tab}
        onChange={setTab}
        tabs={[
          { value: "matrix", label: t("pages.rbac.tabs.matrix") },
          {
            value: "custom",
            label: t("pages.rbac.tabs.custom"),
            count: matrix.data.custom_roles.length,
          },
        ]}
      />
      {tab === "matrix" ? (
        <MatrixTab
          matrix={matrix.data}
          memberships={memberships.data ?? []}
          membershipsFailed={memberships.isError}
        />
      ) : (
        <CustomRolesTab matrix={matrix.data} orgId={scope.orgId} />
      )}
    </PageBody>
  );
}

// ---------------------------------------------------------------------------
// the built-in matrix

function MatrixTab({
  matrix,
  memberships,
  membershipsFailed,
}: {
  matrix: RbacMatrix;
  memberships: { user_id: string; role: string }[];
  membershipsFailed: boolean;
}) {
  const { t } = useTranslation();

  const memberCount = (role: string) =>
    new Set(memberships.filter((m) => m.role === role).map((m) => m.user_id)).size;

  const cols = columns(matrix);
  const grid = `minmax(200px, 1.6fr) repeat(${cols.length}, minmax(132px, 1fr))`;
  const unknown = matrix.custom_roles.flatMap((r) => r.unknown_grants);

  return (
    <>
      <div className="flex flex-wrap items-start gap-3">
        <p className="max-w-2xl text-sm text-muted-foreground">
          {t("pages.rbac.intro")}
        </p>
        {/* each count is its own node: two plural sentences sharing one text
            node cannot be read back, by a test or by a screen reader */}
        <span className="ml-auto inline-flex flex-none items-center gap-1.5 rounded-full border border-[color:var(--border-subtle)] px-2.5 py-[5px] text-xs text-muted-foreground">
          <span>{t("pages.rbac.roleCount", { count: cols.length })}</span>
          <span aria-hidden>·</span>
          <span>
            {t("pages.rbac.resourceCount", { count: matrix.resources.length })}
          </span>
        </span>
      </div>

      {unknown.length > 0 && (
        <p className="rounded-[10px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3.5 py-2.5 text-xs text-muted-foreground">
          {t("pages.rbac.unknownGrants", { count: unknown.length })}
        </p>
      )}

      <div tabIndex={0} className="overflow-x-auto focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring">
        <div className="min-w-[720px] overflow-hidden rounded-[10px] border border-[color:var(--border-subtle)]">
          <div
            className="grid items-end gap-3 border-b border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-4 py-[11px]"
            style={{ gridTemplateColumns: grid }}
          >
            <span className="text-[0.6875rem] uppercase tracking-[0.07em] text-[color:var(--text-subtle)]">
              {t("pages.rbac.resourceColumn")}
            </span>
            {cols.map((col) => (
              <div key={col.key} className="flex flex-col gap-0.5">
                <span className="text-sm font-semibold capitalize">{col.label}</span>
                <span className="text-[10px] text-[color:var(--text-subtle)]">
                  {col.custom
                    ? t("pages.rbac.viaAccessProfiles")
                    : membershipsFailed
                      ? t("pages.rbac.membersUnknown")
                      : t("pages.rbac.memberCount", { count: memberCount(col.key) })}
                </span>
              </div>
            ))}
          </div>

          {byScope(matrix.resources).map(([scopeKey, resources]) => (
            <div key={scopeKey}>
              <div className="border-b border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)]/40 px-4 py-2">
                <span className="text-[0.6875rem] uppercase tracking-[0.07em] text-[color:var(--text-subtle)]">
                  {t(`pages.rbac.scopes.${scopeKey}`, { defaultValue: scopeKey })}
                </span>
              </div>
              {resources.map((resource) => (
                <div
                  key={resource.resource}
                  className="grid items-center gap-3 border-b border-[color:var(--border-subtle)] px-4 py-[11px] last:border-b-0"
                  style={{ gridTemplateColumns: grid }}
                >
                  <span className="truncate font-mono text-xs text-[color:var(--text-secondary)]">
                    {resource.resource}
                  </span>
                  {cols.map((col) => (
                    <div key={col.key} className="flex flex-wrap gap-1">
                      {ACTIONS.map((action) => {
                        const state = cellState(resource, action, col);
                        return (
                          <span
                            key={action}
                            title={t("pages.rbac.cellTitle", {
                              action: t(`pages.rbac.actions.${action}`),
                              state: t(`pages.rbac.states.${state}`),
                            })}
                            className="flex h-5 w-[22px] items-center justify-center rounded-[6px] border font-mono text-[10px] font-semibold"
                            style={CELL_STYLE[state]}
                          >
                            {state === "na" ? "–" : t(`pages.rbac.letters.${action}`)}
                          </span>
                        );
                      })}
                    </div>
                  ))}
                </div>
              ))}
            </div>
          ))}
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-x-3.5 gap-y-2 text-xs text-[color:var(--text-subtle)]">
        {ACTIONS.map((action) => (
          <span key={action} className="inline-flex items-center gap-[5px]">
            <span
              className="flex h-[18px] w-[18px] items-center justify-center rounded-[6px] border font-mono text-[10px] font-semibold"
              style={CELL_STYLE.allowed}
            >
              {t(`pages.rbac.letters.${action}`)}
            </span>
            {t(`pages.rbac.actions.${action}`)}
          </span>
        ))}
        <span className="inline-flex items-center gap-[5px]">
          <span
            aria-hidden
            className="h-[18px] w-[18px] rounded-[6px] border"
            style={CELL_STYLE.superadmin}
          />
          {t("pages.rbac.legendSuperadmin")}
        </span>
        <span className="inline-flex items-center gap-[5px]">
          <span
            aria-hidden
            className="h-[18px] w-[18px] rounded-[6px] border"
            style={CELL_STYLE.granted}
          />
          {t("pages.rbac.legendGranted")}
        </span>
        <span className="inline-flex items-center gap-[5px]">
          <span
            className="flex h-[18px] w-[18px] items-center justify-center rounded-[6px] border font-mono text-[10px] font-semibold"
            style={CELL_STYLE.na}
          >
            –
          </span>
          {t("pages.rbac.legendNa")}
        </span>
      </div>
    </>
  );
}

// ---------------------------------------------------------------------------
// org-defined roles

/** `resource:action`, the key a ticked grid cell is held under. */
const pair = (resource: string, action: RbacAction) => `${resource}:${action}`;

/** The editor's working copy of one role. */
interface RoleDraft {
  /** absent while creating */
  id?: string;
  name: string;
  slug: string;
  description: string;
  base_role: Role;
  /** ticked cells, as `resource:action` */
  grants: string[];
  /**
   * Grants naming a resource this build's capability table does not define.
   *
   * They are not in the grid — there is no row to tick them on — so an editor
   * that sent only the grid would silently drop them, and `grants` is replaced
   * wholesale. Carried through untouched instead, which is what makes a
   * downgrade-then-edit survivable.
   */
  unknown: CustomRoleGrantInput[];
}

const emptyDraft = (): RoleDraft => ({
  name: "",
  slug: "",
  description: "",
  base_role: "viewer",
  grants: [],
  unknown: [],
});

const draftFrom = (role: CustomRoleRow, view?: RbacCustomRoleView): RoleDraft => ({
  id: role.id,
  name: role.name,
  slug: role.slug,
  description: role.description ?? "",
  base_role: role.base_role,
  grants: (view?.grants ?? []).map((g) => pair(g.resource, g.action as RbacAction)),
  unknown: (view?.unknown_grants ?? []).map((g) => ({
    resource: g.resource,
    action: g.action as RbacAction,
  })),
});

/** The wire form of a draft's grants: the ticked grid, then the carried-over. */
function grantsOf(draft: RoleDraft): CustomRoleGrantInput[] {
  const grid = draft.grants.map((key) => {
    const at = key.lastIndexOf(":");
    return { resource: key.slice(0, at), action: key.slice(at + 1) as RbacAction };
  });
  return [...grid, ...draft.unknown];
}

function CustomRolesTab({
  matrix,
  orgId,
}: {
  matrix: RbacMatrix;
  orgId?: string;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const queryClient = useQueryClient();

  const roles = useQuery({
    queryKey: ["custom-roles", orgId],
    queryFn: () => fetchCustomRoles(orgId as string),
    enabled: !!orgId,
    retry: false,
  });

  // which profiles compose each role, so the list can say what a delete would
  // have to be detached from first. a deployment has a handful of profiles, and
  // only the detail call carries the composition — the list endpoint does not.
  // a caller who may read the matrix but not the profiles simply sees no usage
  const profiles = useQuery({
    queryKey: ["access-profiles", orgId],
    queryFn: () => fetchAccessProfiles(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  const details = useQueries({
    queries: (profiles.data ?? []).map((profile) => ({
      queryKey: ["access-profile", profile.id],
      queryFn: () => fetchAccessProfile(profile.id),
      retry: false,
    })),
  });
  // role id -> the profiles composing it. a handful of entries over a handful
  // of profiles, so it is rebuilt per render rather than memoized against a
  // dependency that would have to be the whole `useQueries` result
  // and whether that map is an answer yet. the profiles resolve independently
  // of the roles, so a row rendered "not used by any access profile" — a claim,
  // not a placeholder — for as long as the detail calls were still in flight,
  // and then swapped it for the real composition (#1266)
  const usageKnown =
    !profiles.isLoading && details.every((detail) => !detail.isLoading);
  const usedBy = new Map<string, string[]>();
  for (const detail of details) {
    if (!detail.data) continue;
    for (const composed of detail.data.roles) {
      const names = usedBy.get(composed.role_id) ?? [];
      if (!names.includes(detail.data.name)) names.push(detail.data.name);
      usedBy.set(composed.role_id, names);
    }
  }

  // the matrix is what splits a role's grants into the pairs this build defines
  // and the ones it does not, so the editor seeds from there rather than
  // re-deriving the split against a capability list the dashboard does not have
  const views = React.useMemo(
    () => new Map(matrix.custom_roles.map((r) => [r.id, r])),
    [matrix.custom_roles],
  );

  const [draft, setDraft] = React.useState<RoleDraft | null>(null);
  const [seed, setSeed] = React.useState("");
  const [deleteTarget, setDeleteTarget] = React.useState<CustomRoleRow | null>(null);

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ["custom-roles", orgId] });
    // the matrix carries the same roles as columns, so a save that did not
    // refresh it would leave the other tab showing the role as it was
    void queryClient.invalidateQueries({ queryKey: ["rbac-matrix", orgId] });
  };

  const save = useMutation({
    mutationFn: (input: RoleDraft) =>
      input.id
        ? updateCustomRole(input.id, {
            name: input.name.trim(),
            description: input.description.trim() || null,
            base_role: input.base_role,
            grants: grantsOf(input),
          })
        : createCustomRole(orgId as string, {
            name: input.name.trim(),
            slug: input.slug.trim() || undefined,
            description: input.description.trim() || null,
            base_role: input.base_role,
            grants: grantsOf(input),
          }),
    onSuccess: (role, input) => {
      invalidate();
      setDraft(null);
      toast.push({
        tone: "success",
        title: input.id
          ? t("toast.saved")
          : t("toast.created", { what: role.name }),
      });
    },
    onError: (error, input) =>
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: input.name.trim() }),
        detail: errorDetail(error),
      }),
  });

  const remove = useMutation({
    mutationFn: (role: CustomRoleRow) => deleteCustomRole(role.id),
    onSuccess: (_void, role) => {
      invalidate();
      setDeleteTarget(null);
      toast.push({ tone: "success", title: t("toast.deleted", { what: role.name }) });
    },
    onError: (error, role) =>
      toast.push({
        tone: "error",
        title: t("toast.deleteFailed", { what: role.name }),
        detail: errorDetail(error),
      }),
  });

  const startCreate = () => {
    save.reset();
    setDraft(emptyDraft());
    setSeed(JSON.stringify(emptyDraft()));
  };
  const startEdit = (role: CustomRoleRow) => {
    save.reset();
    const next = draftFrom(role, views.get(role.id));
    setDraft(next);
    setSeed(JSON.stringify(next));
  };
  const startDelete = (role: CustomRoleRow) => {
    remove.reset();
    setDeleteTarget(role);
  };

  return (
    <>
      <div className="flex flex-wrap items-start gap-3">
        <p className="max-w-2xl text-sm text-muted-foreground">
          {t("pages.rbac.custom.intro")}
        </p>
        <GatedButton gate="custom_role:create" className="ml-auto" disabled={!orgId} onClick={startCreate}>
          {t("pages.rbac.custom.add")}
        </GatedButton>
      </div>

      {roles.isLoading && <ListSkeleton rows={3} />}
      {roles.error && (
        <LoadError
          error={roles.error}
          resource={t("errors.resources.customRoles")}
          onRetry={() => void roles.refetch()}
        />
      )}
      {roles.data && roles.data.length === 0 && (
        <EmptyState
          uxTarget="custom-roles"
          icon={<KeySquare />}
          title={t("pages.rbac.custom.emptyTitle")}
          description={t("pages.rbac.custom.emptyBody")}
          actions={
            <GatedButton gate="custom_role:create" disabled={!orgId} onClick={startCreate}>
              {t("pages.rbac.custom.emptyAction")}
            </GatedButton>
          }
        />
      )}

      <div className="grid gap-3">
        {(roles.data ?? []).map((role) => {
          const view = views.get(role.id);
          const grants = (view?.grants.length ?? 0) + (view?.unknown_grants.length ?? 0);
          const profileNames = usedBy.get(role.id) ?? [];
          return (
            <div
              key={role.id}
              className="flex items-start gap-3 rounded-[10px] border border-[color:var(--border-default)] bg-card p-4"
            >
              <span className="mt-0.5 flex h-[34px] w-[34px] flex-none items-center justify-center rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] text-[color:var(--text-secondary)]">
                <KeySquare className="h-4 w-4" />
              </span>
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium">{role.name}</span>
                  <Pill color="var(--text-secondary)" tint="var(--surface-subtle)">
                    {role.slug}
                  </Pill>
                  <span className="text-xs text-muted-foreground">
                    {t("pages.rbac.custom.extends", { role: role.base_role })}
                  </span>
                </div>
                {role.description && (
                  <p className="mt-1 text-sm text-muted-foreground">
                    {role.description}
                  </p>
                )}
                <div className="mt-2 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
                  <span>
                    {grants === 0
                      ? t("pages.rbac.custom.noGrants")
                      : t("pages.rbac.custom.grantCount", { count: grants })}
                  </span>
                  {usageKnown && (
                    <>
                      <span aria-hidden>·</span>
                      <span>
                        {profileNames.length === 0
                          ? t("pages.rbac.custom.usedByNone")
                          : t("pages.rbac.custom.usedBy", {
                              profiles: profileNames.join(", "),
                            })}
                      </span>
                    </>
                  )}
                </div>
              </div>
              <div className="flex flex-none items-center gap-1">
                <RowIconButton
                  gate="custom_role:update"
                  aria-label={t("pages.rbac.custom.editRole", { name: role.name })}
                  title={t("pages.rbac.custom.editRole", { name: role.name })}
                  onClick={() => startEdit(role)}
                >
                  <Pencil className="h-4 w-4" />
                </RowIconButton>
                <RowIconButton
                  danger
                  gate="custom_role:delete"
                  aria-label={t("pages.rbac.custom.deleteRole", { name: role.name })}
                  title={t("pages.rbac.custom.deleteRole", { name: role.name })}
                  onClick={() => startDelete(role)}
                >
                  <Trash2 className="h-4 w-4" />
                </RowIconButton>
              </div>
            </div>
          );
        })}
      </div>

      {draft && (
        <RoleSheet
          draft={draft}
          dirty={JSON.stringify(draft) !== seed}
          resources={matrix.resources}
          saving={save.isPending}
          errorMessage={save.error ? errorDetail(save.error) : undefined}
          onChange={setDraft}
          onClose={() => setDraft(null)}
          onSave={() => save.mutate(draft)}
        />
      )}

      <ConfirmDialog
        open={!!deleteTarget}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title={t("pages.rbac.custom.confirm.title", { name: deleteTarget?.name })}
        description={
          <>
            {t("pages.rbac.custom.confirm.body")}
            {/* the control plane refuses with 409 while a profile still
                composes the role, so naming those profiles here is the
                difference between a refusal and a next step */}
            {deleteTarget && (usedBy.get(deleteTarget.id) ?? []).length > 0 && (
              <span className="mt-1.5 block">
                {t("pages.rbac.custom.confirm.referenced", {
                  profiles: (usedBy.get(deleteTarget.id) ?? []).join(", "),
                })}
              </span>
            )}
          </>
        }
        confirmLabel={t("pages.rbac.custom.confirm.confirm")}
        pending={remove.isPending}
        error={remove.error}
        onConfirm={() => deleteTarget && remove.mutate(deleteTarget)}
      />
    </>
  );
}

function RoleSheet({
  draft,
  dirty,
  resources,
  saving,
  errorMessage,
  onChange,
  onClose,
  onSave,
}: {
  draft: RoleDraft;
  dirty: boolean;
  resources: RbacResourceView[];
  saving: boolean;
  errorMessage?: string;
  onChange: (draft: RoleDraft) => void;
  onClose: () => void;
  onSave: () => void;
}) {
  const { t } = useTranslation();

  return (
    <EditorSheet
      open
      onOpenChange={(open) => !open && onClose()}
      title={
        draft.id
          ? t("pages.rbac.custom.editTitle", { name: draft.name })
          : t("pages.rbac.custom.createTitle")
      }
      subtitle={t("pages.rbac.custom.sheetSubtitle")}
      dirty={dirty}
      errorMessage={errorMessage}
      saveLabel={draft.id ? t("pages.rbac.custom.save") : t("pages.rbac.custom.create")}
      canSave={!!draft.name.trim()}
      saving={saving}
      onSave={onSave}
    >
      <div className="space-y-3.5">
        <Field label={t("pages.rbac.custom.fieldName")}>
          <Input
            value={draft.name}
            placeholder={t("pages.rbac.custom.namePlaceholder")}
            onChange={(e) => onChange({ ...draft, name: e.target.value })}
          />
        </Field>
        {/* the slug is the role's stable handle and the control plane refuses
            to change it, so it is offered on create and never on edit */}
        {!draft.id && (
          <Field
            label={t("pages.rbac.custom.fieldSlug")}
            hint={t("pages.rbac.custom.slugHint")}
          >
            <Input
              value={draft.slug}
              placeholder={t("pages.rbac.custom.slugPlaceholder")}
              onChange={(e) => onChange({ ...draft, slug: e.target.value })}
            />
          </Field>
        )}
        <Field label={t("pages.rbac.custom.fieldDescription")}>
          <Input
            value={draft.description}
            placeholder={t("pages.rbac.custom.descriptionPlaceholder")}
            onChange={(e) => onChange({ ...draft, description: e.target.value })}
          />
        </Field>
        <Field
          label={t("pages.rbac.custom.fieldBaseRole")}
          hint={t("pages.rbac.custom.baseRoleHint")}
        >
          <Select
            value={draft.base_role}
            onChange={(e) => onChange({ ...draft, base_role: e.target.value as Role })}
          >
            {BASE_ROLES.map((role) => (
              <option key={role} value={role}>
                {t(`pages.rbac.baseRoles.${role}`)}
              </option>
            ))}
          </Select>
        </Field>

        <GrantGrid
          resources={resources}
          grants={draft.grants}
          onToggle={(key) =>
            onChange({
              ...draft,
              grants: draft.grants.includes(key)
                ? draft.grants.filter((g) => g !== key)
                : [...draft.grants, key],
            })
          }
        />

        {draft.unknown.length > 0 && (
          <p className="rounded-[10px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3.5 py-2.5 text-xs text-muted-foreground">
            {t("pages.rbac.custom.unknownCarried", { count: draft.unknown.length })}
          </p>
        )}
      </div>
    </EditorSheet>
  );
}

/**
 * The resource × action grid a role's grants are ticked on.
 *
 * Driven by the matrix's own `resources`, never a list written out here: a cell
 * exists exactly when the deployment defines that pair. An action the resource
 * does not have has no checkbox at all, and a superadmin-only action has one
 * that is disabled — visible, because "this exists and you cannot grant it" is
 * a different answer from "this does not exist".
 */
function GrantGrid({
  resources,
  grants,
  onToggle,
}: {
  resources: RbacResourceView[];
  grants: string[];
  onToggle: (key: string) => void;
}) {
  const { t } = useTranslation();
  const [filter, setFilter] = React.useState("");

  const needle = filter.trim().toLowerCase();
  const matching = needle
    ? resources.filter((r) => r.resource.toLowerCase().includes(needle))
    : resources;

  return (
    <fieldset className="space-y-2">
      <legend className="text-sm font-medium leading-none">
        {t("pages.rbac.custom.grantsLabel")}
      </legend>
      <p className="text-xs text-muted-foreground">
        {t("pages.rbac.custom.grantsHint")}
      </p>
      <Input
        value={filter}
        placeholder={t("pages.rbac.custom.filterPlaceholder")}
        aria-label={t("pages.rbac.custom.filterPlaceholder")}
        onChange={(e) => setFilter(e.target.value)}
      />
      <div className="max-h-[420px] overflow-y-auto rounded-[10px] border border-[color:var(--border-subtle)]">
        {matching.length === 0 && (
          <p className="px-3.5 py-6 text-center text-xs text-muted-foreground">
            {t("pages.rbac.custom.noResources")}
          </p>
        )}
        {byScope(matching).map(([scopeKey, group]) => (
          <div key={scopeKey}>
            <div className="sticky top-0 flex items-center gap-3 border-b border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3.5 py-1.5">
              <span className="flex-1 text-[0.6875rem] uppercase tracking-[0.07em] text-[color:var(--text-subtle)]">
                {t(`pages.rbac.scopes.${scopeKey}`, { defaultValue: scopeKey })}
              </span>
              {ACTIONS.map((action) => (
                <span
                  key={action}
                  className="w-6 text-center font-mono text-[0.6875rem] text-[color:var(--text-subtle)]"
                >
                  {t(`pages.rbac.letters.${action}`)}
                </span>
              ))}
            </div>
            {group.map((resource) => (
              <div
                key={resource.resource}
                className="flex items-center gap-3 border-b border-[color:var(--border-subtle)] px-3.5 py-2 last:border-b-0"
              >
                <span className="flex-1 truncate font-mono text-xs text-[color:var(--text-secondary)]">
                  {resource.resource}
                </span>
                {ACTIONS.map((action) => {
                  const view = resource.actions.find((a) => a.action === action);
                  const key = pair(resource.resource, action);
                  const label = t("pages.rbac.custom.grantCell", {
                    action: t(`pages.rbac.actions.${action}`),
                    resource: resource.resource,
                  });
                  if (!view) {
                    return (
                      <span
                        key={action}
                        className="w-6 text-center font-mono text-xs text-[color:var(--text-subtle)] opacity-40"
                        title={t("pages.rbac.states.na")}
                      >
                        –
                      </span>
                    );
                  }
                  return (
                    <span key={action} className="flex w-6 justify-center">
                      <input
                        type="checkbox"
                        className="h-4 w-4 accent-[color:var(--red-folk)] disabled:cursor-not-allowed disabled:opacity-40"
                        aria-label={label}
                        title={
                          view.superadmin_only
                            ? t("pages.rbac.custom.superadminHint")
                            : label
                        }
                        disabled={view.superadmin_only}
                        checked={grants.includes(key)}
                        onChange={() => onToggle(key)}
                      />
                    </span>
                  );
                })}
              </div>
            ))}
          </div>
        ))}
      </div>
      <p className="text-xs text-muted-foreground">
        {t("pages.rbac.custom.superadminHint")}
      </p>
    </fieldset>
  );
}

// the matrix is one table, so the placeholder is one table: a header strip and
// a dozen rows, rather than a spinner that says nothing about what is coming
function MatrixSkeleton() {
  return (
    <PageBody>
      <div className="flex items-center gap-3">
        <Skeleton width={320} height={16} />
        <Skeleton width={140} height={22} radius={999} className="ml-auto" />
      </div>
      <div className="overflow-hidden rounded-[10px] border border-[color:var(--border-subtle)]">
        <div className="flex items-center gap-3 border-b border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-4 py-[13px]">
          <Skeleton width={90} height={12} />
          <Skeleton width={70} height={12} className="ml-auto" />
          <Skeleton width={70} height={12} />
          <Skeleton width={70} height={12} />
        </div>
        {Array.from({ length: 12 }, (_, i) => (
          <div
            key={i}
            className="flex items-center gap-3 border-b border-[color:var(--border-subtle)] px-4 py-[13px] last:border-b-0"
          >
            <Skeleton width={130} height={12} />
            <Skeleton width={96} height={16} className="ml-auto" />
            <Skeleton width={96} height={16} />
            <Skeleton width={96} height={16} />
          </div>
        ))}
      </div>
    </PageBody>
  );
}
