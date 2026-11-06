import {
  ArrowLeftRight,
  BookOpen,
  BookUser,
  Boxes,
  Building,
  Building2,
  Cable,
  ChartColumn,
  CircuitBoard,
  Database,
  Eraser,
  Eye,
  FileText,
  Flag,
  Bot,
  Fingerprint,
  FolderTree,
  FolderGit2,
  Gavel,
  Grid2x2,
  History,
  KeyRound,
  Landmark,
  ListChecks,
  Layers,
  LayoutGrid,
  Megaphone,
  Network,
  Play,
  Plug,
  Puzzle,
  ScrollText,
  Settings,
  Shield,
  ShieldCheck,
  Shuffle,
  SlidersHorizontal,
  Split,
  TrendingUp,
  UserCheck,
  UserCog,
  Users,
  Scale,
  Wallet,
  WalletCards,
  Waypoints,
} from "lucide-react";
import type * as React from "react";
import { useTranslation } from "react-i18next";

// the control-plane IA. key doubles as the route path segment: /<key>.
export interface NavDef {
  /* doubles as the route path segment (/<key>) and the catalog key for the
     sidebar label (`nav.<key>`) and screen header (`screens.<key>.*`) */
  key: string;
  icon: React.ReactNode;
  children?: NavDef[];
  /* the capability-table resource this screen reads, from `CAPABILITIES` in
     crates/rolter-control/src/rbac_matrix.rs. `<resource>:read` is what the
     rail hides the entry on (#1183). a leaf with no resource is one no role
     gates — the playground, the analytics screens, the account's own keys —
     and it stays visible for everyone */
  resource?: string;
}

// grouped by the job a visitor came to do rather than by the subsystem that
// implements it (#3656, ADR-0042): what traffic is doing, how it is routed,
// what it costs, who may send it, what keeps it safe, the MCP gateway, and the
// deployment's own settings. the account's own screens are not here: they
// live in the user menu, see `ACCOUNT_SCREENS`
export const NAV: NavDef[] = [
  {
    key: "observability",
    icon: <Eye />,
    children: [
      { key: "dashboard", icon: <ChartColumn /> },
      { key: "logs", icon: <FileText /> },
      { key: "mcp-logs", icon: <Waypoints />, resource: "mcp_log" },
      { key: "alerting-rules", icon: <Gavel />, resource: "alert_rule" },
      { key: "alerting-channels", icon: <Megaphone />, resource: "alert_channel" },
      { key: "alerting-history", icon: <History />, resource: "alert_history" },
      { key: "cluster", icon: <Network />, resource: "cluster_node" },
    ],
  },
  {
    key: "traffic",
    icon: <Shuffle />,
    children: [
      { key: "playground", icon: <Play /> },
      { key: "model-catalog", icon: <LayoutGrid />, resource: "model" },
      { key: "routing-rules", icon: <Split />, resource: "route" },
      { key: "providers", icon: <Boxes />, resource: "provider" },
      { key: "provider-groups", icon: <Layers />, resource: "provider_group" },
      { key: "complexity-router", icon: <ArrowLeftRight />, resource: "route" },
      {
        key: "adaptive-dashboard",
        icon: <ChartColumn />,
        resource: "adaptive_routing_telemetry",
      },
      {
        key: "adaptive-settings",
        icon: <SlidersHorizontal />,
        resource: "adaptive_routing_policy",
      },
      { key: "circuit-breaker", icon: <CircuitBoard /> },
    ],
  },
  {
    key: "spend",
    icon: <Wallet />,
    children: [
      { key: "budgets", icon: <Wallet />, resource: "budget" },
      { key: "pricing-overrides", icon: <SlidersHorizontal />, resource: "model_price" },
      { key: "savings", icon: <Scale />, resource: "savings" },
      { key: "business-units", icon: <Building2 />, resource: "business_unit" },
      { key: "customers", icon: <WalletCards />, resource: "customer" },
    ],
  },
  {
    key: "access",
    icon: <Landmark />,
    children: [
      { key: "virtual-keys", icon: <KeyRound />, resource: "virtual_key" },
      { key: "gov-users", icon: <Users />, resource: "user" },
      { key: "gov-teams", icon: <Building />, resource: "team" },
      { key: "access-profiles", icon: <Shield />, resource: "access_profile" },
      /* no resource: the role matrix describes what roles can do, not anyone's
         data, so every signed-in role reads it (#2527). the org's custom roles
         on it stay behind the server's own check */
      { key: "rbac", icon: <UserCheck /> },
      { key: "api-tokens", icon: <Bot />, resource: "api_token" },
      { key: "sso", icon: <Fingerprint />, resource: "sso_provider" },
      { key: "ldap", icon: <FolderTree />, resource: "ldap_directory" },
      { key: "user-provisioning", icon: <BookUser />, resource: "scim_token" },
      { key: "audit-logs", icon: <ScrollText />, resource: "audit_log" },
    ],
  },
  {
    key: "safety",
    icon: <ShieldCheck />,
    children: [
      { key: "guardrail-rules", icon: <Gavel />, resource: "guardrail_rule" },
      { key: "guardrail-policies", icon: <ListChecks />, resource: "guardrail_policy" },
      { key: "guardrail-providers", icon: <Boxes />, resource: "guardrail_provider" },
      { key: "log-erasure", icon: <Eraser />, resource: "log_erasure" },
    ],
  },
  {
    key: "mcp",
    icon: <Waypoints />,
    children: [
      { key: "mcp-catalog", icon: <Grid2x2 /> },
      { key: "mcp-library", icon: <Boxes />, resource: "mcp_server" },
      { key: "tool-groups", icon: <Puzzle />, resource: "mcp_tool_group" },
      { key: "auth-sessions", icon: <KeyRound />, resource: "mcp_oauth_session" },
      { key: "oauth-grants", icon: <Shield />, resource: "mcp_oauth_grant" },
      { key: "mcp-settings", icon: <Settings />, resource: "mcp_settings" },
    ],
  },
  {
    key: "platform",
    icon: <Settings />,
    children: [
      { key: "client-settings", icon: <SlidersHorizontal />, resource: "client_settings" },
      { key: "compatibility", icon: <Plug />, resource: "compatibility_policy" },
      { key: "performance", icon: <TrendingUp />, resource: "runtime_policy" },
      { key: "security", icon: <Shield />, resource: "security_settings" },
      { key: "feature-flags", icon: <Flag />, resource: "feature_flags" },
      { key: "effective-config", icon: <Database /> },
      { key: "logs-settings", icon: <Settings />, resource: "logging_settings" },
      { key: "model-settings", icon: <Settings />, resource: "model_defaults" },
      { key: "connectors", icon: <Cable />, resource: "connector" },
      { key: "plugins", icon: <Puzzle />, resource: "plugin" },
      { key: "prompt-repo", icon: <FolderGit2 />, resource: "prompt_template" },
      { key: "skills-repo", icon: <BookOpen />, resource: "skill" },
    ],
  },
];

