import { describe, it, expect } from "bun:test";

import {
  checkAll,
  checkSource,
  collectShapes,
  describeViolation,
  duplicatedShapes,
  importedPrimitives,
  readPrimitiveNames,
  staleShapes,
  stripComments,
  unexplainedShapes,
  waiverAbove,
  type Shape,
} from "./check-ui-primitives";
import { REPEATED_SHAPES } from "./repeated-shapes-allowlist";

const PRIMITIVES = ["Card", "Combobox", "Dialog", "Field", "SwitchRow"];
const SCREEN = "src/pages/Example.tsx";

describe("the element rules", () => {
  it("fails a bare select and names Combobox", () => {
    const [violation] = checkSource(`const f = () => <select value={v} />;`, SCREEN).violations;
    expect(violation).toMatchObject({ rule: "select", line: 1 });
    expect(describeViolation(violation!)).toContain("Combobox");
  });

  it("fails a raw pre and names CodeBlock", () => {
    const source = `export function Panel() {\n  return <pre>{json}</pre>;\n}`;
    const [violation] = checkSource(source, SCREEN).violations;
    expect(violation).toMatchObject({ rule: "pre", line: 2 });
    expect(describeViolation(violation!)).toContain("CodeBlock");
  });

  it("fails window.confirm, alert and prompt alike", () => {
    const source = `window.confirm("x");\nwindow.alert("y");\nwindow.prompt("z");`;
    const rules = checkSource(source, SCREEN).violations.map((v) => v.found);
    expect(rules).toEqual(["window.confirm()", "window.alert()", "window.prompt()"]);
  });

  it("is not fooled by a tag that merely starts with the banned name", () => {
    // `<presence>` and `<selectable>` are not `<pre>` and `<select>`; a guard
    // that cannot tell them apart gets waived everywhere within a week
    const source = `<presence x={1} />\n<selectable y={2} />`;
    expect(checkSource(source, SCREEN).violations).toEqual([]);
  });
});

describe("comments and strings", () => {
  it("ignores the rule named in a comment", () => {
    // every `window.confirm` in the dashboard today is a comment saying what a
    // screen replaced, so this is the case that decides whether the guard is
    // usable at all
    const source =
      `// was a bare window.confirm, which carried the copy but none of the styling\n` +
      `/* and the old markup was a <select> with a <pre> beside it */\n` +
      `const ok = 1;`;
    expect(checkSource(source, SCREEN).violations).toEqual([]);
  });

  it("still reports a violation on the line after a comment", () => {
    const source = `// a note about nothing in particular\n<select />`;
    expect(checkSource(source, SCREEN).violations).toMatchObject([{ rule: "select", line: 2 }]);
  });

  it("keeps line numbers honest across a block comment", () => {
    const source = `/* one\n   two\n   three */\n<pre />`;
    expect(checkSource(source, SCREEN).violations[0]?.line).toBe(4);
  });

  it("does not treat a JSX apostrophe as a string", () => {
    // `maskLiterals` does, which is why this check strips comments only: the
    // apostrophe opened a "string" that swallowed the declaration below it
    const source = `<p>don't</p>\nfunction Field() {}`;
    const { violations } = checkSource(source, SCREEN, PRIMITIVES);
    expect(violations).toMatchObject([{ rule: "shadowed-primitive", found: "Field" }]);
  });
});

describe("stripComments", () => {
  it("blanks comments and keeps every offset", () => {
    const source = `const a = 1; // note\n/* two */ const b = 2;\n`;
    const stripped = stripComments(source);
    expect(stripped).toHaveLength(source.length);
    expect(stripped.split("\n")[0]).toBe("const a = 1;        ");
    expect(stripped).toContain("const b = 2;");
  });
});

describe("the shadowed-primitive rule", () => {
  it("fails a component re-declared under a shared name", () => {
    const [violation] = checkSource(`function Field({ label }) {}`, SCREEN, PRIMITIVES).violations;
    expect(violation).toMatchObject({ rule: "shadowed-primitive", found: "Field" });
    expect(describeViolation(violation!)).toContain("#1044");
  });

  it("fails an exported one and an arrow constant too", () => {
    const source = `export function SwitchRow() {}\nconst Combobox = () => null;`;
    expect(checkSource(source, SCREEN, PRIMITIVES).violations.map((v) => v.found)).toEqual([
      "SwitchRow",
      "Combobox",
    ]);
  });

  it("passes a local wrapper that composes the shared primitive", () => {
    // the shape `McpManagement.tsx` uses: an adapter around the real `Dialog`
    // is reaching for the primitive, not duplicating it, and failing it would
    // push screens away from the shared component rather than towards it
    const source =
      `import { Dialog as BaseDialog } from "@/components/ui/dialog";\n` +
      `function Dialog({ onClose }) { return <BaseDialog open />; }`;
    expect(checkSource(source, SCREEN, PRIMITIVES).violations).toEqual([]);
  });

  it("passes a name no shared module exports", () => {
    expect(checkSource(`function PolicyRow() {}`, SCREEN, PRIMITIVES).violations).toEqual([]);
  });

  it("ignores a nested declaration", () => {
    // only a top-level component shadows the import; a helper inside a render
    // is scoped and cannot be mistaken for the primitive
    expect(checkSource(`  function Field() {}`, SCREEN, PRIMITIVES).violations).toEqual([]);
  });
});

