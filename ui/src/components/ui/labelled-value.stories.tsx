import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, within } from "storybook/test";

import { LabelledValue } from "./labelled-value";
import { atMobile, expectNoHorizontalOverflow } from "@/lib/story-viewport";

const meta = {
  title: "Forms/LabelledValue",
  component: LabelledValue,
  parameters: { layout: "padded" },
  args: { label: "Issuer", value: "https://acme.okta.com" },
  render: (args) => (
    <div className="max-w-md">
      <LabelledValue {...args} />
    </div>
  ),
} satisfies Meta<typeof LabelledValue>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Plain: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("Issuer")).toBeVisible();
    await expect(canvas.getByText("https://acme.okta.com")).toBeVisible();
  },
};

/** A value too long for the card is one truncated line that keeps the whole value as its title. */
export const LongValueIsTruncatedOnOneLine: Story = {
  ...atMobile,
  args: { value: `https://example.com/${"0123456789abcdef".repeat(8)}` },
  play: async ({ canvasElement, args }) => {
    await expect(within(canvasElement).getByTitle(args.value)).toBeVisible();
    await expectNoHorizontalOverflow();
  },
};

export const CopyableWithANote: Story = {
  args: {
    label: "Redirect URI",
    value: "https://rolter.example.com/auth/sso/okta/callback",
    copyLabel: "Copy redirect URI",
    note: "Register this in your identity provider.",
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      canvas.getByRole("button", {
        name: "Copy redirect URI: https://rolter.example.com/auth/sso/okta/callback",
      }),
    ).toBeVisible();
    await expect(canvas.getByText("Register this in your identity provider.")).toBeVisible();
  },
};
