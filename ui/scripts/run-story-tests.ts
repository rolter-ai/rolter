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
//      import path. That catches a server that is not this project, and a stale
//      build missing a story added since
//   4. it identifies the process holding the port, because the index check
//      above compares content only and another worktree of this same project
//      serves the same story ids under the same paths (#1693). The listening
//      pid's working directory has to be inside this worktree's `ui/`
//   5. it opens one story of each file in a headless browser and waits for it
//      to render, so the dev server compiles the preview and that file's module
//      graph before any test is timed (#2637)
//   6. only then does it run `test-storybook`, one file per invocation — the
//      positional pattern goes through `/bin/sh`, so a pattern containing
//      `(`, `|` or `)` dies with a shell syntax error
//
// Usage:
//   bun scripts/run-story-tests.ts src/pages/Keys.stories.tsx [more…]
//   bun scripts/run-story-tests.ts --port 6040 src/pages/Keys.stories.tsx
//   bun scripts/run-story-tests.ts            # every story file
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { createServer, Socket } from "node:net";
import { readFileSync } from "node:fs";
import { join, relative, resolve, sep } from "node:path";

const UI_DIR = join(import.meta.dir, "..");

/** A Storybook index entry, of the fields this script reads. */
export interface IndexEntry {
  id: string;
  importPath: string;
  /** `story` or `docs`; storybook writes it on every entry */
  type?: string;
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

/**
 * The story to open for each file before the timed run, one per file that has
 * any: the first the index lists for it. Any story of a file pulls in that
 * file's whole module graph; a docs entry renders no story, so it is never
 * picked.
 */
export function warmupStories(index: StorybookIndex, files: { path: string }[]): string[] {
  const entries = Object.values(index.entries ?? {});
  return files.flatMap((file) => {
    const first = entries.find(
      (entry) =>
        entry.importPath.replace(/^\.\//, "") === file.path && (entry.type ?? "story") === "story",
    );
    return first ? [first.id] : [];
  });
}

// as long as `waitForIndex` gives storybook to come up: a cold compile of the
// preview is the same order of work
const WARMUP_TIMEOUT_MS = 180_000;

/**
 * Render one story of each file once, untimed, before `test-storybook` starts
 * the clock (#2637).
 *
 * `storybook dev` compiles on demand: nothing is transformed until a browser
 * asks for it. So the first story of a run paid for the whole preview — every
 * vendored font, the i18n catalogs, react, the screen under test and all it
 * imports — inside its own 15 second test budget. `Screens/Users › Loaded`
 * spends ~30s of a cold start there and timed out with nothing wrong with it.
 * jest abandons a timed-out test but not its page: the `postVisit` axe run goes
 * on in the same tab, so the next story's own axe run then fails with "Axe is
 * already running". That is how `ShowsDisplayNamesBesideTheEmail`, the story
 * straight after it, failed with nothing wrong with it either. CI
 * never sees this, because it tests a static build. A failure here is only
 * reported: the timed run that follows says what is wrong with a story more
 * precisely than a warm-up can.
 */
async function warmUp(port: number, storyIds: string[]): Promise<void> {
  if (storyIds.length === 0) return;
  // the playwright the test-runner drives, so no second browser install is needed
  const { chromium } = await import("playwright");
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    for (const id of storyIds) {
      const started = Date.now();
      try {
        await page.goto(
          `http://127.0.0.1:${port}/iframe.html?id=${encodeURIComponent(id)}&viewMode=story`,
          { waitUntil: "load", timeout: WARMUP_TIMEOUT_MS },
        );
        // storybook marks the body once the story's file has been imported and
        // it starts to render, or once it has given up on it
        await page.waitForFunction(
          () =>
            document.body.classList.contains("sb-show-main") ||
            document.body.classList.contains("sb-show-errordisplay"),
          undefined,
          { timeout: WARMUP_TIMEOUT_MS },
        );
        // and the chunks the render itself pulls in, a lazily loaded locale say
        await page.waitForLoadState("networkidle", { timeout: WARMUP_TIMEOUT_MS });
        console.log(`[stories] warmed ${id} in ${((Date.now() - started) / 1000).toFixed(1)}s`);
      } catch (error) {
        console.warn(`[stories] could not warm ${id}, running it cold: ${String(error)}`);
      }
    }
  } finally {
    await browser.close();
  }
}

/**
 * The pids a `lsof -ti :<port>` listing names.
 *
 * One listener answers on both address families, so the same pid comes back
 * twice; the guard asks about each process once.
 */
export function parseListeningPids(output: string): number[] {
  const pids = output
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => /^\d+$/.test(line))
    .map(Number);
  return [...new Set(pids)];
}

/**
 * The working directory a `lsof -a -p <pid> -d cwd -Fn` listing reports, or
 * null when it reports none — a process owned by another user, or one that has
 * exited between the two calls.
 *
 * The field format is one letter per line (`p<pid>`, `fcwd`, `n<path>`), which
 * is parsed rather than the column output because a path with a space in it
 * survives it.
 */
export function parseWorkingDirectory(output: string): string | null {
  for (const line of output.split("\n")) {
    if (line.startsWith("n")) return line.slice(1).trim() || null;
  }
  return null;
}

/**
 * Whether `path` is `root` itself or lives inside it, compared on a path
 * separator so `…/ui-old` is not read as living inside `…/ui`.
 */
export function isInsideDirectory(path: string, root: string): boolean {
  if (path === root) return true;
  return path.startsWith(root.endsWith(sep) ? root : root + sep);
}

