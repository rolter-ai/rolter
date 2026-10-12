// the fuzzy matcher every filterable list in the dashboard ranks with: the
// command palette (#1198), the combobox and the menus (#3058).
//
// one scorer on purpose. a reader who learned that "rr" finds "Routing Rules"
// in the palette expects "oai" to find "openai" in a dropdown, and two
// matchers would disagree about which row comes first.

/** where a query landed in a label, and how well */
export interface FuzzyMatch {
  /** higher is better */
  score: number;
  /** code-point positions in the label the query's characters matched, ascending */
  indices: number[];
}

/**
 * Case- and diacritic-insensitive form of one character, one code point out for
 * one in, so a position in the folded text is a position in the original.
 * Folding the whole string would not give that: a precomposed Hangul syllable
 * decomposes into several jamo, and every index after it would drift.
 */
function foldChar(ch: string): string {
  const folded = ch
    .normalize("NFD")
    .replace(/\p{Diacritic}/gu, "")
    .toLowerCase();
  return Array.from(folded).length === 1 ? folded : ch.toLowerCase();
}

/** the text as folded code points */
function fold(text: string): string[] {
  return Array.from(text, foldChar);
}

/** a character a new word can start after */
function isBoundary(ch: string | undefined): boolean {
  return ch === " " || ch === "-" || ch === "_" || ch === "/" || ch === "." || ch === ":";
}

/**
 * How well `query` matches `label`, with the characters it landed on, or `null`
 * when it does not match at all.
 *
 * A subsequence match rather than a substring one, so "rr" finds "Routing
 * Rules" and "audlog" finds "Audit Logs" — a half-remembered name still lands.
 * Higher is better:
 *
 * - a match that starts at the head of the label beats one in the middle
 * - a match on word boundaries ("rr" over "Routing Rules") beats one inside a
 *   word, which is what keeps initialisms at the top
 * - a run of adjacent characters beats the same characters scattered
 * - a shorter label wins the tie, so "Logs" outranks "Logs Settings"
 */
export function fuzzyMatch(label: string, query: string): FuzzyMatch | null {
  const q = fold(query.trim());
  if (q.length === 0) return { score: 0, indices: [] };
  const l = fold(label);
  const indices: number[] = [];
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
    else if (isBoundary(l[hit - 1])) score += 12;
    // the further into the label the match drifts, the weaker it is
    score -= Math.min(hit, 12);
    indices.push(hit);
    previous = hit;
    at = hit + 1;
  }
  return { score: score - l.length / 100, indices };
}

/** {@link fuzzyMatch} without the positions: the score, or `null` for no match */
export function fuzzyScore(label: string, query: string): number | null {
  return fuzzyMatch(label, query)?.score ?? null;
}

/** one searchable thing: the text it is read as, and the text it is also found by */
export interface Searchable {
  label: string;
  /** found by these too, each at a discount, and never highlighted */
  also?: (string | undefined)[];
}

/** a match on anything but the label, so it never outranks a hit on the label */
const SECONDARY_DISCOUNT = 100;

/**
 * Score `item` for `query`: the label's match, else the best of the secondary
 * texts at a discount. `null` when nothing matches. The indices are the label's
 * and are empty when only a secondary text matched.
 */
export function matchSearchable(item: Searchable, query: string): FuzzyMatch | null {
  const direct = fuzzyMatch(item.label, query);
  if (direct) return direct;
  let best: number | null = null;
  for (const text of item.also ?? []) {
    if (!text) continue;
    const found = fuzzyScore(text, query);
    if (found !== null && (best === null || found > best)) best = found;
  }
  return best === null ? null : { score: best - SECONDARY_DISCOUNT, indices: [] };
}

/**
 * `items` that match `query`, best first, each with its label highlight.
 *
 * An empty query matches everything and keeps the caller's order, since that
 * order is what the list shows before a keystroke. Equal scores keep it too,
 * rather than whatever the sort happened to do.
 */
export function rankSearchable<T>(
  items: T[],
  query: string,
  describe: (item: T) => Searchable,
): { item: T; indices: number[] }[] {
  if (query.trim() === "") return items.map((item) => ({ item, indices: [] }));
  const scored: { item: T; score: number; at: number; indices: number[] }[] = [];
  items.forEach((item, at) => {
    const found = matchSearchable(describe(item), query);
    if (found) scored.push({ item, score: found.score, at, indices: found.indices });
  });
  scored.sort((a, b) => b.score - a.score || a.at - b.at);
  return scored.map(({ item, indices }) => ({ item, indices }));
}
