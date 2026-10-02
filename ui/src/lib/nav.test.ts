import { describe, expect, test } from "bun:test";

import { SCREENS } from "@/App";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { NAV, leafKeys, visibleNav } from "@/lib/nav";

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

  // the role matrix describes what roles can do, not anyone's data, so no
  // capability may hide it (#2527): a project viewer reads it like anyone
  test("Roles & Permissions stays in the rail when nothing is readable", () => {
    const rail = visibleNav(() => false).flatMap((d) => d.children ?? [d]);
    expect(rail.map((d) => d.key)).toContain("rbac");
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

  // two leaves with one label are told apart only by the group they sit under,
  // which the collapsed rail, the palette and a screen reader do not say: the
  // observability "Dashboard" and the adaptive routing one were both
  // "Dashboard" (#1994), and the alerting and guardrail "Rules" (#2430)

  for (const [name, catalog] of [
    ["en", en],
    ["ru", ru],
  ] as const) {
    test(`no two leaves share a label in ${name}`, () => {
      const labels = catalog.nav as Record<string, string>;
      const byLabel = new Map<string, string[]>();
      for (const key of leafKeys()) {
        byLabel.set(labels[key], [...(byLabel.get(labels[key]) ?? []), key]);
      }
      const shared = [...byLabel.values()].filter((keys) => keys.length > 1);
      expect(shared).toEqual([]);
    });
  }
});
