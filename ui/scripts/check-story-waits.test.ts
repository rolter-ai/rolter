import { describe, it, expect } from "bun:test";

import { checkAll, checkSource, inGatedStory, queriedRole, waitedLines } from "./check-story-waits";

/** a story object, which is the unit the gate rule is scoped to */
function story(role: string | null, ...statements: string[]): string {
  return [
    "export const SomeStory: Story = {",
    "  render: () => (",
    `    <Harness fetchStub={stub}${role ? ` role="${role}"` : ""}>`,
    "      <Screen />",
    "    </Harness>",
    "  ),",
    "  play: async ({ canvasElement }) => {",
    ...statements,
    "  },",
    "};",
  ].join("\n");
}

describe("the gate rule", () => {
  it("flags a one-shot toBeDisabled in a story mounted under a role", () => {
    const { violations } = checkSource(
      story(
        "admin",
        '    await expect(canvas.getByRole("button", { name: "Add" })).toBeDisabled();',
      ),
      "a.stories.tsx",
    );
    expect(violations).toHaveLength(1);
    expect(violations[0].rule).toBe("gate");
    expect(violations[0].line).toBe(8);
  });

  it("accepts the same assertion inside a waitFor", () => {
    const { violations } = checkSource(
      story("admin", '    await waitFor(() => expect(canvas.getByRole("button")).toBeDisabled());'),
      "a.stories.tsx",
    );
    expect(violations).toEqual([]);
  });

  it("accepts it inside a multi-line waitFor", () => {
    const { violations } = checkSource(
      story(
        "admin",
        "    await waitFor(() => {",
        '      expect(canvas.getByRole("button")).toBeDisabled();',
        "    });",
      ),
      "a.stories.tsx",
    );
    expect(violations).toEqual([]);
  });

  it("leaves an ungated story alone — with no role there is no gate in flight", () => {
    const { violations } = checkSource(
      story(null, '    await expect(canvas.getByRole("button")).toBeDisabled();'),
      "a.stories.tsx",
    );
    expect(violations).toEqual([]);
  });

  it("does not read one story's role into the next", () => {
    const source = [
      story("admin", "    await expectRefused(canvasElement, /Add/);"),
      story(null, '    await expect(canvas.getByRole("button")).toBeDisabled();'),
    ].join("\n\n");
    expect(checkSource(source, "a.stories.tsx").violations).toEqual([]);
  });

  it("is not fooled by an aria role in the markup", () => {
    const source = [
      "export const SomeStory: Story = {",
      '  render: () => <div role="status" />,',
      "  play: async ({ canvasElement }) => {",
      '    await expect(canvas.getByRole("button")).toBeDisabled();',
      "  },",
      "};",
    ].join("\n");
    expect(checkSource(source, "a.stories.tsx").violations).toEqual([]);
  });
});

describe("the sheet-row rule", () => {
  it("flags a data-shaped getByRole on the statement after a sheet opens", () => {
    const { violations } = checkSource(
      story(
        null,
        "    const form = within(sheet());",
        '    await userEvent.click(form.getByRole("checkbox", { name: /Support/ }));',
      ),
      "a.stories.tsx",
    );
    expect(violations).toHaveLength(1);
    expect(violations[0].rule).toBe("sheet-row");
    expect(violations[0].line).toBe(9);
  });

  it("flags it after the dialog is found by role", () => {
    const { violations } = checkSource(
      story(
        null,
        '    const dialog = within(await within(document.body).findByRole("dialog"));',
        '    await userEvent.click(dialog.getByRole("radio", { name: /OAuth/ }));',
      ),
      "a.stories.tsx",
    );
    expect(violations).toHaveLength(1);
  });

  it("accepts findByRole", () => {
    const { violations } = checkSource(
      story(
        null,
        "    const form = within(sheet());",
        '    await userEvent.click(await form.findByRole("checkbox", { name: /Support/ }));',
      ),
      "a.stories.tsx",
    );
    expect(violations).toEqual([]);
  });

  it("leaves the sheet's own markup alone", () => {
    // a heading and a confirm button paint with the dialog; only a row rendered
    // out of the sheet's own request can be a tick behind it
    const { violations } = checkSource(
      story(
        null,
        "    const form = within(sheet());",
        '    await expect(form.getByRole("heading", { name: "Add model" })).toBeVisible();',
      ),
      "a.stories.tsx",
    );
    expect(violations).toEqual([]);
  });

  it("only looks at the statement the sheet opened on", () => {
    const { violations } = checkSource(
      story(
        null,
        "    const form = within(sheet());",
        '    await userEvent.type(form.getByLabelText("Name"), "Support");',
        '    await userEvent.click(form.getByRole("checkbox", { name: /Support/ }));',
      ),
      "a.stories.tsx",
    );
    expect(violations).toEqual([]);
  });
});

describe("waivers", () => {
  it("honours a marker on the line above and records the reason", () => {
    const { violations, waivers } = checkSource(
      story(
        "admin",
        "    // story-wait-allow: disabled by its own prop",
        '    await expect(canvas.getByRole("button")).toBeDisabled();',
      ),
      "a.stories.tsx",
    );
    expect(violations).toEqual([]);
    expect(waivers).toHaveLength(1);
    expect(waivers[0].reason).toBe("disabled by its own prop");
  });

  it("honours a marker anywhere in the comment block above", () => {
    const { violations, waivers } = checkSource(
      story(
        "admin",
        "    // story-wait-allow: disabled by its own prop from the first paint,",
        "    // so there is no gate answer to wait for",
        '    await expect(canvas.getByRole("button")).toBeDisabled();',
      ),
      "a.stories.tsx",
    );
    expect(violations).toEqual([]);
    expect(waivers).toHaveLength(1);
  });

  it("does not reach past the comment block to an earlier marker", () => {
    const { violations } = checkSource(
      story(
        "admin",
        "    // story-wait-allow: about the line below, not the one after it",
        "    await userEvent.click(canvas.getByRole('button'));",
        '    await expect(canvas.getByRole("button")).toBeDisabled();',
      ),
      "a.stories.tsx",
    );
    expect(violations).toHaveLength(1);
  });
});

describe("helpers", () => {
  it("reports every line inside a multi-line waiter", () => {
    const inside = waitedLines([
      "await waitFor(() => {",
      "  expect(a).toBeDisabled();",
      "});",
      "expect(b).toBeDisabled();",
    ]);
    expect([...inside]).toEqual([1, 2]);
  });

  it("reads the queried role", () => {
    expect(queriedRole('canvas.getByRole("checkbox", { name: /x/ })')).toBe("checkbox");
    expect(queriedRole('canvas.findByRole("checkbox")')).toBeNull();
    expect(queriedRole("canvas.getByLabelText('Name')")).toBeNull();
  });

  it("scopes a role to the story it was declared in", () => {
    const lines = [
      "export const A: Story = {",
      '  render: () => <Harness role="admin" />,',
      "};",
      "export const B: Story = {",
      "  render: () => <Harness />,",
      "};",
    ];
    expect(inGatedStory(lines, 1)).toBe(true);
    expect(inGatedStory(lines, 4)).toBe(false);
  });

  it("ignores a comment that spells the rule out", () => {
    const { violations } = checkSource(
      story("admin", "    // never write expect(x).toBeDisabled() here", "    await done();"),
      "a.stories.tsx",
    );
    expect(violations).toEqual([]);
  });
});

describe("the tree", () => {
  it("has no zero-latency assertion left", () => {
    expect(checkAll().violations).toEqual([]);
  });
});
