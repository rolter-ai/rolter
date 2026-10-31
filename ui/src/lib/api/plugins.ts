import { getJson, sendJson } from "./request";

export interface PluginInstanceRow {
  id: string;
  org_id: string;
  project_id: string | null;
  name: string;
  slug: string;
  description: string;
  kind: "webhook";
  stage: "pre_route" | "pre_upstream" | "post_response";
  enabled: boolean;
  position: number;
  failure_mode: "fail_open" | "fail_closed";
  endpoint: string;
  secret_env: string | null;
  config: Record<string, unknown>;
  created_at: string;
  updated_at: string;
}

export type PluginInstanceInput = Omit<
  PluginInstanceRow,
  "id" | "org_id" | "slug" | "created_at" | "updated_at"
> & { slug?: string };

export const fetchPlugins = (orgId: string) =>
  getJson<PluginInstanceRow[]>(`/api/v1/orgs/${orgId}/plugins`);

export const createPlugin = (orgId: string, body: PluginInstanceInput) =>
  sendJson<PluginInstanceRow>("POST", `/api/v1/orgs/${orgId}/plugins`, body);

export const updatePlugin = (id: string, body: PluginInstanceInput) =>
  sendJson<PluginInstanceRow>("PUT", `/api/v1/plugins/${id}`, body);

export const deletePlugin = (id: string) => sendJson<void>("DELETE", `/api/v1/plugins/${id}`);

// ---------------------------------------------------------------------------
// deployment-wide guardrail registry
