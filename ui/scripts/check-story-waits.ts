#!/usr/bin/env bun
// zero-latency assertion gate for stories (#1700).
//
//   bun run check:waits
//
// Sibling of `check-story-focus.ts` (#1675) and the same idea: an assertion
// that never polls waits zero milliseconds, so #1672's 5s `asyncUtilTimeout`
// buys it nothing. It passes while the fetch stub answers inside the same tick
// and fails the moment a loaded runner, or a `storybook build`, moves the
// answer one tick out. #1689 was six plays of exactly that, and the two shapes
// behind all six are mechanical enough to grep for.
//
// **A one-shot `toBeDisabled()` in a gated story.** `GatedButton` renders
// *enabled* until `/api/v1/rbac/effective` answers — `undefined` is "not known
// yet" by design, and only an explicit `false` refuses — so an assertion with
// no retry is reading the gate before it spoke. It also passes for the wrong
// reason whenever the control is disabled by its own form state, which is why
// `expectRefused` waits for the disabled flag and the `title` together.
//
// **A data-shaped `getByRole` on the statement after a sheet opens.** A sheet
// fires its own query as it opens, so its rows are a request behind the dialog
// — `findByRole` is the fix. The rule looks only at roles that are rendered
// per item out of fetched data (`checkbox`, `radio`, `row`, `option`, …): a
// `heading` or a confirm `button` is part of the sheet's own markup and paints
// with it, and flagging those would mean a waiver on every editor story, which
// is how a guard gets switched off. `getByLabelText` is out for the same
// reason — a sheet's fields are in its first paint; a field whose *label* comes
// from data is rare and reads as a query, not a field.
//
// The waiver is `check-ui-primitives.ts`'s, for the same reason: the genuine
// cases are a minority, they are local, and a reason written at the point of
// use cannot outlive the code the way a central baseline file can. Every
// honoured waiver is printed on every run, so the set stays visible.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Glob } from "bun";
import { blankComments, previousStatement } from "./check-story-focus";

const ROOT = join(import.meta.dir, "..");

/** every story file; only stories have plays */
const SCANNED = ["src/**/*.stories.tsx"];

/**
 * A story mounted under a role, which is the only case where a gate is in
 * flight at all. Written as the role values rather than a bare `role=`, so a
 * `<div role="status">` in a render does not make the whole story look gated.
 */
