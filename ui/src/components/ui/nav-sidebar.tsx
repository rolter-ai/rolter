import {
  ArrowUpCircle,
  ChevronDown,
  ChevronsUpDown,
  PanelLeftClose,
  PanelLeftOpen,
  Search,
  X,
} from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { useModalA11y } from "@/lib/modal-a11y";
import { BELOW_LG, BELOW_MD, useMediaQuery } from "@/lib/use-media-query";
import { cn } from "@/lib/utils";

// left rail: brand + collapse toggle, nav search, nav groups (flat items or
// collapsible parents with sub-items), footer links + version, org/user block
// pinned to the bottom. collapses to an icon-only rail. the active item carries
// the folk-red вышивка thread on its left edge. mirrors the Rolter Design
// System navigation/NavSidebar.
//
// three shapes by viewport (#959) — below `md` an off-canvas drawer over a
// scrim, between `md` and `lg` an icon rail, at `lg` and up the full resizable
// rail. see docs/development/dashboard-navigation.md.
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

/* a newer stable release than the one running (#902). the footer shows it as
   a small link beside the version; the folded rail as an icon with a dot,
   named the same way. absent in every state with nothing to say — checking,
   disabled, offline, current */
export interface NavUpdateHint {
  latest: string;
  url: string;
}

