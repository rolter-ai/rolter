import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { fileURLToPath, URL } from "node:url";

import {
  type Alert,
  type ClosedPull,
  type Manifest,
  SUBJECT,
  applyPlan,
  declinedBy,
  planMarker,
  planUpdates,
  renderCommitMessage,
  renderPullBody,
  renderSummary,
  renderWithdrawal,
} from "./security-updates";

const UI_MANIFEST = fileURLToPath(new URL("../package.json", import.meta.url));

let next = 1;

function alert(
  name: string,
  patched: string | null,
  overrides: {
    state?: string;
    ecosystem?: string;
    manifest?: string;
    vulnerable?: string;
    number?: number;
  } = {},
): Alert {
  const number = overrides.number ?? next++;
  return {
    number,
    state: overrides.state ?? "open",
    html_url: `https://github.com/rolter-ai/rolter/security/dependabot/${number}`,
    dependency: {
      package: { ecosystem: overrides.ecosystem ?? "npm", name },
      manifest_path: overrides.manifest ?? "ui/package.json",
      relationship: "direct",
    },
    security_advisory: { ghsa_id: `GHSA-${number}`, severity: "high" },
    security_vulnerability: {
      vulnerable_version_range: overrides.vulnerable ?? `< ${patched ?? "9.9.9"}`,
      first_patched_version: patched === null ? null : { identifier: patched },
    },
  };
}