const GATED = /\brole[=:]\s*[{"']*\s*(admin|member|viewer|superadmin|role)\b/;

/** the start of a story object, which is the unit `GATED` is asked about */
const STORY_START = /^export const \w+(:\s*\w+)?\s*=\s*[{(]/;

/**
 * The statement that puts a sheet, drawer or confirm dialog on screen.
 *
 * `sheet()` is the harness helper; `findByRole("dialog")` is the same lookup
 * written out, which the stories that hold two dialogs at once have to use.
 */
const OPENS_A_SHEET = /\bsheet\(\)|findByRole\((["'])dialog\1/;

/**
 * Roles a screen renders one of per row of fetched data.
 *
 * These carry a name that came from a request, so the element cannot exist
 * before that request lands however fast the dialog paints.
 */
const DATA_ROLES = ["checkbox", "radio", "row", "cell", "option", "listitem", "treeitem"];

/** an inline `story-wait-allow: <reason>` on the line above */
const WAIVER = /story-wait-allow:\s*(.*)$/;

export type RuleId = "gate" | "sheet-row";

export interface Violation {
  file: string;
  line: number;
  rule: RuleId;
  /** the source text that tripped the rule, for the message */
  text: string;
}

export interface Waiver {
  file: string;
  line: number;
  reason: string;
}

export interface Result {
  violations: Violation[];
  waivers: Waiver[];
}

/**
 * Lines sitting strictly inside an open `waitFor(…)`.
 *
 * Counted on parentheses rather than on the previous statement: a multi-line
 * waiter is the normal way these plays are written once the callback holds more
 * than one assertion, and every line of it retries.
 */
export function waitedLines(lines: string[]): Set<number> {
  const inside = new Set<number>();
  let depth = 0;
  const balance = (text: string) => {
    let bal = 0;
    for (const ch of text) {
      if (ch === "(") bal += 1;
      else if (ch === ")") bal -= 1;
    }
    return bal;
  };
  for (let i = 0; i < lines.length; i += 1) {
    if (depth > 0) {
      inside.add(i);
      depth = Math.max(0, depth + balance(lines[i]));
      continue;
    }
    const at = lines[i].indexOf("waitFor(");
    if (at === -1) continue;
    depth = Math.max(0, balance(lines[i].slice(at + "waitFor".length)));
  }
  return inside;
}

/** is the line at `at` inside a story mounted under a role? */
export function inGatedStory(lines: string[], at: number): boolean {
  let start = 0;
  let end = lines.length;
  for (let i = 0; i < lines.length; i += 1) {
    if (!STORY_START.test(lines[i])) continue;
    if (i <= at) start = i;
    else {
      end = i;
      break;
    }
  }
  return GATED.test(lines.slice(start, end).join("\n"));
}

/** the role a `getByRole(` call on this line asks for, if it names one */
export function queriedRole(line: string): string | null {
  const match = /getByRole\(\s*(["'])([a-z]+)\1/.exec(line);
  return match ? match[2] : null;
}

/** every zero-latency assertion, and every honoured waiver, in one file */
export function checkSource(source: string, file: string): Result {
  const lines = blankComments(source).split("\n");
  const raw = source.split("\n");
  const waited = waitedLines(lines);
  const violations: Violation[] = [];
  const waivers: Waiver[] = [];

  // the marker may sit anywhere in the comment block above, not only on the
  // line immediately before it: a reason worth writing rarely fits in one line,
  // and a waiver whose reason had to be a fragment would be a worse waiver
  const waive = (at: number): boolean => {
    for (let i = at - 1; i >= 0 && /^\s*(\/\/|\*|\/\*)/.test(raw[i] ?? ""); i -= 1) {
      const marker = WAIVER.exec(raw[i]);
      if (!marker) continue;
      waivers.push({ file, line: at + 1, reason: marker[1].trim() });
      return true;
    }
    return false;
  };

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    // a line that opens or continues a waiter retries by definition
    if (waited.has(i) || line.includes("waitFor(")) continue;

    if (line.includes("toBeDisabled(") && inGatedStory(lines, i)) {
      if (!waive(i)) violations.push({ file, line: i + 1, rule: "gate", text: raw[i].trim() });
      continue;
    }

    const role = queriedRole(line);
    if (role && DATA_ROLES.includes(role) && OPENS_A_SHEET.test(previousStatement(lines, i))) {
      if (!waive(i)) violations.push({ file, line: i + 1, rule: "sheet-row", text: raw[i].trim() });
    }
  }
  return { violations, waivers };
}

export function checkAll(): Result {
  const violations: Violation[] = [];
  const waivers: Waiver[] = [];
  for (const pattern of SCANNED) {
    for (const file of new Glob(pattern).scanSync(ROOT)) {
      const result = checkSource(readFileSync(join(ROOT, file), "utf8"), file);
      violations.push(...result.violations);
      waivers.push(...result.waivers);
    }
  }
  const order = <T extends { file: string; line: number }>(a: T, b: T) =>
    a.file.localeCompare(b.file) || a.line - b.line;
  return { violations: violations.sort(order), waivers: waivers.sort(order) };
}

/** what to do about each rule, printed with the failures it belongs to */
const ADVICE: Record<RuleId, string> = {
  gate:
    "a gated control renders enabled until `/api/v1/rbac/effective` answers, so a\n" +
    "one-shot `toBeDisabled()` reads the gate before it spoke — and it passes for\n" +
    "the wrong reason whenever the control is disabled by its own form state. Use\n" +
    "`expectRefused(canvasElement, name, reason)`, which waits for the disabled\n" +
    "flag and the `title` together.",
  "sheet-row":
    "a sheet fires its own query as it opens, so a row it renders out of that\n" +
    "answer is not there the instant the dialog is. Use `await …findByRole(…)`.",
};

if (import.meta.main) {
  const { violations, waivers } = checkAll();
  console.log(`scanned ${SCANNED.join(", ")}`);

  if (waivers.length > 0) {
    console.log(`\n${waivers.length} waived:`);
    for (const w of waivers) console.log(`  ${w.file}:${w.line}  ${w.reason || "(no reason)"}`);
  }

  if (violations.length > 0) {
    console.error(`\n${violations.length} assertion(s) that only pass at zero latency:`);
    for (const rule of Object.keys(ADVICE) as RuleId[]) {
      const hits = violations.filter((v) => v.rule === rule);
      if (hits.length === 0) continue;
      console.error(`\n[${rule}]`);
      for (const v of hits) console.error(`  ${v.file}:${v.line}  ${v.text}`);
      console.error(`\n${ADVICE[rule]}`);
    }
    console.error(
      "\nA case the rule is genuinely not about — a control disabled by a prop from\n" +
        "its first paint — carries `// story-wait-allow: <reason>` on the line above.",
    );
    process.exit(1);
  }

  console.log("\nno story assertion depends on the stub answering inside the same tick");
}
