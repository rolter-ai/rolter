#!/usr/bin/env bun
// story-fixture spread gate (#1373).
//
//   bun run check:stories
//
// A story fixture is one of two shapes, and putting one where the other goes
// is silent:
//
//   src/lib/story-a11y.ts    `withPageA11y` is a *parameters fragment* — it
//                            must be spread inside `parameters: { … }`
//   src/lib/story-viewport.ts `atMobile`/`atTablet` are *story fields* — they
//                            carry their own `parameters` and must be spread
//                            at story or meta level
//
// Both mistakes run green. A fragment spread one level too high lands as a
// bare story field nothing reads; a story-fields fixture spread inside
// `parameters` buries `parameters.parameters`. The first is how #1353 shipped
// an axe override that asserted nothing — the docgen transform appends a
// `parameters: { docs: … }` to every meta and replaced the whole-object spread
// that arrived before it.
//
// The two shapes are read from the fixture modules themselves rather than
// listed here, so a fixture added later is covered the day it is written.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Glob } from "bun";

const ROOT = join(import.meta.dir, "..");
const FIXTURE_MODULES = "src/lib/story-*.ts";
const STORIES = "src/**/*.stories.tsx";

/** What a fixture object is meant to be spread into. */
export type FixtureKind = "parameters-fragment" | "story-fields";

export interface Fixture {
  name: string;
  kind: FixtureKind;
  /** module path the fixture is exported from, for the failure message */
  module: string;
}

export interface Violation {
  file: string;
  line: number;
  name: string;
  kind: FixtureKind;
  module: string;
}

/**
 * Blank out comments and string bodies, keeping every byte offset.
 *
 * The checks below count braces, and a `{` inside a comment or a string is not
 * a scope. Replacing the contents with spaces rather than deleting them keeps
 * indexes lined up with the original source, so line numbers stay honest.
 */
export function maskLiterals(source: string): string {
  const out = source.split("");
  const blank = (from: number, to: number) => {
    for (let i = from; i < to; i += 1) if (out[i] !== "\n") out[i] = " ";
  };
  // depth of `${…}` inside the innermost template literal, one entry per
  // nested template
  const templates: number[] = [];
  let i = 0;
  while (i < source.length) {
    const c = source[i];
    const next = source[i + 1];
    if (c === "/" && next === "/") {
      const end = source.indexOf("\n", i);
      const stop = end === -1 ? source.length : end;
      blank(i, stop);
      i = stop;
    } else if (c === "/" && next === "*") {
      const end = source.indexOf("*/", i + 2);
      const stop = end === -1 ? source.length : end + 2;
      blank(i, stop);
      i = stop;
    } else if (c === '"' || c === "'") {
      let j = i + 1;
      while (j < source.length && source[j] !== c) j += source[j] === "\\" ? 2 : 1;
      blank(i + 1, Math.min(j, source.length));
      i = Math.min(j + 1, source.length);
    } else if (c === "`") {
      templates.push(0);
      i += 1;
    } else if (templates.length > 0) {
      // inside a template: only `${` … `}` is real code, the rest is text
      const depth = templates[templates.length - 1]!;
      if (c === "\\") {
        blank(i, i + 2);
        i += 2;
      } else if (depth === 0 && c === "`") {
        templates.pop();
        i += 1;
      } else if (depth === 0 && c === "$" && next === "{") {
        templates[templates.length - 1] = 1;
        i += 2;
      } else if (depth === 0) {
        blank(i, i + 1);
        i += 1;
      } else {
        if (c === "{") templates[templates.length - 1] = depth + 1;
        if (c === "}") templates[templates.length - 1] = depth - 1;
        i += 1;
      }
    } else {
      i += 1;
    }
  }
  return out.join("");
}

/** Index of the `}` closing the `{` at `open`, or -1. */
function matchBrace(masked: string, open: number): number {
  let depth = 0;
  for (let i = open; i < masked.length; i += 1) {
    if (masked[i] === "{") depth += 1;
    else if (masked[i] === "}") {
      depth -= 1;
      if (depth === 0) return i;
    }
  }
  return -1;
}

/** The keys of the object literal `{ … }` spanning `open`…`close`, top level only. */
function topLevelKeys(masked: string, open: number, close: number): string[] {
  const keys: string[] = [];
  let depth = 0;
  for (let i = open; i < close; i += 1) {
    const c = masked[i]!;
    if ("{[(".includes(c)) depth += 1;
    else if ("}])".includes(c)) depth -= 1;
    else if (depth === 1) {
      const key = /^\s*"?([A-Za-z_$][\w$]*)"?\s*:/.exec(masked.slice(i, i + 64));
      if (key && (masked[i - 1] === "{" || masked[i - 1] === ",")) keys.push(key[1]!);
    }
  }
  return keys;
}

