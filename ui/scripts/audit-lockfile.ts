// transitive-dependency audit for ui/ (#1930).
//
//   bun scripts/audit-lockfile.ts [--audit audit.json] [--accepted audit-accepted.json]
//     [--summary out.md]
//
// GitHub's dependency graph reads `ui/package.json` but not `ui/bun.lock`, so
// Dependabot raises no alert for a vulnerable package that only arrives
// transitively, and `ui-security-updates.yml` only ever sees alerts. This
// script closes that gap by reading what `bun audit --json` reports for the
// lockfile itself. `extended.yml` runs it nightly; it exits 1 when an advisory
// is neither fixed nor accepted, which `report failure` turns into the tracking
// issue.
//
// An advisory that is accepted rather than fixed goes in
// `ui/audit-accepted.json` as `{ "id": "GHSA-...", "reason": "...", "issue":
// "#1234" }`. An entry names the advisory, not the package, so a different
// advisory on the same package still fails. An entry whose advisory no longer
// appears is reported as stale so the file does not collect dead rows.
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

/** One advisory in `bun audit --json`, which maps a package name to a list of these. */
export interface Advisory {
  id: number;
  url: string;
  title: string;
  severity: string;
  vulnerable_versions: string;
}

export type AuditReport = Record<string, Advisory[]>;

export interface Accepted {
  id: string;
  reason: string;
  issue: string;
}

export interface Finding {
  pkg: string;
  ghsa: string;
  severity: string;
  title: string;
  url: string;
  ranges: string[];
}

export interface Result {
  open: Finding[];
  accepted: Finding[];
  stale: Accepted[];
}

const SEVERITY_ORDER = ["critical", "high", "moderate", "low"];

function rank(severity: string): number {
  const i = SEVERITY_ORDER.indexOf(severity);
  return i === -1 ? SEVERITY_ORDER.length : i;
}

/**
 * Collapses the report to one finding per package and advisory. bun lists an
 * advisory once per vulnerable range, so a package resolved at two majors
 * appears twice with the same GHSA.
 */
export function findings(report: AuditReport): Finding[] {
  const byKey = new Map<string, Finding>();
  for (const [pkg, list] of Object.entries(report)) {
    for (const a of list) {
      const ghsa = a.url.split("/").pop() ?? String(a.id);
      const key = `${pkg}\0${ghsa}`;
      const found = byKey.get(key);
      if (found) {
        if (!found.ranges.includes(a.vulnerable_versions)) found.ranges.push(a.vulnerable_versions);
      } else {
        byKey.set(key, {
          pkg,
          ghsa,
          severity: a.severity,
          title: a.title,
          url: a.url,
          ranges: [a.vulnerable_versions],
        });
      }
    }
  }
  return [...byKey.values()].sort(
    (a, b) =>
      rank(a.severity) - rank(b.severity) ||
      a.pkg.localeCompare(b.pkg) ||
      a.ghsa.localeCompare(b.ghsa),
  );
}

export function classify(report: AuditReport, accepted: Accepted[]): Result {
  const ids = new Set(accepted.map((a) => a.id));
  const all = findings(report);
  const seen = new Set(all.map((f) => f.ghsa));
  return {
    open: all.filter((f) => !ids.has(f.ghsa)),
    accepted: all.filter((f) => ids.has(f.ghsa)),
    stale: accepted.filter((a) => !seen.has(a.id)),
  };
}

/** Rejects an allowlist row with no reason or no tracking issue: acceptance has to be explained. */
export function validateAccepted(raw: unknown): Accepted[] {
  if (!Array.isArray(raw)) throw new Error("audit-accepted.json must be an array");
  return raw.map((row, i) => {
    const r = row as Partial<Accepted>;
    if (!r.id || !/^GHSA(-[0-9a-z]{4}){3}$/.test(r.id)) {
      throw new Error(`audit-accepted.json[${i}]: id must be a GHSA id`);
    }
    if (!r.reason?.trim()) throw new Error(`audit-accepted.json[${i}]: ${r.id} needs a reason`);
    if (!r.issue || !/^#\d+$/.test(r.issue)) {
      throw new Error(`audit-accepted.json[${i}]: ${r.id} needs an issue such as "#1234"`);
    }
    return { id: r.id, reason: r.reason.trim(), issue: r.issue };
  });
}

function cell(text: string): string {
  return text.replace(/\|/g, "\\|").replace(/\s+/g, " ").trim();
}

export function renderSummary(result: Result): string {
  const lines = ["### ui/bun.lock audit", ""];
  if (result.open.length === 0) {
    lines.push("No unaccepted advisories in `ui/bun.lock`.", "");
  } else {
    lines.push(
      `${result.open.length} advisories in \`ui/bun.lock\` are neither fixed nor accepted.`,
      "",
      "| package | severity | advisory | vulnerable range |",
      "| --- | --- | --- | --- |",
      ...result.open.map(
        (f) =>
          `| \`${f.pkg}\` | ${f.severity} | [${f.ghsa}](${f.url}) ${cell(f.title)} | ${cell(f.ranges.join(", "))} |`,
      ),
      "",
    );
  }
  if (result.accepted.length > 0) {
    lines.push(
      `${result.accepted.length} accepted through \`ui/audit-accepted.json\`: ${result.accepted
        .map((f) => `${f.pkg} ${f.ghsa}`)
        .join(", ")}.`,
      "",
    );
  }
  for (const s of result.stale) {
    lines.push(`Stale entry: \`${s.id}\` (${s.issue}) no longer appears; remove it.`, "");
  }
  return lines.join("\n");
}

export function annotations(result: Result): string[] {
  return [
    ...result.open.map(
      (f) =>
        `::${f.severity === "low" || f.severity === "moderate" ? "warning" : "error"} title=bun audit::${f.pkg} ${f.ghsa} (${f.severity}) ${cell(f.title)}`,
    ),
    ...result.stale.map(
      (s) => `::warning title=bun audit::${s.id} is in audit-accepted.json but no longer reported`,
    ),
  ];
}

function runAudit(cwd: string): AuditReport {
  // bun audit exits 1 when it finds anything, so the exit code says nothing
  // about whether the audit itself worked; an unparsable stdout does
  const out = Bun.spawnSync(["bun", "audit", "--json"], { cwd, stdout: "pipe", stderr: "pipe" });
  const text = out.stdout.toString();
  try {
    return JSON.parse(text) as AuditReport;
  } catch {
    throw new Error(`bun audit gave no JSON (exit ${out.exitCode}): ${out.stderr.toString()}`);
  }
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const flag = (name: string): string | undefined => {
    const i = args.indexOf(name);
    return i === -1 ? undefined : args[i + 1];
  };
  const root = join(import.meta.dir, "..");
  const auditFile = flag("--audit");
  const report: AuditReport = auditFile
    ? (JSON.parse(readFileSync(auditFile, "utf8")) as AuditReport)
    : runAudit(root);
  const accepted = validateAccepted(
    JSON.parse(readFileSync(flag("--accepted") ?? join(root, "audit-accepted.json"), "utf8")),
  );
  const result = classify(report, accepted);
  const summary = renderSummary(result);
  console.log(summary);
  for (const line of annotations(result)) console.log(line);
  const out = flag("--summary");
  if (out) writeFileSync(out, summary);
  if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, summary);
  process.exit(result.open.length > 0 ? 1 : 0);
}