export interface NavSidebarProps extends React.HTMLAttributes<HTMLElement> {
  brand?: React.ReactNode;
  logoSrc?: string;
  groups: NavGroup[];
  activeKey?: string;
  onNavigate?: (key: string) => void;
  user?: NavUser;
  /* when provided, the user block becomes a menu trigger: clicking it opens a
     popover above the block rendering this content (scope switcher, account,
     sign out). `close` dismisses the popover. takes precedence over
     user.onClick. */
  userMenu?: (close: () => void) => React.ReactNode;
  /* bifrost-style extras — all optional so existing call sites keep working */
  searchable?: boolean;
  collapsible?: boolean;
  defaultCollapsed?: boolean;
  footerLinks?: NavFooterLink[];
  /* rendered in the footer row after the links and before the version — the
     locale picker sits here, so it stays on screen whatever the active route.
     receives the rail state so it can drop its label when collapsed. */
  footerExtra?: (collapsed: boolean) => React.ReactNode;
  version?: string;
  update?: NavUpdateHint | null;
  /* when set, the right edge becomes a draggable, keyboard-operable splitter
     and the rail's width is remembered per browser under `storageKey`. the
     width is clamped to [NAV_MIN_WIDTH, NAV_MAX_WIDTH] on every path — drag,
     keyboard and the value read back from storage — so a stale or hand-edited
     entry cannot restore an unusable rail. */
  resizable?: boolean;
  storageKey?: string;
  /* below `md` the rail is an off-canvas drawer and this is whether it is
     showing. the trigger lives in the screen header, so the state has to be
     owned above both of them. ignored at `md` and up, where the rail is always
     on screen. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}

/** narrowest the rail may be dragged: still wide enough for icon + label */
export const NAV_MIN_WIDTH = 180;
/** widest: past this the rail competes with the content it navigates */
export const NAV_MAX_WIDTH = 420;
/** matches `--sidebar-width` in index.css */
export const NAV_DEFAULT_WIDTH = 232;
const NAV_WIDTH_STORAGE_KEY = "rolter.nav.width";
/** one arrow press; shift multiplies it so a keyboard user is not stuck stepping */
const NAV_KEY_STEP = 16;

const clampWidth = (w: number) =>
  Math.min(NAV_MAX_WIDTH, Math.max(NAV_MIN_WIDTH, Math.round(w)));

function readStoredWidth(key: string): number | null {
  // storage throws outright in some embedding contexts, and can hold anything
  try {
    const raw = window.localStorage.getItem(key);
    if (raw === null) return null;
    const n = Number(raw);
    return Number.isFinite(n) ? clampWidth(n) : null;
  } catch {
    return null;
  }
}

const itemBase =
  "relative flex w-full items-center gap-2 rounded-md border-none bg-transparent px-2 py-1.5 text-left text-sm transition-colors [&>svg]:h-4 [&>svg]:w-4 [&>svg]:flex-none focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";
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
  userMenu,
  searchable,
  collapsible,
  defaultCollapsed,
  footerLinks,
  footerExtra,
  version,
  update,
  resizable,
  storageKey = NAV_WIDTH_STORAGE_KEY,
  // `open` is taken by the expanded-parents map below, so the drawer's flag is
  // renamed rather than the state that predates it
  open: drawerRequested = false,
  onOpenChange,
  className,
  ...props
}: NavSidebarProps) {
  const { t } = useTranslation();
  const isDrawer = useMediaQuery(BELOW_MD);
  const isIconRail = useMediaQuery(BELOW_LG) && !isDrawer;
  const [collapsed, setCollapsed] = React.useState(defaultCollapsed ?? false);
  const [query, setQuery] = React.useState("");
  // parents stay open once toggled; the one holding the active child opens itself
  const [open, setOpen] = React.useState<Record<string, boolean>>({});
  const [userOpen, setUserOpen] = React.useState(false);
  const userRef = React.useRef<HTMLDivElement>(null);
  const navRef = React.useRef<HTMLElement>(null);
  const panelRef = React.useRef<HTMLDivElement>(null);
  const [width, setWidth] = React.useState(NAV_DEFAULT_WIDTH);
  const [dragging, setDragging] = React.useState(false);

  const closeDrawer = React.useCallback(() => onOpenChange?.(false), [onOpenChange]);

  // the drawer is a modal: it covers the screen it navigates, so it owes the
  // same focus trap, Escape and scroll lock a Sheet does (#1181). the user
  // menu opens *inside* it and owns Escape while it is up
  const drawerOpen = isDrawer && drawerRequested;
  const a11y = useModalA11y(panelRef, {
    open: drawerOpen,
    onEscape: () => {
      if (userOpen) return;
      closeDrawer();
    },
  });

  // between `md` and `lg` the rail starts as icons: a 232px rail is a quarter
  // of a 900px screen. the toggle still works, this only picks the starting
  // state each time the viewport crosses a breakpoint
  React.useEffect(() => {
    if (isDrawer) return;
    setCollapsed(isIconRail ? true : (defaultCollapsed ?? false));
  }, [isDrawer, isIconRail, defaultCollapsed]);

  // read the remembered width after mount rather than in the initializer: the
  // first paint then matches what a server-rendered or storage-less browser
  // draws, and the story harness gets a deterministic starting width
  React.useEffect(() => {
    if (!resizable) return;
    const stored = readStoredWidth(storageKey);
    if (stored !== null) setWidth(stored);
  }, [resizable, storageKey]);

  const commitWidth = React.useCallback(
    (next: number) => {
      const w = clampWidth(next);
      setWidth(w);
      try {
        window.localStorage.setItem(storageKey, String(w));
      } catch {
        // a browser that refuses storage still resizes, it just forgets
      }
    },
    [storageKey],
  );

  // the pointer leaves the 4px handle immediately on a fast drag, so the move
  // and up listeners live on the document for the duration of the gesture
  React.useEffect(() => {
    if (!dragging) return;
    const onMove = (e: PointerEvent) => {
      const left = navRef.current?.getBoundingClientRect().left ?? 0;
      setWidth(clampWidth(e.clientX - left));
    };
    const onUp = () => setDragging(false);
    document.addEventListener("pointermove", onMove);
    document.addEventListener("pointerup", onUp);
    document.addEventListener("pointercancel", onUp);
    return () => {
      document.removeEventListener("pointermove", onMove);
      document.removeEventListener("pointerup", onUp);
      document.removeEventListener("pointercancel", onUp);
    };
  }, [dragging]);

  // persist once the gesture ends, not on every pointermove
  const wasDragging = React.useRef(false);
  React.useEffect(() => {
    if (wasDragging.current && !dragging) commitWidth(width);
    wasDragging.current = dragging;
  }, [dragging, width, commitWidth]);

  const onHandleKeyDown = (e: React.KeyboardEvent) => {
    const step = e.shiftKey ? NAV_KEY_STEP * 4 : NAV_KEY_STEP;
    if (e.key === "ArrowLeft") commitWidth(width - step);
    else if (e.key === "ArrowRight") commitWidth(width + step);
    else if (e.key === "Home") commitWidth(NAV_MIN_WIDTH);
    else if (e.key === "End") commitWidth(NAV_MAX_WIDTH);
    else if (e.key === "Enter" || e.key === " ") commitWidth(NAV_DEFAULT_WIDTH);
    else return;
    e.preventDefault();
  };

  React.useEffect(() => {
    if (!userOpen) return;
    // the scope switcher's create/delete dialogs are portaled to the body, so
    // a click inside one lands "outside" the menu; treat any open *modal* as
    // part of the menu, or the first keystroke in the name field closes both.
    // the popover itself is a (non-modal) dialog, and Escape inside it must
    // still close it, hence the aria-modal test rather than the role
    const inDialog = (target: EventTarget | null) =>
      target instanceof Element && target.closest('[aria-modal="true"]') != null;
    const onDoc = (e: MouseEvent) => {
      if (inDialog(e.target)) return;
      if (userRef.current && !userRef.current.contains(e.target as Node)) setUserOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !inDialog(e.target)) setUserOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [userOpen]);

  // the drawer has room for labels whatever the rail was folded to before the
  // viewport shrank, so it always shows them
  const folded = collapsed && !isDrawer;
  // one accessible name for the hint in both shapes of the rail
  const updateHint = update
    ? t("shell.updateAvailableHint", { latest: update.latest })
    : undefined;

  // the splitter belongs to the `lg`-and-up rail: the icon rail has no edge
  // worth dragging and the drawer is sized by the viewport, not by the pointer
  const showHandle = Boolean(resizable) && !folded && !isDrawer && !isIconRail;

  // the search box is hidden while folded, so the filter must not apply
  const q = folded ? "" : query.trim().toLowerCase();

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
          title={folded ? it.label : undefined}
          onClick={() => {
            if (hasKids) {
              setOpen((o) => ({ ...o, [it.key]: !isOpen(it) }));
              return;
            }
            onNavigate?.(it.key);
            // the drawer covers the screen it just navigated to
            if (isDrawer) closeDrawer();
          }}
          className={cn(
            itemBase,
            active ? itemActive : itemIdle,
            folded && "justify-center px-0",
            // a touch target, not a pointer one, once the rail is a drawer
            isDrawer && "py-2.5",
          )}
        >
          {it.icon}
          {!folded && <span className="min-w-0 truncate">{it.label}</span>}
          {!folded && it.count != null && (
            <span className="ml-auto font-mono text-[0.6875rem] text-[color:var(--text-subtle)]">
              {it.count}
            </span>
          )}
          {!folded && hasKids && (
            <ChevronDown
              className={cn(
                "!ml-auto !h-3.5 !w-3.5 text-[color:var(--text-subtle)] transition-transform",
                !expanded && "-rotate-90",
              )}
            />
          )}
        </button>
        {expanded && !folded && (
          <div className="ml-[15px] flex flex-col gap-0.5 border-l border-[color:var(--border-subtle)] pl-1.5">
            {it.children!.map((c) => renderItem(c, depth + 1))}
          </div>
        )}
      </React.Fragment>
    );
  };

  // below `md` the rail is out of the flow entirely: a closed drawer takes no
  // width, which is the whole of #959 — the old rail kept its 232px and left a
  // 375px screen 143px to work in
  if (isDrawer && !drawerRequested) return null;

  const rail = (
    <nav
      ref={navRef}
      aria-label={t("shell.navLabel")}
      style={showHandle ? { width } : undefined}
      className={cn(
        "relative flex h-full flex-col gap-3 border-r border-[color:var(--border-subtle)] bg-[color:var(--surface-app)] px-2 py-3",
        // the width transition would fight the pointer during a drag
        !dragging && "transition-[width]",
        folded ? "w-[52px] items-stretch" : "w-[var(--sidebar-width)]",
        isDrawer && "w-full",
        className,
      )}
      {...props}
    >
      {showHandle && (
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label={t("shell.resizeSidebar")}
          aria-valuenow={width}
          aria-valuemin={NAV_MIN_WIDTH}
          aria-valuemax={NAV_MAX_WIDTH}
          tabIndex={0}
          onPointerDown={(e) => {
            e.preventDefault();
            setDragging(true);
          }}
          onDoubleClick={() => commitWidth(NAV_DEFAULT_WIDTH)}
          onKeyDown={onHandleKeyDown}
          className={cn(
            "absolute inset-y-0 -right-1 z-30 w-2 cursor-col-resize focus-visible:outline-none",
            // a 2px thread on the border line, painted only when the splitter
            // is grabbed or focused so the idle rail stays quiet
            "after:absolute after:inset-y-0 after:left-1/2 after:w-[2px] after:-translate-x-1/2 after:bg-[color:var(--red-folk)] after:opacity-0 after:transition-opacity after:content-['']",
            "hover:after:opacity-60 focus-visible:after:opacity-100",
            dragging && "after:opacity-100",
          )}
        />
      )}
      <div
        className={cn(
          "group relative flex items-center gap-2 px-2 py-1.5",
          folded && "justify-center px-0",
        )}
      >
        {logoSrc ? <img className="block h-6 w-6 flex-none" src={logoSrc} alt="" /> : null}
        {!folded && (
          <span className="font-mono text-lg font-semibold leading-none tracking-[-0.03em] text-foreground">
            {brand}
            <span className="text-[color:var(--red-folk-text)]">.</span>
          </span>
        )}
        {collapsible && !folded && !isDrawer && (
          <button
            title={t("shell.collapseSidebar")}
            aria-label={t("shell.collapseSidebar")}
            onClick={() => setCollapsed(true)}
            className="ml-auto rounded-md p-1 text-[color:var(--text-subtle)] transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          >
            <PanelLeftClose className="h-4 w-4" />
          </button>
        )}
        {/* folded: the expand toggle lives inside the logo/header area,
            hidden by default and revealed only on hover or keyboard focus of
            the header container. it overlays the centered logo so the rail
            stays icon-narrow. */}
        {collapsible && folded && !isDrawer && (
          <button
            title={t("shell.expandSidebar")}
            aria-label={t("shell.expandSidebar")}
            onClick={() => setCollapsed(false)}
            className="absolute inset-0 flex items-center justify-center rounded-md bg-[color:var(--surface-app)] text-[color:var(--text-subtle)] opacity-0 transition-opacity duration-200 hover:bg-muted hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          >
            <PanelLeftOpen className="h-4 w-4" />
          </button>
        )}
      </div>

      {/* the focus ring lives on the wrapper, not the input: the input is
          borderless inside a bordered box, so a ring on it would draw inside
          that box rather than around the control a keyboard user sees (#963) */}
      {searchable && !folded && (
        <label className="flex items-center gap-2 rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-base)] px-2 py-1.5 transition-colors focus-within:border-[color:var(--border-default)] focus-within:ring-1 focus-within:ring-ring">
          <Search className="h-3.5 w-3.5 flex-none text-[color:var(--text-subtle)]" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("common.search")}
            aria-label={t("shell.searchNav")}
            className="w-full min-w-0 bg-transparent text-sm text-foreground outline-none placeholder:text-[color:var(--text-subtle)]"
          />
          {query && (
            <button
              onClick={() => setQuery("")}
              title={t("common.clearSearch")}
              aria-label={t("common.clearSearch")}
              className="-my-1 -mr-1 rounded p-1 text-[color:var(--text-subtle)] transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
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
              {g.label && !folded && (
                <div className="px-2 py-1.5 text-[0.6875rem] uppercase tracking-[0.08em] text-[color:var(--text-subtle)]">
                  {g.label}
                </div>
              )}
              {items}
            </div>
          );
        })}
      </div>

      {(footerLinks?.length || footerExtra || version || update) && (
        <div className={cn("flex flex-col gap-1.5", folded && "items-center")}>
          <div className={cn("flex items-center gap-1 px-1", folded && "flex-col px-0")}>
            {footerLinks?.map((l) =>
              l.href ? (
                <a
                  key={l.key}
                  href={l.href}
                  target="_blank"
                  rel="noreferrer"
                  title={l.title}
                  aria-label={l.title}
                  className="rounded-md p-1.5 text-[color:var(--text-subtle)] transition-colors hover:bg-muted hover:text-foreground [&>svg]:h-4 [&>svg]:w-4 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                >
                  {l.icon}
                </a>
              ) : (
                <button
                  key={l.key}
                  onClick={l.onClick}
                  title={l.title}
                  aria-label={l.title}
                  className="rounded-md p-1.5 text-[color:var(--text-subtle)] transition-colors hover:bg-muted hover:text-foreground [&>svg]:h-4 [&>svg]:w-4 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                >
                  {l.icon}
                </button>
              ),
            )}
            {footerExtra?.(folded)}
            {update && folded && (
              <a
                href={update.url}
                target="_blank"
                rel="noreferrer"
                title={updateHint}
                aria-label={updateHint}
                className="relative rounded-md p-1.5 text-[color:var(--text-subtle)] transition-colors hover:bg-muted hover:text-foreground [&>svg]:h-4 [&>svg]:w-4 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <ArrowUpCircle aria-hidden="true" />
                <span
                  aria-hidden="true"
                  className="absolute right-1 top-1 h-1.5 w-1.5 rounded-full bg-[color:var(--red-folk)]"
                />
              </a>
            )}
            {version && !folded && (
              <span className="ml-auto flex min-w-0 items-center gap-1.5 pr-1">
                <span className="font-mono text-[0.6875rem] text-[color:var(--text-subtle)]">
                  {version}
                </span>
                {update && (
                  <a
                    href={update.url}
                    target="_blank"
                    rel="noreferrer"
                    title={updateHint}
                    aria-label={updateHint}
                    className="inline-flex min-w-0 items-center gap-1 rounded-full border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-1.5 py-px font-mono text-[0.625rem] text-foreground transition-colors hover:border-[color:var(--border-default)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                  >
                    <ArrowUpCircle aria-hidden="true" className="h-3 w-3 flex-none" />
                    <span className="truncate">
                      {t("shell.updateAvailable", { latest: update.latest })}
                    </span>
                  </a>
                )}
              </span>
            )}
          </div>
        </div>
      )}

      {user && (
        <div
          ref={userRef}
          className={cn("relative mt-auto flex flex-col gap-1.5", folded && "items-center")}
        >
          {userMenu && userOpen && (
            <div
              role="dialog"
              aria-label={t("shell.userMenuLabel")}
              className={cn(
                "absolute bottom-[calc(100%+6px)] z-40 rounded-lg border border-[color:var(--border-default)] bg-[color:var(--surface-elevated)] py-1.5 shadow-lg",
                folded ? "left-0 w-[min(260px,calc(100vw_-_1.5rem))]" : "inset-x-0",
              )}
            >
              {userMenu(() => setUserOpen(false))}
            </div>
          )}
          <button
            onClick={() => (userMenu ? setUserOpen((v) => !v) : user.onClick?.())}
            aria-haspopup={userMenu ? "dialog" : undefined}
            aria-expanded={userMenu ? userOpen : undefined}
            title={folded && typeof user.name === "string" ? user.name : undefined}
            className={cn(
              "flex items-center gap-2 rounded-md transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
              folded
                ? "justify-center p-0.5 hover:bg-muted"
                : "border border-[color:var(--border-subtle)] bg-[color:var(--surface-base)] px-2 py-1.5 hover:border-[color:var(--border-default)]",
              userOpen && !folded && "border-[color:var(--border-default)]",
            )}
          >
            <span className="flex h-[26px] w-[26px] flex-none items-center justify-center rounded-full bg-[color:var(--red-folk)] text-xs font-semibold text-white">
              {user.initials}
            </span>
            {!folded && (
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

  if (!isDrawer) return rail;

  return (
    <div className="fixed inset-0 z-50 flex">
      <div
        className="absolute inset-0 bg-black/50 rl-fade-in"
        onClick={closeDrawer}
        aria-hidden
      />
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-label={t("shell.navLabel")}
        className="rl-drawer-in relative flex h-full w-[min(280px,85vw)] max-w-full shadow-[14px_0_44px_rgba(0,0,0,0.42)] focus-visible:outline-none"
        {...a11y}
      >
        {rail}
        <button
          type="button"
          title={t("shell.closeNav")}
          aria-label={t("shell.closeNav")}
          onClick={closeDrawer}
          className="absolute right-2 top-2.5 flex h-9 w-9 items-center justify-center rounded-md text-[color:var(--text-subtle)] transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        >
          <X className="h-4 w-4" />
        </button>
      </div>
    </div>
  );
}
