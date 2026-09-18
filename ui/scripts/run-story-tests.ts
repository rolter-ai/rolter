#!/usr/bin/env bun
// Run the story tests against a Storybook this worktree actually built (#1684).
//
// `storybook dev -p <port>` does not fail when the port is taken: it logs
// `Starting...`, exits, and leaves whatever was already listening in place.
// `test-storybook --url http://localhost:<port>` then runs happily against that
// other server — another worktree, or a stale static server from an earlier
// session — and reports a green suite for a build that never contained the
// stories under test. Nothing in the output says "this is not your build",
// which makes it the worst kind of green.
//
// So this wrapper refuses to guess:
//
//   1. it picks a free port itself, or fails loudly when the one you asked for
//      is taken, rather than letting Storybook shrug and continue
//   2. it starts `storybook dev --ci` and waits for `/index.json`
//   3. it checks that index against the story files it was asked to run: every
//      `export const` in those files must be present, under that file's own
//      import path. A server from another worktree fails here even when it is
//      serving a Storybook of the same project
//   4. only then does it run `test-storybook`, one file per invocation — the
//      positional pattern goes through `/bin/sh`, so a pattern containing
//      `(`, `|` or `)` dies with a shell syntax error
//
// Usage:
//   bun scripts/run-story-tests.ts src/pages/Keys.stories.tsx [more…]
//   bun scripts/run-story-tests.ts --port 6040 src/pages/Keys.stories.tsx
//   bun scripts/run-story-tests.ts            # every story file
import { spawn, spawnSync } from "node:child_process";
import { createServer, Socket } from "node:net";
import { readFileSync } from "node:fs";
import { join, relative, resolve } from "node:path";

const UI_DIR = join(import.meta.dir, "..");

/** A Storybook index entry, of the fields this script reads. */
export interface IndexEntry {
  id: string;
  importPath: string;
}

export interface StorybookIndex {
  entries: Record<string, IndexEntry>;
}

/**
 * The story-file paths an index says it knows, normalised to what this script
 * compares against: Storybook writes them project-relative with a `./` prefix.
 */
