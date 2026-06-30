import { NavLink, Route, Routes } from "react-router-dom";

import Keys from "@/pages/Keys";
import Logs from "@/pages/Logs";
import Models from "@/pages/Models";
import { cn } from "@/lib/utils";

const nav = [
  { to: "/", label: "Models", end: true },
  { to: "/keys", label: "Keys", end: false },
  { to: "/logs", label: "Logs", end: false },
];

export default function App() {
  return (
    <div className="flex min-h-screen">
      <aside className="w-56 shrink-0 border-r p-4">
        <div className="mb-6 text-xl font-semibold">rolter</div>
        <nav className="flex flex-col gap-1">
          {nav.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              end={item.end}
              className={({ isActive }) =>
                cn(
                  "rounded-md px-3 py-2 text-sm transition-colors",
                  isActive
                    ? "bg-accent text-accent-foreground"
                    : "text-muted-foreground hover:bg-accent hover:text-accent-foreground",
                )
              }
            >
              {item.label}
            </NavLink>
          ))}
        </nav>
      </aside>
      <main className="flex-1 p-8">
        <Routes>
          <Route path="/" element={<Models />} />
          <Route path="/keys" element={<Keys />} />
          <Route path="/logs" element={<Logs />} />
        </Routes>
      </main>
    </div>
  );
}
