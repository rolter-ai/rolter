import { getJson, sendJson } from "./request";

/**
 * A registered LDAP directory, as `LdapDirectory` serialises it
 * (crates/rolter-control/src/ldap_signin.rs, #1826).
 *
 * The sealed bind password never leaves the control plane; `has_bind_password`
 * takes its place so the card can tell a directory that binds as a service
 * account from one that searches anonymously.
 */
export interface LdapDirectoryRow {
  id: string;
  org_id: string;
  name: string;
  /** `ldaps://` or `ldap://` address of the directory */
  url: string;
  /** the read-only service account that finds users */
  bind_dn: string;
  has_bind_password: boolean;
  base_dn: string;
  /** search filter; `{login}` is replaced with what the user typed */
  user_filter: string;
  email_attr: string;
  display_name_attr: string;
  group_attr: string;
  /** refuse a plain `ldap://` url so the bind password is never sent in cleartext */
  require_tls: boolean;
  /** role for a user in no mapped group; null refuses that user */
  default_role: string | null;
  enabled: boolean;
  created_at: string;
}

/**
 * The editable fields of a directory. Omitted attributes take the server's
 * defaults (`uid={login}`, `mail`, `cn`, `memberOf`, TLS required).
 *
 * `bind_password` is three-valued on update: omit it to keep the sealed one,
 * send a value to rotate it, send `""` to clear it (an anonymous search).
 */
export interface LdapDirectoryInput {
  name: string;
  url: string;
  bind_dn: string;
  bind_password?: string;
  base_dn: string;
  user_filter?: string;
  email_attr?: string;
  display_name_attr?: string;
  group_attr?: string;
  require_tls?: boolean;
  default_role?: string;
  enabled?: boolean;
}

export function fetchLdapDirectories(orgId: string): Promise<LdapDirectoryRow[]> {
  return getJson<LdapDirectoryRow[]>(`/api/v1/orgs/${orgId}/ldap-directories`);
}

export function createLdapDirectory(
  orgId: string,
  input: LdapDirectoryInput,
): Promise<LdapDirectoryRow> {
  return sendJson<LdapDirectoryRow>("POST", `/api/v1/orgs/${orgId}/ldap-directories`, input);
}

export function updateLdapDirectory(
  id: string,
  input: LdapDirectoryInput,
): Promise<LdapDirectoryRow> {
  return sendJson<LdapDirectoryRow>("PUT", `/api/v1/ldap-directories/${id}`, input);
}

export function deleteLdapDirectory(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/ldap-directories/${id}`);
}

/** a directory group name granting a role at a scope inside the directory's own org */
export interface LdapGroupMappingRow {
  id: string;
  directory_id: string;
  group_name: string;
  org_id: string | null;
  team_id: string | null;
  project_id: string | null;
  role: string;
  created_at: string;
}

export interface CreateLdapGroupMappingInput {
  group_name: string;
  role: string;
  org_id?: string;
  team_id?: string;
  project_id?: string;
}

export function fetchLdapGroupMappings(directoryId: string): Promise<LdapGroupMappingRow[]> {
  return getJson<LdapGroupMappingRow[]>(`/api/v1/ldap-directories/${directoryId}/group-mappings`);
}

export function createLdapGroupMapping(
  directoryId: string,
  input: CreateLdapGroupMappingInput,
): Promise<LdapGroupMappingRow> {
  return sendJson<LdapGroupMappingRow>(
    "POST",
    `/api/v1/ldap-directories/${directoryId}/group-mappings`,
    input,
  );
}

export function deleteLdapGroupMapping(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/ldap-group-mappings/${id}`);
}