/** A listening process, as the guard sees it. */
export interface Listener {
  pid: number;
  /** its working directory, or null when lsof would not say */
  cwd: string | null;
}

/**
 * What is wrong with the server holding the port, or null when it is this
 * worktree's.
 *
 * The index check upstream of this one compares *content*, so it only catches
 * a server that is not this project or is missing a story. It passes happily
 * for another worktree of this same repository, whose build indexes the same
 * story ids under the same import paths — which is the case that actually
 * happens here (#1693, hit for real on port 6032). Identity is the process, not
 * the payload: `storybook dev` is spawned with its cwd in this worktree's `ui/`,
 * so a listener sitting anywhere else is somebody else's.
 */
export function foreignServer(listeners: Listener[], uiDir: string): string | null {
  if (listeners.length === 0) {
    return "nothing is listening on the port storybook was told to serve";
  }
  const strangers = listeners.filter(
    (listener) => listener.cwd === null || !isInsideDirectory(listener.cwd, uiDir),
  );
  if (strangers.length === 0) return null;
  const named = strangers
    .map((listener) => `pid ${listener.pid} (${listener.cwd ?? "working directory unreadable"})`)
    .join(", ");
  return `the server on the port is not this worktree's: ${named}, expected one under ${uiDir}`;
}

/**
 * The processes listening on `port`, or null when lsof cannot be run at all —
 * the identity check is a sharpening of the index check, not a reason to refuse
 * to run the tests on a box without lsof.
 */
export function listenersOn(port: number): Listener[] | null {
  const found = spawnSync("lsof", ["-ti", `:${port}`, "-sTCP:LISTEN"], { encoding: "utf8" });
  // status 1 is lsof's "no match", which is a real answer; only a missing
  // binary is not one
  if (found.error) return null;
  return parseListeningPids(found.stdout ?? "").map((pid) => {
    const where = spawnSync("lsof", ["-a", "-p", String(pid), "-d", "cwd", "-Fn"], {
      encoding: "utf8",
    });
    return { pid, cwd: where.error ? null : parseWorkingDirectory(where.stdout ?? "") };
  });
}

/**
 * Stop the storybook child and resolve only once it is gone, so the port it
 * held is free for the next run (#2661). `process.exit` straight after a
 * SIGTERM left storybook still listening, and a back-to-back run on the same
 * port was refused as "already in use".
 *
 * The child leads its own process group (`detached`), because `bunx` forks the
 * real server and a signal to `bunx` alone would not reach it. SIGTERM goes to
 * the group; after `graceMs` without an exit it escalates to SIGKILL, and after
 * `killMs` more it gives up rather than hang the run.
 */
export async function stopChild(
  child: Pick<ChildProcess, "pid" | "exitCode" | "signalCode" | "once">,
  signal: (sig: NodeJS.Signals) => void,
  graceMs = 5000,
  killMs = 2000,
): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise<boolean>((resolveP) => child.once("exit", () => resolveP(true)));
  const within = (ms: number) =>
    Promise.race([exited, new Promise<boolean>((r) => setTimeout(() => r(false), ms))]);
  signal("SIGTERM");
  if (await within(graceMs)) return;
  signal("SIGKILL");
  await within(killMs);
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
    { cwd: UI_DIR, stdio: ["ignore", "ignore", "inherit"], detached: true },
  );
  const signalGroup = (sig: NodeJS.Signals) => {
    try {
      // a negative pid signals the whole group, bunx's forked server included
      if (storybook.pid !== undefined) process.kill(-storybook.pid, sig);
    } catch {
      // already gone
    }
  };
  const stop = () => stopChild(storybook, signalGroup);
  // last resort for an exit path that did not await stop(): a sync kill
  process.on("exit", () => signalGroup("SIGKILL"));
  const exitAfterStop = async (code: number): Promise<never> => {
    await stop();
    process.exit(code);
  };
  process.on("SIGINT", () => void exitAfterStop(130));
  process.on("SIGTERM", () => void exitAfterStop(143));

  let failed = false;
  try {
    const index = await waitForIndex(port);
    const problems = missingFrom(index, files);
    if (problems.length > 0) {
      console.error(
        `[stories] the storybook on ${port} is not serving this worktree's build:\n  ` +
          problems.join("\n  "),
      );
      await exitAfterStop(1);
    }
    // and who is serving it: the index above compares content, which another
    // worktree of this same project satisfies (#1693)
    const listeners = listenersOn(port);
    if (listeners === null) {
      console.warn(
        "[stories] lsof is not available, so the server on the port was not identified — " +
          "only its index was checked",
      );
    } else {
      const stranger = foreignServer(listeners, UI_DIR);
      if (stranger !== null) {
        console.error(`[stories] ${stranger}`);
        await exitAfterStop(1);
      }
    }
    console.log(`[stories] index confirmed: ${files.length} file(s), this worktree's build`);
    await warmUp(port, warmupStories(index, files));

    for (const file of files) {
      // one file per invocation: the positional pattern is passed through
      // /bin/sh, so a combined regex with ( | ) dies as a shell syntax error
      const run = spawnSync(
        "bunx",
        ["test-storybook", "--url", `http://127.0.0.1:${port}`, file.path],
        {
          cwd: UI_DIR,
          stdio: "inherit",
        },
      );
      if (run.status !== 0) failed = true;
    }
  } finally {
    await stop();
  }
  process.exit(failed ? 1 : 0);
}

if (import.meta.main) await main();
