import { describe, expect, test } from "bun:test";

import {
  allRestricted,
  onlyRestrictedDialled,
  isRestricted,
  refusalCategories,
  refusalCategoryValid,
  toggled,
} from "./failure-classes";

describe("failure classes", () => {
  test("a set with any class is restricted", () => {
    expect(isRestricted([])).toBe(false);
    expect(isRestricted(undefined)).toBe(false);
    expect(isRestricted(["throttled"])).toBe(true);
  });

  test("toggling keeps the order the set was built in", () => {
    expect(toggled(["timeout"], "throttled")).toEqual(["timeout", "throttled"]);
    expect(toggled(["timeout", "throttled"], "timeout")).toEqual(["throttled"]);
  });

  test("a refusal category is what the API takes", () => {
    expect(refusalCategoryValid("content_policy_violation")).toBe(true);
    expect(refusalCategoryValid("self-harm")).toBe(true);
    expect(refusalCategoryValid("")).toBe(false);
    expect(refusalCategoryValid("Violence")).toBe(false);
    expect(refusalCategoryValid("a b")).toBe(false);
    expect(refusalCategoryValid("x".repeat(65))).toBe(false);
    expect(refusalCategories(["refusal", "refusal:violence"])).toEqual(["violence"]);
  });

  test("a list of only fallbacks is refused, an empty one is not", () => {
    expect(allRestricted([])).toBe(false);
    expect(allRestricted([["refusal"], []])).toBe(false);
    expect(allRestricted([["refusal"], ["timeout"]])).toBe(true);
  });

  test("a route with variants dials only the variants' targets (#3487)", () => {
    // without variants the route's own list is dialled
    expect(onlyRestrictedDialled([["refusal"]], [])).toBe(true);
    expect(onlyRestrictedDialled([[], ["refusal"]], [])).toBe(false);
    // fallback-only variants strand the route over an ordinary list
    expect(onlyRestrictedDialled([[]], [[["throttled"]], [["timeout"]]])).toBe(true);
    // an ordinary variant target serves over a fallback-only list
    expect(onlyRestrictedDialled([["refusal"]], [[["throttled"], []]])).toBe(false);
    expect(onlyRestrictedDialled([], [])).toBe(false);
  });
});
