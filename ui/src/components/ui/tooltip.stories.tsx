import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { Badge } from "./badge";
import { Tooltip } from "./tooltip";
import { noteOf } from "@/pages/story-harness";

const ID = "0b9f3c2e-5a41-4e8b-9d7c-2f6a1e4b8c90";

const meta = {
  title: "Primitives/Tooltip",
  component: Tooltip,
  parameters: { layout: "padded" },
  args: { content: ID, children: ID.slice(0, 8) },
} satisfies Meta<typeof Tooltip>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * Closed is the resting state, and the note is already the trigger's
 * description: a screen reader is given the whole id whether the note is open
 * or not (#3655).
 */
export const Default: Story = {
  render: (args) => (
    <span className="font-mono text-xs">
      actor <Tooltip {...args} />
    </span>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const trigger = canvas.getByText(ID.slice(0, 8));
    await expect(canvas.queryByRole("tooltip")).not.toBeInTheDocument();
    await expect(trigger).toHaveAttribute("tabindex", "0");
    await expect(trigger).not.toHaveAttribute("title");
    await expect(trigger).toHaveAccessibleDescription(ID);
    await expect(noteOf(trigger)).toBe(ID);
  },
};

/** Pointing at the value opens the note under it. */
export const OpensOnHover: Story = {
  render: (args) => <Tooltip {...args} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.hover(canvas.getByText(ID.slice(0, 8)));
    await expect(canvas.getByRole("tooltip")).toHaveTextContent(ID);
  },
};

/**
 * WCAG 1.4.13 hoverable: the pointer can leave the value and travel onto the
 * note, to select and copy the id, and the note stays. Leaving both closes it.
 */
export const PointerCanReachTheNote: Story = {
  render: (args) => <Tooltip {...args} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const trigger = canvas.getByText(ID.slice(0, 8));
    await userEvent.hover(trigger);
    const note = canvas.getByRole("tooltip");
    await userEvent.unhover(trigger);
    await userEvent.hover(note);
    await new Promise((resolve) => setTimeout(resolve, 250));
    await expect(canvas.getByRole("tooltip")).toBeVisible();
    await userEvent.unhover(note);
    await waitFor(() => expect(canvas.queryByRole("tooltip")).not.toBeInTheDocument());
  },
};

/** A keyboard reaches the value with Tab and reads the note; Tab away closes it. */
export const OpensOnFocus: Story = {
  render: (args) => <Tooltip {...args} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.tab();
    await expect(canvas.getByText(ID.slice(0, 8))).toHaveFocus();
    await expect(canvas.getByRole("tooltip")).toHaveTextContent(ID);
    await userEvent.tab();
    await expect(canvas.queryByRole("tooltip")).not.toBeInTheDocument();
  },
};

/**
 * A tap opens it, on a phone that does not focus a tapped span too. Unlike
 * `InfoHint` a press does not toggle: the value is not a control, so a second
 * tap is not a request to close.
 */
export const OpensOnTap: Story = {
  render: (args) => <Tooltip {...args} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const trigger = canvas.getByText(ID.slice(0, 8));
    await userEvent.click(trigger);
    await expect(canvas.getByRole("tooltip")).toBeVisible();
    await userEvent.click(trigger);
    await expect(canvas.getByRole("tooltip")).toBeVisible();
  },
};

/** WCAG 1.4.13 dismissible: Escape closes the note while focus stays put. */
export const ClosesOnEscape: Story = {
  render: (args) => <Tooltip {...args} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.tab();
    await expect(canvas.getByRole("tooltip")).toBeVisible();
    await userEvent.keyboard("{Escape}");
    await expect(canvas.queryByRole("tooltip")).not.toBeInTheDocument();
    await expect(canvas.getByText(ID.slice(0, 8))).toHaveFocus();
  },
};

/** A list behind "Ana +2" is one name a line, from `\n` in the content. */
export const ListsOneEntryALine: Story = {
  args: { content: "Ana Petrova\nBo Lindqvist\nChidi Okafor", children: "Ana Petrova +2" },
  render: (args) => <Tooltip {...args} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.hover(canvas.getByText("Ana Petrova +2"));
    const note = canvas.getByRole("tooltip");
    await expect(getComputedStyle(note).whiteSpace).toBe("pre-line");
    await expect(note.getBoundingClientRect().height).toBeGreaterThan(40);
  },
};

/**
 * With nothing to say the value is plain text: no tab stop, no underline, no
 * description, so a call site whose note is only sometimes there needs no
 * branch of its own.
 */
export const NoContentIsPlainText: Story = {
  args: { content: undefined, children: "Ana Petrova" },
  render: (args) => <Tooltip {...args} />,
  play: async ({ canvasElement }) => {
    const value = within(canvasElement).getByText("Ana Petrova");
    await expect(value).not.toHaveAttribute("tabindex");
    await expect(value).not.toHaveAttribute("aria-describedby");
    await expect(getComputedStyle(value).textDecorationLine).toBe("none");
  },
};

/**
 * A `Badge` takes the note as its `hint`: the badge itself is the focusable
 * trigger, with no span around it and no `title`.
 */
export const BadgeTakesAHint: Story = {
  render: () => (
    <Badge tone="warning" hint="The provider has no credential configured.">
      No key
    </Badge>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const badge = canvas.getByText("No key");
    await expect(badge).not.toHaveAttribute("title");
    await expect(badge).toHaveAccessibleDescription("The provider has no credential configured.");
    await userEvent.tab();
    await expect(badge).toHaveFocus();
    await expect(canvas.getByRole("tooltip")).toHaveTextContent(/no credential/);
  },
};
