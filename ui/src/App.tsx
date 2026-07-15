import { LogOut } from "lucide-react";
import { NavLink, Route, Routes } from "react-router-dom";

import { ScopeSwitcher } from "@/components/ScopeSwitcher";
import Account from "@/pages/Account";
import Health from "@/pages/Health";
import Keys from "@/pages/Keys";
import Limits from "@/pages/Limits";
import Login from "@/pages/Login";
import Logs from "@/pages/Logs";
import Models from "@/pages/Models";
import Pricing from "@/pages/Pricing";
import Providers from "@/pages/Providers";
import Users from "@/pages/Users";
import { logout } from "@/lib/api";
import { useAuth } from "@/lib/auth";
import { cn } from "@/lib/utils";

const nav = [
  { to: "/", label: "Models", end: true },
  { to: "/keys", label: "Keys", end: false },
  { to: "/providers", label: "Providers", end: false },
  { to: "/users", label: "Users", end: false },
  { to: "/account", label: "Account", end: false },
  { to: "/limits", label: "Limits", end: false },
  { to: "/pricing", label: "Pricing", end: false },
  { to: "/health", label: "Health", end: false },
  { to: "/logs", label: "Logs", end: false },
];

export default function App() {
  const { email, token, signOut } = useAuth();

  // revoke the server-side session (if any) before clearing local state;
  // best-effort so a network hiccup still logs the user out locally
  const handleSignOut = () => {
    if (token) void logout().catch(() => {});
    signOut();
  };

  if (!email) {
    return <Login />;
  }

  return (
    <div className="flex min-h-screen bg-[color:var(--surface-app)] p-2 text-foreground">
      <aside
        className="flex shrink-0 flex-col"
        style={{ width: "var(--sidebar-width)" }}
      >
        {/* wordmark — Geist Mono 600 with the deep folk-red dot */}
        <div className="flex h-[52px] items-center px-3">
          <span className="font-mono text-base font-semibold tracking-tight">
            rolter
            <span className="text-[color:var(--red-folk)]">.</span>
          </span>
        </div>
        <ScopeSwitcher />
        <nav className="flex flex-1 flex-col gap-0.5 px-2 py-2">
          {nav.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              end={item.end}
              className={({ isActive }) =>
                cn(
                  "relative rounded-md px-3 py-1.5 text-sm transition-colors",
                  "before:absolute before:left-0 before:top-1/2 before:h-4 before:w-0.5 before:-translate-y-1/2 before:rounded-full before:transition-colors",
                  isActive
                    ? "bg-secondary text-foreground before:bg-[color:var(--red-folk)]"
                    : "text-muted-foreground before:bg-transparent hover:bg-secondary/60 hover:text-foreground",
                )
              }
            >
              {item.label}
            </NavLink>
          ))}
        </nav>
        {/* user pinned to the bottom */}
        <div className="mx-2 mb-1 flex items-center justify-between gap-2 rounded-md border border-[color:var(--border-subtle)] px-3 py-2">
          <span className="truncate font-mono text-xs text-muted-foreground">
            {email}
          </span>
          <button
            type="button"
            onClick={handleSignOut}
            aria-label="Sign out"
            className="shrink-0 text-muted-foreground transition-colors hover:text-foreground"
          >
            <LogOut className="h-3.5 w-3.5" />
          </button>
        </div>
      </aside>
      {/* content floats as a crisp bordered card on the graphite backdrop */}
      <main className="flex-1 overflow-hidden rounded-lg border bg-background">
        <div className="h-full overflow-auto p-8">
          <Routes>
            <Route path="/" element={<Models />} />
            <Route path="/keys" element={<Keys />} />
            <Route path="/providers" element={<Providers />} />
            <Route path="/users" element={<Users />} />
            <Route path="/account" element={<Account />} />
            <Route path="/limits" element={<Limits />} />
            <Route path="/pricing" element={<Pricing />} />
            <Route path="/health" element={<Health />} />
            <Route path="/logs" element={<Logs />} />
          </Routes>
        </div>
      </main>
    </div>
  );
}
