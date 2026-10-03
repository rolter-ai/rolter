import { describe, expect, test } from "bun:test";

import { SCREENS } from "@/App";
import { NAV, leafKeys } from "@/lib/nav";

// the nav and the route table are two lists of the same thing, and until #1201
// nothing held them to each other: `App` looked a key up in a `BUILT` set and
// fell back to a branded `Stub` screen for anything missing. Every leaf had
// long since been built, so that branch was unreachable — it rendered nowhere
// while still promising the nav could name a screen that does not exist.
//
// Deleting the fallback is only safe while the two sets agree, so this is the
// test that keeps them agreeing: add a nav entry without a screen and it fails
// here rather than rendering a blank panel in production.
describe("nav", () => {
  test("every navigable leaf has a screen", () => {
    expect([...leafKeys()].sort()).toEqual(Object.keys(SCREENS).sort());
  });

  test("no screen is unreachable from the nav", () => {
    const leaves = new Set(leafKeys());
    expect(Object.keys(SCREENS).filter((k) => !leaves.has(k))).toEqual([]);
  });

  // a duplicate key would make the sets compare equal while `<Routes>` mounted
  // the same path twice, so it is worth its own assertion
  test("leaf keys are unique", () => {
    const keys = leafKeys();
    expect(keys.length).toBe(new Set(keys).size);
  });

  // parents are toggles, not destinations: a group with children never becomes
  // a route, which is why `leafKeys` recurses instead of flattening
  test("a group with children contributes only its children", () => {
    const groups = NAV.filter((d) => d.children);
    expect(groups.length).toBeGreaterThan(0);
    for (const group of groups) {
      expect(leafKeys()).not.toContain(group.key);
    }
  });
});
