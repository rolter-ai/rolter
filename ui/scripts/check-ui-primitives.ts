#!/usr/bin/env bun
// shared-component gate (#1662).
//
//   bun run check:primitives
//
// `AGENTS.md`'s maintenance matrix carries four "never hand-roll this" rules
// for the dashboard, and until now every one of them was convention: nothing
// in `ui/scripts/` looked for a bare `<select>`, a raw `<pre>`, a
// `window.confirm`, or a primitive re-declared inside the screen that needed
// it. #1044 is what that costs — seven form primitives sat trapped in one
// sheet's file for months because no check noticed they were there.
//
// The rules are deliberately grep-level, the way `check-literals.ts` is: this
// is a guard against the obvious mistake, not a type system. Two things keep
// it from crying wolf.
//
// **Comments are blanked first.** Every `window.confirm` in the dashboard
// today sits in a comment explaining what a screen replaced, and both mentions
// of `<select>` are prose in a doc comment. A guard that flagged those would be
// turned off within a week.
//
// Comments only, deliberately — not the string-and-template masking
// `check-story-parameters.ts` does. That pass treats an apostrophe in JSX text
// (`don't`) as the start of a string literal and blanks the rest of the line,
// which for a brace counter is harmless and for this guard would be a silent
// miss: it swallowed `Performance.tsx`'s hand-rolled `Card` outright while this
// check was being written. Stopping at comments errs towards reporting too
// much, and too much is visible and waivable where too little is not.
//
// **The exemptions are rules, not a list of today's files.** There are three,
// and each one names a category that must be allowed to contain the thing it
// bans:
//
//   1. `src/components/ui/` — a primitive's own implementation. `CodeBlock`
//      *is* the `<pre>`; `Combobox` exists because a `<select>` could not be
//      styled. Banning the element inside its own wrapper is incoherent.
//   2. `*.stories.tsx` / `*.test.ts(x)` — fixtures, not shipped UI. A story's
//      job is to stand something concrete next to the component under test,
//      and `check-literals.ts` skips them for the same reason.
//   3. an inline `// ui-primitives-allow: <reason>` on the line before. The
//      reason is mandatory and the waiver lives at the point of use, so it
//      cannot outlive the code it excuses the way an entry in a central
//      allow-list file can. Every honoured waiver is printed on every run, so
//      the set stays visible rather than accumulating quietly.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Glob } from "bun";

const ROOT = join(import.meta.dir, "..");

/** where a primitive is allowed to be the thing it wraps */
const PRIMITIVES_DIR = "src/components/ui/";
const PRIMITIVE_MODULES = "src/components/ui/*.tsx";

/** scanned for every rule */
const SCANNED = [
  "src/components/**/*.tsx",
  "src/pages/**/*.tsx",
  "src/lib/**/*.{ts,tsx}",
  "src/App.tsx",
];

/** fixtures rather than shipped UI, exactly as `check-literals.ts` reads them */
const FIXTURES = /\.(stories|test)\.tsx?$/;

/**
 * Blank out `//` and block comments, keeping every byte offset so line numbers
 * stay honest.
 */
export function stripComments(source: string): string {
  const out = source.split("");
  const blank = (from: number, to: number) => {
    for (let i = from; i < to; i += 1) if (out[i] !== "\n") out[i] = " ";
  };
  let i = 0;
  while (i < source.length) {
    if (source[i] === "/" && source[i + 1] === "/") {
      const end = source.indexOf("\n", i);
      const stop = end === -1 ? source.length : end;
      blank(i, stop);
      i = stop;
    } else if (source[i] === "/" && source[i + 1] === "*") {
      const end = source.indexOf("*/", i + 2);
      const stop = end === -1 ? source.length : end + 2;
      blank(i, stop);
      i = stop;
    } else {
      i += 1;
    }
  }
  return out.join("");
}

/**
 * The waiver marker and its reason.
 *
 * Matched on a line already known to be a comment, so the leading `//`, `*` or
 * `{/*` does not have to be spelled out here — a JSX block comment is the only
 * kind that can sit above an element inside a return, and requiring `//` there
 * would make the waiver unusable at exactly the place the `<pre>` rule bites.
 */
const WAIVER = /ui-primitives-allow:\s*(.*)$/;

export type RuleId = "select" | "pre" | "native-dialog" | "shadowed-primitive";

export interface Violation {
  file: string;
  line: number;
  rule: RuleId;
  /** the source text that tripped the rule, for the message */
  found: string;
}

