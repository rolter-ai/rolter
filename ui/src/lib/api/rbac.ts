import { getJson, sendJson } from "./request";
import { type Role } from "./users";

export function fetchRoles(): Promise<string[]> {
  return getJson<string[]>("/api/v1/roles");
}

// provider stability rollups over provider_health_events (ROL-198)

export interface CustomRoleRow {
  id: string;
  org_id: string;
  slug: string;
  name: string;
  description: string | null;
  /** the built-in role this one extends: `admin` | `member` | `viewer` */
  base_role: Role;
  created_at: string;
  updated_at: string;
}

/** one stored `(resource, action)` pair a custom role grants */
export interface CustomRoleGrantRow {
  id: string;
  role_id: string;
  resource: string;
  action: RbacAction;
}

/**
 * What every custom-role read and write returns.
 *
 * The role is `#[serde(flatten)]`ed on the rust side, so its own fields sit
 * beside `grants` rather than under a `role` key.
 */
export interface CustomRoleDetail extends CustomRoleRow {
  grants: CustomRoleGrantRow[];
}

/** a grant as it is written; the server assigns the id */
export interface CustomRoleGrantInput {
  resource: string;
  action: RbacAction;
}

export function fetchCustomRoles(orgId: string): Promise<CustomRoleRow[]> {
  return getJson<CustomRoleRow[]>(`/api/v1/orgs/${orgId}/custom-roles`);
}

export function createCustomRole(
  orgId: string,
  input: {
    name: string;
    slug?: string;
    description?: string | null;
    /** omitted defaults to `viewer` server-side */
    base_role?: Role;
    grants?: CustomRoleGrantInput[];
  },
): Promise<CustomRoleDetail> {
  return sendJson<CustomRoleDetail>("POST", `/api/v1/orgs/${orgId}/custom-roles`, input);
}

// `grants` absent leaves the stored set alone; present replaces it wholesale, so
// an editor that sends a grid has to send every pair it means to keep — including
// the ones this build's capability table no longer defines
export function updateCustomRole(
  id: string,
  input: {
    name?: string;
    description?: string | null;
    base_role?: Role;
    grants?: CustomRoleGrantInput[];
  },
): Promise<CustomRoleDetail> {
  return sendJson<CustomRoleDetail>("PUT", `/api/v1/custom-roles/${id}`, input);
}

