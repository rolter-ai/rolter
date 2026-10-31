import { describe, expect, test } from "bun:test";

import type { ProviderCacheVerdict } from "@/lib/api";
import { summarizeCacheReporting } from "./cache-reporting";

const v = (id: string, reports: boolean, at = "2026-10-01T00:00:00Z"): ProviderCacheVerdict => ({
  provider_id: id,
  reports,
  spelling: reports ? "cached_tokens" : null,
  model: "m",
  probed_at: at,
});
const by = (...rows: ProviderCacheVerdict[]) => new Map(rows.map((r) => [r.provider_id, r]));

describe("summarizeCacheReporting", () => {
  test("nothing to judge without providers", () => {
    expect(summarizeCacheReporting([], by())).toBeNull();
  });

  test("a provider never probed is unprobed, never 'none'", () => {
    expect(summarizeCacheReporting(["a"], by())?.status).toBe("unprobed");
    expect(summarizeCacheReporting([undefined], by())?.status).toBe("unprobed");
  });

  test("reports only when every provider reports", () => {
    expect(summarizeCacheReporting(["a", "b"], by(v("a", true), v("b", true)))?.status).toBe(
      "reports",
    );
    expect(summarizeCacheReporting(["a", "b"], by(v("a", true)))?.status).toBe("unprobed");
  });

  test("any known silent provider makes it 'none'", () => {
    const r = summarizeCacheReporting(["a", "b"], by(v("a", false)));
    expect(r?.status).toBe("none");
    expect(summarizeCacheReporting(["a", "b"], by(v("a", true), v("b", false)))?.status).toBe(
      "none",
    );
  });

  test("the time is the oldest verdict the claim rests on", () => {
    const r = summarizeCacheReporting(
      ["a", "b"],
      by(v("a", true, "2026-10-05T00:00:00Z"), v("b", true, "2026-09-01T00:00:00Z")),
    );
    expect(r && r.status !== "unprobed" && r.probedAt).toBe("2026-09-01T00:00:00Z");
  });

  test("a repeated provider counts once and a single one carries its model", () => {
    const r = summarizeCacheReporting(["a", "a"], by(v("a", true)));
    expect(r).toMatchObject({ status: "reports", total: 1, model: "m", spelling: "cached_tokens" });
  });
});