describe("importedPrimitives", () => {
  it("reads the local name of an aliased import", () => {
    const names = importedPrimitives(
      `import { Dialog as BaseDialog, DialogTitle } from "@/components/ui/dialog";`,
    );
    expect([...names].sort()).toEqual(["Dialog", "DialogTitle"]);
  });

  it("ignores an import from anywhere else", () => {
    expect(importedPrimitives(`import { Field } from "@/components/Field";`).size).toBe(0);
  });
});

describe("exemptions", () => {
  it("skips a primitive's own implementation", () => {
    // `CodeBlock` *is* the `<pre>`; banning the element inside its own wrapper
    // is incoherent
    const source = `export function CodeBlock() { return <pre />; }`;
    expect(checkSource(source, "src/components/ui/code-block.tsx", PRIMITIVES).violations).toEqual(
      [],
    );
  });

  it("skips stories and tests", () => {
    for (const file of ["src/pages/Example.stories.tsx", "src/lib/thing.test.ts"]) {
      expect(checkSource(`<pre />\nwindow.confirm("x");`, file, PRIMITIVES).violations).toEqual([]);
    }
  });

  it("honours a waiver with a reason and records it", () => {
    const source = `// ui-primitives-allow: prose with its newlines kept, not a payload\n<pre />`;
    const { violations, waivers } = checkSource(source, SCREEN);
    expect(violations).toEqual([]);
    expect(waivers).toMatchObject([
      { rule: "pre", line: 2, reason: "prose with its newlines kept, not a payload" },
    ]);
  });

  it("marks a waiver with no reason as unknown so the run fails", () => {
    const { waivers } = checkSource(`// ui-primitives-allow:\n<pre />`, SCREEN);
    expect(waivers).toMatchObject([{ rule: "unknown", reason: "" }]);
  });

  it("finds the marker anywhere in the comment block above", () => {
    // a reason worth writing rarely fits on one line
    const lines = [
      "/* ui-primitives-allow: the raw-output toggle, prose rather than a payload",
      " * so the copy button and the highlighting would both answer nothing */",
      "<pre />",
    ];
    expect(waiverAbove(lines, 2)).toContain("the raw-output toggle");
  });

  it("does not reach past a line of code", () => {
    // a waiver two statements up is not a waiver for this one
    const lines = ["// ui-primitives-allow: something else entirely", "const x = 1;", "<pre />"];
    expect(waiverAbove(lines, 2)).toBeNull();
  });
});

describe("the dashboard's own tree", () => {
  it("reads the shared components out of src/components/ui", () => {
    const names = readPrimitiveNames();
    expect(names).toContain("Combobox");
    expect(names).toContain("CodeBlock");
    expect(names).toContain("SwitchRow");
    // an all-caps constant is data, not a component
    expect(names).not.toContain("NAV");
  });

  it("hand-rolls no shared component", () => {
    expect(checkAll().violations.map(describeViolation)).toEqual([]);
  });

  it("records no shape that stopped being duplicated", () => {
    // a stale entry is a rule that has quietly stopped applying: the primitive
    // was extracted, and the same copy-paste could come back unnoticed
    expect(checkAll().stale).toEqual([]);
  });

  it("states a reason for every recorded shape", () => {
    expect(unexplainedShapes(REPEATED_SHAPES)).toEqual([]);
  });

  it("states a reason for every waiver it carries", () => {
    for (const waiver of checkAll().waivers) {
      expect(waiver.reason.length).toBeGreaterThan(0);
      expect(waiver.rule).not.toBe("unknown");
    }
  });
});

