import { getJson, sendJson } from "./request";

export interface VirtualKeyRow {
  id: string;
  project_id: string;
  key_hash: string;
  key_prefix: string;
  name?: string | null;
  models: string[];
  /// empty means the key may reach every provider on an allowed route
  providers: string[];
  disabled: boolean;
  expires_at?: string | null;
  /// per-key response-cache override; null inherits the route decision
  cache_enabled?: boolean | null;
  /// local account that minted the key from the self-service panel; null for
  /// admin-created and bootstrap-config keys
  created_by: string | null;
  /// business unit this key's spend rolls up to; null leaves the key
  /// attributed to its tenancy chain only
  business_unit_id: string | null;
  /// customer this key's spend rolls up to; null when unattributed
  customer_id: string | null;
  /// what minted the key, when it was not a person filling in the mint form:
  /// `playground` for one the Playground issued itself. null for every key an
  /// operator created on purpose
  purpose?: string | null;
  created_at: string;
}

// returned only from createVirtualKey — carries the plaintext secret, shown
// once and never persisted beyond the create mutation's immediate result
export interface CreatedVirtualKey extends VirtualKeyRow {
  key: string;
}

export interface CreateVirtualKeyInput {
  /** required — see MintKeyInput; the same rule applies on the admin path */
  name: string;
  models?: string[];
  /** upstream provider allow-list, by slug; empty permits every provider */
  providers?: string[];
  cache?: boolean | null;
  /** key lifetime in days; omitted means "never expires" */
  expires_in_days?: number;
}

export function fetchVirtualKeys(projectId: string): Promise<VirtualKeyRow[]> {
  return getJson<VirtualKeyRow[]>(`/api/v1/projects/${projectId}/virtual-keys`);
}

export function createVirtualKey(
  projectId: string,
  input: CreateVirtualKeyInput,
): Promise<CreatedVirtualKey> {
  return sendJson<CreatedVirtualKey>("POST", `/api/v1/projects/${projectId}/virtual-keys`, input);
}

export function setVirtualKeyDisabled(id: string, disabled: boolean): Promise<VirtualKeyRow> {
  return sendJson<VirtualKeyRow>("PUT", `/api/v1/virtual-keys/${id}`, {
    disabled,
  });
}

export function setVirtualKeyCache(id: string, cache: boolean | null): Promise<VirtualKeyRow> {
  return sendJson<VirtualKeyRow>("PUT", `/api/v1/virtual-keys/${id}/cache`, {
    cache,
  });
}

/**
 * Narrow a key to a set of upstream providers, by slug. An empty list restores
 * the permissive default rather than locking the key out of everything.
 */
export function setVirtualKeyProviders(id: string, providers: string[]): Promise<VirtualKeyRow> {
  return sendJson<VirtualKeyRow>("PUT", `/api/v1/virtual-keys/${id}/providers`, { providers });
}

/**
 * Point a key's spend at a business unit and/or a customer.
 *
 * Both fields follow the server's three-state contract: omit to leave
 * unchanged, `null` to clear, a uuid to set. The editor always sends both, so
 * clearing one dimension is a thing the operator can actually do — omitting a
 * field they emptied would silently keep the old attribution.
 */
export function setVirtualKeyAttribution(
  id: string,
  attribution: {
    business_unit_id?: string | null;
    customer_id?: string | null;
  },
): Promise<VirtualKeyRow> {
  return sendJson<VirtualKeyRow>("PUT", `/api/v1/virtual-keys/${id}/attribution`, attribution);
}

export function deleteVirtualKey(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/virtual-keys/${id}`);
}

// --- prompt template repository (crates/rolter-control/src/crud.rs) ---
