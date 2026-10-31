import type { RouteVariantInput, RouteVariantRow } from "@/lib/api";

/**
 * The route sheet's draft of a route's weighted variants (#391), and the rules
 * `PUT /api/v1/routes/{id}/variants` holds it to, so the sheet refuses what the
 * control plane would refuse before anything is sent.
 *
 * Weights are kept as the text the operator typed, so a half-typed value is a
 * draft rather than a `NaN`.
 */
export interface DraftVariantTarget {
  key: string;
  providerId: string;
  /** the model id sent upstream; blank forwards the route's public name */
  upstream: string;
  weight: string;
  /** the failure classes the target is a fallback for (#3373); empty for an ordinary target */
  takesOver: string[];
}

export interface DraftVariant {
  key: string;
  name: string;
  weight: string;
  /** stored param defaults, carried through a save unchanged */
  params: Record<string, unknown>;
  targets: DraftVariantTarget[];
}

let keys = 0;
const nextKey = (prefix: string) => {
  keys += 1;
  return `${prefix}${keys}`;
};

export function newVariantTarget(
  providerId: string,
  upstream = "",
  weight = "1",
  takesOver: string[] = [],
) {
  return {
    key: nextKey("vt"),
    providerId,
    upstream,
    weight,
    takesOver,
  } satisfies DraftVariantTarget;
}

export function newVariant(name: string, providerId: string): DraftVariant {
  return {
    key: nextKey("v"),
    name,
    weight: "1",
    params: {},
    targets: [newVariantTarget(providerId)],
  };
}

/** `canary`, `variant-2`, …: the first `variant-<n>` no variant is called yet */
export function nextVariantName(variants: DraftVariant[]): string {
  const taken = new Set(variants.map((v) => v.name.trim()));
  let n = variants.length + 1;
  while (taken.has(`variant-${n}`)) n += 1;
  return `variant-${n}`;
}

export function variantsFromRows(rows: RouteVariantRow[]): DraftVariant[] {
  return rows.map((row) => ({
    key: nextKey("v"),
    name: row.name,
    weight: String(row.weight),
    params: row.params ?? {},
    targets: row.targets.map((tg) =>
      newVariantTarget(
        tg.provider_id,
        tg.upstream_model ?? "",
        String(tg.weight),
        tg.takes_over ?? [],
      ),
    ),
  }));
}

export function variantsToApi(variants: DraftVariant[]): RouteVariantInput[] {
  return variants.map((v) => ({
    name: v.name.trim(),
    weight: Number(v.weight),
    params: v.params,
    targets: v.targets.map((tg) => ({
      provider_id: tg.providerId,
      upstream_model: tg.upstream.trim() === "" ? null : tg.upstream.trim(),
      weight: Number(tg.weight),
      takes_over: tg.takesOver,
    })),
  }));
}

/** the most variants a route takes, and targets a variant takes (`MAX_VARIANTS` in the API) */
export const MAX_VARIANTS = 32;
export const MAX_VARIANT_TARGETS = 32;

/** a whole number, 0 or more, that fits the stored integer */
export function wholeWeight(weight: string): boolean {
  return /^\d+$/.test(weight.trim()) && Number(weight) <= 2_147_483_647;
}

/**
 * A variant weight the control plane accepts: a whole number, 0 or more. A 0
 * means "fallback only" to the gateway (#3341, #3427).
 */
export function variantWeightValid(weight: string): boolean {
  return wholeWeight(weight);
}

/**
 * The share of first attempts each weight gets, as the gateway samples them.
 *
 * The gateway never samples a weight of 0 as a primary while another variant
 * has a positive weight (it is still tried as a fallback), and samples a set
 * whose weights are all 0 uniformly (`weighted_index` in `rolter-core`). The
 * sheet and the control plane accept a 0 (#3427).
 */
export function trafficSplit(weights: number[]): number[] {
  if (weights.length === 0) return [];
  const whole = weights.map((w) => Math.max(0, Math.floor(w)));
  const uniform = whole.every((w) => w === 0);
  const sampled = uniform ? whole.map(() => 1) : whole;
  const total = sampled.reduce((sum, w) => sum + w, 0);
  return sampled.map((w) => w / total);
}

