// security-update planner for ui/ (#1148).
//
//   bun scripts/security-updates.ts --alerts alerts.json [--manifest package.json]
//     [--summary out.md] [--commit-message out.txt] [--github-output "$GITHUB_OUTPUT"]
//     [--write]
//
// Dependabot raises security-update pull requests for cargo and for the
// workflow actions, but not for ui/. That entry runs as `package-ecosystem:
// bun` (#1137), and the bun ecosystem does version updates only, so an alert on
// a ui dependency used to sit there until someone raised the bump by hand.
// `.github/workflows/ui-security-updates.yml` stands in for the missing half:
// it downloads the open Dependabot alerts, and this script decides what to do
// with them.
//
// The decision is deliberately narrow. GitHub's dependency graph reads
// `ui/package.json` but not `bun.lock`, so every alert it raises for ui/ is
// about a *direct* dependency, judged by the lowest version its declared range
// admits. The fix that closes such an alert is to raise that range's floor to
// the first patched release, keeping its operator, and let `bun install
// --lockfile-only` bring the lockfile along. That is only done when the patched
// release is semver-compatible with the current floor: a breaking bump is a
// migration, and no scheduled job should start one on its own. Everything the
// script does not raise is listed in the summary with the reason, so the pull
// request says what is still left for a human.
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

/** The fields of a Dependabot alert (`GET /repos/{owner}/{repo}/dependabot/alerts`) read here. */
export interface Alert {
  number: number;
  state: string;
  html_url: string;
  dependency: {
    package: { ecosystem: string; name: string };
    manifest_path: string;
    relationship?: string | null;
  };
  security_advisory: { ghsa_id: string; severity: string };
  security_vulnerability: {
    vulnerable_version_range: string;
    first_patched_version: { identifier: string } | null;
  };
}

/** The two sections of `package.json` a direct dependency can live in. */
export type Section = "dependencies" | "devDependencies";

const SECTIONS: Section[] = ["dependencies", "devDependencies"];

/** The parts of `package.json` the planner reads. */
export interface Manifest {
  dependencies?: Record<string, string>;
  devDependencies?: Record<string, string>;
}

/** One alert as the summary cites it. */
export interface AlertRef {
  number: number;
  url: string;
  ghsa: string;
  severity: string;
  vulnerable: string;
  patched: string | null;
}

/** A range the planner raises: `name` in `section` goes from `from` to `to`. */
export interface Bump {
  name: string;
  section: Section;
  from: string;
  to: string;
  alerts: AlertRef[];
}

/** An alert the planner leaves alone, and why. */
export interface Skip {
  name: string;
  reason: string;
  alerts: AlertRef[];
}

export interface Plan {
  bumps: Bump[];
  skipped: Skip[];
}

// alerts filed against a manifest directly inside ui/. today that is only
// ui/package.json; ui/bun.lock would join it if the dependency graph ever
// learns to read it
const UI_MANIFEST = /^ui\/[^/]+$/;

// the range shapes the planner knows how to raise: `^x.y.z`, `~x.y.z` or an
// exact `x.y.z`. anything else (`>=`, `||`, `*`, a tag, a url) is left to a human
const SPEC = /^([\^~]?)(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)$/;

function ref(alert: Alert): AlertRef {
  return {
    number: alert.number,
    url: alert.html_url,
    ghsa: alert.security_advisory.ghsa_id,
    severity: alert.security_advisory.severity,
    vulnerable: alert.security_vulnerability.vulnerable_version_range,
    patched: alert.security_vulnerability.first_patched_version?.identifier ?? null,
  };
}

