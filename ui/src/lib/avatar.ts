// the categorical chip palette, one token per entry: a raw hex here is not
// retunable and is contrast-checked by nobody, which is how the gold entry
// reached white initials at 3.25:1 (#1181, #1245). the ratios are recorded
// beside the tokens in index.css
const AVATAR_TOKENS = [
  "var(--avatar-1)",
  "var(--avatar-2)",
  "var(--avatar-3)",
  "var(--avatar-4)",
  "var(--avatar-5)",
  "var(--avatar-6)",
] as const;

// 32-bit FNV-1a over the id's UTF-16 units. it has to stay this function: a
// person's chip colour is derived from it, so changing it recolours everybody
function hash(id: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < id.length; i++) {
    h ^= id.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

/**
 * The chip colour for one person, as a `var(--avatar-N)` reference.
 *
 * Derived from the id rather than from the row's position, so the colour stays
 * with the person when the list is filtered, searched or re-ordered (#2059).
 * Give it the id a row is keyed by, never an email the person can change.
 */
export function avatarColor(id: string): string {
  return AVATAR_TOKENS[hash(id) % AVATAR_TOKENS.length];
}

/**
 * The letters on a person's chip, one rule for every surface that draws one
 * (#3660): the table read "AD" for `admin@…` while the rail's chip read "A".
 *
 * Two words give their first letters ("Ada Lovelace" is "AL"); one word, or an
 * email, gives its first letter alone ("admin@rolter.local" is "A"). An email
 * counts only its local part, so the domain never lends a second letter.
 */
export function initialsOf(name: string): string {
  const local = name.trim().split("@")[0] ?? "";
  const words = local.split(/[\s._-]+/u).filter(Boolean);
  const letters = (name.includes("@") ? words.slice(0, 1) : words.slice(0, 2)).map(
    (word) => Array.from(word)[0] ?? "",
  );
  return letters.join("").toUpperCase() || "?";
}
