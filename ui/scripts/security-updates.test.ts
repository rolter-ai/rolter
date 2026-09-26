import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { fileURLToPath, URL } from "node:url";

import {
  type Alert,
  type Manifest,
  SUBJECT,
  applyPlan,
  planUpdates,
  renderCommitMessage,
  renderSummary,
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