/**
 * The exported object fixtures of one module, classified by what they carry.
 *
 * A fixture with a top-level `parameters` key *is* a story object and belongs
 * at story level; one without is a fragment of `parameters` and belongs inside
 * it. Nothing else is a fixture — a function or a plain constant is skipped.
 */
export function collectFixtures(source: string, module: string): Fixture[] {
  const masked = maskLiterals(source);
  const fixtures: Fixture[] = [];
  const decl = /export const ([A-Za-z_$][\w$]*)(?::[^=]+)? = \{/g;
  let match: RegExpExecArray | null;
  while ((match = decl.exec(masked))) {
    const open = masked.indexOf("{", match.index);
    const close = matchBrace(masked, open);
    if (close === -1) continue;
    const keys = topLevelKeys(masked, open, close);
    fixtures.push({
      name: match[1]!,
      kind: keys.includes("parameters") ? "story-fields" : "parameters-fragment",
      module,
    });
  }
  return fixtures;
}

/** Is the spread at `index` directly inside a `parameters: { … }` object? */
function insideParameters(masked: string, index: number): boolean {
  let depth = 0;
  for (let i = index - 1; i >= 0; i -= 1) {
    const c = masked[i];
    if (c === "}") depth += 1;
    else if (c === "{") {
      if (depth === 0) return /\bparameters\s*:\s*$/.test(masked.slice(Math.max(0, i - 32), i));
      depth -= 1;
    }
  }
  return false;
}

/** Every misplaced fixture spread in one story file. */
export function checkStorySource(
  source: string,
  file: string,
  fixtures: Fixture[],
): Violation[] {
  const masked = maskLiterals(source);
  const violations: Violation[] = [];
  for (const fixture of fixtures) {
    const spread = new RegExp(`\\.\\.\\.${fixture.name}\\b`, "g");
    let match: RegExpExecArray | null;
    while ((match = spread.exec(masked))) {
      const nested = insideParameters(masked, match.index);
      if (nested === (fixture.kind === "parameters-fragment")) continue;
      violations.push({
        file,
        line: source.slice(0, match.index).split("\n").length,
        name: fixture.name,
        kind: fixture.kind,
        module: fixture.module,
      });
    }
  }
  return violations.sort((a, b) => a.line - b.line);
}

/** The failure text — it has to say what to write instead, not only what is wrong. */
export function describeViolation(v: Violation): string {
  const where =
    v.kind === "parameters-fragment"
      ? `\`...${v.name}\` is a \`parameters\` fragment and must be spread inside \`parameters: { ... }\`. ` +
        "Spread at story or meta level it lands as a bare field nothing reads, and the docgen " +
        "transform's own `parameters` replaces anything that arrived by whole-object spread — the " +
        "story stays green while asserting nothing (#1373)."
      : `\`...${v.name}\` carries its own \`parameters\` and must be spread at story or meta level, ` +
        "not inside `parameters: { ... }`, which would bury it as `parameters.parameters` (#1373).";
  return `${v.file}:${v.line}: ${where} It is exported from ${v.module}.`;
}

export function readFixtures(root = ROOT): Fixture[] {
  const fixtures: Fixture[] = [];
  for (const path of new Glob(FIXTURE_MODULES).scanSync(root)) {
    if (path.endsWith(".test.ts")) continue;
    const rel = path.replaceAll("\\", "/");
    fixtures.push(...collectFixtures(readFileSync(join(root, path), "utf8"), rel));
  }
  return fixtures.sort((a, b) => a.name.localeCompare(b.name));
}

export function checkAllStories(root = ROOT): Violation[] {
  const fixtures = readFixtures(root);
  const violations: Violation[] = [];
  for (const path of new Glob(STORIES).scanSync(root)) {
    const rel = path.replaceAll("\\", "/");
    violations.push(...checkStorySource(readFileSync(join(root, path), "utf8"), rel, fixtures));
  }
  return violations;
}

if (import.meta.main) {
  const fixtures = readFixtures();
  const violations = checkAllStories();
  console.log(
    `checked ${STORIES} against ${fixtures.length} fixture(s) from ${FIXTURE_MODULES}: ` +
      fixtures.map((f) => `${f.name} (${f.kind})`).join(", "),
  );
  if (violations.length > 0) {
    console.error(`\n${violations.length} misplaced fixture spread(s):`);
    for (const v of violations) console.error(`  ${describeViolation(v)}`);
    process.exit(1);
  }
  console.log("every fixture spread is in the object it belongs to");
}
