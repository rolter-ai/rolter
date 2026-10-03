import type { Meta, StoryObj } from "@storybook/react";

import { SurfacePanel } from "./surface-panel";

const meta = {
  title: "Display/SurfacePanel",
  component: SurfacePanel,
  parameters: { layout: "padded" },
} satisfies Meta<typeof SurfacePanel>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  render: () => (
    <SurfacePanel className="max-w-md">
      <header className="border-b border-[color:var(--border-subtle)] px-4 py-3">
        <h2 className="text-sm font-medium text-foreground">Two-factor authentication</h2>
        <p className="mt-1 text-sm text-muted-foreground">Protect this account with a code.</p>
      </header>
      <div className="px-4 py-3 text-sm">Not enabled.</div>
    </SurfacePanel>
  ),
};