export function indexedPaths(index: StorybookIndex): Set<string> {
  return new Set(
    Object.values(index.entries ?? {}).map((entry) => entry.importPath.replace(/^\.\//, "")),
  );
}

/**
 * The story exports a file declares.
 *
 * Deliberately a regex over the source rather than an import: this runs before
 * the browser has loaded anything, the file imports JSX and project aliases,
 * and the only question being asked is "does the served index know the names
 * this file spells out". `default` is the meta, not a story.
 */
export function declaredStories(source: string): string[] {
  // only the exports annotated as a story: a story file commonly exports a
  // fixture, a helper or a stub beside them, and those are not indexed
  return [...source.matchAll(/^export const (\w+)\s*:\s*Story\b/gm)].map((match) => match[1]);
}

/**
 * Whether anything is listening on `port`, asked by connecting to it.
 *
 * Deliberately not a bind probe. A server that sets `SO_REUSEADDR` — python's
 * `http.server` does, and so does Bun.serve — lets a second bind on the same
 * port succeed, so a bind probe reports a squatted port as free and hands it
 * straight back to Storybook, which is the failure this whole script exists to
 * stop. A connect either reaches a listener or it does not.
 */
export async function somethingIsListening(port: number): Promise<boolean> {
  return new Promise((resolveP) => {
    const socket = new Socket();
    const done = (listening: boolean) => {
      socket.destroy();
      resolveP(listening);
    };
    socket.setTimeout(1000);
    socket.once("connect", () => done(true));
    socket.once("timeout", () => done(false));
    socket.once("error", () => done(false));
    socket.connect(port, "127.0.0.1");
  });
}

/** Whether `port` can be used: nothing listening, and the port bindable. */
export async function portIsFree(port: number): Promise<boolean> {
  if (await somethingIsListening(port)) return false;
  return new Promise((resolveP) => {
    const server = createServer();
    server.once("error", () => resolveP(false));
    server.once("listening", () => server.close(() => resolveP(true)));
    server.listen(port, "127.0.0.1");
  });
}

/** The first free port at or after `from`, so two worktrees never collide. */
export async function findFreePort(from = 6100, tries = 60): Promise<number> {
  for (let port = from; port < from + tries; port += 1) {
    if (await portIsFree(port)) return port;
  }
  throw new Error(`no free port in ${from}..${from + tries}`);
}

async function waitForIndex(port: number, timeoutMs = 180_000): Promise<StorybookIndex> {
  const deadline = Date.now() + timeoutMs;
  let lastError = "";
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/index.json`);
      if (response.ok) return (await response.json()) as StorybookIndex;
      lastError = `HTTP ${response.status}`;
    } catch (error) {
      lastError = String(error);
    }
    await new Promise((r) => setTimeout(r, 1000));
  }
  throw new Error(`storybook on ${port} never served /index.json (${lastError})`);
}

/** A name with its separators and case removed, for comparing the two spellings. */
function squash(value: string): string {
  return value.replace(/[^a-z0-9]/gi, "").toLowerCase();
}

/**
 * What is missing from the served index for these files, as human-readable
 * lines. Empty means the server is serving this worktree's build of them.
 */
export function missingFrom(
  index: StorybookIndex,
  files: { path: string; stories: string[] }[],
): string[] {
  const paths = indexedPaths(index);
  const problems: string[] = [];
  for (const file of files) {
    if (!paths.has(file.path)) {
      problems.push(`${file.path} is not in the served index at all`);
      continue;
    }
    const known = new Set(
      Object.values(index.entries ?? {})
        .filter((entry) => entry.importPath.replace(/^\.\//, "") === file.path)
        .map((entry) => entry.id),
    );
    for (const story of file.stories) {
      // compared with the separators removed on both sides rather than by
      // re-deriving storybook's kebab-casing, which has its own rules for an
      // uppercase run (`RefusedToAViewer` is `refused-to-a-viewer`, not
      // `refused-to-aviewer`) and would fail the guard on a story that is there
      const wanted = squash(story);
      const found = [...known].some((id) => squash(id.split("--").pop() ?? "") === wanted);
      if (!found) problems.push(`${file.path}: story '${story}' is not in the served index`);
    }
  }
  return problems;
}

function storyFiles(args: string[]): string[] {
  if (args.length > 0) return args.map((arg) => relative(UI_DIR, resolve(arg)));
  const found = spawnSync("rg", ["--files", "-g", "*.stories.tsx", "src"], {
    cwd: UI_DIR,
    encoding: "utf8",
  });
  return found.stdout.trim().split("\n").filter(Boolean);
}

async function main() {
  const argv = process.argv.slice(2);
  let requestedPort: number | undefined;
  const portAt = argv.indexOf("--port");
  if (portAt !== -1) {
    requestedPort = Number(argv[portAt + 1]);
    argv.splice(portAt, 2);
  }

  const files = storyFiles(argv).map((path) => ({
    path,
    stories: declaredStories(readFileSync(join(UI_DIR, path), "utf8")),
  }));
  if (files.length === 0) {
    console.error("no story files matched");
    process.exit(1);
  }

  let port: number;
  if (requestedPort !== undefined) {
    if (!(await portIsFree(requestedPort))) {
      // the whole point: a taken port is an error here, not a shrug
      console.error(
        `port ${requestedPort} is already in use. storybook would not fail on this — it would ` +
          `leave the other server listening and the run would pass against somebody else's ` +
          `build (#1684). free the port, or omit --port and let this script pick one.`,
      );
      process.exit(1);
    }
    port = requestedPort;
  } else {
    port = await findFreePort();
  }

  console.log(`[stories] starting storybook on ${port}`);
  const storybook = spawn(
    "bunx",
    ["storybook", "dev", "--ci", "--quiet", "-p", String(port), "--no-open"],
    { cwd: UI_DIR, stdio: ["ignore", "ignore", "inherit"] },
  );
  const stop = () => {
    if (!storybook.killed) storybook.kill("SIGTERM");
  };
  process.on("exit", stop);
  process.on("SIGINT", () => {
    stop();
    process.exit(130);
  });

  let failed = false;
  try {
    const index = await waitForIndex(port);
    const problems = missingFrom(index, files);
    if (problems.length > 0) {
      console.error(
        `[stories] the storybook on ${port} is not serving this worktree's build:\n  ` +
          problems.join("\n  "),
      );
      process.exit(1);
    }
    console.log(`[stories] index confirmed: ${files.length} file(s), this worktree's build`);

    for (const file of files) {
      // one file per invocation: the positional pattern is passed through
      // /bin/sh, so a combined regex with ( | ) dies as a shell syntax error
      const run = spawnSync("bunx", ["test-storybook", "--url", `http://127.0.0.1:${port}`, file.path], {
        cwd: UI_DIR,
        stdio: "inherit",
      });
      if (run.status !== 0) failed = true;
    }
  } finally {
    stop();
  }
  process.exit(failed ? 1 : 0);
}

if (import.meta.main) await main();
