import { describe, it, expect } from "bun:test";

import { blankComments, checkSource, isSettled, previousStatement } from "./check-story-focus";

/** wrap statements in a play function, the only place a story asserts focus */
function play(...statements: string[]): string {
  return ["play: async ({ canvasElement }) => {", ...statements, "},"].join("\n");
}

describe("checkSource", () => {
  it("flags a focus assertion that follows a wait for the element to go", () => {
    const found = checkSource(
      play(
        '    await waitFor(() => expect(canvas.queryByRole("dialog")).toBeNull());',
        '    await expect(canvas.getByRole("button", { name: "open" })).toHaveFocus();',
      ),
      "a.stories.tsx",
    );
    expect(found).toHaveLength(1);
    expect(found[0].line).toBe(3);
    expect(found[0].after).toContain("toBeNull");
  });

  it("flags a focus assertion that follows a query rather than an action", () => {
    // the shape that broke the command palette against the static build: find
    // the field, then assert focus on it before the dialog has handed it over
    const found = checkSource(
      play(
        '    const field = await canvas.findByRole("combobox");',
        "    await expect(field).toHaveFocus();",
      ),
      "a.stories.tsx",
    );
    expect(found).toHaveLength(1);
  });

  it("allows an assertion inside a waiter", () => {
    expect(
      checkSource(
        play('    await waitFor(() => expect(canvas.getByLabelText("name")).toHaveFocus());'),
        "a.stories.tsx",
      ),
    ).toEqual([]);
  });

  it("allows an assertion inside a multi-line waiter", () => {
    expect(
      checkSource(
        play(
          "    await waitFor(() =>",
          '      expect(canvas.getByRole("button", { name: "open" })).toHaveFocus(),',
          "    );",
        ),
        "a.stories.tsx",
      ),
    ).toEqual([]);
  });

  it("allows an assertion straight after the action that moved focus", () => {
    for (const mover of [
      "    await userEvent.tab();",
      '    await userEvent.click(canvas.getByRole("button"));',
      '    await userEvent.keyboard("{ArrowRight}");',
      "    region.focus();",
    ]) {
      expect(
        checkSource(play(mover, "    await expect(region).toHaveFocus();"), "a.stories.tsx"),
      ).toEqual([]);
    }
  });

  it("allows a second read of a moment another assertion already settled", () => {
    expect(
      checkSource(
        play(
          '    await expect(combobox).toHaveAttribute("aria-expanded", "true");',
          "    await expect(combobox).toHaveFocus();",
        ),
        "a.stories.tsx",
      ),
    ).toEqual([]);
  });

  it("allows a negative assertion after a waiter that settled focus", () => {
    expect(
      checkSource(
        play(
          '    await waitFor(() => expect(canvas.getByRole("dialog")).toHaveFocus());',
          '    await expect(canvas.getByRole("button", { name: "delete" })).not.toHaveFocus();',
        ),
        "a.stories.tsx",
      ),
    ).toEqual([]);
  });

  it("allows a negative assertion used as a precondition", () => {
    // how a story says "focus has not moved yet" before the keystroke that
    // moves it. Waiting for an absence that is already true adds nothing
    expect(
      checkSource(
        play(
          '    const search = within(rail).getByRole("textbox", { name: "Search" });',
          "    await expect(search).not.toHaveFocus();",
          '    await userEvent.keyboard("/");',
          "    await waitFor(() => expect(search).toHaveFocus());",
        ),
        "a.stories.tsx",
      ),
    ).toEqual([]);
  });

  it("does not read a rule out of a comment", () => {
    expect(
      checkSource(
        play(
          '    await waitFor(() => expect(canvas.queryByRole("dialog")).toBeNull());',
          "    // toHaveFocus() here would race the handover",
          '    await waitFor(() => expect(canvas.getByRole("button")).toHaveFocus());',
        ),
        "a.stories.tsx",
      ),
    ).toEqual([]);
  });
});

describe("blankComments", () => {
  it("keeps the line count so reported lines still point at the source", () => {
    const source = "a\n/* two\n   lines */\nb\n// tail\n";
    expect(blankComments(source).split("\n")).toHaveLength(source.split("\n").length);
  });

  it("leaves a url in a string alone", () => {
    expect(blankComments('const u = "https://example.com/x";')).toBe(
      'const u = "https://example.com/x";',
    );
  });
});

describe("previousStatement", () => {
  it("skips blank lines and bare braces", () => {
    expect(previousStatement(["await userEvent.tab();", "", "{", "x"], 3)).toBe(
      "await userEvent.tab();",
    );
  });

  it("is empty at the top of a block", () => {
    expect(previousStatement(["", "  ", "x"], 2)).toBe("");
  });
});

describe("isSettled", () => {
  it("rejects a waiter that waited on something other than focus", () => {
    expect(isSettled('await waitFor(() => expect(canvas.queryByRole("dialog")).toBeNull());')).toBe(
      false,
    );
  });

  it("rejects the start of a play function", () => {
    expect(isSettled("")).toBe(false);
  });
});
