import { describe, expect, it } from "bun:test";

import type { MyBudgetRow } from "@/lib/api";
import { capReach, myBudgetFigures } from "./my-budget";

const budget = (over: Partial<MyBudgetRow> = {}): MyBudgetRow => ({
  id: "b1",
  limit_usd: "50.0000",
  period: "monthly",
  within_org_id: null,
  within_project_id: null,
  spent_usd: "12.5000",
  resets_at: null,
  ...over,
});

describe("capReach", () => {
  it("counts every key when neither narrowing is set", () => {
    expect(capReach({ within_org_id: null, within_project_id: null })).toBe("all");
    expect(capReach({})).toBe("all");
  });
  it("narrows to an org or a project, the project winning", () => {
    expect(capReach({ within_org_id: "o" })).toBe("org");
    expect(capReach({ within_project_id: "p" })).toBe("project");
    expect(capReach({ within_org_id: "o", within_project_id: "p" })).toBe("project");
  });
});

describe("myBudgetFigures", () => {
  it("leaves what is left of the cap", () => {
    const f = myBudgetFigures(budget(), "ok");
    expect(f?.remaining).toBe(37.5);
    expect(f?.tone).toBe("ok");
  });
  it("floors the remainder at zero and flags the cap as reached", () => {
    const f = myBudgetFigures(budget({ spent_usd: "61" }), "ok");
    expect(f?.remaining).toBe(0);
    expect(f?.tone).toBe("danger");
  });
  it("is unknown, never zero, for a null counter", () => {
    expect(myBudgetFigures(budget({ spent_usd: null }), "ok")).toBeNull();
  });
  it("is unknown whenever the report is not ok", () => {
    expect(myBudgetFigures(budget(), "unavailable")).toBeNull();
    expect(myBudgetFigures(budget(), "not_configured")).toBeNull();
  });
});
