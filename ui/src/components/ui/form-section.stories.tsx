import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import { FormSection } from "./form-section";
import { Input } from "./input";

const meta = {
  title: "Forms/FormSection",
  component: FormSection,
  parameters: { layout: "padded" },
  args: {
    title: "General",
    open: true,
    onToggle: () => {},
    children: <Input aria-label="Alias" defaultValue="gpt-4o" />,
  },
} satisfies Meta<typeof FormSection>;

export default meta;
type Story = StoryObj<typeof meta>;

function Controlled({ initial = true, info }: { initial?: boolean; info?: string }) {
  const [open, setOpen] = React.useState(initial);
  return (
    <div className="max-w-md">
      <FormSection title="General" info={info} open={open} onToggle={() => setOpen((v) => !v)}>
        <Input aria-label="Alias" defaultValue="gpt-4o" />
      </FormSection>
    </div>
  );
}

export const Open: Story = { render: () => <Controlled /> };
export const Collapsed: Story = { render: () => <Controlled initial={false} /> };

/**
 * A collapsed section holds nothing in the document — not hidden fields that a
 * screen reader would still walk through and that a `Tab` would still land in.
 */
export const CollapsedDropsItsBody: Story = {
  render: () => <Controlled initial={false} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const toggle = canvas.getByRole("button", { name: "General" });
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    // bites if the body renders while closed, or is only hidden with CSS
    await expect(canvas.queryByLabelText("Alias")).not.toBeInTheDocument();
  },
};

/** The header is a real button, so the keyboard opens it like the pointer does. */
export const TogglesFromTheKeyboard: Story = {
  render: () => <Controlled initial={false} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const toggle = canvas.getByRole("button", { name: "General" });
    toggle.focus();
    await userEvent.keyboard("{Enter}");
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
    await expect(canvas.getByLabelText("Alias")).toBeVisible();
    await userEvent.keyboard("{Enter}");
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
  },
};

/**
 * The (i) sits *beside* the toggle rather than inside it. A button inside a
 * button is invalid HTML, announced as one confused control, and an axe
 * `nested-interactive` failure (#1201) — this is the story that holds the fix.
 */
export const InfoSitsBesideTheToggle: Story = {
  render: () => <Controlled info="Identity and endpoint of the routed model." />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const toggle = canvas.getByRole("button", { name: "General" });
    const hint = canvas.getByRole("button", { name: "About General" });
    await expect(toggle.contains(hint)).toBe(false);
    await userEvent.hover(hint);
    await expect(canvas.getByRole("tooltip")).toHaveTextContent(/Identity and endpoint/);
    // pressing the hint is not pressing the toggle, which a nested button
    // would make it: the section is still open afterwards
    await userEvent.click(hint);
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
  },
};
