import { createServer } from "node:net";

import { describe, it, expect } from "bun:test";

import {
  declaredStories,
  findFreePort,
  foreignServer,
  indexedPaths,
  isInsideDirectory,
  listenersOn,
  missingFrom,
  parseListeningPids,
  parseWorkingDirectory,
  portIsFree,
  type StorybookIndex,
} from "./run-story-tests";

/** An index as Storybook serves it, of the fields the guard reads. */
function index(entries: [string, string][]): StorybookIndex {
  return {
    entries: Object.fromEntries(entries.map(([id, importPath]) => [id, { id, importPath }])),
  };
}

describe("declaredStories", () => {
  it("finds the exports annotated as a story", () => {
    const source = [
      "export default meta;",
      "export const Empty: Story = { render: () => null };",
      "export const RefusedToAViewer: Story = {};",
    ].join("\n");
    expect(declaredStories(source)).toEqual(["Empty", "RefusedToAViewer"]);
  });

  it("ignores the fixtures and helpers a story file exports beside them", () => {
    // the realistic mistake: counting `ROWS` as a story, so the guard then
    // reports a missing story that was never a story and fails every run
    const source = [
      "export const ROWS: KeyRow[] = [];",
      "export const harness = () => null;",
      "export const Loading: Story = {};",
    ].join("\n");
    expect(declaredStories(source)).toEqual(["Loading"]);
  });
});

describe("indexedPaths", () => {
  it("strips the ./ storybook writes on an import path", () => {
    // the comparison is against paths relative to ui/, and a mismatch here
    // would make every file look absent from its own index
    const paths = indexedPaths(index([["screens-keys--empty", "./src/pages/Keys.stories.tsx"]]));
    expect(paths.has("src/pages/Keys.stories.tsx")).toBe(true);
  });
});

describe("missingFrom", () => {
  const served = index([
    ["screens-keys--empty", "./src/pages/Keys.stories.tsx"],
    ["screens-keys--refused-to-a-viewer", "./src/pages/Keys.stories.tsx"],
  ]);

  it("passes a file the index serves with all of its stories", () => {
    expect(
      missingFrom(served, [
        { path: "src/pages/Keys.stories.tsx", stories: ["Empty", "RefusedToAViewer"] },
      ]),
    ).toEqual([]);
  });

  it("catches a server that has never heard of the file — the #1684 false green", () => {
    // this is exactly the other-worktree case: a Storybook of the same project,
    // serving a build made before this story file existed
    const problems = missingFrom(served, [
      { path: "src/pages/AdaptiveDashboard.stories.tsx", stories: ["SwitchedOff"] },
    ]);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("not in the served index at all");
  });

  it("catches a stale build of a file it does know", () => {
    // the subtler half: the file is indexed, but the story added since that
    // build is not, so the run would report green without ever loading it
    const problems = missingFrom(served, [
      { path: "src/pages/Keys.stories.tsx", stories: ["Empty", "AddedSinceThatBuild"] },
    ]);
    expect(problems).toEqual([
      "src/pages/Keys.stories.tsx: story 'AddedSinceThatBuild' is not in the served index",
    ]);
  });
});

describe("port probing", () => {
  it("sees a listener that set SO_REUSEADDR, which a bind probe does not", async () => {
    // the real squatter: python's http.server and Bun.serve both set it, so a
    // second bind on the same port succeeds and a bind-only probe calls the
    // port free — then hands it to storybook, which is the #1684 failure
    const port = await findFreePort(6500);
    const squatter = Bun.serve({ port, fetch: () => new Response("busy") });
    try {
      expect(await portIsFree(port)).toBe(false);
    } finally {
      squatter.stop(true);
    }
  });

  it("reports a port nothing is listening on as free", async () => {
    const port = await findFreePort(6300);
    expect(await portIsFree(port)).toBe(true);
  });

  it("reports a port that is taken as taken, which is what storybook will not do", async () => {
    const port = await findFreePort(6400);
    const server = createServer();
    await new Promise<void>((done) => server.listen(port, "127.0.0.1", done));
    try {
      expect(await portIsFree(port)).toBe(false);
      // and the search steps past it rather than handing it out
      expect(await findFreePort(port)).toBeGreaterThan(port);
    } finally {
      await new Promise<void>((done) => server.close(() => done()));
    }
  });
});

describe("who is serving the port", () => {
  const ui = "/Users/dev/rolter/ui";

  it("reads the pids lsof lists, once each", () => {
    // one listener answering on both address families is printed twice, and
    // asking lsof about the same pid twice only makes the error message repeat
    expect(parseListeningPids("4821\n4821\n5190\n")).toEqual([4821, 5190]);
  });

  it("ignores what lsof prints when nothing is listening", () => {
    expect(parseListeningPids("")).toEqual([]);
  });

  it("reads the cwd out of lsof's field output, spaces included", () => {
    // the field format rather than the columns precisely because a worktree
    // path can contain a space, which the column output makes unparsable
    const output = ["p4821", "fcwd", "n/Users/dev/My Work/rolter/ui", ""].join("\n");
    expect(parseWorkingDirectory(output)).toBe("/Users/dev/My Work/rolter/ui");
  });

  it("says nothing when lsof named no directory", () => {
    expect(parseWorkingDirectory("p4821\nfcwd\n")).toBeNull();
  });

  it("does not read a sibling directory as being inside this one", () => {
    // the comparison is on a separator: `…/ui-old` shares the prefix but is a
    // different checkout, and letting it pass is the hole being closed
    expect(isInsideDirectory("/Users/dev/rolter/ui-old", ui)).toBe(false);
    expect(isInsideDirectory(ui, ui)).toBe(true);
    expect(isInsideDirectory("/Users/dev/rolter/ui/.storybook", ui)).toBe(true);
  });

  it("accepts the storybook this worktree started", () => {
    expect(foreignServer([{ pid: 4821, cwd: ui }], ui)).toBeNull();
  });

  it("catches another worktree of this same project — the #1693 false green", () => {
    // what actually happened on port 6032: a sibling worktree's storybook held
    // the port, and the index check passed because that build indexes the same
    // story ids under the same import paths
    const problem = foreignServer(
      [{ pid: 4821, cwd: "/Users/dev/rolter/.worktrees/feat-1654-docs-link/ui" }],
      ui,
    );
    expect(problem).toContain("not this worktree's");
    expect(problem).toContain("feat-1654-docs-link");
  });

  it("catches a listener whose working directory lsof would not give up", () => {
    // another user's squatter reads as unknown, and unknown is not this
    // worktree — the guard refuses rather than assuming the friendly case
    expect(foreignServer([{ pid: 4821, cwd: null }], ui)).toContain("unreadable");
  });

  it("refuses a port with no listener at all", () => {
    expect(foreignServer([], ui)).toContain("nothing is listening");
  });

  it("finds the real process behind a real listening socket", async () => {
    // the parsing above is only worth anything if lsof is actually being asked
    // the right question: this process is listening, so it must come back with
    // this process's pid and cwd
    const port = await findFreePort(6600);
    const squatter = Bun.serve({ port, fetch: () => new Response("busy") });
    try {
      const listeners = listenersOn(port);
      expect(listeners).not.toBeNull();
      expect(listeners!.map((listener) => listener.pid)).toContain(process.pid);
      expect(foreignServer(listeners!, process.cwd())).toBeNull();
    } finally {
      squatter.stop(true);
    }
  });
});
