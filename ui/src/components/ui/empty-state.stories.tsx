import type { Meta, StoryObj } from "@storybook/react";
import { Inbox } from "lucide-react";
import { MemoryRouter, useLocation } from "react-router";
import { expect, userEvent, within } from "storybook/test";

import { Button } from "./button";
import { EmptyState, EmptyStateLink } from "./empty-state";

const meta = {
  title: "Feedback/EmptyState",
  component: EmptyState,
  parameters: { layout: "padded" },
} satisfies Meta<typeof EmptyState>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: {
    icon: <Inbox />,
    title: "No routes yet",
    description: "Create a route to start forwarding traffic to your providers.",
  },
};

export const WithAction: Story = {
  args: {
    icon: <Inbox />,
    title: "No invocations in range",
    description: "Try widening the time window or clearing filters.",
    actions: <Button variant="outline">Clear filters</Button>,
  },
};

function LocationProbe() {
  return <output data-testid="location">{useLocation().pathname}</output>;
}

// a link action routes through the router: the location changes in place and
// the document is never navigated, so the SPA (and its state) survives (#2215)
export const WithLinkAction: Story = {
  args: {
    icon: <Inbox />,
    title: "No traffic yet",
    description: "Send a request from the playground to produce an event.",
  },
  render: (args) => (
    <MemoryRouter initialEntries={["/health"]}>
      <EmptyState
        {...args}
        actions={<EmptyStateLink to="/playground">Open the playground</EmptyStateLink>}
      />
      <LocationProbe />
    </MemoryRouter>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const marker = Symbol("document");
    (window as unknown as Record<symbol, unknown>)[marker] = true;
    const link = canvas.getByRole("link", { name: "Open the playground" });
    await userEvent.click(link);
    await expect(canvas.getByTestId("location")).toHaveTextContent("/playground");
    // a full page load would have replaced the window and dropped the marker
    await expect((window as unknown as Record<symbol, unknown>)[marker]).toBe(true);
  },
};
