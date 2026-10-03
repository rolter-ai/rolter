import { describe, expect, it } from "bun:test";

import {
  type AuditReport,
  annotations,
  classify,
  findings,
  renderSummary,
  validateAccepted,
} from "./audit-lockfile";

function adv(ghsa: string, severity: string, range: string) {
  return {
    id: 1,
    url: `https://github.com/advisories/${ghsa}`,
    title: "title | with pipe",
    severity,
    vulnerable_versions: range,
  };
}

const REPORT: AuditReport = {
  "brace-expansion": [
    adv("GHSA-aaaa-bbbb-cccc", "high", "<1.1.17"),
    adv("GHSA-aaaa-bbbb-cccc", "high", ">=2.0.0 <2.1.3"),
  ],
  valibot: [adv("GHSA-dddd-eeee-ffff", "moderate", "<=1.4.1")],
};

describe("audit-lockfile", () => {
  it("collapses one advisory across ranges and sorts by severity", () => {
    const f = findings(REPORT);
    expect(f.map((x) => x.pkg)).toEqual(["brace-expansion", "valibot"]);
    expect(f[0].ranges).toEqual(["<1.1.17", ">=2.0.0 <2.1.3"]);
  });

  it("accepts by advisory id, not by package", () => {
    const r = classify(REPORT, [{ id: "GHSA-aaaa-bbbb-cccc", reason: "x", issue: "#1" }]);
    expect(r.accepted.map((x) => x.pkg)).toEqual(["brace-expansion"]);
    expect(r.open.map((x) => x.pkg)).toEqual(["valibot"]);
  });

  it("reports an accepted advisory that is gone as stale", () => {
    const r = classify({}, [{ id: "GHSA-aaaa-bbbb-cccc", reason: "x", issue: "#1" }]);
    expect(r.stale).toHaveLength(1);
    expect(annotations(r)[0]).toContain("no longer reported");
  });

  it("is clean when nothing is open", () => {
    const r = classify({}, []);
    expect(r.open).toHaveLength(0);
    expect(renderSummary(r)).toContain("No unaccepted advisories");
  });

  it("escapes a backslash before a pipe so the pipe stays escaped", () => {
    const r = classify(
      { pkg: [{ ...adv("GHSA-aaaa-bbbb-cccc", "high", "<1"), title: "a \\| b" }] },
      [],
    );
    expect(renderSummary(r)).toContain("a \\\\\\| b");
  });

  it("escapes pipes in the table and annotates high as error, moderate as warning", () => {
    const r = classify(REPORT, []);
    expect(renderSummary(r)).toContain("title \\| with pipe");
    const a = annotations(r);
    expect(a[0].startsWith("::error")).toBe(true);
    expect(a[1].startsWith("::warning")).toBe(true);
  });

  it("requires a GHSA id, a reason and an issue on every accepted row", () => {
    expect(validateAccepted([])).toEqual([]);
    expect(() => validateAccepted([{ id: "x", reason: "r", issue: "#1" }])).toThrow(/GHSA/);
    expect(() =>
      validateAccepted([{ id: "GHSA-aaaa-bbbb-cccc", reason: " ", issue: "#1" }]),
    ).toThrow(/reason/);
    expect(() =>
      validateAccepted([{ id: "GHSA-aaaa-bbbb-cccc", reason: "r", issue: "1" }]),
    ).toThrow(/issue/);
  });
});
