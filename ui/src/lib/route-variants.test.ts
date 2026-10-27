import { describe, expect, it } from "bun:test";

import type { RouteVariantRow } from "@/lib/api";
import {
  moved,
  newVariant,
  newVariantTarget,
  nextVariantName,
  trafficSplit,
  validateVariants,
  variantRefusalOf,
  variantsFromRows,
  variantsToApi,
} from "@/lib/route-variants";

const row = (name: string, weight: number): RouteVariantRow => ({
  id: `id-${name}`,
  route_id: "route-1",
  name,
  weight,
  position: 0,
  params: { temperature: 0.7 },
  created_at: "2026-10-01T00:00:00Z",
  targets: [
    {
      id: "t1",
      variant_id: `id-${name}`,
      provider_id: "prov-1",
      upstream_model: null,
      weight: 2,
      position: 0,
    },
  ],
});

describe("trafficSplit", () => {
  it("splits proportionally", () => {
    expect(trafficSplit([9, 1])).toEqual([0.9, 0.1]);
  });

  it("counts a zero weight as 1, the way the gateway samples it", () => {
    expect(trafficSplit([3, 0])).toEqual([0.75, 0.25]);
  });

  it("has no split without variants", () => {
    expect(trafficSplit([])).toEqual([]);
  });
});

describe("validateVariants", () => {
  it("accepts a well-formed set, a fallback-only variant included", () => {
    const a = newVariant("stable", "prov-1");
    const b = { ...newVariant("canary", "prov-2"), weight: "0" };
    const result = validateVariants([a, b]);
    expect(result.count).toBe(0);
    expect(result.allZero).toBe(false);
  });

  it("refuses weights that are all zero, negative or not whole", () => {
    const a = { ...newVariant("a", "p"), weight: "0" };
    const b = { ...newVariant("b", "p"), weight: "0" };
    expect(validateVariants([a, b]).allZero).toBe(true);
    const negative = { ...newVariant("c", "p"), weight: "-1" };
    expect(validateVariants([negative]).byVariant.get(negative.key)?.weight).toBe("weight");
    const fraction = { ...newVariant("d", "p"), weight: "1.5" };
    expect(validateVariants([fraction]).byVariant.get(fraction.key)?.weight).toBe("weight");
  });

  it("needs a unique name and at least one target with weight", () => {
    const a = newVariant("same", "p");
    const b = { ...newVariant(" same ", "p"), targets: [] };
    const c = { ...newVariant("", "p") };
    const idle = { ...newVariant("idle", "p"), targets: [newVariantTarget("p", "", "0")] };
    const unset = { ...newVariant("unset", "") };
    const result = validateVariants([a, b, c, idle, unset]);
    expect(result.byVariant.get(b.key)?.name).toBe("nameTaken");
    expect(result.byVariant.get(b.key)?.targets).toBe("targetsRequired");
    expect(result.byVariant.get(c.key)?.name).toBe("nameRequired");
    expect(result.byVariant.get(idle.key)?.targets).toBe("targetsAllZero");
    expect(result.byVariant.get(unset.key)?.provider.size).toBe(1);
    expect(result.count).toBe(5);
  });
});

describe("round trip", () => {
  it("sends back what it read, blank upstream as null and params kept", () => {
    const drafts = variantsFromRows([row("stable", 9)]);
    expect(variantsToApi(drafts)).toEqual([
      {
        name: "stable",
        weight: 9,
        params: { temperature: 0.7 },
        targets: [{ provider_id: "prov-1", upstream_model: null, weight: 2 }],
      },
    ]);
  });
});

describe("ordering and naming", () => {
  it("moves one place and stays put at an end", () => {
    expect(moved(["a", "b", "c"], 0, 1)).toEqual(["b", "a", "c"]);
    expect(moved(["a", "b", "c"], 0, -1)).toEqual(["a", "b", "c"]);
    expect(moved(["a", "b", "c"], 2, 1)).toEqual(["a", "b", "c"]);
  });

  it("offers the first free variant-<n> name", () => {
    const drafts = [newVariant("variant-2", "p")];
    expect(nextVariantName(drafts)).toBe("variant-3");
    expect(nextVariantName([])).toBe("variant-1");
  });
});

describe("variantRefusalOf", () => {
  it("maps a refused field back onto the draft", () => {
    const drafts = variantsFromRows([row("a", 1), row("b", 1)]);
    expect(variantRefusalOf("variants", drafts)).toEqual({ field: "variants" });
    expect(variantRefusalOf("variants[1].weight", drafts)).toEqual({
      variantKey: drafts[1].key,
      field: "weight",
    });
    expect(variantRefusalOf("variants[0].targets[0].weight", drafts)).toEqual({
      variantKey: drafts[0].key,
      targetKey: drafts[0].targets[0].key,
      field: "targetWeight",
    });
    expect(variantRefusalOf("variants[5].name", drafts)).toBeNull();
    expect(variantRefusalOf("targets[0].weight", drafts)).toBeNull();
  });
});
