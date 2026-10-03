// what the command palette searches over, and how it ranks what it finds
// (#1198).
//
// kept out of the component so the ranking is unit-testable on its own: a
// story can prove the palette shows the entry it typed for, but only a test
// over the scorer can prove "rr" reaches "Routing Rules" ahead of "Providers"
// for every future wording of either label.

/** an entry the palette can open: a nav leaf, or a record on one of them */
export interface PaletteEntry {
  /** stable identity, unique across every section */
  id: string;
  /** nav leaf key the entry opens — `/<key>` */
  screen: string;
  /** what the reader sees and types against, already translated */
  label: string;
  /** the second line: the record's screen, or the group the leaf sits in */
  hint?: string;
}

/**
 * How well `query` matches `label`, or `null` when it does not match at all.
 *
 * A subsequence match rather than a substring one, so "rr" finds "Routing
 * Rules" and "audlog" finds "Audit Logs" — the point of a palette is that a
 * half-remembered name still lands. Higher is better:
 *
 * - a match that starts at the head of the label beats one in the middle
 * - a match on word boundaries ("rr" over "Routing Rules") beats one inside a
 *   word, which is what keeps initialisms at the top
 * - a run of adjacent characters beats the same characters scattered
 * - a shorter label wins the tie, so "Logs" outranks "Logs Settings"
 */
export function fuzzyScore(label: string, query: string): number | null {
  const q = query.trim().toLowerCase();
  if (q === "") return 0;
  const l = label.toLowerCase();
  let score = 0;
  let at = 0;
  let previous = -2;
  for (const ch of q) {
    // a space in the query is a word separator the label need not carry
    if (ch === " ") continue;
    const hit = l.indexOf(ch, at);
    if (hit === -1) return null;
    if (hit === previous + 1) score += 8;
    if (hit === 0) score += 16;
    else if (l[hit - 1] === " " || l[hit - 1] === "-") score += 12;
    // the further into the label the match drifts, the weaker it is
    score -= Math.min(hit, 12);
    previous = hit;
    at = hit + 1;
  }
  return score - label.length / 100;
}

/**
 * `entries` that match `query`, best first.
 *
 * The hint is searched too, at a discount: typing "provider" should reach the
 * providers a record section is listing, without ever outranking the screen
 * that is literally called Providers. An empty query matches everything and
 * keeps the caller's order, which is what the palette shows before a
 * keystroke.
 */
export function rankEntries<T extends PaletteEntry>(entries: T[], query: string): T[] {
  if (query.trim() === "") return entries;
  const scored: { entry: T; score: number; at: number }[] = [];
  entries.forEach((entry, at) => {
    const direct = fuzzyScore(entry.label, query);
    const viaHint = entry.hint === undefined ? null : fuzzyScore(entry.hint, query);
    const score = direct !== null ? direct : viaHint !== null ? viaHint - 100 : null;
    if (score !== null) scored.push({ entry, score, at });
  });
  // the original order breaks ties, so an equal-scoring pair keeps the order
  // the nav put them in rather than whatever the sort happened to do
  scored.sort((a, b) => b.score - a.score || a.at - b.at);
  return scored.map((s) => s.entry);
}

/** where the recently visited screens are remembered, per browser */
export const RECENT_STORAGE_KEY = "rolter.recent-screens";

/** how many the palette offers before a query narrows anything */
export const RECENT_LIMIT = 5;

/** the sliver of `Storage` this module uses, so a test can hand it a fake */
export interface RecentStore {
  getItem: (key: string) => string | null;
  setItem: (key: string, value: string) => void;
}

/** the browser's own storage, absent outside one */
function browserStore(): RecentStore | null {
  return typeof localStorage === "undefined" ? null : localStorage;
}

/**
 * The screens this browser visited last, most recent first.
 *
 * Storage holds anything and throws outright in some embedding contexts, so
 * anything that is not a list of strings reads as "nothing visited yet" — a
 * palette with no recents is a smaller loss than a palette that cannot open.
 */
export function readRecentScreens(store: RecentStore | null = browserStore()): string[] {
  if (!store) return [];
  try {
    const raw = store.getItem(RECENT_STORAGE_KEY);
    if (raw === null) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((k): k is string => typeof k === "string").slice(0, RECENT_LIMIT);
  } catch {
    return [];
  }
}

/**
 * Record a visit and return the new list.
 *
 * Moving a screen that is already listed back to the front rather than
 * appending it keeps the list to the screens actually in rotation instead of
 * the first five ever opened.
 */
export function rememberScreen(
  screen: string,
  store: RecentStore | null = browserStore(),
): string[] {
  const next = [screen, ...readRecentScreens(store).filter((k) => k !== screen)].slice(
    0,
    RECENT_LIMIT,
  );
  if (!store) return next;
  try {
    store.setItem(RECENT_STORAGE_KEY, JSON.stringify(next));
  } catch {
    // a browser that refuses storage still navigates, it just forgets
  }
  return next;
}

/** does this keystroke mean "open the palette"? ⌘K on mac, Ctrl-K elsewhere */
export function isPaletteShortcut(e: { key: string; metaKey: boolean; ctrlKey: boolean }): boolean {
  return (e.key === "k" || e.key === "K") && (e.metaKey || e.ctrlKey);
}

/**
 * Does `/` mean "focus the nav search" right now?
 *
 * Not while the caller is typing: `/` is a character in a model name, a URL
 * and a prompt, and stealing it out of a field would make those unwritable.
 */
export function isNavSearchShortcut(e: {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  target: EventTarget | null;
}): boolean {
  if (e.key !== "/" || e.metaKey || e.ctrlKey || e.altKey) return false;
  return !isTextEntry(e.target);
}

/**
 * Is the event's target somewhere text is being typed?
 *
 * Duck-typed rather than an `instanceof HTMLElement` check: the shortcut rules
 * are the half of the palette worth unit-testing, and the test runner has no
 * DOM to build an element in.
 */
export function isTextEntry(target: EventTarget | null): boolean {
  const el = target as Partial<HTMLElement> | null;
  if (!el || typeof el.tagName !== "string") return false;
  const tag = el.tagName.toLowerCase();
  return tag === "input" || tag === "textarea" || tag === "select" || el.isContentEditable === true;
}
