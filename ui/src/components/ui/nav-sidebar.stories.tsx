import { Boxes, KeyRound, Play, ScrollText } from "lucide-react";
import type { Meta, StoryObj } from "@storybook/react";

import { NavSidebar } from "./nav-sidebar";

const meta = {
  title: "Navigation/NavSidebar",
  component: NavSidebar,
  args: {
    brand: "rolter",
    groups: [
      {
        items: [
          { key: "playground", label: "Playground", icon: <Play /> },
          { key: "models", label: "Models", icon: <Boxes /> },
          { key: "keys", label: "Keys", icon: <KeyRound /> },
          { key: "logs", label: "Logs", icon: <ScrollText /> },
        ],
      },
      {
        label: "Operate",
        items: [
          {
            key: "analytics",
            label: "Analytics",
            icon: <Boxes />,
            children: [
              { key: "usage", label: "Usage" },
              { key: "costs", label: "Costs" },
            ],
          },
        ],
      },
    ],
    activeKey: "models",
    searchable: true,
    collapsible: true,
    version: "v0.0.1",
    user: { name: "admin@rolter.dev", role: "Admin", initials: "A" },
  },
} satisfies Meta<typeof NavSidebar>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const Collapsed: Story = {
  args: { defaultCollapsed: true },
};
