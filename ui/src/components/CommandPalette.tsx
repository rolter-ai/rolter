import { useQuery } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import {
  Dialog,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import { fetchProviders, fetchRoutes, fetchVirtualKeys } from "@/lib/api";
import { useCan } from "@/lib/can";
import { rankEntries, type PaletteEntry } from "@/lib/command-palette";
import { leafKeys, type NavDef } from "@/lib/nav";
import { useScope } from "@/lib/scope";
import { cn } from "@/lib/utils";

// ⌘K: jump to any screen, or to the record you half-remember the name of
// (#1198).
//
// Not the `Combobox` primitive: that is a *value picker* — a labelled trigger
// that opens a list and writes the pick back into a form. This picks nothing
// and edits nothing; it navigates, and it is the only control on screen while
// it is up. It owes the same roles and the same keyboard all the same, so the
// input is a `combobox` over a `listbox` of `option`s driven by
// `aria-activedescendant`: focus stays in the field the reader is typing in
// while the arrow keys move the selection, which is the one pattern a screen
// reader announces correctly.
//
// The record half is deliberately the cheap half. Three lists the dashboard
// already reads elsewhere — virtual keys, providers, routes — fetched only
// once the palette is open and only for the caller allowed to read them. It is
// not a search endpoint and does not pretend to be one: no log, no invocation,
// nothing that would need a query the control plane does not have.

/** how many records of one kind the palette offers before it stops listing */
const RECORDS_PER_KIND = 6;

export interface CommandPaletteProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** the nav as this caller may see it — `visibleNav(can)`, not raw `NAV` */
  nav: NavDef[];
  /** screen keys this browser visited last, most recent first */
  recent?: string[];
  onNavigate: (screen: string) => void;
}

interface Section {
  key: "recent" | "screens" | "records";
  label: string;
  entries: PaletteEntry[];
}

