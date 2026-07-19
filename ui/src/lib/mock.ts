// design-prototype mock data for screens whose backend DTOs don't exist yet
// (MCP catalog, security settings, RBAC matrix, feature flags). each consumer
// labels itself as preview data in the UI; replace with real fetchers as the
// control API grows.

export interface MockMcpServer {
  id: string;
  name: string;
  transport: "stdio" | "http" | "sse";
  status: "ok" | "degraded" | "down";
  enabled: boolean;
  calls24h: number;
  tools: string[];
}

export const MCP_SERVERS: MockMcpServer[] = [
  { id: "mcp_github", name: "github", transport: "stdio", status: "ok", enabled: true, calls24h: 4120, tools: ["search_code", "create_issue", "get_pr", "list_repos"] },
  { id: "mcp_postgres", name: "postgres", transport: "http", status: "ok", enabled: true, calls24h: 9800, tools: ["query", "list_tables", "describe"] },
  { id: "mcp_filesystem", name: "filesystem", transport: "stdio", status: "degraded", enabled: true, calls24h: 2100, tools: ["read_file", "write_file", "list_dir"] },
  { id: "mcp_slack", name: "slack", transport: "http", status: "ok", enabled: true, calls24h: 640, tools: ["post_message", "list_channels"] },
  { id: "mcp_browser", name: "browser", transport: "sse", status: "down", enabled: false, calls24h: 0, tools: ["navigate", "screenshot", "click"] },
  { id: "mcp_sentry", name: "sentry", transport: "http", status: "ok", enabled: true, calls24h: 310, tools: ["list_issues", "get_event"] },
];

export interface RbacResource {
  key: string;
  label: string;
}

export const RBAC_RESOURCES: RbacResource[] = [
  { key: "virtual_keys", label: "Virtual Keys" },
  { key: "providers", label: "Providers" },
  { key: "budgets", label: "Budgets & Limits" },
  { key: "teams", label: "Teams & Customers" },
  { key: "logs", label: "Logs & Analytics" },
  { key: "settings", label: "Settings" },
  { key: "rbac", label: "Roles & Permissions" },
];

export interface RbacRole {
  key: string;
  label: string;
  members: number;
  desc: string;
  // per-resource op string out of "vcud" (view/create/update/delete)
  caps: Record<string, string>;
}

export const RBAC_ROLES: RbacRole[] = [
  {
    key: "admin",
    label: "Admin",
    members: 1,
    desc: "Manage gateway config, keys, and governance across the org.",
    caps: { virtual_keys: "vcud", providers: "vcud", budgets: "vcud", teams: "vcud", logs: "vc", settings: "vu", rbac: "vu" },
  },
  {
    key: "member",
    label: "Member",
    members: 0,
    desc: "Use the gateway and manage their own virtual keys.",
    caps: { virtual_keys: "vcu", providers: "v", budgets: "v", teams: "v", logs: "v", settings: "", rbac: "" },
  },
  {
    key: "viewer",
    label: "Viewer",
    members: 0,
    desc: "Read-only access to dashboards, logs, and config.",
    caps: { virtual_keys: "v", providers: "v", budgets: "v", teams: "v", logs: "v", settings: "", rbac: "" },
  },
];

export interface FeatureFlag {
  key: string;
  label: string;
  desc: string;
  on: boolean;
}

export const FEATURE_FLAGS: FeatureFlag[] = [
  { key: "cache", label: "Response cache", desc: "Direct + semantic response caching", on: true },
  { key: "cache_aware", label: "Cache-aware routing", desc: "Prefer targets with a warm KV cache", on: true },
  { key: "circuit", label: "Circuit breaker", desc: "Fail over when an endpoint degrades", on: true },
  { key: "health", label: "Active health checks", desc: "Probe upstreams on an interval", on: true },
  { key: "complexity", label: "Complexity router", desc: "Route by prompt complexity tier", on: false },
  { key: "guardrails", label: "Guardrails", desc: "Input/output safety checks", on: false },
];