describe("the duplicated-shape rule", () => {
  // the #1658 line, byte for byte: four tokens, two of them arbitrary
  const DANGER = `<p className="px-[22px] pt-2.5 text-xs text-[color:var(--status-danger-text)]">x</p>`;
  const shapesIn = (source: string, file: string) => collectShapes(source, file).shapes;
  const across = (source: string, files: string[]): Shape[] =>
    files.flatMap((file) => shapesIn(source, file));

  it("collects a distinctive intrinsic element", () => {
    expect(shapesIn(DANGER, SCREEN)).toMatchObject([
      {
        tag: "p",
        line: 1,
        key: "p|pt-2.5 px-[22px] text-[color:var(--status-danger-text)] text-xs",
      },
    ]);
  });

  it("sorts the classes so attribute order cannot hide a copy", () => {
    const reordered = `<p className="text-xs text-[color:var(--status-danger-text)] px-[22px] pt-2.5">x</p>`;
    expect(shapesIn(reordered, SCREEN)[0]!.key).toBe(shapesIn(DANGER, SCREEN)[0]!.key);
  });

  it("fails the same shape in three files and names every site", () => {
    const [violation] = duplicatedShapes(
      across(DANGER, ["src/pages/A.tsx", "src/pages/B.tsx", "src/pages/C.tsx"]),
    );
    expect(violation).toMatchObject({ rule: "duplicated-shape", file: "src/pages/A.tsx" });
    const message = describeViolation(violation!);
    expect(message).toContain("in 3 files");
    for (const file of ["src/pages/A.tsx", "src/pages/B.tsx", "src/pages/C.tsx"]) {
      expect(message).toContain(`${file}:1`);
    }
  });

  it("passes the same shape in two files", () => {
    // two is a coincidence; three is a primitive nobody extracted
    expect(duplicatedShapes(across(DANGER, ["src/pages/A.tsx", "src/pages/B.tsx"]))).toEqual([]);
  });

  it("passes many copies inside one file", () => {
    // a screen that renders its own row shape in each of three states is one
    // component, not three; it is the fan-out across files that hides drift
    const source = `${DANGER}\n${DANGER}\n${DANGER}`;
    expect(duplicatedShapes(shapesIn(source, SCREEN))).toEqual([]);
  });

  it("ignores a generic utility stack", () => {
    // `flex items-center gap-2` is a sentence in Tailwind, not a component, and
    // a guard that flagged its eighteen files would be switched off by Friday
    const source = `<div className="flex items-center gap-2 flex-wrap">x</div>`;
    expect(shapesIn(source, SCREEN)).toEqual([]);
  });

  it("ignores a stack with only one arbitrary value", () => {
    const source = `<div className="flex items-center gap-2 border-[color:var(--border-subtle)]">x</div>`;
    expect(shapesIn(source, SCREEN)).toEqual([]);
  });

  it("ignores a shared component rendered the same way everywhere", () => {
    // `<Card className="…">` in five screens is the primitive doing its job;
    // flagging it would punish the composition this whole check encourages
    const source = `<Card className="p-[22px] gap-3.5 rounded-[10px] mx-auto">x</Card>`;
    expect(shapesIn(source, SCREEN)).toEqual([]);
  });

  it("ignores a computed className", () => {
    // `cn(…)` depends on props: two spellings that look alike may render
    // nothing alike, and guessing there is wrong in the direction that costs trust
    const source = `<div className={cn("p-[22px] gap-3.5 rounded-[10px] mx-auto", extra)}>x</div>`;
    expect(shapesIn(source, SCREEN)).toEqual([]);
  });

  it("reads past a handler that puts a > inside the tag", () => {
    // `[^>]*` would stop at the arrow and miss every element with a handler,
    // which is most of the interesting ones
    const source = `<button onClick={() => close()} className="p-[22px] gap-3.5 rounded-[10px] mx-auto">x</button>`;
    expect(shapesIn(source, SCREEN)).toMatchObject([{ tag: "button" }]);
  });

  it("skips the primitives directory and the fixtures", () => {
    for (const file of [
      "src/components/ui/card.tsx",
      "src/pages/Example.stories.tsx",
      "src/lib/thing.test.ts",
    ]) {
      expect(shapesIn(DANGER, file)).toEqual([]);
    }
  });

  it("honours the same inline waiver the other rules take", () => {
    const source = `{/* ui-primitives-allow: the one banner that is not a row */}\n${DANGER}`;
    const { shapes, waivers } = collectShapes(source, SCREEN);
    expect(shapes).toEqual([]);
    expect(waivers).toMatchObject([
      { rule: "duplicated-shape", line: 2, reason: "the one banner that is not a row" },
    ]);
  });

  it("marks a waived shape with no reason as unknown so the run fails", () => {
    const { waivers } = collectShapes(`// ui-primitives-allow:\n${DANGER}`, SCREEN);
    expect(waivers).toMatchObject([{ rule: "unknown", reason: "" }]);
  });
});

describe("the repeated-shape allow list", () => {
  const DANGER = `<p className="px-[22px] pt-2.5 text-xs text-[color:var(--status-danger-text)]">x</p>`;
  const KEY = "p|pt-2.5 px-[22px] text-[color:var(--status-danger-text)] text-xs";
  const three = ["src/pages/A.tsx", "src/pages/B.tsx", "src/pages/C.tsx"].flatMap(
    (file) => collectShapes(DANGER, file).shapes,
  );

  it("silences a recorded shape", () => {
    expect(duplicatedShapes(three, { [KEY]: "the sheet footer, tracked in #1711" })).toEqual([]);
  });

  it("reports an entry the tree no longer duplicates", () => {
    // the entry has to go with the fix, or the same copy-paste comes back
    // under cover of a reason that no longer describes anything
    expect(staleShapes(three.slice(0, 1), { [KEY]: "still here" })).toEqual([KEY]);
    expect(staleShapes(three, { [KEY]: "still here" })).toEqual([]);
  });

  it("reports an entry written without a reason", () => {
    expect(unexplainedShapes({ [KEY]: "  " })).toEqual([KEY]);
    expect(unexplainedShapes({ [KEY]: "a reason" })).toEqual([]);
  });
});
