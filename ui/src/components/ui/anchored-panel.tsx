import * as React from "react";

import { cn } from "@/lib/utils";

// the overlay every floating surface of the rail stands on: the folded rail's
// group flyout (#2803), the scope popover and the menus of the account card and
// of the scope rows (#2805).
//
// It owns the two things each of them used to write for itself. Placement: the
// rail's list scrolls, and a scroll container clips an absolutely positioned
// child, so the panel is `fixed` and placed from the anchor's rectangle. Dismissal:
// a press outside, Escape, focus leaving, and the anchor scrolling out of
// sight all close it, and Escape closes only the panel on top, so a menu opened
// from inside the scope popover does not take the popover down with it.
//
// What goes inside is the caller's — a disclosure of navigation buttons, a
// popover of form controls, or a `Menu` — and so is where focus lands on open.
// The panel renders inline, as a sibling of its anchor, so Tab from the anchor
// reaches it next and an ancestor that is itself a modal (the nav drawer) still
// contains it.

/** space between the anchor and the panel */
const GAP = 6;
/** space between the panel and the viewport's edge */
const MARGIN = 8;

export type PanelSide = "right" | "below" | "above";
export type PanelAlign = "start" | "end";

export interface Box {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export interface PlaceOptions {
  anchor: Box;
  /** where the panel sits against, for `right`: the rail's edge, not the icon's */
  edge: number;
  panel: { width: number; height: number };
  viewport: { width: number; height: number };
  side: PanelSide;
  align: PanelAlign;
}

/**
 * Where the panel's top-left corner goes.
 *
 * `right` stands beside the anchor's container, level with the anchor's top
 * (`start`) or its bottom (`end`), so a menu opened from the card at the foot of
 * the rail grows upward instead of off the screen. `below` and `above` stand
 * against the anchor's edge and line up with its left (`start`) or right (`end`)
 * side. `below` turns `above` when the viewport has no room under the anchor and
 * more over it. Every answer is clamped so the panel stays inside the viewport.
 */
export function placePanel({ anchor, edge, panel, viewport, side, align }: PlaceOptions): {
  left: number;
  top: number;
} {
  const clamp = (value: number, size: number, available: number) =>
    Math.max(MARGIN, Math.min(value, available - MARGIN - size));
  const across = (from: number, to: number) => (align === "end" ? to - panel.width : from);
  let left: number;
  let top: number;
  if (side === "right") {
    left = edge + GAP;
    top = align === "end" ? anchor.bottom - panel.height : anchor.top;
  } else {
    const fitsBelow = anchor.bottom + GAP + panel.height <= viewport.height - MARGIN;
    const fitsAbove = anchor.top - GAP - panel.height >= MARGIN;
    const above = side === "above" || (!fitsBelow && fitsAbove);
    left = across(anchor.left, anchor.right);
    top = above ? anchor.top - GAP - panel.height : anchor.bottom + GAP;
  }
  return {
    left: clamp(left, panel.width, viewport.width),
    top: clamp(top, panel.height, viewport.height),
  };
}

// open panels, innermost last. module-level on purpose: Escape is a fact about
// the document, not about one react tree
const stack: symbol[] = [];

/**
 * How many panels are open.
 *
 * The nav drawer is a modal that closes on Escape; it asks this first, so the
 * key that dismisses a menu inside it does not also dismiss the drawer.
 */
export function openPanelCount(): number {
  return stack.length;
}

export interface AnchoredPanelProps extends Omit<React.HTMLAttributes<HTMLDivElement>, "onBlur"> {
  /** the element the panel is placed against, and which toggles it */
  anchor: HTMLElement;
  side: PanelSide;
  align?: PanelAlign;
  /** exactly as wide as the anchor, for a menu over the card or field that opened it */
  matchWidth?: boolean;
  /**
   * Asked to close. `restoreFocus` is true for the keyboard's own dismissal
   * (Escape), where focus is inside the panel and would otherwise be lost, and
   * false for a press or a focus change that has already put it somewhere.
   */
  onClose: (restoreFocus: boolean) => void;
}

export const AnchoredPanel = React.forwardRef<HTMLDivElement, AnchoredPanelProps>(
  function AnchoredPanel(
    { anchor, side, align = "start", matchWidth, onClose, className, style, children, ...props },
    forwarded,
  ) {
    const ref = React.useRef<HTMLDivElement>(null);
    React.useImperativeHandle(forwarded, () => ref.current as HTMLDivElement);
    const [pos, setPos] = React.useState({ left: 0, top: 0, width: 0 });
    // read at event time, so a new `onClose` each render does not re-bind the
    // document listeners below
    const close = React.useEffectEvent(onClose);

    const place = React.useCallback(() => {
      const panel = ref.current;
      if (!panel) return;
      const box = anchor.getBoundingClientRect();
      const next = placePanel({
        anchor: box,
        edge: (anchor.closest("nav") ?? anchor).getBoundingClientRect().right,
        panel: { width: panel.offsetWidth, height: panel.offsetHeight },
        viewport: { width: window.innerWidth, height: window.innerHeight },
        side,
        align,
      });
      const width = matchWidth ? Math.round(box.width) : 0;
      setPos((p) =>
        p.left === next.left && p.top === next.top && p.width === width ? p : { ...next, width },
      );
    }, [anchor, side, align, matchWidth]);

    // measured before paint, so the panel never shows at the corner first, and
    // again whenever its own size changes (a hint appearing under a row)
    React.useLayoutEffect(() => {
      place();
      window.addEventListener("resize", place);
      const observer =
        typeof ResizeObserver === "undefined" || !ref.current ? null : new ResizeObserver(place);
      if (observer && ref.current) observer.observe(ref.current);
      return () => {
        window.removeEventListener("resize", place);
        observer?.disconnect();
      };
    }, [place]);

    React.useEffect(() => {
      const token = Symbol("panel");
      stack.push(token);
      const onDown = (e: MouseEvent) => {
        const target = e.target as Node;
        if (ref.current?.contains(target) || anchor.contains(target)) return;
        close(false);
      };
      // Escape works from the anchor too, once focus has gone back to it
      const onKey = (e: KeyboardEvent) => {
        if (e.key !== "Escape" || stack[stack.length - 1] !== token) return;
        e.preventDefault();
        close(true);
      };
      // the anchor moves when a list it sits in scrolls: the panel follows it,
      // and goes once the anchor has scrolled out of sight
      const onScroll = (e: Event) => {
        if (!(e.target instanceof Element) || !e.target.contains(anchor)) return;
        const view = e.target.getBoundingClientRect();
        const box = anchor.getBoundingClientRect();
        if (box.bottom < view.top || box.top > view.bottom) close(false);
        else place();
      };
      document.addEventListener("mousedown", onDown);
      document.addEventListener("keydown", onKey);
      document.addEventListener("scroll", onScroll, true);
      return () => {
        const at = stack.indexOf(token);
        if (at >= 0) stack.splice(at, 1);
        document.removeEventListener("mousedown", onDown);
        document.removeEventListener("keydown", onKey);
        document.removeEventListener("scroll", onScroll, true);
      };
    }, [anchor, place]);

    // Tab past either end leaves the panel open behind a focus that has moved
    // on; a null target (the press landed on plain text) is not leaving
    const onBlur = (e: React.FocusEvent<HTMLDivElement>) => {
      const to = e.relatedTarget;
      if (to instanceof Node && !e.currentTarget.contains(to) && !anchor.contains(to)) {
        onClose(false);
      }
    };

    return (
      <div
        {...props}
        ref={ref}
        onBlur={onBlur}
        style={{ ...style, left: pos.left, top: pos.top, width: pos.width || undefined }}
        className={cn(
          "fixed z-50 max-h-[calc(100vh-1rem)] rounded-lg border border-[color:var(--border-default)] bg-[color:var(--surface-elevated)] shadow-[var(--shadow-lg)]",
          className,
        )}
      >
        {children}
      </div>
    );
  },
);

/**
 * The keyboard of a vertical list of controls: up and down move focus and wrap,
 * Home and End jump to the ends. Returns `true` when it handled the key.
 */
export function moveFocus(e: React.KeyboardEvent, items: HTMLElement[]): boolean {
  if (items.length === 0) return false;
  const at = items.indexOf(document.activeElement as HTMLElement);
  let to: number | null = null;
  if (e.key === "ArrowDown") to = (at + 1) % items.length;
  else if (e.key === "ArrowUp") to = (at - 1 + items.length) % items.length;
  else if (e.key === "Home") to = 0;
  else if (e.key === "End") to = items.length - 1;
  if (to === null) return false;
  e.preventDefault();
  items[to]?.focus();
  return true;
}
