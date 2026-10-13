import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, within } from "storybook/test";

import { FieldDiff, TextDiff } from "./text-diff";
import ru from "@/lib/i18n/locales/ru.json";
import { diffFields, diffLines } from "@/lib/text-diff";
import { atMobile, expectNoHorizontalOverflow } from "@/lib/story-viewport";

const before = [
  "# Incident coordinator",
  "",
  "1. Collect evidence.",
  "2. Assign severity.",
  "3. Record the handoff.",
  "",
  "Keep the tone calm.",
  "",
  "## Escalation",
  "",
  "Page the on-call engineer.",
  "Notify the incident channel.",
  "Open a ticket.",
  "Link the ticket in the channel.",
  "Review the timeline the next morning.",
].join("\n");
const after = before
  .replace("2. Assign severity.", "2. Assign severity from the runbook table.")
  .replace("Open a ticket.", "Open a ticket and link it in the channel.");

const lines = diffLines(before, after);

const meta = {
  title: "Display/TextDiff",
  component: TextDiff,
  parameters: { layout: "padded" },
  args: { lines, label: "SKILL.md content" },
} satisfies Meta<typeof TextDiff>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * Inline by default: removed and added lines, each marked in the gutter and
 * named for a screen reader, with the unchanged stretch between the two
 * edits folded away.
 */
export const Inline: Story = {
  play: async ({ canvasElement }) => {
    const region = within(
      await within(canvasElement).findByRole("region", { name: "SKILL.md content" }),
    );
    await expect(region.getByText("2. Assign severity.").closest("div")).toHaveTextContent(
      /Removed:/,
    );
    await expect(
      region.getByText("2. Assign severity from the runbook table.").closest("div"),
    ).toHaveTextContent(/Added:/);
    await expect(region.getByText("2 unchanged lines")).toBeVisible();
    // the region scrolls sideways, so the keyboard has to be able to reach it
    await expect(
      within(canvasElement).getByRole("region", { name: "SKILL.md content" }),
    ).toHaveAttribute("tabindex", "0");
  },
};

export const SideBySide: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const layout = await canvas.findByRole("radio", { name: "Side by side" });
    await userEvent.click(layout);
    await expect(layout).toBeChecked();
    const region = within(canvas.getByRole("region", { name: "SKILL.md content" }));
    // the removed line and its replacement sit in one row
    const removed = region.getByText("2. Assign severity.");
    const added = region.getByText("2. Assign severity from the runbook table.");
    await expect(removed.closest("[class*=grid-cols-2]")).toBe(
      added.closest("[class*=grid-cols-2]"),
    );
  },
};

export const Identical: Story = {
  args: { lines: diffLines(before, before) },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("No differences.")).toBeVisible();
    await expect(canvas.queryByRole("region")).toBeNull();
    await expect(canvas.queryByRole("radiogroup")).toBeNull();
  },
};

export const OnAPhone: Story = {
  ...atMobile,
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByRole("region", { name: "SKILL.md content" });
    await expectNoHorizontalOverflow();
  },
};

export const InRussian: Story = {
  globals: { locale: "ru" },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByRole("radio", { name: ru.repository.diff.split }),
    ).toBeVisible();
    await expect(canvas.getByText(/2 строки без изменений/)).toBeVisible();
  },
};

const fields = diffFields(
  { runtime: '"node"', capabilities: '["logs","handoff"]', tier: '"gold"' },
  { runtime: '"bun"', capabilities: '["logs","handoff"]', owner: '"ops"' },
);

/** a record's fields, one row per field that differs; the unchanged ones are left out */
export const Fields: Story = {
  render: () => <FieldDiff changes={fields} label="Version metadata" />,
  play: async ({ canvasElement }) => {
    const list = within(
      await within(canvasElement).findByRole("list", { name: "Version metadata" }),
    );
    await expect(list.getAllByRole("listitem")).toHaveLength(3);
    await expect(list.getByText("runtime").closest("li")).toHaveTextContent(/changed/);
    await expect(list.getByText("runtime").closest("li")).toHaveTextContent('"node"');
    await expect(list.getByText("runtime").closest("li")).toHaveTextContent('"bun"');
    await expect(list.getByText("tier").closest("li")).toHaveTextContent(/removed/);
    await expect(list.getByText("owner").closest("li")).toHaveTextContent(/added/);
    await expect(list.queryByText("capabilities")).toBeNull();
  },
};

export const FieldsIdentical: Story = {
  render: () => <FieldDiff changes={diffFields({ a: "1" }, { a: "1" })} label="Version metadata" />,
  play: async ({ canvasElement }) => {
    await expect(await within(canvasElement).findByText("No differences.")).toBeVisible();
  },
};