describe("planUpdates", () => {
  it("raises a range to the first patched release and keeps its operator", () => {
    const manifest: Manifest = {
      dependencies: { caret: "^1.2.0", tilde: "~2.0.1" },
      devDependencies: { exact: "3.4.5" },
    };
    const plan = planUpdates(
      [alert("caret", "1.2.7"), alert("tilde", "2.0.4"), alert("exact", "3.4.9")],
      manifest,
    );
    expect(plan.skipped).toEqual([]);
    expect(plan.bumps.map((b) => [b.name, b.section, b.from, b.to])).toEqual([
      ["caret", "dependencies", "^1.2.0", "^1.2.7"],
      ["exact", "devDependencies", "3.4.5", "3.4.9"],
      ["tilde", "dependencies", "~2.0.1", "~2.0.4"],
    ]);
  });

  it("aims for the highest first patched release when one package has several alerts", () => {
    // one bump has to clear every advisory; the lowest patched release would
    // leave the later ones open
    const plan = planUpdates(
      [alert("vite", "8.3.2"), alert("vite", "8.4.1"), alert("vite", "8.3.9")],
      {
        devDependencies: { vite: "^8.3.0" },
      },
    );
    expect(plan.bumps).toHaveLength(1);
    expect(plan.bumps[0].to).toBe("^8.4.1");
    expect(plan.bumps[0].alerts.map((a) => a.patched)).toEqual(["8.3.2", "8.4.1", "8.3.9"]);
  });

  it("reads only open npm alerts filed against a manifest directly in ui/", () => {
    const manifest: Manifest = { dependencies: { left: "^1.0.0" } };
    const plan = planUpdates(
      [
        alert("left", "1.0.1", { state: "dismissed" }),
        alert("left", "1.0.1", { state: "fixed" }),
        alert("left", "1.0.1", { ecosystem: "rust" }),
        alert("left", "1.0.1", { manifest: "package.json" }),
        alert("left", "1.0.1", { manifest: "integration/e2e/package.json" }),
        alert("left", "1.0.1", { manifest: "ui/e2e/fixtures/package.json" }),
      ],
      manifest,
    );
    expect(plan).toEqual({ bumps: [], skipped: [] });
  });

  it("raises a dependency listed in both sections in both", () => {
    const plan = planUpdates([alert("dual", "1.1.0")], {
      dependencies: { dual: "^1.0.0" },
      devDependencies: { dual: "~1.0.3" },
    });
    expect(plan.bumps.map((b) => [b.section, b.to])).toEqual([
      ["dependencies", "^1.1.0"],
      ["devDependencies", "~1.1.0"],
    ]);
  });

  it("leaves an alert with no patched release for a human, but still raises the patched ones", () => {
    const plan = planUpdates([alert("pkg", null), alert("pkg", "2.1.0")], {
      dependencies: { pkg: "^2.0.0" },
    });
    expect(plan.bumps.map((b) => b.to)).toEqual(["^2.1.0"]);
    expect(plan.skipped).toHaveLength(1);
    expect(plan.skipped[0].reason).toContain("no patched release");
    expect(plan.skipped[0].alerts.map((a) => a.patched)).toEqual([null]);
  });

  it("does not raise a package that is not a direct dependency", () => {
    // a transitive fix is an `overrides` entry, which changes what every
    // other dependent resolves; that is a judgement call, not a bump
    const plan = planUpdates([alert("brace-expansion", "2.0.2", { manifest: "ui/bun.lock" })], {
      dependencies: { react: "^19.0.0" },
    });
    expect(plan.bumps).toEqual([]);
    expect(plan.skipped.map((s) => s.name)).toEqual(["brace-expansion"]);
    expect(plan.skipped[0].reason).toContain("not a direct dependency");
  });

  it("does not rewrite a range shape it cannot raise safely", () => {
    for (const spec of [
      ">=1.0.0",
      "1.x || 2.x",
      "*",
      "latest",
      "workspace:*",
      "npm:other@^1.0.0",
    ]) {
      const plan = planUpdates([alert("odd", "1.0.5")], { dependencies: { odd: spec } });
      expect(plan.bumps).toEqual([]);
      expect(plan.skipped[0].reason).toContain("not a plain");
    }
  });

  it("does not start a breaking upgrade", () => {
    const major = planUpdates([alert("lib", "3.0.0")], { dependencies: { lib: "^2.9.0" } });
    expect(major.bumps).toEqual([]);
    expect(major.skipped[0].reason).toContain("breaking bump");

    // below 1.0 a minor release is the breaking one, as `^` reads it
    const zero = planUpdates([alert("otel", "0.223.0")], { dependencies: { otel: "^0.222.0" } });
    expect(zero.bumps).toEqual([]);
    expect(zero.skipped[0].reason).toContain("breaking bump");

    const zeroPatch = planUpdates([alert("otel", "0.222.4")], {
      dependencies: { otel: "^0.222.0" },
    });
    expect(zeroPatch.bumps.map((b) => b.to)).toEqual(["^0.222.4"]);
  });

  it("raises the compatible fix even when another advisory on the package is patched only in the next major", () => {
    // judging the package by its highest patched release would call the whole
    // bump breaking and drop the fix that clears the first alert
    const plan = planUpdates(
      [alert("vite", "8.3.2", { number: 1 }), alert("vite", "9.0.0", { number: 2 })],
      { devDependencies: { vite: "^8.3.0" } },
    );
    expect(plan.bumps.map((b) => [b.to, b.alerts.map((a) => a.number)])).toEqual([["^8.3.2", [1]]]);
    expect(plan.skipped).toHaveLength(1);
    expect(plan.skipped[0].reason).toContain(
      "the first patched release, 9.0.0, is a breaking bump",
    );
    expect(plan.skipped[0].alerts.map((a) => a.number)).toEqual([2]);
  });

  it("reports an alert the floor already clears apart from the one it still raises", () => {
    const plan = planUpdates(
      [alert("pkg", "1.2.0", { number: 1 }), alert("pkg", "1.4.0", { number: 2 })],
      { dependencies: { pkg: "^1.3.0" } },
    );
    expect(plan.bumps.map((b) => [b.to, b.alerts.map((a) => a.number)])).toEqual([["^1.4.0", [2]]]);
    expect(plan.skipped.map((s) => [s.reason, s.alerts.map((a) => a.number)])).toEqual([
      ["`^1.3.0` in dependencies already starts at or above 1.2.0", [1]],
    ]);
  });

  it("raises a tilde range past its own upper bound when the release is still semver-compatible", () => {
    // `~1.2.0` stops before 1.3.0, but 1.3.0 is a minor release: raising the
    // floor to it is the same kind of change a `^` bump is
    const plan = planUpdates([alert("tl", "1.3.0")], { dependencies: { tl: "~1.2.0" } });
    expect(plan.bumps.map((b) => b.to)).toEqual(["~1.3.0"]);
  });

  it("reports a range that already starts at a patched release instead of rewriting it", () => {
    const plan = planUpdates([alert("fresh", "1.4.0")], { dependencies: { fresh: "^1.4.2" } });
    expect(plan.bumps).toEqual([]);
    expect(plan.skipped[0].reason).toContain("already starts at or above 1.4.0");
  });

  it("lists packages in name order so the pull request diff is stable run to run", () => {
    const plan = planUpdates([alert("zeta", "1.0.1"), alert("alpha", "1.0.1")], {
      dependencies: { alpha: "^1.0.0", zeta: "^1.0.0" },
    });
    expect(plan.bumps.map((b) => b.name)).toEqual(["alpha", "zeta"]);
  });
});

