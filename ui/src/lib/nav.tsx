import {
  ArrowLeftRight,
  BellRing,
  BookOpen,
  BookUser,
  Boxes,
  Building,
  Building2,
  Cable,
  ChartColumn,
  CircuitBoard,
  Database,
  Eye,
  FileText,
  Flag,
  Fingerprint,
  FolderGit2,
  Gavel,
  Grid2x2,
  History,
  KeyRound,
  Landmark,
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
  Users,
  Wallet,
  WalletCards,
  Waypoints,
} from "lucide-react";
import type * as React from "react";
import { useTranslation } from "react-i18next";

// the control-plane IA, mirrored 1:1 from the design prototype's nav
// (cp-data.js NAV). key doubles as the route path segment: /<key>.
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

export const NAV: NavDef[] = [
  { key: "playground", icon: <Play /> },
  {
    key: "observability",
    icon: <Eye />,
    children: [
      { key: "dashboard", icon: <ChartColumn /> },
      { key: "logs", icon: <FileText /> },
      { key: "mcp-logs", icon: <Waypoints />, resource: "mcp_log" },
      { key: "connectors", icon: <Cable />, resource: "connector" },
      { key: "logs-settings", icon: <Settings />, resource: "logging_settings" },
    ],
  },
  {
    key: "models",
    icon: <Layers />,
    children: [
      { key: "model-catalog", icon: <LayoutGrid />, resource: "model" },
      { key: "providers", icon: <Boxes />, resource: "provider" },
      { key: "provider-groups", icon: <Layers />, resource: "provider_group" },
      { key: "budgets", icon: <Wallet />, resource: "budget" },
      { key: "routing-rules", icon: <Split />, resource: "route" },
      { key: "complexity-router", icon: <ArrowLeftRight />, resource: "route" },
      { key: "circuit-breaker", icon: <CircuitBoard /> },
      { key: "pricing-overrides", icon: <SlidersHorizontal />, resource: "model_price" },
      { key: "model-settings", icon: <Settings />, resource: "model_defaults" },
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
  { key: "plugins", icon: <Puzzle />, resource: "plugin" },
  {
    key: "alerting",
    icon: <BellRing />,
    children: [
      { key: "alerting-channels", icon: <Megaphone />, resource: "alert_channel" },
      { key: "alerting-rules", icon: <Gavel />, resource: "alert_rule" },
      { key: "alerting-history", icon: <History />, resource: "alert_history" },
    ],
  },
  {
    key: "governance",
    icon: <Landmark />,
    children: [
      { key: "virtual-keys", icon: <KeyRound />, resource: "virtual_key" },
      { key: "gov-users", icon: <Users />, resource: "user" },
      { key: "gov-teams", icon: <Building />, resource: "team" },
      { key: "business-units", icon: <Building2 />, resource: "business_unit" },
      { key: "customers", icon: <WalletCards />, resource: "customer" },
      { key: "user-provisioning", icon: <BookUser />, resource: "scim_token" },
      { key: "sso", icon: <Fingerprint />, resource: "sso_provider" },
      { key: "rbac", icon: <UserCheck />, resource: "custom_role" },
      { key: "access-profiles", icon: <Shield />, resource: "access_profile" },
      { key: "audit-logs", icon: <ScrollText />, resource: "audit_log" },
    ],
  },
  {
    key: "guardrails",
    icon: <ShieldCheck />,
    children: [
      { key: "guardrail-rules", icon: <Gavel />, resource: "guardrail_rule" },
      { key: "guardrail-providers", icon: <Boxes />, resource: "guardrail_provider" },
    ],
  },
  { key: "cluster", icon: <Network />, resource: "cluster_node" },
  {
    key: "adaptive-routing",
    icon: <Shuffle />,
    children: [
      {
        key: "adaptive-dashboard",
        icon: <ChartColumn />,
        resource: "adaptive_routing_telemetry",
      },
      {
        key: "adaptive-settings",
        icon: <Settings />,
        resource: "adaptive_routing_policy",
      },
    ],
  },
  { key: "prompt-repo", icon: <FolderGit2 />, resource: "prompt_template" },
  { key: "skills-repo", icon: <BookOpen />, resource: "skill" },
  {
    key: "settings",
    icon: <Settings />,
    children: [
      { key: "client-settings", icon: <SlidersHorizontal />, resource: "client_settings" },
      { key: "compatibility", icon: <Plug />, resource: "compatibility_policy" },
      { key: "effective-config", icon: <Database /> },
      { key: "security", icon: <Shield />, resource: "security_settings" },
      // the account's own keys and profile: self-service, so no role gates it
      { key: "api-keys", icon: <KeyRound /> },
      { key: "performance", icon: <TrendingUp />, resource: "runtime_policy" },
      { key: "feature-flags", icon: <Flag />, resource: "feature_flags" },
    ],
  },
];

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
