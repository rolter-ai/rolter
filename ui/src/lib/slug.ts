// the slug the control plane derives from a display name, mirrored so a form can
// preview it before the round trip.
//
// the rule is `rolter_core::slug::slugify`, which `resolve_new_slug` in
// crates/rolter-control/src/crud.rs applies to a create that sent no slug:
// lower-case, every run of characters outside `a-z0-9` becomes one hyphen, no
// hyphen at either end, at most 63 characters. `slug.test.ts` carries the Rust
// unit tests' cases. A name with no ASCII letter or digit derives nothing.

/** the longest slug the control plane accepts, in characters */
export const SLUG_MAX_LEN = 63;

/** The slug the control plane would derive from `name`, or `""` when it derives none. */
export function slugify(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, SLUG_MAX_LEN);
}
