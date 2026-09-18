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

import { REPEATED_SHAPES } from "./repeated-shapes-allowlist";

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

export type RuleId = "select" | "pre" | "native-dialog" | "shadowed-primitive" | "duplicated-shape";

export interface Violation {
  file: string;
  line: number;
  rule: RuleId;
  /** the source text that tripped the rule, for the message */
  found: string;
  /**
   * Every `file:line` sharing this shape, for `duplicated-shape` only.
   *
   * The whole point of that rule is the set, not the one site it happens to
   * report at: a message naming a single line would send the reader to fix one
   * copy of five.
   */
  sites?: string[];
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
  "duplicated-shape":
    "the same element and the same design-system classes, hand-written in three " +
    "or more files — a primitive that was never extracted, which is how #1658 " +
    'shipped the same save failure with `role="alert"` in one file and without ' +
    "it in four. Pull it into `src/components/ui/` and import it.",
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
      const name = part
        .trim()
        .split(/\s+as\s+/)[0]
        ?.trim();
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
    ? new RegExp(
        `^(?:export )?(?:function (${shadowable.join("|")})\\b|const (${shadowable.join("|")})\\s*[:=])`,
      )
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

/**
 * How many design-system tokens make a className distinctive (#1686).
 *
 * `flex items-center gap-2` repeats in eighteen files and always will — a stack
 * of generic utilities is a sentence in Tailwind, not a component. What marks a
 * *deliberate* shape is the arbitrary value: `rounded-[10px]`,
 * `text-[color:var(--text-subtle)]`, `max-w-[840px]`. Those are the design
 * system spelled out by hand, and when the same handful of them lands on the
 * same tag in three files, somebody has re-typed a primitive.
 *
 * So the filter is two knobs rather than one length. Both were tuned against
 * the tree: at four tokens with two arbitrary ones, the footer line #1658 had
 * to fix by hand (`p` with `px-[22px]` and
 * `text-[color:var(--status-danger-text)]`) is caught, and not one generic flex
 * row is.
 */
const MIN_TOKENS = 4;
const MIN_ARBITRARY = 2;

/** a shape may live in at most this many files before it is a missing primitive */
const MAX_FILES = 2;

export interface Shape {
  file: string;
  line: number;
  /** `tag|class class class`, sorted so attribute order cannot hide a copy */
  key: string;
  tag: string;
}

/** `rounded-[10px]`, `text-[color:var(--x)]`, `[grid-template-columns:…]` */
function isArbitrary(token: string): boolean {
  return token.includes("[");
}

/**
 * The end of the JSX opening tag that starts at `from`.
 *
 * Walked rather than matched with `[^>]*`: an `onClick={() => x}` before the
 * `className` puts a `>` inside the tag, and a regex that stopped there would
 * miss every element with a handler on it — which is most of the interesting
 * ones.
 */
function endOfTag(source: string, from: number): number {
  let i = from;
  let depth = 0;
  let quote = "";
  while (i < source.length) {
    const c = source[i]!;
    if (quote) {
      if (c === quote) quote = "";
    } else if (c === '"' || c === "'" || c === "`") {
      quote = c;
    } else if (c === "{") {
      depth += 1;
    } else if (c === "}") {
      depth -= 1;
    } else if (c === ">" && depth === 0) {
      break;
    }
    i += 1;
  }
  return i;
}

/**
 * Every distinctive intrinsic element in one file, and the waivers that silence
 * one.
 *
 * Two deliberate narrowings, both of which trade recall for a guard nobody
 * turns off:
 *
 *   - **intrinsic tags only** (`div`, `section`, `button`), never `<Card>` or
 *     `<GatedButton>`. A shared component rendered with the same className in
 *     five screens is the primitive doing its job; flagging it would punish
 *     exactly the composition this whole check exists to encourage.
 *   - **literal classNames only**. `className={cn(…)}` is not compared: its
 *     value depends on props, so two spellings that look alike may render
 *     nothing alike, and a guard that guessed there would be wrong in the
 *     direction that costs trust.
 */
export function collectShapes(
  source: string,
  file: string,
): { shapes: Shape[]; waivers: Waiver[] } {
  const shapes: Shape[] = [];
  const waivers: Waiver[] = [];
  if (file.startsWith(PRIMITIVES_DIR) || FIXTURES.test(file)) return { shapes, waivers };

  const masked = stripComments(source);
  const lines = source.split("\n");
  // lowercase initial is what makes a JSX tag an intrinsic element
  const start = /<([a-z][a-z0-9.-]*)(?=[\s/>])/g;
  let match: RegExpExecArray | null;
  while ((match = start.exec(masked))) {
    const body = masked.slice(match.index, endOfTag(masked, match.index + match[0].length));
    const className = /\sclassName="([^"]*)"/.exec(body);
    if (!className) continue;
    const tokens = className[1]!.trim().split(/\s+/).filter(Boolean);
    const arbitrary = tokens.filter(isArbitrary).length;
    if (tokens.length < MIN_TOKENS || arbitrary < MIN_ARBITRARY) continue;

    const line = masked.slice(0, match.index).split("\n").length;
    const reason = waiverAbove(lines, line - 1);
    if (reason !== null) {
      waivers.push({
        file,
        line,
        rule: reason ? "duplicated-shape" : "unknown",
        reason,
      });
      continue;
    }
    shapes.push({
      file,
      line,
      tag: match[1]!,
      key: `${match[1]!}|${[...tokens].sort().join(" ")}`,
    });
  }
  return { shapes, waivers };
}

