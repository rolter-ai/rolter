import * as React from "react";

import {
  AnchoredPanel,
  moveFocus,
  type PanelAlign,
  type PanelSide,
} from "@/components/ui/anchored-panel";
import { useGate, type Capability } from "@/lib/can";
import { useRefusedClick } from "@/lib/ux-react";
import { cn } from "@/lib/utils";

// an actions menu, the WAI-ARIA kind: a `role="menu"` of `menuitem`s that the
// arrow keys walk, rather than a disclosure of buttons in the tab order. It is
// for a short list of verbs behind one control — the account card's, and the
// overflow button on each row of the scope popover — where a list of links the
// reader tabs through (the rail's group flyout) would be the wrong shape.
//
// The panel, its placement and its dismissal are `AnchoredPanel`'s. This adds
// the keyboard: focus lands on the first entry that can be chosen, Up and Down
// wrap, Home and End jump, Tab closes the menu and carries on to the next
// control after the one that opened it, and Escape closes and hands focus back.

const FOCUSABLE_ITEM = '[role="menuitem"]:not(:disabled)';

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
  onClose,
  className,
  children,
}: MenuProps) {
  const list = React.useRef<HTMLDivElement>(null);
  const entries = () =>
    Array.from(list.current?.querySelectorAll<HTMLElement>(FOCUSABLE_ITEM) ?? []);

  // the keyboard opens a menu onto its first entry; so does the pointer, which
  // costs a pointer user nothing and spares a keyboard one a stray Tab
  React.useEffect(() => {
    entries()[0]?.focus({ preventScroll: true });
  }, []);

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (moveFocus(e, entries())) return;
    // Tab leaves for the next control after the anchor: focus goes back to it
    // first, and the browser's own step then carries it on
    if (e.key === "Tab") onClose(true);
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
      <div
        ref={list}
        role="menu"
        aria-label={label}
        onKeyDown={onKeyDown}
        className="flex flex-col gap-0.5 p-1"
      >
        {children}
      </div>
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
}

export function MenuItem({
  icon,
  tone = "default",
  onSelect,
  gate,
  control = "menu-item",
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
        className={cn(
          "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm transition-colors [&>svg]:h-4 [&>svg]:w-4 [&>svg]:flex-none",
          "hover:bg-[color:var(--surface-hover)] focus-visible:bg-[color:var(--surface-hover)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
          "disabled:cursor-not-allowed disabled:text-[color:var(--text-subtle)] disabled:hover:bg-transparent",
          tone === "danger"
            ? "text-[color:var(--text-secondary)] hover:text-[color:var(--status-danger-text)] focus-visible:text-[color:var(--status-danger-text)]"
            : "text-foreground",
          className,
        )}
      >
        {icon}
        <span className="min-w-0 flex-1">
          <span className="block truncate">{children}</span>
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

/** a hairline between two groups of entries */
export function MenuSeparator() {
  return <div role="separator" className="-mx-1 my-0.5 h-px bg-[color:var(--border-subtle)]" />;
}
