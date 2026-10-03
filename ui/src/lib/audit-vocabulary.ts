import vocabulary from "./audit-vocabulary.json";

// the actions and target types the control plane writes to the audit log,
// generated from its source by `bun run gen:audit` so the Audit Log filters
// cannot drift from what is recorded (#2127)
export const AUDIT_ACTIONS: readonly string[] = vocabulary.actions;
export const AUDIT_TARGET_TYPES: readonly string[] = vocabulary.targets;

export const AUDIT_GROUPS = [
  "identity",
  "tenancy",
  "keys",
  "routing",
  "guardrails",
  "mcp",
  "alerts",
  "content",
  "settings",
  "account",
  "other",
] as const;
export type AuditGroup = (typeof AUDIT_GROUPS)[number];

// the area of the product an action belongs to, by the resource it names
const GROUP_OF_FAMILY: Record<string, AuditGroup> = {
  sso_provider: "identity",
  sso_group_mapping: "identity",
  scim: "identity",
  scim_token: "identity",
  scim_group_mapping: "identity",
  invitation: "identity",
  custom_role: "identity",
  access_profile: "identity",
  access_profile_assignment: "identity",
  org_auth_policy: "identity",
  user: "identity",
  membership: "identity",
  org: "tenancy",
  team: "tenancy",
  project: "tenancy",
  business_unit: "tenancy",
  customer: "tenancy",
  virtual_key: "keys",
  budget: "keys",
  rate_limit: "keys",
  provider: "routing",
  provider_group: "routing",
  route: "routing",
  route_target: "routing",
  model_label: "routing",
  model_defaults: "routing",
  adaptive_routing_policy: "routing",
  guardrail_provider: "guardrails",
  guardrail_rule: "guardrails",
  mcp_server: "mcp",
  mcp_tool_group: "mcp",
  mcp_settings: "mcp",
  mcp_oauth_client: "mcp",
  mcp_oauth_grant: "mcp",
  mcp_oauth_session: "mcp",
  connector: "mcp",
  alert: "alerts",
  prompt_template: "content",
  skill: "content",
  plugin: "content",
  label: "content",
  security: "settings",
  client_settings: "settings",
  logging_settings: "settings",
  compatibility_policy: "settings",
  feature_flags: "settings",
  runtime_policy: "settings",
  cluster_node: "settings",
  auth: "account",
};

/** the filter group an audited action sits under */
export function auditGroup(action: string): AuditGroup {
  return GROUP_OF_FAMILY[action.split(".")[0]!] ?? "other";
}

/** the actions in filter order: by group, then by name, so each group is one run */
export function groupedActions(): string[] {
  const rank = (action: string) => AUDIT_GROUPS.indexOf(auditGroup(action));
  return [...AUDIT_ACTIONS].sort((a, b) => rank(a) - rank(b) || a.localeCompare(b));
}
