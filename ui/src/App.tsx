import { Bug, KeyRound, LogOut } from "lucide-react";
import * as React from "react";
import { Navigate, Route, Routes, useLocation, useNavigate } from "react-router-dom";

import { ScopeSwitcher } from "@/components/ScopeSwitcher";
import { ScreenHeader } from "@/components/ScreenHeader";
import { NavSidebar, type NavGroup, type NavItem } from "@/components/ui/nav-sidebar";
import { BUILT, META, NAV, leafKeys, type NavDef } from "@/lib/nav";
import { logout } from "@/lib/api";
import { useAuth } from "@/lib/auth";
import { useScope } from "@/lib/scope";
import { cn } from "@/lib/utils";
import Account from "@/pages/Account";
import { AlertChannels, AlertHistory, AlertRules } from "@/pages/Alerting";
import AuditLog from "@/pages/AuditLog";
import ComplexityRouter from "@/pages/ComplexityRouter";
import Config from "@/pages/Config";
import Connectors from "@/pages/Connectors";
import Dashboard from "@/pages/Dashboard";
import Health from "@/pages/Health";
import Keys from "@/pages/Keys";
import Limits from "@/pages/Limits";
import Login from "@/pages/Login";
import Logs from "@/pages/Logs";
import McpCatalog from "@/pages/McpCatalog";
import McpLogs from "@/pages/McpLogs";
import Models from "@/pages/Models";
import Playground from "@/pages/Playground";
import Pricing from "@/pages/Pricing";
import ProviderGroups from "@/pages/ProviderGroups";
import Providers from "@/pages/Providers";
import Rbac from "@/pages/Rbac";
import RoutingRules from "@/pages/RoutingRules";
import Security from "@/pages/Security";
import Stub from "@/pages/Stub";
import Teams from "@/pages/Teams";
import Users from "@/pages/Users";

// screen key → element for every built screen; anything else in the nav
// renders the branded stub. keys double as route paths (/<key>).
const SCREENS: Record<string, React.ReactNode> = {
  playground: <Playground />,
  dashboard: <Dashboard />,
  logs: <Logs />,
  "mcp-logs": <McpLogs />,
  connectors: <Connectors />,
  "model-catalog": <Models />,
  providers: <Providers />,
  "provider-groups": <ProviderGroups />,
  budgets: <Limits />,
  "routing-rules": <RoutingRules />,
  "complexity-router": <ComplexityRouter />,
  "circuit-breaker": <Health />,
  "pricing-overrides": <Pricing />,
  "alerting-channels": <AlertChannels />,
  "alerting-rules": <AlertRules />,
  "alerting-history": <AlertHistory />,
  "virtual-keys": <Keys />,
  "gov-users": <Users />,
  "gov-teams": <Teams />,
  rbac: <Rbac />,
  "audit-logs": <AuditLog />,
  "mcp-catalog": <McpCatalog />,
  "api-keys": <Account />,
  security: <Security />,
  caching: <Config />,
};

// old bookmarkable paths → new IA keys
const LEGACY: Record<string, string> = {
  "": "dashboard",
  keys: "virtual-keys",
  analytics: "dashboard",
  config: "caching",
  users: "gov-users",
  limits: "budgets",
  pricing: "pricing-overrides",
  health: "circuit-breaker",
  "audit-log": "audit-logs",
  account: "api-keys",
  models: "model-catalog",
};

const LEAVES = new Set(leafKeys());

// lucide's github glyph inlined — typescript 7 drops the deprecated brand-icon
// exports from lucide-react's types, so the named import no longer resolves
const GithubIcon = (
  <svg
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="2"
    strokeLinecap="round"
    strokeLinejoin="round"
  >
    <path d="M15 22v-4a4.8 4.8 0 0 0-1-3.5c3 0 6-2 6-5.5.08-1.25-.27-2.48-1-3.5.28-1.15.28-2.35 0-3.5 0 0-1 0-3 1.5-2.64-.5-5.36-.5-8 0C6 2 5 2 5 2c-.3 1.15-.3 2.35 0 3.5A5.403 5.403 0 0 0 4 9c0 3.5 3 5.5 6 5.5-.39.49-.68 1.05-.85 1.65-.17.6-.22 1.23-.15 1.85v4" />
    <path d="M9 18c-4.51 2-5-2-7-2" />
  </svg>
);

function toNavItem(def: NavDef): NavItem {
  return {
    key: def.key,
    label: def.label,
    icon: def.icon,
    children: def.children?.map(toNavItem),
  };
}

