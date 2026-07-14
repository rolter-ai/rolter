import { NavLink, Route, Routes } from "react-router-dom";

import Health from "@/pages/Health";
import Keys from "@/pages/Keys";
import Logs from "@/pages/Logs";
import Models from "@/pages/Models";
import { cn } from "@/lib/utils";

const nav = [
  { to: "/", label: "Models", end: true },
  { to: "/keys", label: "Keys", end: false },
  { to: "/health", label: "Health", end: false },
  { to: "/logs", label: "Logs", end: false },
];

export default function App() {
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
      </aside>
      {/* content floats as a crisp bordered card on the graphite backdrop */}
      <main className="flex-1 overflow-hidden rounded-lg border bg-background">
        <div className="h-full overflow-auto p-8">
          <Routes>
            <Route path="/" element={<Models />} />
            <Route path="/keys" element={<Keys />} />
            <Route path="/health" element={<Health />} />
            <Route path="/logs" element={<Logs />} />
          </Routes>
        </div>
      </main>
    </div>
  );
}
