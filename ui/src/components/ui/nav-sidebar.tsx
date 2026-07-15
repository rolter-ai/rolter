import { ChevronsUpDown } from "lucide-react";
import * as React from "react";

import { cn } from "@/lib/utils";

// left rail: nav groups at the top, org/user block pinned to the bottom. the
// active item carries the folk-red вышивка thread on its left edge. mirrors the
// Rolter Design System navigation/NavSidebar.
export interface NavItem {
  key: string;
  label: React.ReactNode;
  icon?: React.ReactNode;
  count?: number;
}

export interface NavGroup {
  label?: string;
  items: NavItem[];
}

export interface NavUser {
  name: React.ReactNode;
  role?: React.ReactNode;
  initials: React.ReactNode;
  onClick?: () => void;
}

export interface NavSidebarProps extends React.HTMLAttributes<HTMLElement> {
  brand?: React.ReactNode;
  logoSrc?: string;
  groups: NavGroup[];
  activeKey?: string;
  onNavigate?: (key: string) => void;
  user?: NavUser;
}

export function NavSidebar({
  brand = "rolter",
  logoSrc,
  groups = [],
  activeKey,
  onNavigate,
  user,
  className,
  ...props
}: NavSidebarProps) {
  return (
    <nav
      className={cn(
        "flex h-full w-[var(--sidebar-width)] flex-col gap-3 border-r border-[color:var(--border-subtle)] bg-[color:var(--surface-app)] px-2 py-3",
        className,
      )}
      {...props}
    >
      <div className="flex items-center gap-2 px-2 py-1.5">
        {logoSrc ? <img className="block h-6 w-6 flex-none" src={logoSrc} alt="" /> : null}
        <span className="font-mono text-lg font-semibold leading-none tracking-[-0.03em] text-foreground">
          {brand}
          <span className="text-[color:var(--red-folk)]">.</span>
        </span>
      </div>

      <div className="flex flex-1 flex-col gap-4 overflow-y-auto">
        {groups.map((g, gi) => (
          <div className="flex flex-col gap-0.5" key={g.label || gi}>
            {g.label && (
              <div className="px-2 py-1.5 text-[0.6875rem] uppercase tracking-[0.08em] text-[color:var(--text-subtle)]">
                {g.label}
              </div>
            )}
            {g.items.map((it) => {
              const active = it.key === activeKey;
              return (
                <button
                  key={it.key}
                  aria-current={active ? "page" : undefined}
                  onClick={() => onNavigate?.(it.key)}
                  className={cn(
                    "relative flex w-full items-center gap-2 rounded-md border-none bg-transparent px-2 py-1.5 text-left text-sm transition-colors",
                    "[&>svg]:h-4 [&>svg]:w-4 [&>svg]:flex-none",
                    active
                      ? "bg-[color:var(--surface-subtle)] text-foreground before:absolute before:-left-px before:top-1/2 before:h-4 before:w-[3px] before:-translate-y-1/2 before:rounded-full before:bg-[color:var(--red-folk)] before:content-['']"
                      : "text-muted-foreground hover:bg-muted hover:text-foreground",
                  )}
                >
                  {it.icon}
                  <span>{it.label}</span>
                  {it.count != null && (
                    <span className="ml-auto font-mono text-[0.6875rem] text-[color:var(--text-subtle)]">
                      {it.count}
                    </span>
                  )}
                </button>
              );
            })}
          </div>
        ))}
      </div>

      {user && (
        <div className="mt-auto flex flex-col gap-1.5">
          <button
            onClick={user.onClick}
            className="flex items-center gap-2 rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-base)] px-2 py-1.5 transition-colors hover:border-[color:var(--border-default)]"
          >
            <span className="flex h-[26px] w-[26px] flex-none items-center justify-center rounded-full bg-[color:var(--red-folk)] text-xs font-semibold text-white">
              {user.initials}
            </span>
            <span className="flex min-w-0 flex-col">
              <span className="overflow-hidden text-ellipsis whitespace-nowrap text-xs font-medium text-foreground">
                {user.name}
              </span>
              {user.role && (
                <span className="overflow-hidden text-ellipsis whitespace-nowrap text-[0.6875rem] text-muted-foreground">
                  {user.role}
                </span>
              )}
            </span>
            <ChevronsUpDown className="ml-auto h-3.5 w-3.5 text-[color:var(--text-subtle)]" />
          </button>
        </div>
      )}
    </nav>
  );
}
