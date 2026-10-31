import type { Meta, StoryObj } from "@storybook/react";
import { expect, within } from "storybook/test";

import { CacheReportingBadge } from "./CacheReportingBadge";

const meta = {
  title: "Components/CacheReportingBadge",
  component: CacheReportingBadge,
  parameters: { layout: "padded" },
} satisfies Meta<typeof CacheReportingBadge>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * Reports, does not report and never probed: three wordings and three tones.
 * Unknown must not wear the warning tone of "no", and it carries no time since
 * it rests on no verdict.
 */
export const ThreeStates: Story = {
  args: { state: null },
  render: () => (
    <div className="flex flex-wrap gap-2">
      <CacheReportingBadge
        state={{
          status: "reports",
          probedAt: "2026-10-01T10:00:00Z",
          model: "gpt-4o-mini",
          spelling: "cached_tokens",
          count: 1,
          total: 1,
        }}
      />
      <CacheReportingBadge
        state={{
          status: "none",
          probedAt: "2026-09-01T10:00:00Z",
          model: "claude-haiku",
          spelling: null,
          count: 1,
          total: 1,
        }}
      />
      <CacheReportingBadge state={{ status: "unprobed", count: 0, total: 1 }} />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const reports = canvas.getByText(/^Reports cache/);
    const none = canvas.getByText(/^No cache metrics/);
    const unprobed = canvas.getByText("Not probed yet");
    await expect(reports.className).toContain("status-success");
    await expect(none.className).toContain("status-warning");
    await expect(unprobed.className).not.toContain("status-warning");
    await expect(reports).toHaveAttribute("title", expect.stringContaining("Model: gpt-4o-mini"));
    await expect(reports).toHaveAttribute("title", expect.stringContaining("Field: cached_tokens"));
  },
};

/** Several providers behind one route: the title says how many the claim covers. */
export const CoversSeveralProviders: Story = {
  args: {
    state: { status: "none", probedAt: "2026-09-01T10:00:00Z", count: 1, total: 3 },
  },
  play: async ({ canvasElement }) => {
    await expect(within(canvasElement).getByText(/^No cache metrics/)).toHaveAttribute(
      "title",
      expect.stringContaining("1 of 3 providers"),
    );
  },
};

/** Nothing known (verdicts unread) draws nothing at all. */
export const UnreadRendersNothing: Story = {
  args: { state: null },
  play: async ({ canvasElement }) => {
    await expect(canvasElement.textContent).toBe("");
  },
};
