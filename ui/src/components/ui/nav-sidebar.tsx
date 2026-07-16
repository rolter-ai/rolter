import { ChevronDown, ChevronsUpDown, PanelLeftClose, PanelLeftOpen, Search, X } from "lucide-react";
import * as React from "react";

import { cn } from "@/lib/utils";

// left rail: brand + collapse toggle, nav search, nav groups (flat items or
// collapsible parents with sub-items), footer links + version, org/user block
// pinned to the bottom. collapses to an icon-only rail. the active item carries
// the folk-red вышивка thread on its left edge. mirrors the Rolter Design
// System navigation/NavSidebar.
export interface NavItem {
  key: string;
  label: string;
  icon?: React.ReactNode;
  count?: number;
  children?: NavItem[];
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

export interface NavFooterLink {
  key: string;
  icon: React.ReactNode;
  title: string;
  onClick?: () => void;
  href?: string;
}

export interface NavSidebarProps extends React.HTMLAttributes<HTMLElement> {
  brand?: React.ReactNode;
  logoSrc?: string;
  groups: NavGroup[];
  activeKey?: string;
  onNavigate?: (key: string) => void;
  user?: NavUser;
  /* bifrost-style extras — all optional so existing call sites keep working */
  searchable?: boolean;
  collapsible?: boolean;
  defaultCollapsed?: boolean;
  footerLinks?: NavFooterLink[];
  version?: string;
}

const itemBase =
  "relative flex w-full items-center gap-2 rounded-md border-none bg-transparent px-2 py-1.5 text-left text-sm transition-colors [&>svg]:h-4 [&>svg]:w-4 [&>svg]:flex-none";
const itemIdle = "text-muted-foreground hover:bg-muted hover:text-foreground";
const itemActive =
  "bg-[color:var(--surface-subtle)] text-foreground before:absolute before:-left-px before:top-1/2 before:h-4 before:w-[3px] before:-translate-y-1/2 before:rounded-full before:bg-[color:var(--red-folk)] before:content-['']";

function matches(it: NavItem, q: string): boolean {
  if (it.label.toLowerCase().includes(q)) return true;
  return (it.children ?? []).some((c) => matches(c, q));
}

export function NavSidebar({
  brand = "rolter",
  logoSrc,
  groups = [],
  activeKey,
  onNavigate,
  user,
  searchable,
  collapsible,
  defaultCollapsed,
  footerLinks,
  version,
  className,
  ...props
}: NavSidebarProps) {
  const [collapsed, setCollapsed] = React.useState(defaultCollapsed ?? false);
  const [query, setQuery] = React.useState("");
  // parents stay open once toggled; the one holding the active child opens itself
  const [open, setOpen] = React.useState<Record<string, boolean>>({});

  // the search box is hidden while collapsed, so the filter must not apply
  const q = collapsed ? "" : query.trim().toLowerCase();

  const isOpen = (it: NavItem) =>
    q !== "" ||
    (open[it.key] ?? (it.children ?? []).some((c) => c.key === activeKey));

  const renderItem = (it: NavItem, depth: number) => {
    if (q && !matches(it, q)) return null;
    const hasKids = (it.children?.length ?? 0) > 0;
    const active = it.key === activeKey;
    const expanded = hasKids && isOpen(it);
    return (
      <React.Fragment key={it.key}>
        <button
          aria-current={active ? "page" : undefined}
          aria-expanded={hasKids ? expanded : undefined}
          title={collapsed ? it.label : undefined}
          onClick={() =>
            hasKids
              ? setOpen((o) => ({ ...o, [it.key]: !isOpen(it) }))
              : onNavigate?.(it.key)
          }
          className={cn(
            itemBase,
            active ? itemActive : itemIdle,
            collapsed && "justify-center px-0",
          )}
        >
          {it.icon}
          {!collapsed && <span className="min-w-0 truncate">{it.label}</span>}
          {!collapsed && it.count != null && (
            <span className="ml-auto font-mono text-[0.6875rem] text-[color:var(--text-subtle)]">
              {it.count}
            </span>
          )}
          {!collapsed && hasKids && (
            <ChevronDown
              className={cn(
                "!ml-auto !h-3.5 !w-3.5 text-[color:var(--text-subtle)] transition-transform",
                !expanded && "-rotate-90",
              )}
            />
          )}
        </button>
        {expanded && !collapsed && (
          <div className="ml-[15px] flex flex-col gap-0.5 border-l border-[color:var(--border-subtle)] pl-1.5">
            {it.children!.map((c) => renderItem(c, depth + 1))}
          </div>
        )}
      </React.Fragment>
    );
  };

  return (
    <nav
      className={cn(
        "flex h-full flex-col gap-3 border-r border-[color:var(--border-subtle)] bg-[color:var(--surface-app)] px-2 py-3 transition-[width]",
        collapsed ? "w-[52px] items-stretch" : "w-[var(--sidebar-width)]",
        className,
      )}
      {...props}
    >
      <div className={cn("flex items-center gap-2 px-2 py-1.5", collapsed && "justify-center px-0")}>
        {logoSrc ? <img className="block h-6 w-6 flex-none" src={logoSrc} alt="" /> : null}
        {!collapsed && (
          <span className="font-mono text-lg font-semibold leading-none tracking-[-0.03em] text-foreground">
            {brand}
            <span className="text-[color:var(--red-folk)]">.</span>
          </span>
        )}
        {collapsible && !collapsed && (
          <button
            title="Collapse sidebar"
            onClick={() => setCollapsed(true)}
            className="ml-auto rounded-md p-1 text-[color:var(--text-subtle)] transition-colors hover:bg-muted hover:text-foreground"
          >
            <PanelLeftClose className="h-4 w-4" />
          </button>
        )}
      </div>

      {searchable && !collapsed && (
        <label className="flex items-center gap-2 rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-base)] px-2 py-1.5 transition-colors focus-within:border-[color:var(--border-default)]">
          <Search className="h-3.5 w-3.5 flex-none text-[color:var(--text-subtle)]" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search…"
            aria-label="Search navigation"
            className="w-full min-w-0 bg-transparent text-sm text-foreground outline-none placeholder:text-[color:var(--text-subtle)]"
          />
          {query && (
            <button
              onClick={() => setQuery("")}
              title="Clear search"
              className="-my-1 -mr-1 rounded p-1 text-[color:var(--text-subtle)] transition-colors hover:bg-muted hover:text-foreground"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          )}
        </label>
      )}

      <div className="flex flex-1 flex-col gap-4 overflow-y-auto">
        {groups.map((g, gi) => {
          const items = g.items.map((it) => renderItem(it, 0)).filter(Boolean);
          if (q && items.length === 0) return null;
          return (
            <div className="flex flex-col gap-0.5" key={g.label || gi}>
              {g.label && !collapsed && (
                <div className="px-2 py-1.5 text-[0.6875rem] uppercase tracking-[0.08em] text-[color:var(--text-subtle)]">
                  {g.label}
                </div>
              )}
              {items}
            </div>
          );
        })}
      </div>

      {(footerLinks?.length || version || (collapsible && collapsed)) && (
        <div className={cn("flex flex-col gap-1.5", collapsed && "items-center")}>
          <div className={cn("flex items-center gap-1 px-1", collapsed && "flex-col px-0")}>
            {footerLinks?.map((l) =>
              l.href ? (
                <a
                  key={l.key}
                  href={l.href}
                  target="_blank"
                  rel="noreferrer"
                  title={l.title}
                  className="rounded-md p-1.5 text-[color:var(--text-subtle)] transition-colors hover:bg-muted hover:text-foreground [&>svg]:h-4 [&>svg]:w-4"
                >
                  {l.icon}
                </a>
              ) : (
                <button
                  key={l.key}
                  onClick={l.onClick}
                  title={l.title}
                  className="rounded-md p-1.5 text-[color:var(--text-subtle)] transition-colors hover:bg-muted hover:text-foreground [&>svg]:h-4 [&>svg]:w-4"
                >
                  {l.icon}
                </button>
              ),
            )}
            {collapsible && collapsed && (
              <button
                title="Expand sidebar"
                onClick={() => setCollapsed(false)}
                className="rounded-md p-1.5 text-[color:var(--text-subtle)] transition-colors hover:bg-muted hover:text-foreground"
              >
                <PanelLeftOpen className="h-4 w-4" />
              </button>
            )}
            {version && !collapsed && (
              <span className="ml-auto pr-1 font-mono text-[0.6875rem] text-[color:var(--text-subtle)]">
                {version}
              </span>
            )}
          </div>
        </div>
      )}

      {user && (
        <div className={cn("mt-auto flex flex-col gap-1.5", collapsed && "items-center")}>
          <button
            onClick={user.onClick}
            title={collapsed && typeof user.name === "string" ? user.name : undefined}
            className={cn(
              "flex items-center gap-2 rounded-md transition-colors",
              collapsed
                ? "justify-center p-0.5 hover:bg-muted"
                : "border border-[color:var(--border-subtle)] bg-[color:var(--surface-base)] px-2 py-1.5 hover:border-[color:var(--border-default)]",
            )}
          >
            <span className="flex h-[26px] w-[26px] flex-none items-center justify-center rounded-full bg-[color:var(--red-folk)] text-xs font-semibold text-white">
              {user.initials}
            </span>
            {!collapsed && (
              <>
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
              </>
            )}
          </button>
        </div>
      )}
    </nav>
  );
}
