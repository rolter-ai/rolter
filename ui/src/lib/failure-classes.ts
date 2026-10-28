/**
 * The failure classes a route target can take over (#1584, #3319): a target
 * with any is a fallback, never dialled first, and reached only after another
 * target failed with one of them. The tokens are the API's
 * (`rolter_core::failover::FailureClass`), so they are notation and never
 * translated: the editor shows them as they are sent, and its hint and the
 * fallback-targets docs say what each one covers.
 */
export const FAILURE_CLASSES = [
  "throttled",
  "timeout",
  "transport",
  "server_error",
  "refusal",
] as const;

export type FailureClassToken = (typeof FAILURE_CLASSES)[number];

/** the prefix of a refusal narrowed to one category, `refusal:<category>` */
export const REFUSAL_PREFIX = "refusal:";

/** the longest refusal category the control plane takes */
export const MAX_REFUSAL_CATEGORY_LEN = 64;

/** A refusal category the control plane accepts: 1 to 64 of `a-z`, `0-9`, `_`, `-`. */
export function refusalCategoryValid(category: string): boolean {
  return new RegExp(`^[a-z0-9_-]{1,${MAX_REFUSAL_CATEGORY_LEN}}$`).test(category);
}

/** Whether a target with this set is a restricted fallback rather than a pool member. */
export function isRestricted(takesOver: readonly string[] | undefined): boolean {
  return (takesOver?.length ?? 0) > 0;
}

/** the categories of the `refusal:<category>` entries of a set, in order */
export function refusalCategories(takesOver: readonly string[]): string[] {
  return takesOver
    .filter((token) => token.startsWith(REFUSAL_PREFIX))
    .map((token) => token.slice(REFUSAL_PREFIX.length));
}

/**
 * Turn one class on or off in a set, keeping the order the operator built it
 * in, so a save of an unchanged set sends what is stored.
 */
export function toggled(takesOver: readonly string[], token: string): string[] {
  return takesOver.includes(token)
    ? takesOver.filter((entry) => entry !== token)
    : [...takesOver, token];
}

/**
 * Whether every target of a list is a restricted fallback, which the control
 * plane refuses as `all_targets_restricted`: such a route could never dial
 * anything. An empty list is not, it has nothing to dial for another reason.
 */
export function allRestricted(sets: readonly (readonly string[] | undefined)[]): boolean {
  return sets.length > 0 && sets.every((set) => isRestricted(set));
}

/** the stable code the control plane names that refusal by */
export const ALL_TARGETS_RESTRICTED = "all_targets_restricted";
