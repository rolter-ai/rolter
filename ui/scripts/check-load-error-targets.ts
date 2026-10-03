#!/usr/bin/env bun
// LoadError / EmptyState region pairing gate (#2641).
//
//   bun run check:load-error-targets
//
// The dead-states query pairs a list's `error_state` with its `empty_state` by
// region name: a `LoadError`'s `target` against an `EmptyState`'s `uxTarget`.
// #2444 found sixteen alerts that had drifted to a name of their own, so one
// list showed up as two regions, and renamed them by hand. Nothing kept them
// together afterwards; this does.
//
// **A `LoadError` whose literal `target` matches none of the `uxTarget`s in the
// same file** fails. The file is the unit because a list and its alert render
// side by side, and a pairing that needs another file to hold it is the drift
// the rule exists to stop.
//
// A file with no `EmptyState` literal at all is not a list screen — a settings
// form, a profile card, a scope picker — and has no second row to drift from,
// so it is not read. The rule bites where a screen has both states and one of
// them names a region the other does not.
//
// Only string literals are read. A `target={target}` that a wrapper forwards
// (`McpOAuth`, `Dashboard`) names nothing in this file, so there is nothing to
// compare; its caller's literal is checked where that is written. A ternary of
// literals contributes every branch, on either side.
//
// A region that genuinely has no empty state — a usage figure, a drawer fed by
// one row, a read that only decorates a list — carries
// `// load-error-allow: <reason>` in the comment block above its `<LoadError`,
// the way `check:primitives` takes its waiver. Every honoured waiver is printed
// on every run, so the set stays visible.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Glob } from "bun";
import { blankComments } from "./check-story-focus";

const ROOT = join(import.meta.dir, "..");

const SCANNED = ["src/**/*.tsx"];
/** stories and the component that declares the prop are not call sites */
const SKIPPED = /\.stories\.tsx$|(^|\/)LoadError\.tsx$/;

const WAIVER = /load-error-allow:\s*(.*)$/;

export interface Violation {
  file: string;
  line: number;
  target: string;
}

export interface Waiver {
  file: string;
  line: number;
  target: string;
  reason: string;
}

export interface Result {
  violations: Violation[];
  waivers: Waiver[];
}

/** every string literal in an expression, which is how a ternary is read */
function literals(expression: string): string[] {
  return [...expression.matchAll(/(["'`])([^"'`\n]*?)\1/g)].map((m) => m[2]);
}

/** the expression of a JSX prop: `"x"`, or the balanced `{ … }` after `name=` */
function propValue(text: string, name: string): string | null {
  const start = new RegExp(`\\b${name}=`).exec(text);
  if (!start) return null;
  let i = start.index + start[0].length;
  if (text[i] !== "{") return text.slice(i).match(/^(["'])[^"']*\1/)?.[0] ?? null;
  let depth = 0;
  for (const from = i; i < text.length; i += 1) {
    if (text[i] === "{") depth += 1;
    if (text[i] === "}" && (depth -= 1) === 0) return text.slice(from, i + 1);
  }
  return null;
}

/** the `<LoadError … />` element starting at `at`, up to its closing `/>` */
function element(source: string, at: number): string {
  let depth = 0;
  for (let i = at; i < source.length; i += 1) {
    if (source[i] === "{") depth += 1;
    if (source[i] === "}") depth -= 1;
    if (depth === 0 && source[i] === "/" && source[i + 1] === ">") return source.slice(at, i);
  }
  return source.slice(at);
}

export function checkSource(source: string, file: string): Result {
  const blanked = blankComments(source);
  const raw = source.split("\n");
  const violations: Violation[] = [];
  const waivers: Waiver[] = [];

  const paired = new Set<string>();
  for (const m of blanked.matchAll(/\buxTarget=/g)) {
    const value = propValue(blanked.slice(m.index), "uxTarget");
    for (const lit of literals(value ?? "")) paired.add(lit);
  }

  // a file with no empty state has no list to pair with: a settings form, a
  // card or a drawer fails alone, and there is no second row to drift from
  if (paired.size === 0) return { violations, waivers };

  for (const m of blanked.matchAll(/<LoadError\b/g)) {
    const props = element(blanked, m.index);
    const value = propValue(props, "target");
    const targets = literals(value ?? "");
    if (targets.length === 0 || targets.some((t) => paired.has(t))) continue;

    const line = blanked.slice(0, m.index).split("\n").length - 1;
    const target = targets.join(" | ");
    // the marker may sit anywhere in the comment block above the element
    let waived: string | null = null;
    for (let i = line - 1; i >= 0 && /^\s*(\/\/|\*|\/\*|\{\/\*)/.test(raw[i] ?? ""); i -= 1) {
      const marker = WAIVER.exec(raw[i]);
      if (marker) {
        waived = marker[1].replace(/\*\/\}?\s*$/, "").trim();
        break;
      }
    }
    if (waived !== null) waivers.push({ file, line: line + 1, target, reason: waived });
    else violations.push({ file, line: line + 1, target });
  }
  return { violations, waivers };
}

export function checkAll(): Result {
  const violations: Violation[] = [];
  const waivers: Waiver[] = [];
  for (const pattern of SCANNED) {
    for (const file of new Glob(pattern).scanSync(ROOT)) {
      if (SKIPPED.test(file)) continue;
      const result = checkSource(readFileSync(join(ROOT, file), "utf8"), file);
      violations.push(...result.violations);
      waivers.push(...result.waivers);
    }
  }
  const order = <T extends { file: string; line: number }>(a: T, b: T) =>
    a.file.localeCompare(b.file) || a.line - b.line;
  return { violations: violations.sort(order), waivers: waivers.sort(order) };
}

if (import.meta.main) {
  const { violations, waivers } = checkAll();
  console.log(`scanned ${SCANNED.join(", ")}`);

  if (waivers.length > 0) {
    console.log(`\n${waivers.length} waived:`);
    for (const w of waivers) {
      console.log(`  ${w.file}:${w.line}  ${w.target}  ${w.reason || "(no reason)"}`);
    }
  }

  if (violations.length > 0) {
    console.error(`\n${violations.length} LoadError target(s) with no EmptyState partner:`);
    for (const v of violations) console.error(`  ${v.file}:${v.line}  target="${v.target}"`);
    console.error(
      "\nThe dead-states query pairs an error state with its empty state by region\n" +
        "name. Name the `LoadError` target like the list's `EmptyState` `uxTarget`.\n" +
        "A region that genuinely has no empty state carries\n" +
        "`// load-error-allow: <reason>` in the comment block above the `<LoadError`.",
    );
    process.exit(1);
  }

  console.log("\nevery LoadError target pairs with an EmptyState uxTarget in its file");
}
