import * as React from "react";

// what a modal owes the keyboard and the screen reader, shared by Dialog,
// Sheet and the nav drawer so none can claim `aria-modal` without honouring it
// (#1181):
//
// - focus moves into the panel when it opens and back to the opener when it
//   closes, so a keyboard user is never left on an element behind the scrim
// - Tab and Shift+Tab cycle inside the panel instead of walking into the page
// - everything outside the topmost modal is `inert` (#1998). the Tab trap
//   above only sees keys pressed inside the panel, and a busy modal disables
//   the very button that held focus, which drops focus onto <body> where the
//   trap never hears the next Tab. inert takes the page out of the tab order,
//   out of hit testing and out of the accessibility tree whatever holds focus
// - focus that falls out of the panel — its control disabled or removed, a
//   click on the scrim — is handed back to the panel, so the next Tab starts
//   inside rather than from the top of the document
// - Escape closes only the topmost modal, so a dialog raised over a sheet does
//   not take the sheet down with it
// - the page behind stops scrolling while a modal is up

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

const FORM_CONTROL =
  'input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled])';

// an element drawn above every modal stays live while one is open: the toast
// stack (`ui/toaster.tsx`), whose live regions an inert ancestor would silence
const ABOVE_MODALS = "[data-above-modals]";

interface OpenModal {
  token: symbol;
  /** the panel's fixed box, which stays live along with the panel */
  layer: HTMLElement;
}

// open modals, innermost last. module-level on purpose: the stack is a fact
// about the document, not about any one react tree
const stack: OpenModal[] = [];

// what this module set `inert` on. only these are ever un-set, so an element
// that was inert before any modal opened is still inert after the last closes
const madeInert = new Set<HTMLElement>();

// the body's overflow from before the first modal opened, restored when the
// last one closes — whichever of them that turns out to be
let overflowBefore = "";

const NEVER_INERT = new Set(["SCRIPT", "STYLE", "TEMPLATE"]);

function focusables(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (el) => el.offsetParent !== null || el === document.activeElement,
  );
}

function isTopmost(token: symbol | null): boolean {
  return token !== null && stack[stack.length - 1]?.token === token;
}

// the box a modal paints in: the fixed overlay holding the panel and its scrim
// (Dialog, Sheet, the nav drawer), or the panel itself when it is the fixed box
function layerOf(panel: HTMLElement): HTMLElement {
  for (let el: HTMLElement | null = panel; el && el !== document.body; el = el.parentElement) {
    if (getComputedStyle(el).position === "fixed") return el;
  }
  return panel;
}

// a scrim drawn beside the layer rather than inside it (the logs filter rail)
// stays live too: inert would pass its dismissing click through to the page
function isScrim(el: Element): boolean {
  return el.getAttribute("aria-hidden") === "true" && el.querySelector(FOCUSABLE) === null;
}

// every element that is neither kept nor an ancestor of something kept: the
// siblings of each kept element and of each of its ancestors, up to <body>
function outside(keep: Set<Element>): HTMLElement[] {
  const ancestors = new Set<Element>();
  for (const el of keep) {
    for (let up = el.parentElement; up; up = up.parentElement) ancestors.add(up);
  }
  const found: HTMLElement[] = [];
  const walk = (parent: Element) => {
    for (const child of Array.from(parent.children)) {
      if (keep.has(child)) continue;
      if (ancestors.has(child)) walk(child);
      else if (child instanceof HTMLElement && !NEVER_INERT.has(child.tagName)) found.push(child);
    }
  };
  walk(document.body);
  return found;
}

// make the page match the stack: everything outside the topmost modal inert,
// nothing inert once the stack is empty. a modal whose layer has already left
// the document is closing in this same commit and no longer counts, so two
// modals closing together restore the page for whichever cleanup runs first
function syncInert() {
  const top = [...stack].reverse().find((m) => m.layer.isConnected);
  const want = new Set<HTMLElement>();
  if (top) {
    const keep = new Set<Element>([top.layer, ...document.querySelectorAll(ABOVE_MODALS)]);
    for (const sibling of Array.from(top.layer.parentElement?.children ?? [])) {
      if (isScrim(sibling)) keep.add(sibling);
    }
    for (const el of outside(keep)) want.add(el);
  }
  for (const el of madeInert) {
    if (want.has(el)) continue;
    el.inert = false;
    madeInert.delete(el);
  }
  for (const el of want) {
    // already inert: either ours, or someone else's and so not ours to undo
    if (el.inert) continue;
    el.inert = true;
    madeInert.add(el);
  }
}

