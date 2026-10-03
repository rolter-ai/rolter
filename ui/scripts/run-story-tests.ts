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
//   1. it picks a free port itself and locks it against concurrent runs (#2323), or fails loudly when the one you asked for
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
//   5. only then does it run `test-storybook`, one file per invocation — the
//      positional pattern goes through `/bin/sh`, so a pattern containing
//      `(`, `|` or `)` dies with a shell syntax error
//
// Usage:
//   bun scripts/run-story-tests.ts src/pages/Keys.stories.tsx [more…]
//   bun scripts/run-story-tests.ts --port 6040 src/pages/Keys.stories.tsx
//   bun scripts/run-story-tests.ts            # every story file
import { spawn, spawnSync } from "node:child_process";
import { createServer, Socket } from "node:net";
import { closeSync, mkdirSync, openSync, readFileSync, unlinkSync, writeSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative, resolve, sep } from "node:path";

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

/** The first free port at or after `from`, unclaimed: see `claimFreePort` for the locked one. */
export async function findFreePort(from = 6100, tries = 60): Promise<number> {
  for (let port = from; port < from + tries; port += 1) {
    if (await portIsFree(port)) return port;
  }
  throw new Error(`no free port in ${from}..${from + tries}`);
}

/** A port this run holds: the lock file stays until `release` is called. */
export interface PortClaim {
  port: number;
  release: () => void;
}

/** Where the per-port lock files live: shared by every worktree on the machine. */
export function defaultLockDir(): string {
  return join(tmpdir(), "rolter-story-ports");
}

function processIsAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    // EPERM means it exists but belongs to someone else
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
}

/**
 * Claim `port` with an exclusive lock file (`open(…, "wx")`, so O_EXCL) that
 * names this process, or null when another live run holds it (#2323).
 *
 * Probing for a free port and releasing the probe leaves a window of seconds
 * before Storybook binds, in which a second run picks the same port. The lock
 * closes it: the kernel lets exactly one `wx` open succeed. A lock whose pid is
 * dead (a run killed with SIGKILL) is stale and is taken over.
 */
export function claimPort(port: number, lockDir = defaultLockDir()): PortClaim | null {
  mkdirSync(lockDir, { recursive: true });
  const file = join(lockDir, `${port}.lock`);
  for (let attempt = 0; attempt < 3; attempt += 1) {
    try {
      const fd = openSync(file, "wx");
      writeSync(fd, String(process.pid));
      closeSync(fd);
      let released = false;
      return {
        port,
        release: () => {
          if (released) return;
          released = true;
          try {
            // only remove a lock that is still ours
            if (readFileSync(file, "utf8").trim() === String(process.pid)) unlinkSync(file);
          } catch {
            // already gone
          }
        },
      };
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
    }
    let holder: number;
    try {
      holder = Number(readFileSync(file, "utf8").trim());
    } catch {
      continue; // released between the open and the read: try again
    }
    // an empty file is a claim caught between open and write; give it a beat
    if (Number.isFinite(holder) && holder > 0 && processIsAlive(holder)) return null;
    if (Number.isNaN(holder) || holder === 0) {
      Bun.sleepSync(20);
      continue;
    }
    try {
      unlinkSync(file);
    } catch {
      // someone else cleared it first
    }
  }
  return null;
}

/**
 * The first port at or after `from` that is both unlocked by another run and
 * free on the machine, claimed for the caller. The lock is what keeps two
 * concurrent runs apart; the free check still skips ports other software holds.
 */
export async function claimFreePort(
  from = 6100,
  tries = 60,
  lockDir = defaultLockDir(),
  isFree: (port: number) => Promise<boolean> = portIsFree,
): Promise<PortClaim> {
  for (let port = from; port < from + tries; port += 1) {
    const claim = claimPort(port, lockDir);
    if (claim === null) continue;
    if (await isFree(port)) return claim;
    claim.release();
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

  let claim: PortClaim;
  if (requestedPort !== undefined) {
    const held = claimPort(requestedPort);
    if (held === null) {
      console.error(
        `port ${requestedPort} is in use by another run of this script (#2323). pick another ` +
          `port, or omit --port and let this script pick one.`,
      );
      process.exit(1);
    }
    if (!(await portIsFree(requestedPort))) {
      held.release();
      // the whole point: a taken port is an error here, not a shrug
      console.error(
        `port ${requestedPort} is already in use. storybook would not fail on this — it would ` +
          `leave the other server listening and the run would pass against somebody else's ` +
          `build (#1684). free the port, or omit --port and let this script pick one.`,
      );
      process.exit(1);
    }
    claim = held;
  } else {
    claim = await claimFreePort();
  }
  const port = claim.port;
  // held for the run's lifetime and dropped on every way out, signals included
  process.on("exit", claim.release);
  for (const [signal, code] of [
    ["SIGINT", 130],
    ["SIGTERM", 143],
    ["SIGHUP", 129],
  ] as const) {
    process.on(signal, () => process.exit(code));
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

  let failed = false;
  try {
    const index = await waitForIndex(port);
    const problems = missingFrom(index, files);
    if (problems.length > 0) {
      console.error(
        `[stories] the storybook on ${port} is not serving this worktree's build:\n  ` +
          problems.join("\n  ") +
          `\n  (this run holds the port's lock, so the listener is not another test:stories run)`,
      );
      process.exit(1);
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
        process.exit(1);
      }
    }
    console.log(`[stories] index confirmed: ${files.length} file(s), this worktree's build`);

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
    stop();
  }
  process.exit(failed ? 1 : 0);
}

if (import.meta.main) await main();
