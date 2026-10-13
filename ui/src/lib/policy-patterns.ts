// the matcher in crates/rolter-core/src/access_policy.rs reads a policy line as
// an exact name, or as a prefix when it ends in `*`. a `*` anywhere else is a
// literal character, so `o1-*-mini` names a model nobody serves: in an allow-list
// that one line permits nothing, in a deny-list it blocks nothing

/** The lines of a one-pattern-per-line field that carry a `*` the matcher takes literally. */
export function misplacedWildcards(raw: string): string[] {
  return raw
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.slice(0, -1).includes("*"));
}
