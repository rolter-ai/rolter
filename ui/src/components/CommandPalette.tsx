import { useQuery } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { Dialog, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import { KbdChord } from "@/components/ui/kbd";
import { fetchProviders, fetchRoutes, fetchVirtualKeys } from "@/lib/api";
import { useCan } from "@/lib/can";
import { pastedIdLookup, rankEntries, type PaletteEntry } from "@/lib/command-palette";
import { logLookupSearch } from "@/lib/log-lookup";
import { shortcutChord } from "@/lib/shortcuts";
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
// not a search endpoint and does not pretend to be one: no log, no invocation.
// A pasted request id or trace id is the one exception, and it searches
// nothing here either: it offers to open LLM Logs on that id, which is where
// the control plane answers it (#1861).

/** how many records of one kind the palette offers before it stops listing */
const RECORDS_PER_KIND = 6;

const SECTION_HEADING =
  "px-1 py-1.5 text-[0.6875rem] uppercase tracking-[0.08em] text-[color:var(--text-subtle)]";

export interface CommandPaletteProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** the nav as this caller may see it — `visibleNav(can)`, not raw `NAV` */
  nav: NavDef[];
  /** screen keys this browser visited last, most recent first */
  recent?: string[];
  /** `search` is the query string the screen opens with, when an entry names one */
  onNavigate: (screen: string, search?: string) => void;
}

interface Section {
  key: "recent" | "screens" | "lookup" | "records";
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
  const recordsSettling = recordsPending || scope.isLoading;
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
        else
          out.push({
            id: `screen:${def.key}`,
            screen: def.key,
            label: t(`nav.${def.key}`),
            hint: parent,
          });
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

  // LLM Logs is where an id is answered, so a caller who cannot open it is not
  // offered the lookup either
  const canOpenLogs = React.useMemo(() => leafKeys(nav).includes("logs"), [nav]);

