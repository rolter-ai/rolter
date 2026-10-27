// control-plane CRUD, only mounted when rolter-control runs with --database-url
// (crates/rolter-control/src/crud/)
import { getJson, sendJson } from "./request";

export interface OrgRow {
  id: string;
  name: string;
  slug: string;
  created_at: string;
}

export interface TeamRow {
  id: string;
  org_id: string;
  name: string;
  created_at: string;
}

export interface ProjectRow {
  id: string;
  team_id: string;
  name: string;
  created_at: string;
}

export function fetchOrgs(): Promise<OrgRow[]> {
  return getJson<OrgRow[]>("/api/v1/orgs");
}

export function fetchTeams(orgId: string): Promise<TeamRow[]> {
  return getJson<TeamRow[]>(`/api/v1/orgs/${orgId}/teams`);
}

/**
 * A project listed org-wide, carrying the name of the team that owns it.
 *
 * Two teams may each own a "prod", so a control that names a project anywhere
 * in the org groups by team — and the team's name rides along with the row so
 * that grouping costs no extra request.
 */
export interface OrgProjectRow extends ProjectRow {
  team_name: string;
}

export function fetchProjects(teamId: string): Promise<ProjectRow[]> {
  return getJson<ProjectRow[]>(`/api/v1/teams/${teamId}/projects`);
}

/**
 * Every project in the org, across all of its teams, in one request.
 *
 * The per-team `fetchProjects` is what a caller wants when it already knows the
 * team. Anything that has to name a project *anywhere* in the org would
 * otherwise pay one request per team (#1357).
 */
export function fetchOrgProjects(orgId: string): Promise<OrgProjectRow[]> {
  return getJson<OrgProjectRow[]>(`/api/v1/orgs/${orgId}/projects`);
}

export function createOrg(input: { name: string; slug: string }): Promise<OrgRow> {
  return sendJson<OrgRow>("POST", "/api/v1/orgs", input);
}

export function deleteOrg(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/orgs/${id}`);
}

export function createTeam(orgId: string, input: { name: string }): Promise<TeamRow> {
  return sendJson<TeamRow>("POST", `/api/v1/orgs/${orgId}/teams`, input);
}

/** Renames a team; a name another team of the org holds answers `409 name_taken` (#3126). */
export function renameTeam(id: string, input: { name: string }): Promise<TeamRow> {
  return sendJson<TeamRow>("PUT", `/api/v1/teams/${id}`, input);
}

export function deleteTeam(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/teams/${id}`);
}

// ---------------------------------------------------------------------------
// cost attribution: spend reaches a business unit or customer through the virtual keys assigned to it;
// customers can also roll up into a unit. both are retired rather than deleted once they have
// history, so `retired_at` is part of the row, not a separate lookup

export interface BusinessUnitRow {
  id: string;
  org_id: string;
  name: string;
  slug: string;
  retired_at: string | null;
  created_at: string;
  /**
   * Virtual keys attributed to this unit that are live: not disabled and not
   * past their expiry (#2581). Only the org-wide listing carries it; a create
   * or update answer, or a control plane older than the field, leaves it out,
   * so absent means "not known" rather than zero.
   */
  live_key_count?: number;
}

export interface CustomerRow {
  id: string;
  org_id: string;
  business_unit_id: string | null;
  name: string;
  slug: string;
  retired_at: string | null;
  created_at: string;
  /** live virtual keys attributed to this customer; see `BusinessUnitRow` */
  live_key_count?: number;
}

// ---------------------------------------------------------------------------
// org-defined roles (#534, crates/rolter-control/src/access_control.rs): a base
// role plus the explicit `(resource, action)` pairs it also allows. A grant only
// ever widens what the base role already permitted, and none can reach a
// superadmin capability — deployment-wide policy has no org to define a role in,
// so granting it from inside a tenant would be a way out of that tenant.

export function fetchBusinessUnits(orgId: string): Promise<BusinessUnitRow[]> {
  return getJson<BusinessUnitRow[]>(`/api/v1/orgs/${orgId}/business-units`);
}

export function createBusinessUnit(
  orgId: string,
  input: { name: string; slug?: string },
): Promise<BusinessUnitRow> {
  return sendJson<BusinessUnitRow>("POST", `/api/v1/orgs/${orgId}/business-units`, input);
}

// a slug change breaks whatever already attributes spend by it, so the server
// demands allow_slug_change before it will move one
export function updateBusinessUnit(
  id: string,
  input: {
    name?: string;
    slug?: string;
    allow_slug_change?: boolean;
    retired?: boolean;
  },
): Promise<BusinessUnitRow> {
  return sendJson<BusinessUnitRow>("PUT", `/api/v1/business-units/${id}`, input);
}

export function deleteBusinessUnit(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/business-units/${id}`);
}

export function fetchCustomers(orgId: string): Promise<CustomerRow[]> {
  return getJson<CustomerRow[]>(`/api/v1/orgs/${orgId}/customers`);
}

export function createCustomer(
  orgId: string,
  input: { name: string; slug?: string; business_unit_id?: string | null },
): Promise<CustomerRow> {
  return sendJson<CustomerRow>("POST", `/api/v1/orgs/${orgId}/customers`, input);
}

export function updateCustomer(
  id: string,
  // business_unit_id follows the server's three-state contract: omit to leave
  // unchanged, null to unassign, an id to move it
  input: {
    name?: string;
    slug?: string;
    allow_slug_change?: boolean;
    business_unit_id?: string | null;
    retired?: boolean;
  },
): Promise<CustomerRow> {
  return sendJson<CustomerRow>("PUT", `/api/v1/customers/${id}`, input);
}

export function deleteCustomer(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/customers/${id}`);
}

export function createProject(teamId: string, input: { name: string }): Promise<ProjectRow> {
  return sendJson<ProjectRow>("POST", `/api/v1/teams/${teamId}/projects`, input);
}

export function deleteProject(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/projects/${id}`);
}

/// who may read the request and response bodies payload capture stored for a
/// project's traffic: members (the default) or viewers too. an admin always may
export type PayloadMinRole = "member" | "viewer";

/// a project's own settings (#1820)
export interface ProjectSettings {
  payload_min_role: PayloadMinRole;
}

export function fetchProjectSettings(id: string): Promise<ProjectSettings> {
  return getJson<ProjectSettings>(`/api/v1/projects/${id}/settings`);
}

export function updateProjectSettings(
  id: string,
  input: ProjectSettings,
): Promise<ProjectSettings> {
  return sendJson<ProjectSettings>("PUT", `/api/v1/projects/${id}/settings`, input);
}
