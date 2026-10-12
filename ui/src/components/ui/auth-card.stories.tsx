import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, within } from "storybook/test";

import { Button } from "./button";
import { AuthCard } from "./auth-card";

const meta = {
  title: "Primitives/AuthCard",
  component: AuthCard,
  parameters: { layout: "fullscreen" },
  args: { title: "Sign in" },
} satisfies Meta<typeof AuthCard>;

export default meta;
type Story = StoryObj<typeof meta>;

// the card is the whole page signed out, so it owns the landmark and the h1
export const OwnsLandmarkAndHeading: Story = {
  args: { description: "Use your rolter account." },
  render: (args) => (
    <AuthCard {...args}>
      <Button type="button">Continue</Button>
    </AuthCard>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("main")).toBeInTheDocument();
    await expect(canvas.getByRole("heading", { level: 1, name: "Sign in" })).toBeInTheDocument();
    await expect(canvas.getByText("Use your rolter account.")).toBeInTheDocument();
  },
};

export const StatusDescription: Story = {
  args: { title: "Join Acme", description: "You are in.", descriptionRole: "status" },
  play: async ({ canvasElement }) => {
    await expect(within(canvasElement).getByRole("status")).toHaveTextContent("You are in.");
  },
};
