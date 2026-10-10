import { describe, it, expect } from "bun:test";

import {
  declaredStories,
  missingFrom,
  narrowsTheRun,
  splitArgs,
  type VitestReport,
} from "./run-story-tests";

const ROOT = "/work/rolter/ui";

/** A Vitest JSON report, of the fields the guard reads: file → statuses of its tests. */
function report(files: Record<string, string[]>): VitestReport {
  return {
    testResults: Object.entries(files).map(([path, statuses]) => ({
      name: `${ROOT}/${path}`,
      assertionResults: statuses.map((status) => ({ status })),
    })),
  };
}

const file = (path: string, ...stories: string[]) => ({ path, stories });

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

describe("missingFrom", () => {
  it("passes a file that ran every story it declares", () => {
    const ran = report({ "src/pages/Keys.stories.tsx": ["passed", "passed"] });
    expect(missingFrom(ran, [file("src/pages/Keys.stories.tsx", "A", "B")], ROOT)).toEqual([]);
  });

  it("counts a failed story as run: the run reports it, the guard is about absence", () => {
    const ran = report({ "src/pages/Keys.stories.tsx": ["passed", "failed"] });
    expect(missingFrom(ran, [file("src/pages/Keys.stories.tsx", "A", "B")], ROOT)).toEqual([]);
  });

  it("catches a file vitest never picked up — the green that ran nothing", () => {
    const ran = report({ "src/pages/Keys.stories.tsx": ["passed"] });
    const problems = missingFrom(
      ran,
      [file("src/pages/Keys.stories.tsx", "A"), file("src/pages/Users.stories.tsx", "A")],
      ROOT,
    );
    expect(problems).toEqual(["src/pages/Users.stories.tsx did not run at all"]);
  });

  it("catches a file that ran fewer tests than it has stories", () => {
    // a `!test` tag, or an export the transform skipped
    const ran = report({ "src/pages/Keys.stories.tsx": ["passed"] });
    const problems = missingFrom(ran, [file("src/pages/Keys.stories.tsx", "A", "B", "C")], ROOT);
    expect(problems).toEqual(["src/pages/Keys.stories.tsx declares 3 stories but ran 1"]);
  });

  it("does not count a skipped story as a story that ran", () => {
    const ran = report({ "src/pages/Keys.stories.tsx": ["passed", "skipped"] });
    const problems = missingFrom(ran, [file("src/pages/Keys.stories.tsx", "A", "B")], ROOT);
    expect(problems).toEqual(["src/pages/Keys.stories.tsx declares 2 stories but ran 1"]);
  });

  it("is not fooled by a report with no files at all", () => {
    expect(missingFrom({ testResults: [] }, [file("src/a.stories.tsx", "A")], ROOT)).toEqual([
      "src/a.stories.tsx did not run at all",
    ]);
  });

  it("does not mind a file that holds more tests than the regex found", () => {
    // `export const X = {…} satisfies Story` is a story the regex does not see
    const ran = report({ "src/a.stories.tsx": ["passed", "passed", "passed"] });
    expect(missingFrom(ran, [file("src/a.stories.tsx", "A")], ROOT)).toEqual([]);
  });
});

describe("narrowsTheRun", () => {
  it("treats a name filter, a bail, a shard and a changed-only run as partial on purpose", () => {
    expect(narrowsTheRun(["-t", "Loaded"])).toBe(true);
    expect(narrowsTheRun(["--testNamePattern=Loaded"])).toBe(true);
    expect(narrowsTheRun(["--bail=1"])).toBe(true);
    expect(narrowsTheRun(["--shard=1/3"])).toBe(true);
    expect(narrowsTheRun(["--changed"])).toBe(true);
  });

  it("leaves a run that only tunes vitest to the count check", () => {
    expect(narrowsTheRun([])).toBe(false);
    expect(narrowsTheRun(["--maxWorkers=2", "--reporter=verbose"])).toBe(false);
  });
});

describe("splitArgs", () => {
  it("takes what precedes `--` as story files and what follows as vitest arguments", () => {
    expect(splitArgs(["a.stories.tsx", "b.stories.tsx", "--", "--maxWorkers=2"])).toEqual({
      files: ["a.stories.tsx", "b.stories.tsx"],
      vitestArgs: ["--maxWorkers=2"],
    });
  });

  it("takes everything as story files without a `--`", () => {
    expect(splitArgs(["a.stories.tsx"])).toEqual({ files: ["a.stories.tsx"], vitestArgs: [] });
    expect(splitArgs([])).toEqual({ files: [], vitestArgs: [] });
  });
});
