#!/usr/bin/env bun
// focus-assertion gate for stories (#1675).
//
//   bun run check:focus
//
// A dialog, a drawer and a sheet all move focus from an effect, one step after
// the element is in the document — and they move it *back* from that effect's
// cleanup, one step after the element leaves. A story that asserts focus with
// no waiter is therefore reading a value that may not have been written yet:
//
//     await waitFor(() => expect(canvas.queryByRole("dialog")).toBeNull());
//     await expect(canvas.getByRole("button", { name: "open" })).toHaveFocus();
//
// The second line has no retry. It passes while the handover happens to land
// inside the incidental gap between the two statements, and fails the moment it
// does not — which is why #1675's pair passed against `storybook dev` and failed
// against `build-storybook`, the build the `storybook build + play tests` job
// runs. Deferring the restoration by 600ms reproduces it exactly.
//
// #1672 does not cover this. It raised `asyncUtilTimeout` to 5s, which is how
// long `waitFor` and `findBy*` are *willing to wait*. An assertion that never
// polls waits zero milliseconds at any budget. The two changes are orthogonal:
// that one stopped slow-but-correct screens timing out, this one is about
// assertions that do not wait at all.
//
// The rule is grep-level, like `check-ui-primitives.ts`: an unwrapped
// `toHaveFocus()` is only allowed when the statement before it is one that has
// already settled where focus is. Three shapes qualify, and between them they
// cover every honest use in the dashboard today.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Glob } from "bun";

const ROOT = join(import.meta.dir, "..");

/** every story file; only stories assert focus */
const SCANNED = ["src/**/*.stories.tsx"];

/**
 * Calls that move focus synchronously as part of the action.
 *
 * `element.focus()` is the DOM call itself, and `user-event` dispatches the
 * real event sequence and awaits it, so focus has moved by the time the promise
 * settles. An assertion straight after any of these is reading a value that is
 * already written.
 */
const FOCUS_MOVERS = [
  ".focus(",
  "userEvent.tab(",
  "userEvent.click(",
  "userEvent.keyboard(",
  "userEvent.type(",
  "userEvent.pointer(",
];

export interface FocusViolation {
  file: string;
  line: number;
  text: string;
  /** the statement the assertion is leaning on, for the error message */
  after: string;
}

/**
 * Blank out `//` and block comments, keeping line count and offsets.
 *
 * Same reason `check-ui-primitives.ts` does it: a comment explaining the rule
 * must not trip the rule. Strings are deliberately left alone — an apostrophe
 * in prose would swallow the rest of the line and turn this into a silent miss.
 */
export function blankComments(source: string): string {
  let out = source.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, " "));
  out = out.replace(
    /(^|[^:])\/\/[^\n]*/g,
    (m, lead: string) => lead + " ".repeat(m.length - lead.length),
  );
  return out;
}

/** the previous non-blank statement, or "" at the top of a block */
export function previousStatement(lines: string[], at: number): string {
  for (let i = at - 1; i >= 0; i -= 1) {
    const line = lines[i].trim();
    if (line === "" || line === "{" || line === "}") continue;
    return line;
  }
  return "";
}

/**
 * Is this unwrapped `toHaveFocus` leaning on something that already settled?
 *
 * - a focus-moving call — focus is written by the action itself
 * - another `expect(…)` — the assertion before it read the same settled moment
 * - a `waitFor` that itself waited on focus — that is the waiter, and a second
 *   read of where focus already is does not need its own
 *
 * A negative assertion never reaches here; see `checkSource`.
 */
export function isSettled(previous: string): boolean {
  if (FOCUS_MOVERS.some((call) => previous.includes(call))) return true;
  if (/\bexpect\(/.test(previous) && !previous.includes("waitFor(")) return true;
  if (previous.includes("waitFor(") && previous.includes("toHaveFocus")) return true;
  return false;
}

/** every unwrapped, unsettled `toHaveFocus` in one file */
export function checkSource(source: string, file: string): FocusViolation[] {
  const lines = blankComments(source).split("\n");
  const raw = source.split("\n");
  const found: FocusViolation[] = [];
  for (let i = 0; i < lines.length; i += 1) {
    if (!lines[i].includes("toHaveFocus")) continue;
    // `not.toHaveFocus()` is a precondition, not the thing being waited for.
    // Waiting for an absence that is already true adds nothing — `waitFor`
    // returns on its first poll — and the shape is how a story says "focus has
    // not moved yet" before the keystroke that moves it (`App.stories.tsx`)
    if (lines[i].includes("not.toHaveFocus")) continue;
    // inside a waiter already — on this line or carried over from an open one
    if (lines[i].includes("waitFor(") || lines[i].includes("findBy")) continue;
    if (/^\s*(\)|\}\)|\},?)/.test(lines[i])) continue;
    const opensAWaiter = previousStatement(lines, i).endsWith("waitFor(() =>");
    if (opensAWaiter) continue;
    const previous = previousStatement(lines, i);
    if (isSettled(previous)) continue;
    found.push({ file, line: i + 1, text: raw[i].trim(), after: previous });
  }
  return found;
}

export function checkAll(): FocusViolation[] {
  const found: FocusViolation[] = [];
  for (const pattern of SCANNED) {
    for (const file of new Glob(pattern).scanSync(ROOT)) {
      found.push(...checkSource(readFileSync(join(ROOT, file), "utf8"), file));
    }
  }
  return found.sort((a, b) => a.file.localeCompare(b.file) || a.line - b.line);
}

if (import.meta.main) {
  const violations = checkAll();
  console.log(`scanned ${SCANNED.join(", ")}`);

  if (violations.length > 0) {
    console.error(`\n${violations.length} focus assertion(s) with no waiter:`);
    for (const v of violations) {
      console.error(`  ${v.file}:${v.line}  ${v.text}`);
      console.error(`      follows: ${v.after || "(start of the play function)"}`);
    }
    console.error(
      "\nfocus moves from an effect, a step after the element enters or leaves the\n" +
        "document, so an assertion with no retry is reading a value that may not be\n" +
        "written yet. Wrap it:\n\n" +
        "  await waitFor(() => expect(target).toHaveFocus());\n\n" +
        "An assertion straight after `.focus()`, a `userEvent` call or another\n" +
        "`expect(…)` is fine — those have already settled where focus is.",
    );
    process.exit(1);
  }

  console.log("\nevery focus assertion either waits or follows a settled action");
}
