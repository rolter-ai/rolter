import type { Meta, StoryObj } from "@storybook/react";
import { expect, within } from "storybook/test";

import { Badge } from "./badge";

const meta = {
  title: "Primitives/Badge",
  component: Badge,
  parameters: { layout: "padded" },
  args: { children: "neutral" },
  argTypes: {
    tone: {
      control: "select",
      options: ["neutral", "outline", "success", "warning", "danger", "info", "accent"],
    },
    dot: { control: "boolean" },
  },
} satisfies Meta<typeof Badge>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

// the token each coloured tone must resolve its label to. the theme is
// dark-only (docs/dev-docs/development/dashboard-theme.md), so a tone that drifts back
// to a raw Tailwind palette colour — or grows a tailwind `dark` variant nothing
// can ever activate — is the regression this story guards (#1199)
const TONE_TOKENS = {
  success: "--status-success-text",
  warning: "--status-warning-text",
  danger: "--status-danger-text",
  info: "--status-info-text",
  accent: "--red-folk-text",
} as const;

// resolve a custom property the way the browser would, so the assertion follows
// the token if the design retunes it instead of pinning a literal rgb()
function resolve(token: string): string {
  const probe = document.createElement("span");
  probe.style.color = `var(${token})`;
  document.body.appendChild(probe);
  const color = getComputedStyle(probe).color;
  probe.remove();
  return color;
}

export const AllTones: Story = {
  render: () => (
    <div className="flex flex-wrap gap-2">
      <Badge tone="neutral" dot>
        neutral
      </Badge>
      <Badge tone="outline" dot>
        outline
      </Badge>
      <Badge tone="success" dot>
        healthy
      </Badge>
      <Badge tone="warning" dot>
        degraded
      </Badge>
      <Badge tone="danger" dot>
        down
      </Badge>
      <Badge tone="info" dot>
        info
      </Badge>
      <Badge tone="accent" dot>
        draining
      </Badge>
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const labels: Record<keyof typeof TONE_TOKENS, string> = {
      success: "healthy",
      warning: "degraded",
      danger: "down",
      info: "info",
      accent: "draining",
    };

    for (const [tone, token] of Object.entries(TONE_TOKENS)) {
      const badge = canvas.getByText(labels[tone as keyof typeof TONE_TOKENS]);
      await expect(getComputedStyle(badge).color).toBe(resolve(token));
    }

    // no tone may carry a variant the dark-only theme can never turn on
    for (const badge of canvasElement.querySelectorAll("span")) {
      await expect(badge.className).not.toContain("dark:");
    }
  },
};
