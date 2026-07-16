import {
  Activity,
  BarChart3,
  BookText,
  Boxes,
  ChevronDown,
  DollarSign,
  Gauge,
  History,
  KeyRound,
  Play,
  ScrollText,
  Server,
  SlidersHorizontal,
  UserPlus,
  Users as UsersIcon,
} from "lucide-react";
import * as React from "react";
import { Route, Routes, useLocation, useNavigate } from "react-router-dom";

import { ScopeSwitcher } from "@/components/ScopeSwitcher";
import { Button } from "@/components/ui/button";
import { NavSidebar, type NavGroup } from "@/components/ui/nav-sidebar";
import Account from "@/pages/Account";
import AuditLog from "@/pages/AuditLog";
import Analytics from "@/pages/Analytics";
import Config from "@/pages/Config";
import Health from "@/pages/Health";
import Keys from "@/pages/Keys";
import Limits from "@/pages/Limits";
import Login from "@/pages/Login";
import Logs from "@/pages/Logs";
import Models from "@/pages/Models";
import Playground from "@/pages/Playground";
import Pricing from "@/pages/Pricing";
import Providers from "@/pages/Providers";
import Users from "@/pages/Users";
import { logout } from "@/lib/api";
import { useAuth } from "@/lib/auth";
import { useScope } from "@/lib/scope";

// nav item → route path. `key` is what NavSidebar reports on click; `path` is
// where react-router takes us. keeps the DS NavSidebar (callback-driven) and
// react-router in sync without threading <NavLink> through the DS component.
interface NavEntry {
  key: string;
  label: string;
  path: string;
  icon: React.ReactNode;
}

const GROUPS: { label?: string; items: NavEntry[] }[] = [
  {
    items: [
      { key: "playground", label: "Playground", path: "/playground", icon: <Play /> },
      { key: "models", label: "Models", path: "/", icon: <Boxes /> },
      { key: "keys", label: "Keys", path: "/keys", icon: <KeyRound /> },
      { key: "logs", label: "Logs", path: "/logs", icon: <ScrollText /> },
    ],
  },
  {
    label: "Operate",
    items: [
      { key: "analytics", label: "Analytics", path: "/analytics", icon: <BarChart3 /> },
      { key: "config", label: "Config", path: "/config", icon: <SlidersHorizontal /> },
    ],
  },
  {
    label: "Admin",
    items: [
      { key: "providers", label: "Providers", path: "/providers", icon: <Server /> },
      { key: "users", label: "Users", path: "/users", icon: <UsersIcon /> },
      { key: "limits", label: "Limits", path: "/limits", icon: <Gauge /> },
      { key: "pricing", label: "Pricing", path: "/pricing", icon: <DollarSign /> },
      { key: "health", label: "Health", path: "/health", icon: <Activity /> },
      { key: "audit-log", label: "Audit Log", path: "/audit-log", icon: <History /> },
      { key: "account", label: "Account", path: "/account", icon: <BookText /> },
    ],
  },
];

const ALL = GROUPS.flatMap((g) => g.items);

// longest-prefix match so nested paths still light up their top-level item;
// "/" only matches Models exactly.
function activeKeyFor(pathname: string): string {
  let best: NavEntry | undefined;
  for (const e of ALL) {
    if (e.path === "/") {
      if (pathname === "/") best = e;
    } else if (pathname === e.path || pathname.startsWith(e.path + "/")) {
      if (!best || e.path.length > best.path.length) best = e;
    }
  }
  return best?.key ?? "models";
}

// topbar org picker: shows the active org/project and drops down the full
// ScopeSwitcher (the old sidebar control, now folded into the DS topbar).
function OrgPicker() {
  const scope = useScope();
  const [open, setOpen] = React.useState(false);
  const ref = React.useRef<HTMLDivElement>(null);

  React.useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [open]);

  const orgName = scope.orgs.find((o) => o.id === scope.orgId)?.name;
  const projName = scope.projects.find((p) => p.id === scope.projectId)?.name;
  const summary = orgName
    ? projName
      ? `${orgName} · ${projName}`
      : orgName
    : "Select scope";

  return (
    <div className="relative" ref={ref}>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex items-center gap-2 rounded-md border border-[color:var(--border-subtle)] px-2.5 py-1.5 text-sm font-medium text-foreground transition-colors hover:border-[color:var(--border-default)]"
      >
        <span className="h-2 w-2 rounded-full bg-[color:var(--red-folk)]" />
        <span className="max-w-[240px] truncate">{summary}</span>
        <ChevronDown className="h-3.5 w-3.5 text-[color:var(--text-subtle)]" />
      </button>
      {open && (
        <div className="absolute left-0 top-[calc(100%+6px)] z-20 w-[300px] rounded-lg border border-[color:var(--border-default)] bg-[color:var(--surface-elevated)] py-2 shadow-lg">
          <ScopeSwitcher />
        </div>
      )}
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

  const activeKey = activeKeyFor(location.pathname);
  const orgName = scope.orgs.find((o) => o.id === scope.orgId)?.name;

  const navGroups: NavGroup[] = GROUPS.map((g) => ({
    label: g.label,
    items: g.items.map((it) => ({ key: it.key, label: it.label, icon: it.icon })),
  }));

  const goto = (key: string) => {
    const entry = ALL.find((e) => e.key === key);
    if (entry) navigate(entry.path);
  };

  const initials = (email.trim()[0] ?? "?").toUpperCase();

  return (
    <div className="flex h-screen bg-[color:var(--surface-app)] p-2 text-foreground">
      <NavSidebar
        className="rounded-lg border"
        logoSrc="/logo-mark.svg"
        brand="rolter"
        groups={navGroups}
        activeKey={activeKey}
        onNavigate={goto}
        user={{
          name: email,
          role: orgName ? `Admin · ${orgName}` : "Admin",
          initials,
          onClick: handleSignOut,
        }}
      />
      <div className="flex min-w-0 flex-1 flex-col pl-2">
        {/* topbar — org picker on the left, quick actions on the right */}
        <header className="flex h-[52px] items-center gap-4 rounded-t-lg border border-b-0 bg-background px-5">
          <OrgPicker />
          <div className="ml-auto flex items-center gap-2">
            <Button
              variant="ghost"
              size="sm"
              onClick={() =>
                window.open("https://github.com/rolter-ai/rolter", "_blank")
              }
            >
              Docs
            </Button>
            <Button variant="outline" size="sm" onClick={() => goto("users")}>
              <UserPlus className="h-4 w-4" /> Invite
            </Button>
          </div>
        </header>
        {/* content floats as a crisp bordered card on the graphite backdrop */}
        <main className="flex-1 overflow-hidden rounded-b-lg border bg-background">
          <div className="h-full overflow-auto p-8">
            <Routes>
              <Route path="/" element={<Models />} />
              <Route path="/playground" element={<Playground />} />
              <Route path="/keys" element={<Keys />} />
              <Route path="/logs" element={<Logs />} />
              <Route path="/analytics" element={<Analytics />} />
              <Route path="/config" element={<Config />} />
              <Route path="/providers" element={<Providers />} />
              <Route path="/users" element={<Users />} />
              <Route path="/limits" element={<Limits />} />
              <Route path="/pricing" element={<Pricing />} />
              <Route path="/health" element={<Health />} />
              <Route path="/audit-log" element={<AuditLog />} />
              <Route path="/account" element={<Account />} />
            </Routes>
          </div>
        </main>
      </div>
    </div>
  );
}
