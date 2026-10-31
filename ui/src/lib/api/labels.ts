import { getJson, sendJson } from "./request";

/**
 * A label on a provider, provider group, route or model (#985).
 *
 * `source` is the whole of what a reader has to keep straight: a `custom`
 * label is a statement somebody made, and an `auto` one is an observation a
 * subsystem recorded at `observed_at` and can withdraw by observing again. The
 * two may carry the same key on the same subject deliberately, so nothing may
 * present them as duplicates of each other.
 */
export interface LabelRow {
  id: string;
  subject_type: "provider" | "provider_group" | "route" | "model";
  subject_id: string;
  /**
   * the organization whose model a model label is on (#3476); `null` for a
   * deployment-wide label and for every provider, group or route label.
   * Absent from a control plane that predates it, where every label is
   * deployment-wide
   */
  org_id?: string | null;
  key: string;
  value?: string | null;
  source: "auto" | "custom";
  /** when the observation was made; `auto` only */
  observed_at?: string | null;
  /** what was observed, in the subsystem's own words; `auto` only */
  observation?: string | null;
  created_at: string;
  updated_at: string;
}

export interface LabelFilter {
  subject_type?: string;
  subject_id?: string;
  key?: string;
  value?: string;
  source?: "auto" | "custom";
  /** model labels only: what applies to this organization's models (#3476) */
  org_id?: string;
}

function labelQuery(filter: LabelFilter): string {
  const params = new URLSearchParams();
  for (const [k, v] of Object.entries(filter)) if (v) params.set(k, v);
  const q = params.toString();
  return q ? `?${q}` : "";
}

export function fetchLabels(orgId: string, filter: LabelFilter = {}): Promise<LabelRow[]> {
  return getJson<LabelRow[]>(`/api/v1/orgs/${orgId}/labels${labelQuery(filter)}`);
}

export function createLabel(
  orgId: string,
  input: { subject_type: string; subject_id: string; key: string; value?: string },
): Promise<LabelRow> {
  return sendJson<LabelRow>("POST", `/api/v1/orgs/${orgId}/labels`, input);
}

/** only a value can change: a key is the identity the 409 is about */
export function updateLabel(orgId: string, id: string, value?: string): Promise<LabelRow> {
  return sendJson<LabelRow>("PUT", `/api/v1/orgs/${orgId}/labels/${id}`, { value });
}

export function deleteLabel(orgId: string, id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/orgs/${orgId}/labels/${id}`);
}

/**
 * The same four calls for models, which live in the deployment-wide pricing
 * catalog rather than in an org — so they are a superadmin's to write, and the
 * subject is the model's own name rather than a row id. A label may still name
 * the organization whose model it is (`org_id`, #3476), since two
 * organizations may each serve a model of one name.
 */
export function fetchModelLabels(filter: LabelFilter = {}): Promise<LabelRow[]> {
  return getJson<LabelRow[]>(`/api/v1/model-labels${labelQuery(filter)}`);
}

export function createModelLabel(input: {
  model: string;
  /** the organization whose model this is; omitted for a deployment-wide label */
  org_id?: string;
  key: string;
  value?: string;
}): Promise<LabelRow> {
  return sendJson<LabelRow>("POST", "/api/v1/model-labels", input);
}

export function updateModelLabel(id: string, value?: string): Promise<LabelRow> {
  return sendJson<LabelRow>("PUT", `/api/v1/model-labels/${id}`, { value });
}

export function deleteModelLabel(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/model-labels/${id}`);
}