// 409 for as long as any access profile still composes the role: detaching is an
// audited profile update, so nobody's access changes without a record that names
// the profile it changed
export function deleteCustomRole(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/custom-roles/${id}`);
}

// ---------------------------------------------------------------------------
// access profiles (#534 / #830): reusable permission bundles handed to users or
// whole teams. the merged model/route policy they carry is enforced on the
// gateway, not just reported — see ADR-0023

export type RbacAction = "read" | "create" | "update" | "delete";

/**
 * What one action on one resource takes.
 *
 * An action the resource does not have at all is *absent* from `actions`
 * rather than present with a null authority — the backend `filter_map`s it
 * away — so a UI must render a missing entry as "not applicable" and never as
 * "denied".
 */
/** a delegable deployment role (#1834): held on the account, granted by a superadmin only */
export type DeploymentRole = "operator" | "security_auditor";

export interface RbacActionView {
  action: RbacAction;
  /** minimum scoped role; null when the action is superadmin- or auth-only */
  minimum_role: Role | null;
  superadmin_only: boolean;
  /** the deployment roles that also reach this action besides a superadmin; absent for the rest */
  deployment_roles?: DeploymentRole[];
  /** no membership required: any authenticated caller may perform it */
  authenticated_only: boolean;
  /** the scope this action is decided at when it differs from the resource's; absent otherwise */
  scope?: string;
}

export interface RbacResourceView {
  resource: string;
  /** `deployment` | `org` | `team` | `project` — where the resource lives */
  scope: string;
  actions: RbacActionView[];
}

export interface RbacRoleView {
  role: Role;
  /** total order over roles: viewer 0 < member 1 < admin 2 */
  rank: number;
}

/** one `resource:action` pair an org-defined role grants explicitly */
export interface RbacCustomGrant {
  resource: string;
  action: string;
}

export interface RbacCustomRoleView {
  id: string;
  slug: string;
  name: string;
  description: string | null;
  /** the built-in role this custom role is at least equivalent to */
  base_role: Role;
  base_rank: number;
  /** the pairs it grants on top of `base_role` */
  grants: RbacCustomGrant[];
  /** pairs this build's capability table does not define; they grant nothing */
  unknown_grants: RbacCustomGrant[];
}

export interface RbacMatrix {
  roles: RbacRoleView[];
  /** the delegable deployment roles, which no scope holds (#1834) */
  deployment_roles?: DeploymentRole[];
  resources: RbacResourceView[];
  /** org-defined roles; empty when no org was asked for, or none are defined */
  custom_roles: RbacCustomRoleView[];
}

// any authenticated principal may read it: it describes the rules, not anyone's
// access. `orgId` adds that org's custom roles and needs a membership there
export function fetchRbacMatrix(orgId?: string): Promise<RbacMatrix> {
  const query = orgId ? `?org_id=${encodeURIComponent(orgId)}` : "";
  return getJson<RbacMatrix>(`/api/v1/rbac/matrix${query}`);
}

/** A custom role the caller holds at the requested scope. */
export interface RbacHeldRole {
  profile_id: string;
  role_id: string;
  role_slug: string;
  base_role: Role;
}

/** The model/route visibility a caller's access profiles impose. */
export interface RbacModelPolicy {
  allowed_models?: string[];
  denied_models?: string[];
  allowed_routes?: string[];
  denied_routes?: string[];
}

/**
 * What *this* caller may do at one org/team/project chain.
 *
 * The counterpart to [`RbacMatrix`], which describes the rules for everyone:
 * this is the caller's own answer, evaluated server-side from their
 * memberships and access profiles. The dashboard disables controls with it
 * (#1183); enforcement stays on the control plane, so the answer is advisory
 * here and authoritative only there.
 */
export interface RbacEffective {
  /** the admin token, a superadmin user, or any caller in open mode */
  superadmin: boolean;
  /** the caller's deployment role (#1834); null for a superadmin and for anyone holding none */
  deployment_role?: DeploymentRole | null;
  /** the resolved role at the chain, null when no membership reaches it */
  role: Role | null;
  /** the `resource:action` pairs the caller may perform, default-deny */
  allowed: string[];
  /** the custom roles behind any pair `role` alone does not explain */
  custom_roles: RbacHeldRole[];
  /** absent when no access profile restricts anything, which means "all" */
  model_policy: RbacModelPolicy | null;
}

/** The org/team/project chain a capability question is asked at. */
export interface RbacScope {
  orgId?: string;
  teamId?: string;
  projectId?: string;
}

// every part of the chain is optional: the deployment-scoped resources need no
// chain at all, and a caller who is not in an org yet still gets an answer —
// an empty one, which is exactly what `authorize` would decide
export function fetchEffective(scope: RbacScope = {}): Promise<RbacEffective> {
  const params = new URLSearchParams();
  if (scope.orgId) params.set("org_id", scope.orgId);
  if (scope.teamId) params.set("team_id", scope.teamId);
  if (scope.projectId) params.set("project_id", scope.projectId);
  const query = params.size ? `?${params}` : "";
  return getJson<RbacEffective>(`/api/v1/rbac/effective${query}`);
}

// --- scim provisioning tokens (crates/rolter-control/src/scim.rs, #540) ---
//
// the dashboard only manages the tokens an IdP authenticates with; the SCIM
// resource endpoints (/scim/v2/Users) are driven by the IdP itself and are
// deliberately not called from here. every one of these requires Admin on the
// org (rolter_auth::Role::Admin).