export interface ModalA11yOptions {
  open: boolean;
  /** invoked for Escape; the modal decides whether that actually closes it */
  onEscape: () => void;
  /**
   * where focus lands on open. `"first"` picks the first focusable control,
   * which suits a form; `"panel"` focuses the container itself, which suits a
   * confirmation whose first control is a destructive button
   */
  initialFocus?: "first" | "panel";
}

/**
 * Wire the modal behaviours above onto the element in `ref`.
 *
 * The returned props go on the panel: it needs `tabIndex={-1}` so it can
 * receive focus when it has no controls, and the key handler that traps Tab.
 */
export function useModalA11y(
  ref: React.RefObject<HTMLElement | null>,
  { open, onEscape, initialFocus = "first" }: ModalA11yOptions,
) {
  const id = React.useRef<symbol | null>(null);
  const onEscapeRef = React.useRef(onEscape);
  onEscapeRef.current = onEscape;

  React.useEffect(() => {
    if (!open) return;
    const panel = ref.current;
    if (!panel) return;
    const token = Symbol("modal");
    id.current = token;

    // read before the page goes inert, which takes focus off the opener
    const opener = document.activeElement as HTMLElement | null;
    if (stack.length === 0) {
      overflowBefore = document.body.style.overflow;
      document.body.style.overflow = "hidden";
    }
    stack.push({ token, layer: layerOf(panel) });
    syncInert();

    // focus after paint so the enter animation has laid the panel out. a
    // pointer user may already have clicked into a field by then; their focus
    // wins. "first" prefers a form control over the close button that opens
    // every header, and falls back to the panel when there is none
    // a macrotask rather than requestAnimationFrame: a hidden tab pauses
    // animation frames, and a modal raised there still needs its focus set
    let placed = false;
    const frame = setTimeout(() => {
      placed = true;
      if (panel.contains(document.activeElement)) return;
      const target =
        initialFocus === "first"
          ? (panel.querySelector<HTMLElement>(FORM_CONTROL) ?? focusables(panel)[0])
          : undefined;
      (target ?? panel).focus({ preventScroll: true });
    }, 0);

    // a busy modal disables the button that was just pressed, and the browser
    // moves focus off a disabled control onto <body>; a removed control and a
    // click on the scrim end there too. the panel takes it instead. not before
    // the first placement, which would otherwise lose to a panel that rendered
    // content before its initial focus landed
    const rescue = () => {
      if (!placed || !isTopmost(token)) return;
      const active = document.activeElement;
      const lost =
        !active ||
        active === document.body ||
        (panel.contains(active) && active.matches(":disabled"));
      if (lost) panel.focus({ preventScroll: true });
    };
    // disabling fires no event of its own, so the attribute is watched. a
    // focusout with nowhere to go is a click on the scrim or a removal; the
    // window losing focus is one too, but leaves activeElement where it was
    const watch = new MutationObserver(rescue);
    watch.observe(panel, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ["disabled"],
    });
    const onFocusOut = (e: FocusEvent) => {
      if (e.relatedTarget === null) queueMicrotask(rescue);
    };
    panel.addEventListener("focusout", onFocusOut);

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (!isTopmost(token)) return;
      e.stopPropagation();
      onEscapeRef.current();
    };
    document.addEventListener("keydown", onKeyDown);

    return () => {
      clearTimeout(frame);
      watch.disconnect();
      panel.removeEventListener("focusout", onFocusOut);
      document.removeEventListener("keydown", onKeyDown);
      const at = stack.findIndex((m) => m.token === token);
      if (at >= 0) stack.splice(at, 1);
      // the page has to be live again before focus can go back into it
      syncInert();
      if (stack.length === 0) document.body.style.overflow = overflowBefore;
      // the opener may have unmounted with the row it sat in; then there is
      // nothing sensible to return to and the browser's default is fine
      if (opener && opener.isConnected) opener.focus({ preventScroll: true });
    };
  }, [open, ref, initialFocus]);

  const onKeyDown = React.useCallback(
    (e: React.KeyboardEvent<HTMLElement>) => {
      if (e.key !== "Tab") return;
      const panel = ref.current;
      if (!panel) return;
      // a modal raised over this one traps its own Tab; the key still bubbles
      // here through the react tree when it was rendered as this one's child
      if (!isTopmost(id.current)) return;
      const items = focusables(panel);
      if (items.length === 0) {
        e.preventDefault();
        panel.focus();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement;
      if (e.shiftKey && (active === first || active === panel)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
    },
    [ref],
  );

  return { tabIndex: -1, onKeyDown } as const;
}

/** how many modals are open — tests and the nav's outside-click guard use it */
export function openModalCount(): number {
  return stack.length;
}
