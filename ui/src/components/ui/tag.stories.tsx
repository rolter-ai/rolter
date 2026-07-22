import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import { Tag } from "./tag";

const meta = {
  title: "Primitives/Tag",
  component: Tag,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Tag>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = { args: { children: "production" } };

function RemovableList() {
  const [tags, setTags] = React.useState(["openai", "anthropic", "ollama"]);
  return (
    <div className="flex flex-wrap gap-2">
      {tags.map((t) => (
        <Tag key={t} onRemove={() => setTags((prev) => prev.filter((x) => x !== t))}>
          {t}
        </Tag>
      ))}
    </div>
  );
}

export const Removable: Story = { render: () => <RemovableList /> };

// interaction: clicking a tag's remove control drops it from the list
export const RemovesOnClick: Story = {
  render: () => <RemovableList />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("anthropic")).toBeInTheDocument();
    const removeButtons = canvas.getAllByRole("button", { name: "Remove" });
    await userEvent.click(removeButtons[1]);
    await expect(canvas.queryByText("anthropic")).toBeNull();
    await expect(canvas.getByText("openai")).toBeInTheDocument();
  },
};