/** Work out which ranges in `manifest` to raise for the open ui/ alerts in `alerts`. */
export function planUpdates(alerts: Alert[], manifest: Manifest): Plan {
  const byPackage = new Map<string, Alert[]>();
  for (const alert of alerts) {
    if (alert.state !== "open") continue;
    if (alert.dependency.package.ecosystem !== "npm") continue;
    if (!UI_MANIFEST.test(alert.dependency.manifest_path)) continue;
    const name = alert.dependency.package.name;
    byPackage.set(name, [...(byPackage.get(name) ?? []), alert]);
  }

  const plan: Plan = { bumps: [], skipped: [] };
  for (const name of [...byPackage.keys()].sort()) {
    const all = (byPackage.get(name) ?? []).sort((a, b) => a.number - b.number);
    const unpatched = all.filter((a) => !a.security_vulnerability.first_patched_version);
    const patched = all.filter((a) => a.security_vulnerability.first_patched_version);

    if (unpatched.length > 0) {
      plan.skipped.push({
        name,
        reason: "no patched release has been published yet",
        alerts: unpatched.map(ref),
      });
    }
    if (patched.length === 0) continue;
    const alertRefs = patched.map(ref);

    // one bump has to clear every advisory at once, so aim for the highest of
    // the first patched releases
    const target = patched
      .map((a) => a.security_vulnerability.first_patched_version?.identifier ?? "")
      .reduce((best, v) => (Bun.semver.order(v, best) > 0 ? v : best));

    const sections = SECTIONS.filter((s) => manifest[s]?.[name] !== undefined);
    if (sections.length === 0) {
      plan.skipped.push({
        name,
        reason:
          "not a direct dependency of ui/package.json; a transitive fix is an `overrides` entry, which is raised by hand",
        alerts: alertRefs,
      });
      continue;
    }

    for (const section of sections) {
      const spec = manifest[section]?.[name] ?? "";
      const match = SPEC.exec(spec);
      if (!match) {
        plan.skipped.push({
          name,
          reason: `\`${spec}\` in ${section} is not a plain \`^\`, \`~\` or exact version`,
          alerts: alertRefs,
        });
        continue;
      }
      const [, operator, floor] = match;
      if (Bun.semver.order(floor, target) >= 0) {
        // the range already starts at a patched release. the alert is stale
        // and closes once github re-reads the manifest; nothing to raise
        plan.skipped.push({
          name,
          reason: `\`${spec}\` in ${section} already starts at or above ${target}`,
          alerts: alertRefs,
        });
        continue;
      }
      if (!Bun.semver.satisfies(target, `^${floor}`)) {
        plan.skipped.push({
          name,
          reason: `the first patched release, ${target}, is a breaking bump from \`${spec}\` in ${section}`,
          alerts: alertRefs,
        });
        continue;
      }
      plan.bumps.push({ name, section, from: spec, to: `${operator}${target}`, alerts: alertRefs });
    }
  }
  return plan;
}

/**
 * Return `manifestText` with every bump in `plan` applied.
 *
 * The file is rewritten through `JSON.stringify` with two-space indentation,
 * which is the form prettier's `json-stringify` parser gives `package.json`,
 * so a rewrite changes the raised lines and nothing else.
 */
export function applyPlan(manifestText: string, plan: Plan): string {
  const manifest = JSON.parse(manifestText) as Record<string, unknown>;
  for (const bump of plan.bumps) {
    const section = manifest[bump.section] as Record<string, string>;
    section[bump.name] = bump.to;
  }
  return `${JSON.stringify(manifest, null, 2)}\n`;
}

// a table cell ends at the first bare `|`, and ranges such as `1.x || 2.x`
// carry one, so every value that goes into a cell is escaped
function cell(text: string): string {
  return text.replace(/\|/g, "\\|");
}

function alertLinks(alerts: AlertRef[]): string {
  return alerts
    .map((a) => `[#${a.number}](${a.url}) ${a.severity} ${a.ghsa} (\`${cell(a.vulnerable)}\`)`)
    .join("<br>");
}

