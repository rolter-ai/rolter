import { getJson, sendJson } from "./request";

export interface AccessProfileRow {
  id: string;
  org_id: string;
  slug: string;
  name: string;
  description: string | null;
  created_at: string;
  updated_at: string;
}

export interface AccessProfilePolicy {
  profile_id: string;
  allowed_models: string[];
  denied_models: string[];
  allowed_routes: string[];
  denied_routes: string[];
  updated_at: string;
}

export interface AccessProfileAssignmentRow {
  id: string;
  profile_id: string;
  user_id: string | null;
  team_id: string | null;
  created_at: string;
}

/**
 * One `(custom role, scope)` pair inside a profile.
 *
 * Scope follows the same most-specific-non-null convention as a membership: a
 * project pins the role to that project, a team to every project under it, and
 * an org to the whole tenant.
 */
export interface AccessProfileRoleRow {
  id: string;
  profile_id: string;
  role_id: string;
  org_id: string | null;
  team_id: string | null;
  project_id: string | null;
  created_at: string;
}

/** the profile plus everything hanging off it, `#[serde(flatten)]`ed as above */
export interface AccessProfileDetail extends AccessProfileRow {
  roles: AccessProfileRoleRow[];
  assignments: AccessProfileAssignmentRow[];
  /** null until a policy has been written for the profile */
  policy: AccessProfilePolicy | null;
}

/** a composition as it is written: a role, and the scope it applies at */
export interface AccessProfileRoleInput {
  role_id: string;
  org_id?: string;
  team_id?: string;
  project_id?: string;
}

export interface AccessProfilePolicyInput {
  allowed_models: string[];
  denied_models: string[];
  allowed_routes: string[];
  denied_routes: string[];
}

export type AccessProfileInclude = "roles" | "assignments" | "policy";

export function fetchAccessProfiles(orgId: string): Promise<AccessProfileRow[]>;
/**
 * `include` embeds what the detail endpoint returns, so a screen that needs each
 * profile's composition reads it in one request however many profiles there are
 * (#1996).
 */
export function fetchAccessProfiles(
  orgId: string,
  include: AccessProfileInclude[],
): Promise<AccessProfileDetail[]>;
export function fetchAccessProfiles(
  orgId: string,
  include?: AccessProfileInclude[],
): Promise<AccessProfileRow[] | AccessProfileDetail[]> {
  const query = include?.length ? `?include=${include.join(",")}` : "";
  return getJson<AccessProfileRow[]>(`/api/v1/orgs/${orgId}/access-profiles${query}`);
}

// the one call that answers "what does this profile actually carry": the roles
// composed into it, everyone it reaches, and the policy `setAccessProfilePolicy`
// wrote — none of which the list endpoint can show back
export function fetchAccessProfile(id: string): Promise<AccessProfileDetail> {
  return getJson<AccessProfileDetail>(`/api/v1/access-profiles/${id}`);
}

// a profile is created whole: the roles it composes and the policy it carries go
// in the same request, so it is never assignable in a half-written state
export function createAccessProfile(
  orgId: string,
  input: {
    name: string;
    slug?: string;
    description?: string | null;
    roles?: AccessProfileRoleInput[];
    policy?: AccessProfilePolicyInput;
  },
): Promise<AccessProfileDetail> {
  return sendJson<AccessProfileDetail>("POST", `/api/v1/orgs/${orgId}/access-profiles`, input);
}

// `roles` absent leaves the composition alone, present replaces it wholesale —
// which is also how a role is detached before it can be deleted
export function updateAccessProfile(
  id: string,
  input: {
    name?: string;
    description?: string | null;
    roles?: AccessProfileRoleInput[];
    policy?: AccessProfilePolicyInput;
  },
): Promise<AccessProfileDetail> {
  return sendJson<AccessProfileDetail>("PUT", `/api/v1/access-profiles/${id}`, input);
}

export function deleteAccessProfile(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/access-profiles/${id}`);
}

export function fetchAccessProfileAssignments(id: string): Promise<AccessProfileAssignmentRow[]> {
  return getJson<AccessProfileAssignmentRow[]>(`/api/v1/access-profiles/${id}/assignments`);
}

// every assignment of the org's profiles in one read, narrowed to one user or
// team when given. The Users and Teams screens read it instead of one detail
// per profile (#3025)
export function fetchOrgAccessProfileAssignments(
  orgId: string,
  subject?: { user_id: string } | { team_id: string },
): Promise<AccessProfileAssignmentRow[]> {
  const query = subject ? `?${new URLSearchParams(subject).toString()}` : "";
  return getJson<AccessProfileAssignmentRow[]>(
    `/api/v1/orgs/${orgId}/access-profile-assignments${query}`,
  );
}

// exactly one of user_id / team_id is set; a team assignment reaches every
// member, which is the point of a profile over a direct grant
export function createAccessProfileAssignment(
  id: string,
  input: { user_id?: string; team_id?: string },
): Promise<AccessProfileAssignmentRow> {
  return sendJson<AccessProfileAssignmentRow>(
    "POST",
    `/api/v1/access-profiles/${id}/assignments`,
    input,
  );
}

export function deleteAccessProfileAssignment(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/access-profile-assignments/${id}`);
}

// an empty allow-list means "no restriction"; deny beats allow. entries are
// exact names or a trailing-* prefix glob
export function setAccessProfilePolicy(
  id: string,
  policy: {
    allowed_models: string[];
    denied_models: string[];
    allowed_routes: string[];
    denied_routes: string[];
  },
): Promise<AccessProfilePolicy> {
  return sendJson<AccessProfilePolicy>("PUT", `/api/v1/access-profiles/${id}/policy`, policy);
}