/**
 * The signed-in account's own screens: its keys and profile, and its
 * preferences. The user menu holds them rather than the rail, which is about
 * the deployment (#3656). Self-service, so no role gates either. They route
 * like any leaf.
 */
export const ACCOUNT_SCREENS: NavDef[] = [
  { key: "api-keys", icon: <KeyRound /> },
  // the account's own language, default scope, Playground model and zone
  { key: "preferences", icon: <UserCog /> },
];

/** every key that resolves to a route: the rail's leaves and the account's screens */
export function screenKeys(): string[] {
  return [...leafKeys(), ...ACCOUNT_SCREENS.map((def) => def.key)];
}

// every navigable leaf key (parents with children are toggles, not screens)
export function leafKeys(defs: NavDef[] = NAV): string[] {
  return defs.flatMap((d) => (d.children ? leafKeys(d.children) : [d.key]));
}

/** The nav entry for a route key, wherever it sits in the tree. */
export function findLeaf(key: string, defs: NavDef[] = NAV): NavDef | undefined {
  for (const def of defs) {
    if (def.children) {
      const hit = findLeaf(key, def.children);
      if (hit) return hit;
    } else if (def.key === key) {
      return def;
    }
  }
  return undefined;
}

/**
 * The nav as this caller may see it (#1183).
 *
 * A leaf whose resource is explicitly unreadable is dropped, and a group left
 * with no children goes with it — a rail full of entries that all open the
 * same "you do not have access" is a worse map of the product than a shorter
 * rail. Only an explicit `false` hides anything: while the answer is unknown
 * the whole nav renders, so a slow or unanswerable capability query never
 * empties the dashboard.
 */
export function visibleNav(
  can: (resource: string, action: "read") => boolean | undefined,
  defs: NavDef[] = NAV,
): NavDef[] {
  return defs.flatMap((def) => {
    if (def.children) {
      const children = visibleNav(can, def.children);
      return children.length ? [{ ...def, children }] : [];
    }
    if (def.resource && can(def.resource, "read") === false) return [];
    return [def];
  });
}

// screen header copy lives in the catalog under `screens.<key>` (#489) rather
// than in a table here, so it translates with everything else. an unknown key
// degrades to the key itself, which is what the old META lookup did.
export function useScreenMeta(screen: string): [title: string, subtitle: string] {
  const { t } = useTranslation();
  return [
    t(`screens.${screen}.title`, { defaultValue: screen }),
    t(`screens.${screen}.subtitle`, { defaultValue: "" }),
  ];
}