export interface Waiver {
  file: string;
  line: number;
  rule: RuleId | "unknown";
  reason: string;
}

/** What to write instead, per rule. A guard that only says "no" gets ignored. */
const ADVICE: Record<RuleId, string> = {
  select:
    "a bare `<select>` cannot be styled to the design system and has no search, " +
    "no secondary line and no empty state. Use `Combobox` from " +
    "`@/components/ui/combobox`.",
  pre:
    "a raw `<pre>` is a payload without a copy button, without a focusable " +
    "scroll region and without highlighting. Render JSON, YAML, TOML, CSV or " +
    "logs with `CodeBlock` from `@/components/ui/code-block`.",
  "native-dialog":
    "a native `window.confirm`/`alert`/`prompt` is unstyled, untranslatable, " +
    "and in a test runner it blocks the thread. Use `ConfirmDialog` from " +
    "`@/components/ConfirmDialog`.",
  "shadowed-primitive":
    "this re-declares a component `src/components/ui/` already exports, " +
    "without importing it — a second copy of a primitive, which is how #1044 " +
    "happened. Import the shared one, or compose it under a name of its own.",
};

/**
 * The PascalCase components `src/components/ui/` exports.
 *
 * Read from the modules rather than listed here, so a primitive added tomorrow
 * is covered the day it lands — the same reason `check-story-parameters.ts`
 * reads its fixtures out of their modules. `NAV` and friends are skipped: an
 * all-caps constant is data, not a component, and nothing hand-rolls one.
 */
export function readPrimitiveNames(root = ROOT): string[] {
  const names = new Set<string>();
  const decl = /^export (?:function|const) ([A-Z][A-Za-z0-9]*)/gm;
  for (const path of new Glob(PRIMITIVE_MODULES).scanSync(root)) {
    if (FIXTURES.test(path)) continue;
    const source = stripComments(readFileSync(join(root, path), "utf8"));
    let match: RegExpExecArray | null;
    while ((match = decl.exec(source))) {
      const name = match[1]!;
      // PascalCase only: `NAV` is a nav table, not a primitive
      if (/[a-z]/.test(name)) names.add(name);
    }
  }
  return [...names].sort();
}

/**
 * The names this file pulls in from `@/components/ui/…`.
 *
 * Reads the *local* name of an aliased import — `Dialog as BaseDialog` counts
 * as having `Dialog`, because the file demonstrably reaches for the shared one.
 */
export function importedPrimitives(source: string): Set<string> {
  const imported = new Set<string>();
  const spec = /import\s*\{([^}]*)\}\s*from\s*["']@\/components\/ui\//g;
  let match: RegExpExecArray | null;
  while ((match = spec.exec(source))) {
    for (const part of match[1]!.split(",")) {
      const name = part.trim().split(/\s+as\s+/)[0]?.trim();
      if (name) imported.add(name);
    }
  }
  return imported;
}

/** is this line part of a comment: a line comment, a block body, or a JSX one? */
function isCommentLine(line: string): boolean {
  return /^\s*(\/\/|\/\*|\*|\{\s*\/\*)/.test(line) || /\*\/\s*\}?\s*$/.test(line);
}

/**
 * The waiver reason for the code at `index`, or `null` when there is none.
 *
 * Walks up through the contiguous comment block directly above, so a reason
 * may be as long as it needs to be. An empty string means the marker was
 * written without one, which the caller reports rather than honours.
 */
export function waiverAbove(lines: string[], index: number): string | null {
  for (let i = index - 1; i >= 0; i -= 1) {
    const line = lines[i] ?? "";
    if (!isCommentLine(line)) return null;
    const match = WAIVER.exec(line);
    if (match) return match[1]!.replace(/\*\/\s*\}?\s*$/, "").trim();
  }
  return null;
}

/**
 * Every rule broken in one file.
 *
 * `primitives` is the set of shared component names; pass `[]` to check only
 * the three element rules.
 */
