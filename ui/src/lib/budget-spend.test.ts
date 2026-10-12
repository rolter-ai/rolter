import { describe, expect, test } from "bun:test";

import { spendFigures } from "./budget-spend";

const row = (spent: string | null) => ({ id: "b", spent_usd: spent, resets_at: null });

describe("spendFigures", () => {
  test("an unreadable counter has no figures, not zero", () => {
    expect(spendFigures(row(null), "100")).toBeNull();
    expect(spendFigures(undefined, "100")).toBeNull();
    expect(spendFigures(row("nope"), "100")).toBeNull();
  });

  test("a real zero is a figure", () => {
    expect(spendFigures(row("0.0000"), "100")).toMatchObject({ remaining: 100, tone: "ok" });
  });

  test("warns at 80% and turns to danger at the cap", () => {
    expect(spendFigures(row("79.9"), "100")?.tone).toBe("ok");
    expect(spendFigures(row("80"), "100")?.tone).toBe("warning");
    expect(spendFigures(row("100"), "100")?.tone).toBe("danger");
  });

  test("overspend clamps the bar and the remainder but not the ratio", () => {
    const f = spendFigures(row("112"), "100");
    expect(f).toMatchObject({ remaining: 0, fraction: 1, tone: "danger" });
    expect(f?.ratio).toBeCloseTo(1.12);
  });

  test("a zero cap is spent", () => {
    expect(spendFigures(row("0"), "0")?.tone).toBe("danger");
  });
});
