import { Check } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import {
  AnchoredPanel,
  moveFocus,
  type PanelAlign,
  type PanelSide,
} from "@/components/ui/anchored-panel";
import { HighlightedText } from "@/components/ui/highlighted-text";
import { useGate, type Capability } from "@/lib/can";
import { fuzzyMatch } from "@/lib/fuzzy";
import { useRefusedClick } from "@/lib/ux-react";
import { cn } from "@/lib/utils";

// an actions menu, the WAI-ARIA kind: a `role="menu"` of `menuitem`s that the
// arrow keys walk, rather than a disclosure of buttons in the tab order. It is
// for a short list of verbs behind one control — the account card's, and the
// overflow button on each row of the scope popover — where a list of links the
// reader tabs through (the rail's group flyout) would be the wrong shape. A
// short list of exclusive choices behind one control (the language picker) is
// the same menu made of `MenuItemRadio` entries.
//
// The panel, its placement and its dismissal are `AnchoredPanel`'s. This adds
// the keyboard: focus lands on the entry that is checked, else the first one
// that can be chosen, Up and Down wrap, Home and End jump, Tab closes the menu
// and carries on to the next control after the one that opened it, and Escape
// closes and hands focus back.
//
// Every menu also filters (#3058): a search field above the entries narrows
// them with the scorer the command palette and the comboboxes use, best match
// first, matched characters marked. Typing while an entry has focus moves to
// the field, so a menu is searched by just starting to type; ArrowDown goes
// back down to the entries and Enter takes the best one. The entries do the
// matching themselves, through `MenuFilterContext`, because a menu's children
// are arbitrary nodes and not a list the menu could filter.

/** what the entries of the menu around them are being filtered by */
const MenuFilterContext = React.createContext("");

/** the text an entry is read and matched as, from the node it was given */
function textOf(node: React.ReactNode): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (React.isValidElement<{ children?: React.ReactNode }>(node))
    return textOf(node.props.children);
  return "";
}

/**
 * How an entry takes part in the filter. `null` hides it; otherwise `style`
 * ranks it (a flex `order`, so the DOM order the arrow keys fall back on is
 * untouched while the query is empty) and `hits` are the characters to mark.
 */
function useEntryFilter(label: string): {
  visible: boolean;
  style?: React.CSSProperties;
  hits: number[];
} {
  const query = React.useContext(MenuFilterContext);
  if (!query.trim() || !label) return { visible: true, hits: [] };
  const found = fuzzyMatch(label, query);
  if (!found) return { visible: false, hits: [] };
  // higher score first: flex sorts ascending, and two decimals keep the order
  return {
    visible: true,
    style: { order: 100000 - Math.round(found.score * 100) },
    hits: found.indices,
  };
}

const FOCUSABLE_ITEM = '[role="menuitem"]:not(:disabled), [role="menuitemradio"]:not(:disabled)';

// what an entry of either kind looks like, so a radio is not a second menu row
const ENTRY =
  "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm transition-colors hover:bg-[color:var(--surface-hover)] focus-visible:bg-[color:var(--surface-hover)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";

export interface MenuProps {
  /** the control that opened the menu */
  anchor: HTMLElement;
  /** the menu's accessible name */
  label: string;
  side?: PanelSide;
  align?: PanelAlign;
  matchWidth?: boolean;
  /**
   * Who or what the menu is about, shown above the entries. It sits outside the
   * `menu` element on purpose: a menu may own only entries, groups and
   * separators, and an identity block is none of those.
   */
  header?: React.ReactNode;
  /**
   * Show the filter field. On unless a menu has a reason to go without — a
   * list that can never be longer than the field is tall.
   */
  filterable?: boolean;
  onClose: (restoreFocus: boolean) => void;
  className?: string;
  children: React.ReactNode;
}

