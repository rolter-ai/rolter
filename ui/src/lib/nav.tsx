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
}

export const NAV: NavDef[] = [
  { key: "playground", icon: <Play /> },
  {
    key: "observability",
    icon: <Eye />,
    children: [
      { key: "dashboard", icon: <ChartColumn /> },
      { key: "logs", icon: <FileText /> },
      { key: "mcp-logs", icon: <Waypoints /> },
      { key: "connectors", icon: <Cable /> },
      { key: "logs-settings", icon: <Settings /> },
    ],
  },
  {
    key: "models",
    icon: <Layers />,
    children: [
      { key: "model-catalog", icon: <LayoutGrid /> },
      { key: "providers", icon: <Boxes /> },
      { key: "provider-groups", icon: <Layers /> },
      { key: "budgets", icon: <Wallet /> },
      { key: "routing-rules", icon: <Split /> },
      { key: "complexity-router", icon: <ArrowLeftRight /> },
      { key: "circuit-breaker", icon: <CircuitBoard /> },
      { key: "pricing-overrides", icon: <SlidersHorizontal /> },
      { key: "model-settings", icon: <Settings /> },
    ],
  },
  {
    key: "mcp",
    icon: <Waypoints />,
    children: [
      { key: "mcp-catalog", icon: <Grid2x2 /> },
      { key: "mcp-library", icon: <Boxes /> },
      { key: "tool-groups", icon: <Puzzle /> },
      { key: "auth-sessions", icon: <KeyRound /> },
      { key: "oauth-grants", icon: <Shield /> },
      { key: "mcp-settings", icon: <Settings /> },
    ],
  },
  { key: "plugins", icon: <Puzzle /> },
  {
    key: "alerting",
    icon: <BellRing />,
    children: [
      { key: "alerting-channels", icon: <Megaphone /> },
      { key: "alerting-rules", icon: <Gavel /> },
      { key: "alerting-history", icon: <History /> },
    ],
  },
  {
    key: "governance",
    icon: <Landmark />,
    children: [
      { key: "virtual-keys", icon: <KeyRound /> },
      { key: "gov-users", icon: <Users /> },
      { key: "gov-teams", icon: <Building /> },
      { key: "business-units", icon: <Building2 /> },
      { key: "customers", icon: <WalletCards /> },
      { key: "user-provisioning", icon: <BookUser /> },
      { key: "rbac", icon: <UserCheck /> },
      { key: "access-profiles", icon: <Shield /> },
      { key: "audit-logs", icon: <ScrollText /> },
    ],
  },
  {
    key: "guardrails",
    icon: <ShieldCheck />,
    children: [
      { key: "guardrail-rules", icon: <Gavel /> },
      { key: "guardrail-providers", icon: <Boxes /> },
    ],
  },
  { key: "cluster", icon: <Network /> },
  {
    key: "adaptive-routing",
    icon: <Shuffle />,
    children: [
      { key: "adaptive-dashboard", icon: <ChartColumn /> },
      { key: "adaptive-settings", icon: <Settings /> },
    ],
  },
  { key: "prompt-repo", icon: <FolderGit2 /> },
  { key: "skills-repo", icon: <BookOpen /> },
  {
    key: "settings",
    icon: <Settings />,
    children: [
      { key: "client-settings", icon: <SlidersHorizontal /> },
      { key: "compatibility", icon: <Plug /> },
      { key: "caching", icon: <Database /> },
      { key: "security", icon: <Shield /> },
      { key: "api-keys", icon: <KeyRound /> },
      { key: "performance", icon: <TrendingUp /> },
      { key: "feature-flags", icon: <Flag /> },
    ],
  },
];

// screens with a real implementation; everything else renders the branded stub
export const BUILT = new Set([
  "access-profiles",
  "plugins",
  "guardrail-rules",
  "guardrail-providers",
  "playground",
  "dashboard",
  "logs",
  "providers",
  "provider-groups",
  "model-catalog",
  "virtual-keys",
  "budgets",
  "pricing-overrides",
  "audit-logs",
  "gov-users",
  "circuit-breaker",
  "caching",
  "routing-rules",
  "gov-teams",
  "mcp-catalog",
  "mcp-library",
  "tool-groups",
  "auth-sessions",
  "oauth-grants",
  "mcp-settings",
  "api-keys",
  "security",
  "rbac",
  "mcp-logs",
  "connectors",
  "complexity-router",
  "alerting-channels",
  "alerting-rules",
  "alerting-history",
  "feature-flags",
  "logs-settings",
  "performance",
  "business-units",
  "customers",
  "compatibility",
  "cluster",
  "adaptive-dashboard",
  "adaptive-settings",
  "user-provisioning",
  "prompt-repo",
  "skills-repo",
  "client-settings",
  "model-settings",
]);

// every navigable leaf key (parents with children are toggles, not screens)
export function leafKeys(defs: NavDef[] = NAV): string[] {
  return defs.flatMap((d) => (d.children ? leafKeys(d.children) : [d.key]));
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
