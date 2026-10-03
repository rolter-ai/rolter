import type { Meta, StoryObj } from "@storybook/react";
import { expect, within } from "storybook/test";

import { AnalyticsUnavailableError } from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";

import { AnalyticsUnavailable } from "./AnalyticsUnavailable";

const meta = {
  title: "Components/AnalyticsUnavailable",
  component: AnalyticsUnavailable,
  args: {
    i18nKey: "pages.dashboard.noAnalytics",
    error: new AnalyticsUnavailableError("no clickhouse_url"),
  },
} satisfies Meta<typeof AnalyticsUnavailable>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * A deployment with no analytics store answered, and nothing a retry can do
 * will change it. It is a `status`, not the `alert` a 500 gets, so a screen
 * reader is not told on every visit that something broke (#1984, #1976). The
 * setting to change is named in monospace, and there is no retry.
 */
export const OnTheDashboard: Story = {
  play: async ({ canvas }) => {
    const panel = await canvas.findByRole("status");
    await expect(panel).toHaveTextContent(en.pages.dashboard.noAnalytics.title);
    await expect(canvas.queryByRole("alert")).toBeNull();
    // the setting is named twice: what is missing, and what to set
    const names = within(panel).getAllByText("CLICKHOUSE_URL");
    await expect(names).toHaveLength(2);
    for (const name of names) await expect(name.tagName).toBe("CODE");
    // the control plane's own words stay under it (#962)
    await expect(within(panel).getByText("no clickhouse_url")).toBeVisible();
    await expect(canvas.queryByRole("button")).toBeNull();
  },
};

/** The same panel in another screen's words: the copy is the caller's, the shape is shared. */
export const OnLogs: Story = {
  args: { i18nKey: "pages.logs.noAnalytics" },
  play: async ({ canvas }) => {
    const panel = await canvas.findByRole("status");
    await expect(panel).toHaveTextContent(en.pages.logs.noAnalytics.title);
    await expect(canvas.queryByRole("alert")).toBeNull();
  },
};

/** Nothing to quote: a value that is not an error adds no line under the body. */
export const WithoutTheControlPlanesMessage: Story = {
  args: { error: null },
  play: async ({ canvas }) => {
    const panel = await canvas.findByRole("status");
    await expect(panel).toHaveTextContent(en.pages.dashboard.noAnalytics.title);
    await expect(within(panel).queryByText("no clickhouse_url")).toBeNull();
  },
};
