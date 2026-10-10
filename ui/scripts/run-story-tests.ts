#!/usr/bin/env bun
// Run the story tests on the Vitest addon, and refuse a green that ran nothing (#2907).
//
// Every story is a Vitest test: `@storybook/addon-vitest` reads `.storybook/main.ts`,
// turns each story file into a test file and runs it in headless chromium
// through `@vitest/browser-playwright`. Its play function is the body, and the
// axe check is the preview's `afterEach` (`.storybook/preview.ts`).
//
// This wrapper used to start `storybook dev` on a free port and run
// `test-storybook` against it, to stop a taken port from letting the run pass
// against somebody else's build (#1684, #1693, #2323). Those guards are
// structural now. Vitest starts its own Vite server from this checkout's `ui/`
// and the plugin reads this checkout's `.storybook`, so there is no Storybook
// on a port to mistake for ours, and no port to pick: the browser server takes
// the first free one itself.
//
// What is left to guard is the other way a run goes green while proving nothing:
// a story file Vitest never picked up. A glob in `main.ts` that stopped matching
// a directory, a `!test` tag, or an export the transform skipped all report
// success with fewer tests than the file declares. So after the run this reads
// Vitest's JSON report and fails when a file it was asked for ran fewer tests
// than it has `export const … : Story` declarations, or did not run at all.
//
// Usage:
//   bun scripts/run-story-tests.ts src/pages/Keys.stories.tsx [more…]
//   bun scripts/run-story-tests.ts            # every story file
//   bun scripts/run-story-tests.ts src/pages/Keys.stories.tsx -- --maxWorkers=2
//
// Everything after `--` goes to vitest. A run narrowed by `-t` (a test name
// pattern) or `--bail` is partial on purpose, so the count check is skipped.
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative, resolve } from "node:path";
import { Glob } from "bun";

const UI_DIR = join(import.meta.dir, "..");

/**
 * The story exports a file declares.
 *
 * Deliberately a regex over the source rather than an import: this runs before
 * the browser has loaded anything, the file imports JSX and project aliases,
 * and the only question being asked is "does the report hold at least as many
 * tests as this file has stories". `default` is the meta, not a story.
 */
export function declaredStories(source: string): string[] {
  // only the exports annotated as a story: a story file commonly exports a
  // fixture, a helper or a stub beside them, and those are not tests
  return [...source.matchAll(/^export const (\w+)\s*:\s*Story\b/gm)].map((match) => match[1]);
}

/** The fields of Vitest's JSON report this script reads. */
export interface VitestReport {
  testResults: { name: string; assertionResults: { status: string }[] }[];
}

/**
 * What the report is missing for these files, as human-readable lines. Empty
 * means every file ran at least as many tests as it has stories.
 *
 * `names` are absolute in Vitest's report; they are compared project-relative,
 * the spelling the story files are asked for in.
 */
export function missingFrom(
  report: VitestReport,
  files: { path: string; stories: string[] }[],
  root: string,
): string[] {
  const ran = new Map<string, number>();
  for (const result of report.testResults ?? []) {
    const path = relative(root, result.name);
    // a skipped story is a story that did not run; todo and pending likewise
    const passed = result.assertionResults.filter(
      (test) => test.status === "passed" || test.status === "failed",
    ).length;
    ran.set(path, (ran.get(path) ?? 0) + passed);
  }
  const problems: string[] = [];
  for (const file of files) {
    const count = ran.get(file.path);
    if (count === undefined) {
      problems.push(`${file.path} did not run at all`);
    } else if (count < file.stories.length) {
      problems.push(`${file.path} declares ${file.stories.length} stories but ran ${count}`);
    }
  }
  return problems;
}

/**
 * Whether the arguments hand Vitest a filter that makes a run partial on
 * purpose, so a count below the declared stories is the point and no fault.
 */
export function narrowsTheRun(vitestArgs: string[]): boolean {
  return vitestArgs.some(
    (arg) =>
      arg === "-t" ||
      arg.startsWith("--testNamePattern") ||
      arg.startsWith("--bail") ||
      arg.startsWith("--changed") ||
      arg.startsWith("--shard"),
  );
}

/** Split `a.stories.tsx b.stories.tsx -- --flag` into the files and the vitest arguments. */
export function splitArgs(argv: string[]): { files: string[]; vitestArgs: string[] } {
  const dashes = argv.indexOf("--");
  const files = dashes === -1 ? argv : argv.slice(0, dashes);
  const vitestArgs = dashes === -1 ? [] : argv.slice(dashes + 1);
  return { files, vitestArgs };
}

function storyFiles(args: string[]): string[] {
  if (args.length > 0) return args.map((arg) => relative(UI_DIR, resolve(arg)));
  return [...new Glob("src/**/*.stories.tsx").scanSync(UI_DIR)].sort();
}

async function main() {
  const { files: asked, vitestArgs } = splitArgs(process.argv.slice(2));
  const files = storyFiles(asked).map((path) => ({
    path,
    stories: declaredStories(readFileSync(join(UI_DIR, path), "utf8")),
  }));
  if (files.length === 0) {
    console.error("no story files matched");
    process.exit(1);
  }

  const scratch = mkdtempSync(join(tmpdir(), "rolter-stories-"));
  const reportPath = join(scratch, "report.json");
  const started = Date.now();
  console.log(`[stories] running ${files.length} story file(s) on the vitest addon`);

  const vitest = spawn(
    join(UI_DIR, "node_modules", ".bin", "vitest"),
    [
      "run",
      "--project=storybook",
      // vitest treats a positional argument as a filter on the file name, so an
      // unfiltered run passes none and takes every story file `main.ts` lists
      ...(asked.length > 0 ? files.map((file) => file.path) : []),
      ...vitestArgs,
    ],
    // the JSON report is asked for by vitest.config.ts, where every reporter is
    // listed: a `--reporter` here would replace that list, not add to it
    { cwd: UI_DIR, stdio: "inherit", env: { ...process.env, ROLTER_STORY_REPORT: reportPath } },
  );
  for (const signal of ["SIGINT", "SIGTERM"] as const) {
    process.on(signal, () => vitest.kill(signal));
  }
  const status = await new Promise<number>((done) => {
    vitest.once("exit", (code, signal) => done(code ?? (signal ? 1 : 0)));
  });

  let problems: string[] = [];
  if (!narrowsTheRun(vitestArgs)) {
    try {
      const report = JSON.parse(readFileSync(reportPath, "utf8")) as VitestReport;
      problems = missingFrom(report, files, UI_DIR);
    } catch (error) {
      problems = [`vitest wrote no readable report (${String(error)})`];
    }
  }
  rmSync(scratch, { recursive: true, force: true });

  const seconds = ((Date.now() - started) / 1000).toFixed(1);
  if (problems.length > 0) {
    console.error(
      `[stories] a green run is not a run of these files (${seconds}s):\n  ${problems.join("\n  ")}`,
    );
    process.exit(1);
  }
  console.log(`[stories] ${files.length} file(s) checked against the report in ${seconds}s`);
  process.exit(status);
}

if (import.meta.main) await main();