function MenuRow({
  icon,
  onClick,
  danger,
  children,
}: {
  icon: React.ReactNode;
  onClick: () => void;
  danger?: boolean;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      role="menuitem"
      onClick={onClick}
      className={cn(
        "flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm transition-colors hover:bg-[color:var(--surface-hover)] [&>svg]:h-4 [&>svg]:w-4 [&>svg]:flex-none",
        danger
          ? "text-[color:var(--text-secondary)] hover:text-[color:var(--status-danger)]"
          : "text-foreground",
      )}
    >
      {icon}
      {children}
    </button>
  );
}

function Screen({ screen }: { screen: string }) {
  const [title, subtitle] = META[screen] ?? [screen, ""];
  return (
    <div className="flex h-full min-h-0 flex-col">
      <ScreenHeader title={title} subtitle={subtitle} />
      <div className="min-h-0 flex-1 overflow-y-auto">
        {BUILT.has(screen) ? SCREENS[screen] : <Stub screen={screen} />}
      </div>
    </div>
  );
}

export default function App() {
  const { email, token, signOut } = useAuth();
  const navigate = useNavigate();
  const location = useLocation();
  const scope = useScope();

  // revoke the server-side session (if any) before clearing local state;
  // best-effort so a network hiccup still logs the user out locally
  const handleSignOut = () => {
    if (token) void logout().catch(() => {});
    signOut();
  };

  if (!email) {
    return <Login />;
  }

  const key = location.pathname.replace(/^\/+|\/+$/g, "");
  const redirect = LEGACY[key];
  const activeKey = LEAVES.has(key) ? key : "dashboard";
  const orgName = scope.orgs.find((o) => o.id === scope.orgId)?.name;
  const navGroups: NavGroup[] = [{ items: NAV.map(toNavItem) }];
  const initials = (email.trim()[0] ?? "?").toUpperCase();

  return (
    <div className="flex h-screen bg-[color:var(--surface-app)] text-foreground">
      <NavSidebar
        groups={navGroups}
        logoSrc="/logo-mark.svg"
        brand="rolter"
        activeKey={activeKey}
        onNavigate={(k) => navigate(`/${k}`)}
        searchable
        collapsible
        footerLinks={[
          {
            key: "github",
            title: "GitHub repository",
            icon: GithubIcon,
            href: "https://github.com/rolter-ai/rolter",
          },
          {
            key: "bug",
            title: "Report a bug",
            icon: <Bug />,
            href: "https://github.com/rolter-ai/rolter/issues/new",
          },
        ]}
        version={`v${__APP_VERSION__}`}
        user={{
          name: email,
          role: orgName ? `Admin · ${orgName}` : "Admin",
          initials,
          onClick: handleSignOut,
        }}
        userMenu={(close) => (
          <div>
            <div className="flex items-center gap-2 px-3 pb-2 pt-1">
              <span className="flex h-8 w-8 flex-none items-center justify-center rounded-full bg-[color:var(--red-folk)] text-xs font-semibold text-white">
                {initials}
              </span>
              <div className="min-w-0">
                <p className="truncate text-xs font-medium text-foreground">{email}</p>
                <p className="truncate text-[0.6875rem] text-muted-foreground">
                  {orgName ? `Admin · ${orgName}` : "Admin"}
                </p>
              </div>
            </div>
            <div className="border-t border-[color:var(--border-subtle)] py-1.5">
              <p className="px-3 pb-1 text-[0.625rem] uppercase tracking-[0.08em] text-[color:var(--text-subtle)]">
                Scope
              </p>
              <ScopeSwitcher />
            </div>
            <div className="border-t border-[color:var(--border-subtle)] pt-1">
              <MenuRow
                icon={<KeyRound />}
                onClick={() => {
                  navigate("/api-keys");
                  close();
                }}
              >
                Account &amp; API keys
              </MenuRow>
              <MenuRow
                icon={<LogOut />}
                danger
                onClick={() => {
                  close();
                  handleSignOut();
                }}
              >
                Sign out
              </MenuRow>
            </div>
          </div>
        )}
      />
      <main className="min-w-0 flex-1 overflow-hidden border-l border-[color:var(--border-subtle)] bg-background">
        {redirect != null ? (
          <Navigate to={`/${redirect}`} replace />
        ) : (
          <Routes>
            {[...LEAVES].map((k) => (
              <Route key={k} path={`/${k}`} element={<Screen screen={k} />} />
            ))}
            <Route path="*" element={<Navigate to="/dashboard" replace />} />
          </Routes>
        )}
      </main>
    </div>
  );
}