export function CommandPalette({
  open,
  onOpenChange,
  nav,
  recent = [],
  onNavigate,
}: CommandPaletteProps) {
  const { t } = useTranslation();
  const can = useCan();
  const scope = useScope();
  const [query, setQuery] = React.useState("");
  const [active, setActive] = React.useState(0);
  const listId = React.useId();

  // a palette that reopened holding the last query would answer a question
  // nobody asked this time
  React.useEffect(() => {
    if (!open) {
      setQuery("");
      setActive(0);
    }
  }, [open]);

  // `false` is the only answer that hides anything, the same rule the rail
  // follows: while the capability query is still out the list is fetched, and
  // a 403 simply leaves that section empty
  const mayRead = (resource: string) => open && can(resource, "read") !== false;
  const orgId = scope.orgId;
  const projectId = scope.projectId;

  const providers = useQuery({
    queryKey: ["palette", "providers", orgId],
    queryFn: () => fetchProviders(orgId!),
    enabled: !!orgId && mayRead("provider"),
  });
  const routes = useQuery({
    queryKey: ["palette", "routes", projectId],
    queryFn: () => fetchRoutes(projectId!),
    enabled: !!projectId && mayRead("route"),
  });
  const keys = useQuery({
    queryKey: ["palette", "virtual-keys", projectId],
    queryFn: () => fetchVirtualKeys(projectId!),
    enabled: !!projectId && mayRead("virtual_key"),
  });

  const recordQueries = [providers, routes, keys];
  const recordsPending = recordQueries.some((q) => q.isFetching);
  const recordsError = recordQueries.find((q) => q.error)?.error;
  const retryRecords = () => {
    for (const q of recordQueries) if (q.error) void q.refetch();
  };

  // every navigable leaf, named by the catalog and hinted with the group it
  // sits under, so "Settings" reaches the seven screens filed beneath it
  const screens = React.useMemo<PaletteEntry[]>(() => {
    const out: PaletteEntry[] = [];
    const walk = (defs: NavDef[], parent?: string) => {
      for (const def of defs) {
        if (def.children) walk(def.children, t(`nav.${def.key}`));
        else out.push({ id: `screen:${def.key}`, screen: def.key, label: t(`nav.${def.key}`), hint: parent });
      }
    };
    walk(nav);
    return out;
  }, [nav, t]);

  const records = React.useMemo<PaletteEntry[]>(() => {
    const out: PaletteEntry[] = [];
    for (const key of (keys.data ?? []).slice(0, RECORDS_PER_KIND)) {
      out.push({
        id: `key:${key.id}`,
        screen: "virtual-keys",
        label: key.name?.trim() || key.key_prefix,
        hint: t("shell.palette.kinds.virtualKey"),
      });
    }
    for (const provider of (providers.data ?? []).slice(0, RECORDS_PER_KIND)) {
      out.push({
        id: `provider:${provider.id}`,
        screen: "providers",
        label: provider.name,
        hint: t("shell.palette.kinds.provider"),
      });
    }
    for (const route of (routes.data ?? []).slice(0, RECORDS_PER_KIND)) {
      out.push({
        id: `route:${route.id}`,
        screen: "routing-rules",
        label: route.model,
        hint: t("shell.palette.kinds.route"),
      });
    }
    // a record on a screen this caller cannot open is a dead end
    const reachable = new Set(leafKeys(nav));
    return out.filter((entry) => reachable.has(entry.screen));
  }, [keys.data, providers.data, routes.data, nav, t]);

  const sections = React.useMemo<Section[]>(() => {
    const typed = query.trim() !== "";
    const out: Section[] = [];
    if (!typed) {
      const byKey = new Map(screens.map((s) => [s.screen, s]));
      const seen = recent
        .map((key) => byKey.get(key))
        .filter((s): s is PaletteEntry => s !== undefined)
        .map((s) => ({ ...s, id: `recent:${s.screen}` }));
      if (seen.length) out.push({ key: "recent", label: t("shell.palette.sections.recent"), entries: seen });
    }
    const matchedScreens = rankEntries(screens, query);
    if (matchedScreens.length) {
      out.push({ key: "screens", label: t("shell.palette.sections.screens"), entries: matchedScreens });
    }
    // records only once there is something to match them against: the whole
    // list of every key, provider and route is not a useful thing to open onto
    const matchedRecords = typed ? rankEntries(records, query) : [];
    if (matchedRecords.length || (typed && (recordsPending || recordsError))) {
      out.push({ key: "records", label: t("shell.palette.sections.records"), entries: matchedRecords });
    }
    return out;
  }, [query, screens, records, recent, recordsPending, recordsError, t]);

  const flat = React.useMemo(() => sections.flatMap((s) => s.entries), [sections]);

  // the selection follows the results: a narrowing query leaves the old index
  // pointing past the end, and the palette would then open nothing on Enter
  React.useEffect(() => {
    setActive((at) => (at < flat.length ? at : 0));
  }, [flat.length]);

  const activeEntry = flat[active];
  const optionId = (entry: PaletteEntry) => `${listId}-${entry.id}`;

  const go = (entry: PaletteEntry | undefined) => {
    if (!entry) return;
    onNavigate(entry.screen);
    onOpenChange(false);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (flat.length === 0) return;
    if (e.key === "ArrowDown") setActive((at) => (at + 1) % flat.length);
    else if (e.key === "ArrowUp") setActive((at) => (at - 1 + flat.length) % flat.length);
    else if (e.key === "Home") setActive(0);
    else if (e.key === "End") setActive(flat.length - 1);
    else if (e.key === "Enter") go(activeEntry);
    else return;
    // Escape is the dialog's, and arrows inside a text field would otherwise
    // move the caret instead of the selection
    e.preventDefault();
  };

  const typed = query.trim() !== "";
  const nothing = flat.length === 0 && !recordsPending && !recordsError;
  // a `listbox` role with no `option` inside it is a broken widget, not an
  // empty one: while the palette is showing a skeleton, a failure or "nothing
  // matches", the container is a plain box and the field says it is not
  // expanded
  const expanded = flat.length > 0;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>{t("shell.palette.title")}</DialogTitle>
        <DialogDescription>{t("shell.palette.description")}</DialogDescription>
      </DialogHeader>
      <input
        role="combobox"
        aria-expanded={expanded}
        aria-controls={listId}
        aria-autocomplete="list"
        aria-activedescendant={activeEntry ? optionId(activeEntry) : undefined}
        aria-label={t("shell.palette.label")}
        placeholder={t("shell.palette.placeholder")}
        value={query}
        onChange={(e) => {
          setQuery(e.target.value);
          setActive(0);
        }}
        onKeyDown={onKeyDown}
        className="w-full rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-base)] px-3 py-2 text-sm text-foreground outline-none transition-colors placeholder:text-[color:var(--text-subtle)] focus-visible:border-[color:var(--border-default)] focus-visible:ring-1 focus-visible:ring-ring"
      />
      {/* a live count, so a screen reader hears the list narrow while the
          caller keeps typing rather than only on arrow-down */}
      <p className="sr-only" role="status">
        {t("shell.palette.results", { count: flat.length })}
      </p>
      <div
        id={listId}
        role={expanded ? "listbox" : undefined}
        aria-label={expanded ? t("shell.palette.label") : undefined}
        className="mt-3 max-h-[min(60vh,360px)] overflow-y-auto"
      >
        {sections.map((section) => (
          <div key={section.key} role="group" aria-label={section.label}>
            <p className="px-1 py-1.5 text-[0.6875rem] uppercase tracking-[0.08em] text-[color:var(--text-subtle)]">
              {section.label}
            </p>
            {section.entries.map((entry) => {
              const selected = entry.id === activeEntry?.id;
              return (
                <div
                  key={entry.id}
                  id={optionId(entry)}
                  role="option"
                  aria-selected={selected}
                  onMouseEnter={() => setActive(flat.findIndex((e) => e.id === entry.id))}
                  onClick={() => go(entry)}
                  className={cn(
                    "flex cursor-pointer items-baseline gap-2 rounded-md px-2 py-1.5 text-sm",
                    selected
                      ? "bg-[color:var(--surface-subtle)] text-foreground"
                      : "text-muted-foreground",
                  )}
                >
                  <span className="min-w-0 flex-1 truncate">{entry.label}</span>
                  {entry.hint && (
                    <span className="flex-none text-[0.6875rem] text-[color:var(--text-subtle)]">
                      {entry.hint}
                    </span>
                  )}
                </div>
              );
            })}
            {section.key === "records" && recordsError && (
              <LoadError
                error={recordsError}
                resource={t("errors.resources.paletteRecords")}
                onRetry={retryRecords}
              />
            )}
            {section.key === "records" && !recordsError && recordsPending && (
              <ListSkeleton rows={2} className="px-1 py-1" />
            )}
          </div>
        ))}
        {nothing && (
          <EmptyState
            uxTarget="command-palette"
            title={t("shell.palette.noMatches")}
            description={
              typed
                ? t("shell.palette.noMatchesBody", { query: query.trim() })
                : undefined
            }
          />
        )}
      </div>
    </Dialog>
  );
}