export function Menu({
  anchor,
  label,
  side = "below",
  align = "start",
  matchWidth,
  header,
  filterable = true,
  onClose,
  className,
  children,
}: MenuProps) {
  const { t } = useTranslation();
  const list = React.useRef<HTMLDivElement>(null);
  const field = React.useRef<HTMLInputElement>(null);
  const [query, setQuery] = React.useState("");
  const [shown, setShown] = React.useState(0);
  // the entries in the order they are drawn: a filter ranks them with CSS, so
  // the document order is no longer the order the arrow keys should walk
  const entries = () =>
    Array.from(list.current?.querySelectorAll<HTMLElement>(FOCUSABLE_ITEM) ?? [])
      .map((el, at) => ({ el, at, order: Number(el.style.order || 0) }))
      .sort((a, b) => a.order - b.order || a.at - b.at)
      .map((e) => e.el);

  // how many entries survive the query, for the empty message and the
  // announcement; the entries decide that, so it is read back from the DOM
  React.useLayoutEffect(() => {
    setShown(list.current?.querySelectorAll('[role^="menuitem"]').length ?? 0);
  });

  // the keyboard opens a menu onto its first entry, or onto the one a radio
  // group already has checked; so does the pointer, which costs a pointer user
  // nothing and spares a keyboard one a stray Tab
  React.useEffect(() => {
    const all = entries();
    const checked = all.find((el) => el.getAttribute("aria-checked") === "true");
    (checked ?? all[0])?.focus({ preventScroll: true });
  }, []);

  const typeInto = (next: string) => {
    setQuery(next);
    field.current?.focus();
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (filterable && !e.ctrlKey && !e.metaKey && !e.altKey) {
      // a printable key starts (or extends) a search rather than being lost
      if (e.key.length === 1 && e.key !== " ") {
        e.preventDefault();
        typeInto(query + e.key);
        return;
      }
      if (e.key === "Backspace" && query) {
        e.preventDefault();
        typeInto(query.slice(0, -1));
        return;
      }
    }
    if (moveFocus(e, entries())) return;
    // Tab leaves for the next control after the anchor: focus goes back to it
    // first, and the browser's own step then carries it on
    if (e.key === "Tab") onClose(true);
  };

  const onFieldKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    const all = entries();
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      (e.key === "ArrowDown" ? all[0] : all[all.length - 1])?.focus();
    } else if (e.key === "Enter") {
      // the best match, which the filter has put first
      e.preventDefault();
      all[0]?.click();
    } else if (e.key === "Tab") {
      onClose(true);
    }
  };

  return (
    <AnchoredPanel
      anchor={anchor}
      side={side}
      align={align}
      matchWidth={matchWidth}
      onClose={onClose}
      className={cn("w-max min-w-[11rem] max-w-[min(20rem,calc(100vw-1rem))]", className)}
    >
      {header}
      {filterable && (
        <div className="px-2 pt-2">
          <input
            ref={field}
            type="text"
            autoComplete="off"
            spellCheck={false}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onFieldKeyDown}
            aria-label={t("common.menu.filter", { menu: label })}
            placeholder={t("common.menu.filterPlaceholder")}
            className="h-7 w-full rounded-md border border-input bg-[color:var(--surface-subtle)] px-2 text-xs placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          />
        </div>
      )}
      <MenuFilterContext.Provider value={query}>
        <div
          ref={list}
          role="menu"
          aria-label={label}
          onKeyDown={onKeyDown}
          className="flex flex-col gap-0.5 p-1"
        >
          {children}
        </div>
      </MenuFilterContext.Provider>
      {query.trim() !== "" && (
        <p
          aria-live="polite"
          className={cn(shown === 0 ? "px-3 pb-3 pt-1 text-xs text-muted-foreground" : "sr-only")}
        >
          {shown === 0
            ? t("common.menu.noMatches")
            : t("common.combobox.results", { count: shown })}
        </p>
      )}
    </AnchoredPanel>
  );
}

export interface MenuItemProps extends Omit<
  React.ButtonHTMLAttributes<HTMLButtonElement>,
  "role" | "type" | "onSelect"
> {
  icon?: React.ReactNode;
  /** a destructive entry: quiet at rest, danger-coloured on hover and focus */
  tone?: "default" | "danger";
  onSelect: () => void;
  /**
   * The `resource:action` the entry needs. A refused entry is a real `disabled`
   * button that says which role it would take, in its `title` and in a line
   * under its label, since a menu is walked with the arrow keys and a disabled
   * button is not a stop on that walk: the reason has to be on screen to be
   * read at all.
   */
  gate?: Capability;
  /** names a refused entry in the UX stream (#1750); required with `gate` */
  control?: string;
  /** what the filter matches when the children are not plain text */
  searchLabel?: string;
}