describe("applyPlan", () => {
  it("leaves the real ui/package.json byte for byte when there is nothing to raise", () => {
    // the rewrite goes through JSON.stringify; if that ever stops matching
    // the committed formatting, every automated bump would reflow the file
    const text = readFileSync(UI_MANIFEST, "utf8");
    expect(applyPlan(text, { bumps: [], skipped: [] })).toBe(text);
  });

  it("changes only the raised lines of the real ui/package.json", () => {
    const text = readFileSync(UI_MANIFEST, "utf8");
    const manifest = JSON.parse(text) as Required<Manifest>;
    const [name, spec] = Object.entries(manifest.dependencies)[0];
    const plan = {
      bumps: [{ name, section: "dependencies" as const, from: spec, to: "^99.0.0", alerts: [] }],
      skipped: [],
    };

    const before = text.split("\n");
    const after = applyPlan(text, plan).split("\n");
    expect(after).toHaveLength(before.length);
    const changed = after.filter((line, i) => line !== before[i]);
    expect(changed).toEqual([`    "${name}": "^99.0.0",`]);
  });
});

describe("renderSummary", () => {
  it("says so when there is no open alert", () => {
    expect(renderSummary({ bumps: [], skipped: [] })).toBe(
      "No open Dependabot alert on `ui/` dependencies.\n",
    );
  });

  it("tables the raised ranges and what was left for a hand bump", () => {
    const plan = planUpdates(
      [alert("vite", "8.3.2", { number: 7 }), alert("left-pad", null, { number: 8 })],
      {
        devDependencies: { vite: "^8.3.0", "left-pad": "^1.0.0" },
      },
    );
    const summary = renderSummary(plan);
    expect(summary).toContain(
      "| `vite` | devDependencies | `^8.3.0` | `^8.3.2` | [#7](https://github.com/rolter-ai/rolter/security/dependabot/7) high GHSA-7 (`< 8.3.2`) |",
    );
    expect(summary).toContain("Left for a hand bump:");
    expect(summary).toContain("| `left-pad` | no patched release has been published yet |");
  });

  it("says nothing was raised when every alert needs a human", () => {
    const summary = renderSummary(
      planUpdates([alert("x", null)], { dependencies: { x: "^1.0.0" } }),
    );
    expect(summary).toContain("Nothing to raise automatically.");
  });

  it("escapes a pipe so a range like `1.x || 2.x` cannot split a table cell", () => {
    const plan = planUpdates([alert("odd", "1.0.5", { vulnerable: ">= 1.0.0 || < 0.9" })], {
      dependencies: { odd: "1.x || 2.x" },
    });
    const row = renderSummary(plan)
      .split("\n")
      .find((line) => line.startsWith("| `odd`"));
    expect(row).toBeDefined();
    // a three-cell row has four bare pipes and so splits into five pieces;
    // an unescaped `||` from the range would add two more
    expect(row?.replace(/\\\|/g, "").split("|")).toHaveLength(5);
  });

  it("escapes a backslash before the pipe it precedes", () => {
    // `a\|b` escaped pipe-only would read as an escaped backslash and then a
    // bare pipe, which splits the cell all the same
    const plan = planUpdates([alert("odd", "1.0.5")], { dependencies: { odd: "a\\|b" } });
    const row = renderSummary(plan)
      .split("\n")
      .find((line) => line.startsWith("| `odd`"));
    expect(row).toContain("`a\\\\\\|b`");
  });
});

