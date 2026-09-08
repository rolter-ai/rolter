import { Bug, KeyRound, LogOut } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import {
  Navigate,
  Route,
  Routes,
  useLocation,
  useNavigate,
} from "react-router";

import { ForbiddenScreen } from "@/components/ForbiddenScreen";
import { LocalePicker } from "@/components/LocalePicker";
import { OpenModeBanner } from "@/components/OpenModeBanner";
import { Toaster } from "@/components/ui/toaster";
import { ScopeSwitcher } from "@/components/ScopeSwitcher";
import { ScreenHeader } from "@/components/ScreenHeader";
import { ShellSkeleton } from "@/components/ShellSkeleton";
import {
  NavSidebar,
  type NavGroup,
  type NavItem,
} from "@/components/ui/nav-sidebar";
import {
  findLeaf,
  leafKeys,
  useScreenMeta,
  visibleNav,
  type NavDef,
} from "@/lib/nav";
import { logout, ROLES, type MeMembership } from "@/lib/api";
import { useAuth, type SessionUser } from "@/lib/auth";
import { CapabilityProvider, useCan } from "@/lib/can";
import { useScope } from "@/lib/scope";
import { cn } from "@/lib/utils";
import { useVersionStatus } from "@/lib/version";
import { isOpenMode } from "@/lib/telemetry";
import {
  UxScreenProvider,
  useRouteTelemetry,
  useUxContext,
} from "@/lib/ux-react";
import Account from "@/pages/Account";
import { AlertChannels, AlertHistory, AlertRules } from "@/pages/Alerting";
import AdaptiveDashboard from "@/pages/AdaptiveDashboard";
import AdaptiveSettings from "@/pages/AdaptiveSettings";
import AuditLog from "@/pages/AuditLog";
import ClientSettings from "@/pages/ClientSettings";
import Cluster from "@/pages/Cluster";
import ComplexityRouter from "@/pages/ComplexityRouter";
import Compatibility from "@/pages/Compatibility";
import Config from "@/pages/Config";
import AccessProfiles from "@/pages/AccessProfiles";
import Connectors from "@/pages/Connectors";
import { BusinessUnits, Customers } from "@/pages/CostAttribution";
import Dashboard from "@/pages/Dashboard";
import FeatureFlags from "@/pages/FeatureFlags";
import Health from "@/pages/Health";
import GuardrailProviders from "@/pages/GuardrailProviders";
import GuardrailRules from "@/pages/GuardrailRules";
import LogsSettings from "@/pages/LogsSettings";
import Keys from "@/pages/Keys";
import Limits from "@/pages/Limits";
import AcceptInvite from "@/pages/AcceptInvite";
import Login from "@/pages/Login";
import Logs from "@/pages/Logs";
import McpCatalog from "@/pages/McpCatalog";
import { McpLibrary, McpSettings, ToolGroups } from "@/pages/McpManagement";
import McpLogs from "@/pages/McpLogs";
import { AuthSessions, OAuthGrants } from "@/pages/McpOAuth";
import ModelSettings from "@/pages/ModelSettings";
import Models from "@/pages/Models";
import Performance from "@/pages/Performance";
import Playground from "@/pages/Playground";
import Plugins from "@/pages/Plugins";
import Pricing from "@/pages/Pricing";
import PromptRepository from "@/pages/PromptRepository";
import ProviderGroups from "@/pages/ProviderGroups";
import Providers from "@/pages/Providers";
import Rbac from "@/pages/Rbac";
import RoutingRules from "@/pages/RoutingRules";
import Security from "@/pages/Security";
import SingleSignOn from "@/pages/SingleSignOn";
import SkillsRepository from "@/pages/SkillsRepository";
import Teams from "@/pages/Teams";
import UserProvisioning from "@/pages/UserProvisioning";
import Users from "@/pages/Users";