export function MenuItem({
  icon,
  tone = "default",
  onSelect,
  gate,
  control = "menu-item",
  searchLabel,
  disabled,
  title,
  className,
  children,
  ...props
}: MenuItemProps) {
  const { denied, reason } = useGate(gate);
  const refusal = useRefusedClick(denied, control, gate);
  const generated = React.useId();
  const id = props.id ?? generated;
  const filter = useEntryFilter(searchLabel ?? textOf(children));
  if (!filter.visible) return null;
  return (
    // the wrapper catches the pointer a disabled button swallows, so a reach
    // for a refused entry still lands in the UX stream. it is not a tab stop,
    // unlike the `RefusalWrap` a lone gated button gets: a menu is walked with
    // the arrow keys, and the reason is printed under the label instead
    <span role="none" className="contents" {...refusal}>
      <button
        {...props}
        id={id}
        type="button"
        role="menuitem"
        tabIndex={-1}
        disabled={disabled || denied}
        title={denied ? reason : title}
        onClick={onSelect}
        style={{ ...filter.style, ...props.style }}
        className={cn(
          ENTRY,
          "[&>svg]:h-4 [&>svg]:w-4 [&>svg]:flex-none",
          "disabled:cursor-not-allowed disabled:text-[color:var(--text-subtle)] disabled:hover:bg-transparent",
          tone === "danger"
            ? "text-[color:var(--text-secondary)] hover:text-[color:var(--status-danger-text)] focus-visible:text-[color:var(--status-danger-text)]"
            : "text-foreground",
          className,
        )}
      >
        {icon}
        <span className="min-w-0 flex-1">
          <span className="block truncate">
            {typeof children === "string" && filter.hits.length > 0 ? (
              <HighlightedText text={children} indices={filter.hits} />
            ) : (
              children
            )}
          </span>
          {denied && reason && (
            <span aria-hidden="true" className="block truncate text-[0.6875rem]">
              {reason}
            </span>
          )}
        </span>
      </button>
    </span>
  );
}

export interface MenuItemRadioProps extends Omit<
  React.ButtonHTMLAttributes<HTMLButtonElement>,
  "role" | "type" | "onSelect" | "aria-checked"
> {
  /** whether this is the choice in force; the others in the menu are not */
  checked: boolean;
  onSelect: () => void;
  /** what the filter matches when the children are not plain text */
  searchLabel?: string;
}

/**
 * One choice of an exclusive set, a `menuitemradio`. The chosen one reads
 * `aria-checked` and carries a tick; picking runs `onSelect`, and closing the
 * menu is the caller's call, as it is for a `MenuItem`.
 */
export function MenuItemRadio({
  checked,
  onSelect,
  searchLabel,
  className,
  children,
  ...props
}: MenuItemRadioProps) {
  const filter = useEntryFilter(searchLabel ?? textOf(children));
  if (!filter.visible) return null;
  return (
    <button
      {...props}
      type="button"
      role="menuitemradio"
      aria-checked={checked}
      tabIndex={-1}
      onClick={onSelect}
      style={{ ...filter.style, ...props.style }}
      className={cn(
        ENTRY,
        checked ? "text-foreground" : "text-[color:var(--text-secondary)]",
        className,
      )}
    >
      <span className="min-w-0 flex-1 truncate">
        {typeof children === "string" && filter.hits.length > 0 ? (
          <HighlightedText text={children} indices={filter.hits} />
        ) : (
          children
        )}
      </span>
      {checked && (
        <Check aria-hidden className="h-3.5 w-3.5 flex-none text-[color:var(--red-folk-text)]" />
      )}
    </button>
  );
}

/** a hairline between two groups of entries */
export function MenuSeparator() {
  // between groups that a filter has taken apart, a rule separates nothing
  if (React.useContext(MenuFilterContext).trim()) return null;
  return <div role="separator" className="-mx-1 my-0.5 h-px bg-[color:var(--border-subtle)]" />;
}