describe("planMarker", () => {
  it("names each raised range and target, in a stable order", () => {
    const plan = planUpdates([alert("zeta", "1.0.1"), alert("@scope/alpha", "2.0.3")], {
      dependencies: { "@scope/alpha": "^2.0.0", zeta: "~1.0.0" },
    });
    expect(planMarker(plan)).toBe(
      "<!-- ui-security-updates plan: @scope/alpha dependencies ^2.0.3; zeta dependencies ~1.0.1 -->",
    );
  });

  it("does not move when master changes a floor but not the target", () => {
    // an unrelated bump on master can change `from`; the decision a person
    // made about the target still stands
    const before = planUpdates([alert("pkg", "1.2.7")], { dependencies: { pkg: "^1.2.0" } });
    const after = planUpdates([alert("pkg", "1.2.7")], { dependencies: { pkg: "^1.2.3" } });
    expect(planMarker(after)).toBe(planMarker(before));
  });
});

describe("declinedBy", () => {
  const plan = planUpdates([alert("foo", "1.2.7")], { dependencies: { foo: "^1.2.0" } });
  const offered = renderPullBody(plan);

  it("finds the pull request a person closed while it offered exactly this plan", () => {
    const closed: ClosedPull[] = [
      { number: 40, mergedAt: null, body: offered },
      { number: 52, mergedAt: null, body: offered },
    ];
    expect(declinedBy(plan, closed)).toBe(52);
  });

  it("ignores a merged pull request, and one that offered a different set of bumps", () => {
    const wider = planUpdates([alert("foo", "1.2.7"), alert("bar", "2.0.1")], {
      dependencies: { foo: "^1.2.0", bar: "^2.0.0" },
    });
    const closed: ClosedPull[] = [
      { number: 40, mergedAt: "2026-09-20T10:00:00Z", body: offered },
      { number: 41, mergedAt: null, body: renderPullBody(wider) },
      { number: 42, mergedAt: null, body: null },
    ];
    expect(declinedBy(plan, closed)).toBeNull();
  });

  it("ignores a pull request the workflow withdrew itself", () => {
    // the withdrawal body replaces the proposal before the close, marker and all
    const withdrawn = renderPullBody(planUpdates([], {}));
    expect(withdrawn).not.toContain("ui-security-updates plan:");
    expect(declinedBy(plan, [{ number: 40, mergedAt: null, body: withdrawn }])).toBeNull();
  });

  it("never reports a declined plan when there is nothing to raise", () => {
    const empty = planUpdates([], {});
    const closed: ClosedPull[] = [{ number: 40, mergedAt: null, body: renderPullBody(empty) }];
    expect(declinedBy(empty, closed)).toBeNull();
  });
});

describe("renderPullBody and renderWithdrawal", () => {
  it("ends a proposal with the plan marker", () => {
    const plan = planUpdates([alert("foo", "1.2.7")], { dependencies: { foo: "^1.2.0" } });
    const body = renderPullBody(plan);
    expect(body.startsWith(renderSummary(plan).trimEnd())).toBe(true);
    expect(body.trimEnd().endsWith(planMarker(plan))).toBe(true);
  });

  it("says every alert is gone only when none is left for a person", () => {
    const clean = planUpdates([], {});
    expect(renderWithdrawal(clean)).toContain("No open Dependabot alert");

    const waiting = planUpdates([alert("lib", "3.0.0")], { dependencies: { lib: "^2.9.0" } });
    expect(waiting.bumps).toEqual([]);
    expect(renderWithdrawal(waiting)).not.toContain("No open Dependabot alert");
    expect(renderWithdrawal(waiting)).toContain("1 package(s)");
    // the withdrawn body keeps the table of what is left, so the reason survives the close
    expect(renderPullBody(waiting)).toContain("| `lib` | the first patched release, 3.0.0");
  });
});

describe("renderCommitMessage", () => {
  it("is the fixed subject plus one line per raised range", () => {
    const plan = planUpdates([alert("vite", "8.3.2", { number: 7 })], {
      devDependencies: { vite: "^8.3.0" },
    });
    const message = renderCommitMessage(plan);
    const [subject, blank, first] = message.split("\n");
    expect(subject).toBe(SUBJECT);
    expect(blank).toBe("");
    expect(first).toBe("- vite (devDependencies): ^8.3.0 -> ^8.3.2 for GHSA-7");
  });

  it("keeps the subject a lowercase conventional commit within 72 characters", () => {
    expect(SUBJECT).toMatch(/^build\(deps\): [a-z][^.]*$/);
    expect(SUBJECT.length).toBeLessThanOrEqual(72);
  });
});