// screen key → element, one entry per navigable leaf; keys double as route
// paths (/<key>). exported so `nav.test.ts` can hold the two lists to each
// other: the nav used to be allowed to name a screen nobody had built, and the
// branded `Stub` stood in for it. every leaf has been built for a long time,
// so the placeholder was unreachable code that only made the gap look filled —
// the test is what keeps this table complete now that it is gone (#1201).
export const SCREENS: Record<string, React.ReactNode> = {
  playground: <Playground />,
  plugins: <Plugins />,
  dashboard: <Dashboard />,
  logs: <Logs />,
  "mcp-logs": <McpLogs />,
  "access-profiles": <AccessProfiles />,
  connectors: <Connectors />,
  "model-catalog": <Models />,
  "model-settings": <ModelSettings />,
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
  "mcp-library": <McpLibrary />,
  "tool-groups": <ToolGroups />,
  "auth-sessions": <AuthSessions />,
  "oauth-grants": <OAuthGrants />,
  "mcp-settings": <McpSettings />,
  "api-keys": <Account />,
  security: <Security />,
  "effective-config": <Config />,
  "client-settings": <ClientSettings />,
  "feature-flags": <FeatureFlags />,
  "guardrail-rules": <GuardrailRules />,
  "guardrail-providers": <GuardrailProviders />,
  "logs-settings": <LogsSettings />,
  performance: <Performance />,
  compatibility: <Compatibility />,
  cluster: <Cluster />,
  "adaptive-dashboard": <AdaptiveDashboard />,
  "adaptive-settings": <AdaptiveSettings />,
  "business-units": <BusinessUnits />,
  customers: <Customers />,
  "user-provisioning": <UserProvisioning />,
  sso: <SingleSignOn />,
  "prompt-repo": <PromptRepository />,
  "skills-repo": <SkillsRepository />,
};