  const { sections, holding } = React.useMemo<{ sections: Section[]; holding: boolean }>(() => {
    const typed = query.trim() !== "";
    const out: Section[] = [];
    if (!typed) {
      const byKey = new Map(screens.map((s) => [s.screen, s]));
      const seen = recent
        .map((key) => byKey.get(key))
        .filter((s): s is PaletteEntry => s !== undefined)
        .map((s) => ({ ...s, id: `recent:${s.screen}` }));
      if (seen.length)
        out.push({ key: "recent", label: t("shell.palette.sections.recent"), entries: seen });
    }
    const matchedScreens = rankEntries(screens, query);
    if (matchedScreens.length) {
      out.push({
        key: "screens",
        label: t("shell.palette.sections.screens"),
        entries: matchedScreens,
      });
    }
    // records only once there is something to match them against: the whole
    // list of every key, provider and route is not a useful thing to open onto
    const matchedRecords = typed ? rankEntries(records, query) : [];
    // a pasted id is offered when no name matched it, never ahead of one: a
    // route called `gpt-4o-mini` has a digit and eleven characters too, and
    // Enter on it must still open Routing Rules
    const unmatched = canOpenLogs && !matchedScreens.length && !matchedRecords.length;
    const pasted = unmatched ? pastedIdLookup(query) : null;
    // a request-id-shaped word waits for the record lists, which may hold the
    // name it is: while the scope that enables them is resolving they are not
    // fetching yet, so that counts as loading too, or the offer would show and
    // then vanish the moment they started. a trace id cannot be a name
    const holding = pasted?.kind === "request_id" && recordsSettling;
    const id = pasted && !holding ? pasted : null;
    if (id) {
      const isTrace = id.kind === "trace_id";
      out.push({
        key: "lookup",
        label: t("shell.palette.sections.lookup"),
        entries: [
          {
            id: `lookup:${id.kind}`,
            screen: "logs",
            search: logLookupSearch(id),
            label: t(isTrace ? "shell.palette.openTrace" : "shell.palette.openRequest", {
              id: id.value,
            }),
            hint: t(isTrace ? "shell.palette.kinds.traceId" : "shell.palette.kinds.requestId"),
            wrap: true,
          },
        ],
      });
    }
    if (matchedRecords.length) {
      out.push({
        key: "records",
        label: t("shell.palette.sections.records"),
        entries: matchedRecords,
      });
    }
    return { sections: out, holding };
  }, [query, screens, records, recent, recordsSettling, canOpenLogs, t]);

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
    onNavigate(entry.screen, entry.search);
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
  // a held request-id offer is not "nothing matches" either, only not yet
  const nothing = flat.length === 0 && !recordsPending && !recordsError && !holding;
  // the record lists' own state, still loading or failed, is said beside the
  // listbox rather than in it: a listbox holds options and nothing else, and a
  // skeleton or an alert inside one is what axe calls a broken widget
  const recordsNote = typed && (recordsPending || !!recordsError || holding);
  const recordsListed = sections.some((s) => s.key === "records");
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
      {/* the chord that opens this, printed where it is reachable: a reader
          who found the palette through the rail's footer link learns the
          keystroke from the thing it opens (#1676). the hint sits inside the
          field's box rather than beside it, so it survives the narrow shell,
          and the field is padded to its width so a long query never runs
          under it */}
      <div className="relative">
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
          className="w-full rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-base)] py-2 pl-3 pr-16 text-sm text-foreground outline-none transition-colors placeholder:text-[color:var(--text-subtle)] focus-visible:border-[color:var(--border-default)] focus-visible:ring-1 focus-visible:ring-ring"
        />
        <KbdChord
          chord={shortcutChord("palette")}
          className="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2"
        />
      </div>
      {/* a live count, so a screen reader hears the list narrow while the
          caller keeps typing rather than only on arrow-down */}
      <p className="sr-only" role="status">
        {t("shell.palette.results", { count: flat.length })}
      </p>
      <div className="mt-3">
        {/* the listbox is the scroll region, as it was before the records' own
            state moved out of it: axe lets a scrolling listbox stand on the
            field's aria-activedescendant, but not a plain box around one */}
        <div
          id={listId}
          role={expanded ? "listbox" : undefined}
          aria-label={expanded ? t("shell.palette.label") : undefined}
          className="max-h-[min(60vh,360px)] overflow-y-auto"
        >
          {sections.map((section) => (
            <div key={section.key} role="group" aria-label={section.label}>
              <p className={SECTION_HEADING}>{section.label}</p>
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
                    <span
                      className={cn(
                        "min-w-0 flex-1",
                        entry.wrap ? "[overflow-wrap:anywhere]" : "truncate",
                      )}
                    >
                      {entry.label}
                    </span>
                    {entry.hint && (
                      <span className="flex-none text-[0.6875rem] text-[color:var(--text-subtle)]">
                        {entry.hint}
                      </span>
                    )}
                  </div>
                );
              })}
            </div>
          ))}
        </div>
        {recordsNote && (
          <div
            role={recordsListed ? undefined : "group"}
            aria-label={recordsListed ? undefined : t("shell.palette.sections.records")}
          >
            {!recordsListed && (
              <p className={SECTION_HEADING}>{t("shell.palette.sections.records")}</p>
            )}
            {recordsError ? (
              <LoadError
                error={recordsError}
                resource={t("errors.resources.paletteRecords")}
                onRetry={retryRecords}
                target="command-palette"
              />
            ) : (
              <ListSkeleton rows={2} className="px-1 py-1" />
            )}
          </div>
        )}
        {nothing && (
          <EmptyState
            uxTarget="command-palette"
            title={t("shell.palette.noMatches")}
            description={
              typed ? t("shell.palette.noMatchesBody", { query: query.trim() }) : undefined
            }
          />
        )}
      </div>
    </Dialog>
  );
}
