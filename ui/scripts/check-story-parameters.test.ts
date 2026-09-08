import { describe, it, expect } from "bun:test";

import {
  checkAllStories,
  checkStorySource,
  collectFixtures,
  describeViolation,
  maskLiterals,
  readFixtures,
  type Fixture,
} from "./check-story-parameters";

const FRAGMENT: Fixture = {
  name: "withPageA11y",
  kind: "parameters-fragment",
  module: "src/lib/story-a11y.ts",
};
const STORY_FIELDS: Fixture = {
  name: "atMobile",
  kind: "story-fields",
  module: "src/lib/story-viewport.ts",
};

describe("fixture classification", () => {
  it("reads a parameters fragment and a story-fields fixture apart", () => {
    const fixtures = collectFixtures(
      `
export const withPageA11y = {
  a11y: { rules: { region: { enabled: true } } },
};

export const atMobile = {
  parameters: { viewportSize: MOBILE },
  globals: { viewport: { value: "rolterMobile" } },
};

export function expectNoHorizontalOverflow() {}
`,
      "src/lib/story-fixtures.ts",
    );
    expect(fixtures).toEqual([
      { name: "withPageA11y", kind: "parameters-fragment", module: "src/lib/story-fixtures.ts" },
      { name: "atMobile", kind: "story-fields", module: "src/lib/story-fixtures.ts" },
    ]);
  });

  it("is not fooled by a nested `parameters` key", () => {
    // `parameters` one level down is not the fixture's own: this is a
    // fragment, and demanding it be spread at story level would put it exactly
    // where #1373 says it must never go
    const [fixture] = collectFixtures(
      `export const withDocs = { docs: { parameters: { source: {} } } };`,
      "src/lib/story-docs.ts",
    );
    expect(fixture?.kind).toBe("parameters-fragment");
  });
});

describe("misplaced spreads", () => {
  it("fails a parameters fragment spread at meta level", () => {
    const source = `const meta: Meta<typeof Login> = {
  title: "Screens/Login",
  component: Login,
  ...withPageA11y,
};`;
    const [violation] = checkStorySource(source, "src/pages/Login.stories.tsx", [FRAGMENT]);
    expect(violation).toMatchObject({ file: "src/pages/Login.stories.tsx", line: 4 });
    expect(describeViolation(violation!)).toContain("must be spread inside `parameters: { ... }`");
  });

  it("fails a parameters fragment spread at story level", () => {
    // the shape the trap actually took: the story runs, `parameters.a11y` is
    // undefined and the override asserts nothing
    const source = `export const Desktop: Story = {\n  ...withPageA11y,\n  play: async () => {},\n};`;
    expect(checkStorySource(source, "src/App.stories.tsx", [FRAGMENT])).toHaveLength(1);
  });

  it("passes a parameters fragment spread inside parameters", () => {
    const source = `const meta = {
  title: "Shell/App",
  parameters: { layout: "fullscreen", ...withPageA11y },
};`;
    expect(checkStorySource(source, "src/App.stories.tsx", [FRAGMENT])).toEqual([]);
  });

  it("fails a story-fields fixture buried inside parameters", () => {
    const source = `export const Mobile: Story = {\n  parameters: { ...atMobile },\n};`;
    const [violation] = checkStorySource(source, "src/pages/Keys.stories.tsx", [STORY_FIELDS]);
    expect(describeViolation(violation!)).toContain("`parameters.parameters`");
  });

  it("passes a story-fields fixture spread at story level", () => {
    const source = `export const Mobile: Story = {\n  ...atMobile,\n  render: () => null,\n};`;
    expect(checkStorySource(source, "src/pages/Keys.stories.tsx", [STORY_FIELDS])).toEqual([]);
  });

  it("ignores a spread named in a comment or a string", () => {
    const source = `// spread it as ...withPageA11y and the story asserts nothing
const note = "...withPageA11y";
const meta = { parameters: { ...withPageA11y } };`;
    expect(checkStorySource(source, "src/App.stories.tsx", [FRAGMENT])).toEqual([]);
  });
});

describe("maskLiterals", () => {
  it("keeps offsets and blanks braces inside strings, comments and templates", () => {
    const source = 'const a = "{{"; /* { */ const b = `x${ { c: 1 } }`; // {\n';
    const masked = maskLiterals(source);
    expect(masked).toHaveLength(source.length);
    // the only surviving braces are the object literal inside `${ … }`
    expect(masked.split("{")).toHaveLength(3);
    expect(masked.split("}")).toHaveLength(3);
  });
});

describe("the dashboard's own stories", () => {
  it("exports the fixtures the guard knows about", () => {
    const names = readFixtures().map((f) => f.name);
    expect(names).toContain("withPageA11y");
    expect(names).toContain("atMobile");
    expect(names).toContain("atTablet");
  });

  it("spreads every fixture into the object it belongs to", () => {
    expect(checkAllStories().map(describeViolation)).toEqual([]);
  });
});
