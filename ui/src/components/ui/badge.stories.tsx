import type { Meta, StoryObj } from "@storybook/react";
import { expect, within } from "storybook/test";

import { Badge, HttpStatusBadge } from "./badge";
import { resolveColorToken } from "@/lib/story-tokens";

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
      await expect(getComputedStyle(badge).color).toBe(resolveColorToken(token));
    }

    // no tone may carry a variant the dark-only theme can never turn on
    for (const badge of canvasElement.querySelectorAll("span")) {
      await expect(badge.className).not.toContain("dark:");
    }
  },
};

/**
 * #1989: the Dashboard and LLM Logs share one HTTP-status pill. A 4xx is the
 * caller's to fix, a 5xx or no answer at all is the gateway's, and each takes
 * its label from the same token a `Badge` of that tone does.
 */
export const HttpStatuses: Story = {
  render: () => (
    <div className="flex flex-wrap gap-2">
      <HttpStatusBadge status={200} />
      <HttpStatusBadge status={429} />
      <HttpStatusBadge status={502} />
      <HttpStatusBadge status={0}>no response</HttpStatusBadge>
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const expected: [string, `--${string}`][] = [
      ["200", "--status-success-text"],
      ["429", "--status-warning-text"],
      ["502", "--status-danger-text"],
      ["no response", "--status-danger-text"],
    ];
    for (const [label, token] of expected) {
      await expect(getComputedStyle(canvas.getByText(label)).color).toBe(resolveColorToken(token));
    }
    // a number is set in mono, words are not
    await expect(canvas.getByText("200").className).toContain("font-mono");
    await expect(canvas.getByText("no response").className).not.toContain("font-mono");
  },
};
