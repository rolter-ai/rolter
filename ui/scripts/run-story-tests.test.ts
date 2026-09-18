import { createServer } from "node:net";

import { describe, it, expect } from "bun:test";

import {
  declaredStories,
  findFreePort,
  indexedPaths,
  missingFrom,
  portIsFree,
  type StorybookIndex,
} from "./run-story-tests";

/** An index as Storybook serves it, of the fields the guard reads. */
function index(entries: [string, string][]): StorybookIndex {
  return {
    entries: Object.fromEntries(
      entries.map(([id, importPath]) => [id, { id, importPath }]),
    ),
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
    expect(problems).toEqual(["src/pages/Keys.stories.tsx: story 'AddedSinceThatBuild' is not in the served index"]);
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