/**
 * A recorded shape and why it is still here.
 *
 * Modelled on `literals-allowlist.ts` rather than on a baseline JSON: an entry
 * carries the reason a reviewer accepted it, and `staleShapes()` deletes it the
 * moment the duplication is gone, so the list can only shrink.
 */
export type ShapeAllowList = Record<string, string>;

/** Every shape that outgrew `MAX_FILES` and is not recorded. */
export function duplicatedShapes(shapes: Shape[], allowed: ShapeAllowList = {}): Violation[] {
  const byKey = new Map<string, Shape[]>();
  for (const shape of shapes) {
    const at = byKey.get(shape.key);
    if (at) at.push(shape);
    else byKey.set(shape.key, [shape]);
  }
  const violations: Violation[] = [];
  for (const [key, group] of byKey) {
    const files = [...new Set(group.map((s) => s.file))];
    if (files.length <= MAX_FILES || key in allowed) continue;
    // reported at the first site so the message has a place to point, but the
    // whole set rides along: fixing one copy of five is not fixing this
    const first = group[0]!;
    violations.push({
      file: first.file,
      line: first.line,
      rule: "duplicated-shape",
      found: key,
      sites: group.map((s) => `${s.file}:${s.line}`),
    });
  }
  return violations;
}

/** Recorded shapes that no longer duplicate, so the entry has to go. */
export function staleShapes(shapes: Shape[], allowed: ShapeAllowList = {}): string[] {
  const files = new Map<string, Set<string>>();
  for (const shape of shapes) {
    const at = files.get(shape.key) ?? new Set<string>();
    at.add(shape.file);
    files.set(shape.key, at);
  }
  return Object.keys(allowed)
    .filter((key) => (files.get(key)?.size ?? 0) <= MAX_FILES)
    .sort();
}

/** Allow-list entries written without a reason, which silence rather than explain. */
export function unexplainedShapes(allowed: ShapeAllowList = {}): string[] {
  return Object.keys(allowed)
    .filter((key) => !allowed[key]?.trim())
    .sort();
}

/** The failure text — it has to name the replacement, not only the offence. */
export function describeViolation(v: Violation): string {
  if (v.sites) {
    const [tag, className] = v.found.split("|");
    return (
      `<${tag} className="${className}"> in ${new Set(v.sites.map((s) => s.split(":")[0])).size}` +
      ` files — ${ADVICE[v.rule]}\n      ${v.sites.join("\n      ")}`
    );
  }
  return `${v.file}:${v.line}: ${v.found} — ${ADVICE[v.rule]}`;
}

export function checkAll(
  root = ROOT,
  allowedShapes: ShapeAllowList = REPEATED_SHAPES,
): { violations: Violation[]; waivers: Waiver[]; stale: string[] } {
  const primitives = readPrimitiveNames(root);
  const violations: Violation[] = [];
  const waivers: Waiver[] = [];
  const shapes: Shape[] = [];
  const seen = new Set<string>();
  for (const pattern of SCANNED) {
    for (const path of new Glob(pattern).scanSync(root)) {
      const rel = path.replace(/\\/g, "/");
      if (seen.has(rel)) continue;
      seen.add(rel);
      const source = readFileSync(join(root, path), "utf8");
      const result = checkSource(source, rel, primitives);
      violations.push(...result.violations);
      waivers.push(...result.waivers);
      const shaped = collectShapes(source, rel);
      shapes.push(...shaped.shapes);
      waivers.push(...shaped.waivers);
    }
  }
  violations.push(...duplicatedShapes(shapes, allowedShapes));
  const byPlace = (a: { file: string; line: number }, b: { file: string; line: number }) =>
    a.file.localeCompare(b.file) || a.line - b.line;
  return {
    violations: violations.sort(byPlace),
    waivers: waivers.sort(byPlace),
    stale: staleShapes(shapes, allowedShapes),
  };
}

if (import.meta.main) {
  const primitives = readPrimitiveNames();
  const { violations, waivers, stale } = checkAll();

  console.log(`scanned ${SCANNED.join(", ")}`);
  console.log(`  ${primitives.length} shared component(s) from ${PRIMITIVE_MODULES}`);
  console.log(`  ${Object.keys(REPEATED_SHAPES).length} repeated shape(s) recorded`);
  console.log(`  ${waivers.length} waiver(s) honoured`);
  for (const w of waivers) console.log(`    ${w.file}:${w.line}  [${w.rule}]  ${w.reason}`);

  // a waiver with no reason is not a waiver, it is a silencer
  const silencers = unexplainedShapes(REPEATED_SHAPES);
  if (silencers.length > 0) {
    console.error(`\n${silencers.length} recorded shape(s) with no reason:`);
    for (const key of silencers) console.error(`  ${key}`);
    console.error("\nsay why the duplication is still there, or extract the primitive instead.");
    process.exit(1);
  }

  if (stale.length > 0) {
    console.error(`\n${stale.length} recorded shape(s) no longer duplicated:`);
    for (const key of stale) console.error(`  ${key}`);
    console.error(
      "\nthe primitive was extracted, or the copies were deleted. drop the entry from\n" +
        "scripts/repeated-shapes-allowlist.ts so the same shape cannot come back unnoticed.",
    );
    process.exit(1);
  }

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