export function checkSource(
  source: string,
  file: string,
  primitives: string[] = [],
): { violations: Violation[]; waivers: Waiver[] } {
  const violations: Violation[] = [];
  const waivers: Waiver[] = [];
  if (file.startsWith(PRIMITIVES_DIR) || FIXTURES.test(file)) {
    return { violations, waivers };
  }

  const masked = stripComments(source);
  const lines = source.split("\n");
  const maskedLines = masked.split("\n");
  const imported = importedPrimitives(source);
  // a name is only shadowed when the file does not pull the shared one in: a
  // thin wrapper that adapts `Dialog as BaseDialog` composes the primitive, it
  // does not duplicate it, and banning that would push screens away from the
  // shared component rather than towards it
  const shadowable = primitives.filter((name) => !imported.has(name));
  const shadow = shadowable.length
    ? new RegExp(`^(?:export )?(?:function (${shadowable.join("|")})\\b|const (${shadowable.join("|")})\\s*[:=])`)
    : null;

  const record = (index: number, rule: RuleId, found: string) => {
    // the waiver sits in the comment block directly above, so a reader meets
    // the reason before the code it excuses. the whole block is searched
    // rather than one line: a reason worth writing rarely fits on one, and a
    // guard that only reads the last line of a paragraph would quietly ignore
    // waivers that look perfectly correct in the editor
    const found_waiver = waiverAbove(lines, index);
    if (found_waiver !== null) {
      waivers.push({
        file,
        line: index + 1,
        rule: found_waiver ? rule : "unknown",
        reason: found_waiver,
      });
      return;
    }
    violations.push({ file, line: index + 1, rule, found });
  };

  maskedLines.forEach((line, index) => {
    // `<select` and `<pre` as JSX tags — `<select>` and `<select\n` both, but
    // not `<presence>` or a word ending in "pre"
    if (/<select[\s/>]/.test(line) || /<select$/.test(line)) record(index, "select", "<select>");
    if (/<pre[\s/>]/.test(line) || /<pre$/.test(line)) record(index, "pre", "<pre>");
    const native = /\bwindow\.(confirm|alert|prompt)\s*\(/.exec(line);
    if (native) record(index, "native-dialog", `window.${native[1]}()`);
    const shadowed = shadow?.exec(line);
    if (shadowed) {
      record(index, "shadowed-primitive", (shadowed[1] ?? shadowed[2])!);
    }
  });

  return { violations, waivers };
}

/** The failure text — it has to name the replacement, not only the offence. */
export function describeViolation(v: Violation): string {
  return `${v.file}:${v.line}: ${v.found} — ${ADVICE[v.rule]}`;
}

export function checkAll(root = ROOT): { violations: Violation[]; waivers: Waiver[] } {
  const primitives = readPrimitiveNames(root);
  const violations: Violation[] = [];
  const waivers: Waiver[] = [];
  const seen = new Set<string>();
  for (const pattern of SCANNED) {
    for (const path of new Glob(pattern).scanSync(root)) {
      const rel = path.replace(/\\/g, "/");
      if (seen.has(rel)) continue;
      seen.add(rel);
      const result = checkSource(readFileSync(join(root, path), "utf8"), rel, primitives);
      violations.push(...result.violations);
      waivers.push(...result.waivers);
    }
  }
  const byPlace = (a: { file: string; line: number }, b: { file: string; line: number }) =>
    a.file.localeCompare(b.file) || a.line - b.line;
  return { violations: violations.sort(byPlace), waivers: waivers.sort(byPlace) };
}

if (import.meta.main) {
  const primitives = readPrimitiveNames();
  const { violations, waivers } = checkAll();

  console.log(`scanned ${SCANNED.join(", ")}`);
  console.log(`  ${primitives.length} shared component(s) from ${PRIMITIVE_MODULES}`);
  console.log(`  ${waivers.length} waiver(s) honoured`);
  for (const w of waivers) console.log(`    ${w.file}:${w.line}  [${w.rule}]  ${w.reason}`);

  // a waiver with no reason is not a waiver, it is a silencer
  const unexplained = waivers.filter((w) => w.rule === "unknown");
  if (unexplained.length > 0) {
    console.error(`\n${unexplained.length} waiver(s) with no reason:`);
    for (const w of unexplained) console.error(`  ${w.file}:${w.line}`);
    console.error(
      "\nwrite `// ui-primitives-allow: <why this one is not the mistake the rule\n" +
        "is about>` — an unexplained waiver is indistinguishable from the bug.",
    );
    process.exit(1);
  }

  if (violations.length > 0) {
    console.error(`\n${violations.length} hand-rolled shared component(s):`);
    for (const v of violations) console.error(`  ${describeViolation(v)}`);
    console.error(
      "\nthese are the maintenance-matrix rules in AGENTS.md. If a case genuinely is\n" +
        "not what the rule is about, waive it with `// ui-primitives-allow: <reason>`\n" +
        "on the line above and say why.",
    );
    process.exit(1);
  }

  console.log("\nno hand-rolled shared components");
}