/** Render `plan` as the markdown the workflow puts in the pull request and the run summary. */
export function renderSummary(plan: Plan): string {
  const lines: string[] = [];
  if (plan.bumps.length === 0 && plan.skipped.length === 0) {
    lines.push("No open Dependabot alert on `ui/` dependencies.");
    return `${lines.join("\n")}\n`;
  }

  if (plan.bumps.length > 0) {
    lines.push(
      "Raises each vulnerable direct dependency of `ui/` to its first patched release, keeping the range operator, and refreshes `bun.lock` to match.",
      "",
      "| Package | Section | From | To | Alerts |",
      "| --- | --- | --- | --- | --- |",
    );
    for (const b of plan.bumps) {
      lines.push(
        `| \`${b.name}\` | ${b.section} | \`${cell(b.from)}\` | \`${b.to}\` | ${alertLinks(b.alerts)} |`,
      );
    }
  } else {
    lines.push("Nothing to raise automatically.");
  }

  if (plan.skipped.length > 0) {
    lines.push(
      "",
      "Left for a hand bump:",
      "",
      "| Package | Why | Alerts |",
      "| --- | --- | --- |",
    );
    for (const s of plan.skipped) {
      lines.push(`| \`${s.name}\` | ${cell(s.reason)} | ${alertLinks(s.alerts)} |`);
    }
  }
  return `${lines.join("\n")}\n`;
}

/**
 * The subject every security-update commit and pull request carries. It is
 * fixed rather than derived from the plan because one standing pull request
 * carries whatever the open alerts need today, and its title has to stay true
 * as that set changes. It uses the `build(deps)` prefix dependabot gives its
 * own bumps, which the pull-request title check accepts.
 */
export const SUBJECT = "build(deps): raise vulnerable ui dependencies to patched releases";

/** Render the commit message for `plan`: the fixed subject, then one line per raised range. */
export function renderCommitMessage(plan: Plan): string {
  const lines = [SUBJECT, ""];
  for (const b of plan.bumps) {
    const ghsas = b.alerts.map((a) => a.ghsa).join(", ");
    lines.push(`- ${b.name} (${b.section}): ${b.from} -> ${b.to} for ${ghsas}`);
  }
  lines.push("", "Raised by .github/workflows/ui-security-updates.yml (#1148).");
  return `${lines.join("\n")}\n`;
}

function arg(args: string[], flag: string): string | undefined {
  const i = args.indexOf(flag);
  return i >= 0 ? args[i + 1] : undefined;
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const alertsPath = arg(args, "--alerts");
  if (!alertsPath) {
    console.error(
      "usage: bun scripts/security-updates.ts --alerts <alerts.json> [--manifest <package.json>] [--summary <out.md>] [--commit-message <out.txt>] [--github-output <file>] [--write]",
    );
    process.exit(2);
  }
  const manifestPath = arg(args, "--manifest") ?? join(import.meta.dir, "..", "package.json");
  const alerts = JSON.parse(readFileSync(alertsPath, "utf8")) as Alert[];
  const manifestText = readFileSync(manifestPath, "utf8");
  const plan = planUpdates(alerts, JSON.parse(manifestText) as Manifest);
  const summary = renderSummary(plan);

  const summaryPath = arg(args, "--summary");
  if (summaryPath) writeFileSync(summaryPath, summary);
  const commitPath = arg(args, "--commit-message");
  if (commitPath) writeFileSync(commitPath, renderCommitMessage(plan));
  if (args.includes("--write") && plan.bumps.length > 0) {
    writeFileSync(manifestPath, applyPlan(manifestText, plan));
  }
  // the counts as step outputs, so the workflow branches on numbers rather
  // than on the wording of the summary
  const outputPath = arg(args, "--github-output");
  if (outputPath) {
    appendFileSync(outputPath, `bumps=${plan.bumps.length}\nskipped=${plan.skipped.length}\n`);
  }
  console.log(summary);
  console.log(`${plan.bumps.length} range(s) raised, ${plan.skipped.length} left for a hand bump`);
}