/** What is wrong with one variant, each a key under `routeSheet.variants.errors`. */
export interface VariantErrors {
  name?: "nameRequired" | "nameTaken";
  weight?: "weight";
  targets?: "targetsRequired" | "targetsAllZero" | "targetsTooMany";
  /** target keys whose provider is unset */
  provider: Set<string>;
  /** target keys whose weight is not a whole number 0 or more */
  targetWeight: Set<string>;
}

export interface VariantValidation {
  byVariant: Map<string, VariantErrors>;
  /** more variants than a route takes */
  tooMany: boolean;
  /** how many separate problems there are, for the sheet's summary */
  count: number;
}

export function validateVariants(variants: DraftVariant[]): VariantValidation {
  const byVariant = new Map<string, VariantErrors>();
  const seen = new Set<string>();
  let count = 0;
  for (const v of variants) {
    const errors: VariantErrors = { provider: new Set(), targetWeight: new Set() };
    const name = v.name.trim();
    if (name === "") errors.name = "nameRequired";
    else if (seen.has(name)) errors.name = "nameTaken";
    seen.add(name);
    if (!variantWeightValid(v.weight)) errors.weight = "weight";
    if (v.targets.length === 0) errors.targets = "targetsRequired";
    for (const tg of v.targets) {
      if (tg.providerId === "") errors.provider.add(tg.key);
      if (!wholeWeight(tg.weight)) errors.targetWeight.add(tg.key);
    }
    if (v.targets.length > MAX_VARIANT_TARGETS) errors.targets = "targetsTooMany";
    if (
      !errors.targets &&
      v.targets.length > 0 &&
      errors.targetWeight.size === 0 &&
      v.targets.every((tg) => Number(tg.weight) === 0)
    ) {
      errors.targets = "targetsAllZero";
    }
    count +=
      Number(!!errors.name) +
      Number(!!errors.weight) +
      Number(!!errors.targets) +
      Number(errors.provider.size > 0) +
      Number(errors.targetWeight.size > 0);
    byVariant.set(v.key, errors);
  }
  const tooMany = variants.length > MAX_VARIANTS;
  if (tooMany) count += 1;
  return { byVariant, tooMany, count };
}

/** Move the entry at `from` one place up (`-1`) or down (`1`). */
export function moved<T>(list: T[], from: number, by: -1 | 1): T[] {
  const to = from + by;
  if (to < 0 || to >= list.length) return list;
  const next = [...list];
  [next[from], next[to]] = [next[to], next[from]];
  return next;
}

/**
 * A field the control plane refused, mapped back onto the draft:
 * `variants`, `variants[1].weight`, `variants[0].targets[2].weight`.
 */
export interface VariantRefusal {
  /**
   * the control plane's own sentence, for a refusal of the set as a whole: one
   * field name (`variants`) covers more than one rule, so it is said as sent
   */
  message?: string;
  /** absent for the set as a whole */
  variantKey?: string;
  targetKey?: string;
  field:
    | "variants"
    | "name"
    | "weight"
    | "targets"
    | "params"
    | "targetWeight"
    | "provider"
    | "takesOver";
}

export function variantRefusalOf(
  field: string,
  variants: DraftVariant[],
  message?: string,
): VariantRefusal | null {
  if (field === "variants") return { field: "variants", message };
  const match =
    /^variants\[(\d+)\]\.(name|weight|params|targets)(?:\[(\d+)\]\.(weight|provider_id|takes_over))?$/.exec(
      field,
    );
  if (!match) return null;
  const variant = variants[Number(match[1])];
  if (!variant) return null;
  if (match[3] === undefined) {
    return { variantKey: variant.key, field: match[2] as VariantRefusal["field"] };
  }
  const target = variant.targets[Number(match[3])];
  if (!target) return null;
  return {
    variantKey: variant.key,
    targetKey: target.key,
    field:
      match[4] === "weight" ? "targetWeight" : match[4] === "takes_over" ? "takesOver" : "provider",
  };
}
