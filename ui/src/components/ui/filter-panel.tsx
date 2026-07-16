import { Check, ChevronDown, PanelLeftClose, Search } from "lucide-react";
import * as React from "react";

import { cn } from "@/lib/utils";

// secondary filter rail: a stack of collapsible sections next to the nav
// sidebar. sections hold check rows or arbitrary content; checked state is
// controlled by the caller. mirrors the Rolter Design System
// navigation/FilterPanel.
export interface FilterOption {
  value: string;
  label: React.ReactNode;
}

export interface FilterSectionProps {
  title: string;
  defaultOpen?: boolean;
  count?: number;
  children: React.ReactNode;
}

export function FilterSection({ title, defaultOpen, count, children }: FilterSectionProps) {
  const [open, setOpen] = React.useState(defaultOpen ?? false);
  return (
    <div className="flex flex-col">
      <button
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm font-medium text-foreground transition-colors hover:bg-muted"
      >
        <ChevronDown
          className={cn(
            "h-3.5 w-3.5 flex-none text-[color:var(--text-subtle)] transition-transform",
            !open && "-rotate-90",
          )}
        />
        <span className="min-w-0 truncate">{title}</span>
        {count != null && count > 0 && (
          <span className="ml-auto rounded-full bg-[color:var(--red-tint)] px-1.5 font-mono text-[0.6875rem] text-[color:var(--red-500)]">
            {count}
          </span>
        )}
      </button>
      {open && <div className="flex flex-col gap-1 px-2 pb-2 pt-1">{children}</div>}
    </div>
  );
}

export interface FilterCheckListProps {
  options: FilterOption[];
  selected: string[];
  onChange: (selected: string[]) => void;
}

export function FilterCheckList({ options, selected, onChange }: FilterCheckListProps) {
  const toggle = (v: string) =>
    onChange(
      selected.includes(v) ? selected.filter((s) => s !== v) : [...selected, v],
    );
  return (
    <div className="flex flex-col overflow-hidden rounded-md border border-[color:var(--border-subtle)]">
      {options.map((o, i) => {
        const checked = selected.includes(o.value);
        return (
          <button
            key={o.value}
            role="checkbox"
            aria-checked={checked}
            onClick={() => toggle(o.value)}
            className={cn(
              "flex items-center gap-2 bg-[color:var(--surface-base)] px-2.5 py-2 text-left text-sm transition-colors hover:bg-muted",
              i > 0 && "border-t border-[color:var(--border-subtle)]",
              checked ? "text-foreground" : "text-muted-foreground",
            )}
          >
            <span
              className={cn(
                "flex h-4 w-4 flex-none items-center justify-center rounded-[4px] border transition-colors",
                checked
                  ? "border-[color:var(--red-folk)] bg-[color:var(--red-folk)] text-white"
                  : "border-[color:var(--border-default)] bg-transparent",
              )}
            >
              {checked && <Check className="h-3 w-3" />}
            </span>
            <span className="min-w-0 truncate">{o.label}</span>
          </button>
        );
      })}
    </div>
  );
}

export interface FilterSearchListProps {
  options: FilterOption[];
  selected: string[];
  onChange: (selected: string[]) => void;
  placeholder?: string;
}

export function FilterSearchList({
  options,
  selected,
  onChange,
  placeholder = "Search…",
}: FilterSearchListProps) {
  const [query, setQuery] = React.useState("");
  const q = query.trim().toLowerCase();
  const shown = q
    ? options.filter((o) => String(o.label).toLowerCase().includes(q))
    : options;
  return (
    <div className="flex flex-col overflow-hidden rounded-md border border-[color:var(--border-subtle)]">
      <label className="flex items-center gap-2 bg-[color:var(--surface-base)] px-2.5 py-2">
        <Search className="h-3.5 w-3.5 flex-none text-[color:var(--text-subtle)]" />
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={placeholder}
          className="w-full min-w-0 bg-transparent text-sm text-foreground outline-none placeholder:text-[color:var(--text-subtle)]"
        />
      </label>
      {shown.length === 0 ? (
        <div className="border-t border-[color:var(--border-subtle)] bg-[color:var(--surface-base)] px-2.5 py-2 text-sm text-[color:var(--text-subtle)]">
          No results
        </div>
      ) : (
        <FilterCheckListInner options={shown} selected={selected} onChange={onChange} />
      )}
    </div>
  );
}

// rows without the outer border so they stack under the search input
function FilterCheckListInner({ options, selected, onChange }: FilterCheckListProps) {
  const toggle = (v: string) =>
    onChange(
      selected.includes(v) ? selected.filter((s) => s !== v) : [...selected, v],
    );
  return (
    <>
      {options.map((o) => {
        const checked = selected.includes(o.value);
        return (
          <button
            key={o.value}
            role="checkbox"
            aria-checked={checked}
            onClick={() => toggle(o.value)}
            className={cn(
              "flex items-center gap-2 border-t border-[color:var(--border-subtle)] bg-[color:var(--surface-base)] px-2.5 py-2 text-left text-sm transition-colors hover:bg-muted",
              checked ? "text-foreground" : "text-muted-foreground",
            )}
          >
            <span
              className={cn(
                "flex h-4 w-4 flex-none items-center justify-center rounded-[4px] border transition-colors",
                checked
                  ? "border-[color:var(--red-folk)] bg-[color:var(--red-folk)] text-white"
                  : "border-[color:var(--border-default)] bg-transparent",
              )}
            >
              {checked && <Check className="h-3 w-3" />}
            </span>
            <span className="min-w-0 truncate">{o.label}</span>
          </button>
        );
      })}
    </>
  );
}

export interface FilterPanelProps extends React.HTMLAttributes<HTMLElement> {
  title?: string;
  onHide?: () => void;
  children: React.ReactNode;
}

export function FilterPanel({
  title = "Filters",
  onHide,
  children,
  className,
  ...props
}: FilterPanelProps) {
  return (
    <aside
      className={cn(
        "flex h-full w-[248px] flex-col gap-1 overflow-y-auto border-r border-[color:var(--border-subtle)] bg-[color:var(--surface-app)] px-2 py-3",
        className,
      )}
      {...props}
    >
      <div className="flex items-center justify-between px-2 pb-1.5">
        <span className="text-sm font-semibold text-foreground">{title}</span>
        {onHide && (
          <button
            title="Hide filters"
            onClick={onHide}
            className="rounded-md p-1 text-[color:var(--text-subtle)] transition-colors hover:bg-muted hover:text-foreground"
          >
            <PanelLeftClose className="h-4 w-4" />
          </button>
        )}
      </div>
      {children}
    </aside>
  );
}
