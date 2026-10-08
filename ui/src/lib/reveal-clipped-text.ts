import type * as React from "react";

/**
 * Names whatever the pointer is over that its container has clipped.
 *
 * A label that does not fit ends in an ellipsis, and the rest of the text was
 * nowhere on screen. A list table clips (`truncate` on a name, an address, a key
 * prefix) and so does the navigation rail (a Russian screen name in a 232px
 * rail), so the container answers once for all of them rather than each entry
 * wiring a `title` onto itself: on pointer over, the elements from the target up
 * to the container are read, and the first one that is cut short with an
 * ellipsis gets its full text as a `title`, the browser's own tooltip. An
 * element whose `title` its author wrote keeps it, and one that has since been
 * given room, a wider window or a resized rail, loses the title this set. The
 * text is also in the document in full, so a screen reader reads it all.
 *
 * It is the container's `onPointerOver`: `ListTable` and the rail's list and
 * group flyout hand it down, and the container itself is where the walk stops.
 */
const REVEALED = "data-rl-revealed";
export function revealClippedText(e: React.PointerEvent<HTMLElement>) {
  const container = e.currentTarget;
  for (let el = e.target as HTMLElement | null; el && el !== container; el = el.parentElement) {
    const mine = el.hasAttribute(REVEALED);
    if (el.hasAttribute("title") && !mine) continue;
    const clipped =
      el.scrollWidth > el.clientWidth + 1 && getComputedStyle(el).textOverflow === "ellipsis";
    if (clipped && el.textContent) {
      el.setAttribute("title", el.textContent);
      el.setAttribute(REVEALED, "");
      return;
    }
    if (mine) {
      el.removeAttribute("title");
      el.removeAttribute(REVEALED);
    }
  }
}
