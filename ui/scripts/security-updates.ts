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
// admits. A vulnerable package that only arrives transitively through
// `bun.lock` raises no alert at all, so this script never hears of it (#1930).
// The fix that closes a direct alert is to raise that range's floor to the
// first patched release, keeping its operator, and let `bun install
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

function firstPatched(alert: Alert): string {
  return alert.security_vulnerability.first_patched_version?.identifier ?? "";
}

// the highest first patched release among `alerts`, which must not be empty
function highest(alerts: Alert[]): string {
  return alerts.map(firstPatched).reduce((best, v) => (Bun.semver.order(v, best) > 0 ? v : best));
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

    const sections = SECTIONS.filter((s) => manifest[s]?.[name] !== undefined);
    if (sections.length === 0) {
      plan.skipped.push({
        name,
        reason:
          "not a direct dependency of ui/package.json; a transitive fix is an `overrides` entry, which is raised by hand",
        alerts: patched.map(ref),
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
          alerts: patched.map(ref),
        });
        continue;
      }
      const [, operator, floor] = match;

      // each alert is judged on its own, so one advisory patched only in the
      // next major cannot hold back the compatible fix for the others
      const stale: Alert[] = [];
      const compatible: Alert[] = [];
      const breaking: Alert[] = [];
      for (const alert of patched) {
        const fixedIn = firstPatched(alert);
        if (Bun.semver.order(floor, fixedIn) >= 0) stale.push(alert);
        else if (Bun.semver.satisfies(fixedIn, `^${floor}`)) compatible.push(alert);
        else breaking.push(alert);
      }

      if (compatible.length > 0) {
        // one bump has to clear every compatible advisory at once, so aim for
        // the highest of their first patched releases
        const target = highest(compatible);
        plan.bumps.push({
          name,
          section,
          from: spec,
          to: `${operator}${target}`,
          alerts: compatible.map(ref),
        });
      }
      if (breaking.length > 0) {
        plan.skipped.push({
          name,
          reason: `the first patched release, ${highest(breaking)}, is a breaking bump from \`${spec}\` in ${section}`,
          alerts: breaking.map(ref),
        });
      }
      if (stale.length > 0) {
        // the range already starts at a patched release. the alert is stale
        // and closes once github re-reads the manifest; nothing to raise
        plan.skipped.push({
          name,
          reason: `\`${spec}\` in ${section} already starts at or above ${highest(stale)}`,
          alerts: stale.map(ref),
        });
      }
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
// carry one, so every value that goes into a cell is escaped. backslashes go
// first: with a `\` already in front of a pipe, escaping the pipe alone would
// leave an escaped backslash followed by a bare pipe
function cell(text: string): string {
  return text.replace(/\\/g, "\\\\").replace(/\|/g, "\\|");
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

// what every body the workflow writes ends with, proposal or withdrawal
const ABOUT =
  "Opened by `.github/workflows/ui-security-updates.yml`, which stands in for Dependabot security updates on `ui/`: the `bun` ecosystem does version updates only (#1148). The workflow rebuilds this branch from `master` when the change it needs moves, and dispatches `ci.yml` against it, since a pull request opened with the repository token triggers no `pull_request` run.";

// how a person says no, which only a proposal needs to say
const DECLINE =
  "Close this pull request unmerged to decline these exact bumps: the workflow opens a new one only once the set of bumps changes.";

/**
 * The hidden line that ties a pull request body to the exact set of ranges it
 * raises. It is what `declinedBy` looks for, so it names each raised range and
 * its target and nothing that moves with `master`: an unrelated dependency
 * change leaves it alone, a new or different bump changes it.
 */
export function planMarker(plan: Plan): string {
  const key = plan.bumps
    .map((b) => `${b.name} ${b.section} ${b.to}`)
    .sort()
    .join("; ");
  return `<!-- ui-security-updates plan: ${key} -->`;
}

/** The fields of a closed pull request (`gh pr list --json number,mergedAt,body`) read here. */
export interface ClosedPull {
  number: number;
  mergedAt: string | null;
  body: string | null;
}

/**
 * The pull request a person closed unmerged while it offered exactly `plan`,
 * or null when there is none.
 *
 * Only a person's close counts. When the workflow withdraws its own pull
 * request it first rewrites the body without the marker (`renderPullBody` for
 * an empty plan), so that close never matches. The decision reads pull request
 * state rather than the branch: a branch can outlive its pull request for
 * reasons that are not a decision, such as a run that pushed and then failed to
 * open the pull request, and its bytes change whenever `master` does.
 */
export function declinedBy(plan: Plan, closed: ClosedPull[]): number | null {
  if (plan.bumps.length === 0) return null;
  const marker = planMarker(plan);
  const hits = closed
    .filter((p) => !p.mergedAt && (p.body ?? "").includes(marker))
    .map((p) => p.number)
    .sort((a, b) => b - a);
  return hits[0] ?? null;
}

/**
 * The first paragraph of a withdrawn pull request, also used as the comment
 * that closes it. It says why from the plan itself, so it never claims every
 * alert is gone while some are still open and waiting on a person.
 */
export function renderWithdrawal(plan: Plan): string {
  if (plan.skipped.length === 0) {
    return "No open Dependabot alert on `ui/` needs a bump any more, so this pull request is withdrawn.";
  }
  return `Nothing in this pull request is still needed as it stands, so it is withdrawn. ${plan.skipped.length} package(s) with an open Dependabot alert on \`ui/\` are left for a person; the description lists each one and why.`;
}

/**
 * The pull request body for `plan`. With something to raise it is the summary,
 * the footer and the plan marker. With nothing to raise it is the withdrawal
 * notice and the summary, with no marker, which is the body the workflow writes
 * just before it closes its own pull request.
 */
export function renderPullBody(plan: Plan): string {
  if (plan.bumps.length === 0) {
    // with nothing open at all, the summary would only repeat the withdrawal
    const summary = plan.skipped.length > 0 ? `${renderSummary(plan).trimEnd()}\n\n` : "";
    return `${renderWithdrawal(plan)}\n\n${summary}---\n\n${ABOUT}\n`;
  }
  return `${renderSummary(plan).trimEnd()}\n\n---\n\n${ABOUT} ${DECLINE}\n\n${planMarker(plan)}\n`;
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
      "usage: bun scripts/security-updates.ts --alerts <alerts.json> [--manifest <package.json>] [--closed-pulls <pulls.json>] [--summary <out.md>] [--pr-body <out.md>] [--close-comment <out.txt>] [--commit-message <out.txt>] [--github-output <file>] [--write]",
    );
    process.exit(2);
  }
  const manifestPath = arg(args, "--manifest") ?? join(import.meta.dir, "..", "package.json");
  const alerts = JSON.parse(readFileSync(alertsPath, "utf8")) as Alert[];
  const manifestText = readFileSync(manifestPath, "utf8");
  const plan = planUpdates(alerts, JSON.parse(manifestText) as Manifest);
  const summary = renderSummary(plan);
  const closedPath = arg(args, "--closed-pulls");
  const closed = closedPath ? (JSON.parse(readFileSync(closedPath, "utf8")) as ClosedPull[]) : [];
  const declined = declinedBy(plan, closed);

  const summaryPath = arg(args, "--summary");
  if (summaryPath) writeFileSync(summaryPath, summary);
  const bodyPath = arg(args, "--pr-body");
  if (bodyPath) writeFileSync(bodyPath, renderPullBody(plan));
  const commentPath = arg(args, "--close-comment");
  if (commentPath) writeFileSync(commentPath, `${renderWithdrawal(plan)}\n`);
  const commitPath = arg(args, "--commit-message");
  if (commitPath) writeFileSync(commitPath, renderCommitMessage(plan));
  if (args.includes("--write") && plan.bumps.length > 0) {
    writeFileSync(manifestPath, applyPlan(manifestText, plan));
  }
  // the counts as step outputs, so the workflow branches on numbers rather
  // than on the wording of the summary
  const outputPath = arg(args, "--github-output");
  if (outputPath) {
    appendFileSync(
      outputPath,
      `bumps=${plan.bumps.length}\nskipped=${plan.skipped.length}\ndeclined=${declined ?? ""}\n`,
    );
  }
  console.log(summary);
  console.log(`${plan.bumps.length} range(s) raised, ${plan.skipped.length} left for a hand bump`);
  if (declined !== null) {
    console.log(`#${declined} offered exactly these bumps and was closed unmerged`);
  }
}