// old bookmarkable paths → new IA keys
const LEGACY: Record<string, string> = {
  "": "dashboard",
  keys: "virtual-keys",
  analytics: "dashboard",
  config: "effective-config",
  // the entry was labelled "Caching" for a while although it always opened the
  // effective-config viewer; keep the old path alive
  caching: "effective-config",
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

function toNavItem(def: NavDef, t: TFunction): NavItem {
  return {
    key: def.key,
    label: t(`nav.${def.key}`),
    icon: def.icon,
    children: def.children?.map((child) => toNavItem(child, t)),
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
      onClick={onClick}
      className={cn(
        "flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm transition-colors hover:bg-[color:var(--surface-hover)] [&>svg]:h-4 [&>svg]:w-4 [&>svg]:flex-none",
        danger
          ? "text-[color:var(--text-secondary)] hover:text-[color:var(--status-danger-text)]"
          : "text-foreground",
      )}
    >
      {icon}
      {children}
    </button>
  );
}

/**
 * What to call the signed-in account in the rail.
 *
 * The server's answer, not the client's guess: `is_superadmin` comes from
 * `/auth/me` (falling back to the blob cached at login), and a plain account
 * is named by its membership role in the org currently in scope. Before #1196
 * every session read "Admin", including the ones that were not.
 */
function roleLabel(
  t: TFunction,
  user: SessionUser | null,
  memberships: MeMembership[],
  orgId: string | undefined,
): string {
  if (user && !user.is_superadmin) {
    const membership =
      memberships.find((m) => m.org_id && m.org_id === orgId) ?? memberships[0];
    // an unknown role string from a newer control plane has no label here, so
    // it falls through rather than rendering a raw key
    if (membership && (ROLES as readonly string[]).includes(membership.role)) {
      return t(`shell.roles.${membership.role}`);
    }
  }
  return t("shell.role");
}

function Screen({ screen, onOpenNav }: { screen: string; onOpenNav: () => void }) {
  const { t } = useTranslation();
  const [title, subtitle] = useScreenMeta(screen);
  const can = useCan();
  // a leaf the rail hides is still reachable by URL — a bookmark, a shared
  // link, the browser's history. it renders the refusal in the shell rather
  // than mounting a screen whose every request is already known to be a 403
  // (#1183). the screen's own name is the noun, since it is the whole screen
  // that is refused and not one of its queries
  const resource = findLeaf(screen)?.resource;
  const forbidden = !!resource && can(resource, "read") === false;
  return (
    // names the screen for every UX event emitted below it (#805), so shared
    // components like EmptyState are instrumented without a prop per screen
    <UxScreenProvider screen={screen}>
      <div className="flex h-full min-h-0 flex-col">
        <ScreenHeader title={title} subtitle={subtitle} onOpenNav={onOpenNav} />
        <div className="min-h-0 flex-1 overflow-y-auto">
          {forbidden ? (
            <ForbiddenScreen resource={t(`nav.${screen}`)} />
          ) : (
            SCREENS[screen]
          )}
        </div>
      </div>
    </UxScreenProvider>
  );
}

/**
 * The dashboard, under one answer to "what may this caller do" (#1183).
 *
 * The provider sits above the shell rather than inside it because the rail is
 * gated too: a group whose every leaf is unreadable is hidden, so the nav has
 * to be built with the answer already in hand. It asks nothing while signed
 * out — the login screen has no capabilities to ask about.
 */
export default function App() {
  return (
    <CapabilityProvider>
      <Shell />
    </CapabilityProvider>
  );
}

function Shell() {
  const { t } = useTranslation();
  const { email, token, user, memberships, status, signOut } = useAuth();
  const can = useCan();
  const navigate = useNavigate();
  const location = useLocation();
  const scope = useScope();

  // route key, computed before the early returns below so the telemetry hooks
  // that follow are called unconditionally
  const key = location.pathname.replace(/^\/+|\/+$/g, "");
  const activeKey = LEAVES.has(key) ? key : "dashboard";

  // below `md` the rail is a drawer opened from the screen header, so its
  // state is owned here, between the two (#959). any route change closes it,
  // which covers the paths the rail does not know it took — the account link
  // in the user menu, a redirect, the browser's back button
  const [navOpen, setNavOpen] = React.useState(false);
  React.useEffect(() => setNavOpen(false), [location.pathname]);

  // the navigation half of the UX stream (#805): screen_view, navigate and
  // back_out for the whole dashboard, emitted once from the shell rather than
  // per screen. an empty key while signed out is dropped by `track`, which is
  // what keeps the login screen out of the funnel
  useRouteTelemetry(email ? activeKey : "");
  useUxContext(scope);

  // the running build and, when the control plane knows of one, the newer
  // release for the rail footer (#902). the compiled version is the fallback
  // while the endpoint is unreachable or the session is still being checked
  const { version, update } = useVersionStatus(__APP_VERSION__, !!email && status !== "checking");

  // revoke the server-side session (if any) before clearing local state;
  // best-effort so a network hiccup still logs the user out locally
  const handleSignOut = () => {
    if (token) void logout().catch(() => {});
    signOut();
  };

  // an invite link is reachable without a session: the invitee does not have
  // one yet, and the token in the url is what stands in for it
  const invite = location.pathname.match(/^\/invite\/([^/]+)$/);
  if (invite) {
    return <AcceptInvite token={decodeURIComponent(invite[1])} />;
  }

  // the stored token has not been re-checked yet: hold the shape of the shell
  // rather than flashing Login at someone whose session turns out to be fine,
  // and rather than letting every screen fire a request with a token that may
  // already be dead (#1196)
  if (status === "checking") {
    return <ShellSkeleton />;
  }

  if (!email) {
    return <Login />;
  }

  const redirect = LEGACY[key];
  const orgName = scope.orgs.find((o) => o.id === scope.orgId)?.name;
  const navGroups: NavGroup[] = [
    { items: visibleNav(can).map((def) => toNavItem(def, t)) },
  ];
  const roleName = roleLabel(t, user, memberships, scope.orgId);
  const role = orgName
    ? t("shell.roleWithOrg", { role: roleName, org: orgName })
    : roleName;
  const initials = (email.trim()[0] ?? "?").toUpperCase();

  return (
    // the open-mode warning spans the full width above the shell rather than
    // sitting inside a screen: it is a property of the control plane, not of
    // whatever screen happens to be open (#970)
    <div className="flex h-screen flex-col bg-[color:var(--surface-app)] text-foreground">
      <OpenModeBanner open={isOpenMode()} />
      <Toaster />
      <div className="flex min-h-0 flex-1">
        <NavSidebar
          groups={navGroups}
          logoSrc="/logo-mark.svg"
          brand="rolter"
          activeKey={activeKey}
          onNavigate={(k) => navigate(`/${k}`)}
          open={navOpen}
          onOpenChange={setNavOpen}
          searchable
          collapsible
          resizable
          footerLinks={[
            {
              key: "github",
              title: t("shell.githubRepo"),
              icon: GithubIcon,
              href: "https://github.com/rolter-ai/rolter",
            },
            {
              key: "bug",
              title: t("shell.reportBug"),
              icon: <Bug />,
              href: "https://github.com/rolter-ai/rolter/issues/new",
            },
          ]}
          footerExtra={(collapsed) => <LocalePicker collapsed={collapsed} />}
          version={`v${version}`}
          update={update}
          user={{
            name: email,
            role,
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
                  <p className="truncate text-xs font-medium text-foreground">
                    {email}
                  </p>
                  <p className="truncate text-[0.6875rem] text-muted-foreground">
                    {role}
                  </p>
                </div>
              </div>
              <div className="border-t border-[color:var(--border-subtle)] py-1.5">
                <p className="px-3 pb-1 text-[0.625rem] uppercase tracking-[0.08em] text-[color:var(--text-subtle)]">
                  {t("shell.scope")}
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
                  {t("shell.accountAndKeys")}
                </MenuRow>
                <MenuRow
                  icon={<LogOut />}
                  danger
                  onClick={() => {
                    close();
                    handleSignOut();
                  }}
                >
                  {t("shell.signOut")}
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
                <Route
                  key={k}
                  path={`/${k}`}
                  element={<Screen screen={k} onOpenNav={() => setNavOpen(true)} />}
                />
              ))}
              <Route path="*" element={<Navigate to="/dashboard" replace />} />
            </Routes>
          )}
        </main>
      </div>
    </div>
  );
}
