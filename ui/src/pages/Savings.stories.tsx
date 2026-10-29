import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Savings from "./Savings";
import {
  expectAnalyticsUnavailable,
  expectEmptyState,
  expectForbidden,
  expectListTable,
  expectLoadError,
  expectNoFalseEmpty,
  expectSkeleton,
  Harness,
  json,
  openOptions,
  pending,
  pickOption,
  recording,
  PROJECT,
  routes,
  scoped,
  TEAM,
  type FetchStub,
} from "./story-harness";
import type { SavingsReport } from "@/lib/api";
import { setChartTimeZone } from "@/lib/i18n/format";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { atMobile, expectNoHorizontalOverflow } from "@/lib/story-viewport";

const REPORT: SavingsReport = {
  currency: "USD",
  limits: [
    "This report re-prices recorded traffic at another model's price. It does not evaluate whether that model answers as well, so it is a shortlist to test, never a recommendation to switch.",
    "Only models already configured in the same route's pool are considered.",
  ],
  assumptions: { max_prompt_tokens: 4000, max_completion_tokens: 1000, output_margin: 0.25 },
  spend: {
    requests: 12000,
    cost_usd: 480.25,
    unpriced_requests: 300,
    multi_attempt: { requests: 640, cost_usd: 31.5 },
    failed: { requests: 120, cost_usd: 4.75 },
    cache: { hits: 900, misses: 5000, miss_cost_usd: 210 },
    by_route: [
      { route: "chat", requests: 9000, cost_usd: 400, unpriced_requests: 0 },
      { route: "summarize", requests: 3000, cost_usd: 80.25, unpriced_requests: 300 },
    ],
    by_model: [
      { model: "gpt-4o", requests: 9000, cost_usd: 400, unpriced_requests: 0 },
      { model: "mystery-1", requests: 3000, cost_usd: 80.25, unpriced_requests: 300 },
    ],
    by_caller: [
      {
        project_id: "project-1",
        virtual_key_id: "vk-1",
        requests: 9000,
        cost_usd: 400,
        unpriced_requests: 0,
      },
    ],
  },
  substitutable: {
    requests: 4000,
    current_cost_usd: 160,
    projected_savings_usd: { low: 12.5, high: 98.25 },
  },
  candidates: [
    {
      route: "chat",
      current_model: "gpt-4o",
      requests: 4000,
      current_cost_usd: 160,
      candidates: [
        {
          model: "gpt-4o-mini",
          projected_cost_usd: { low: 9.75, high: 147.5 },
          savings_usd: { low: 12.5, high: 150.25 },
        },
        {
          model: "claude-haiku",
          projected_cost_usd: { low: 20, high: 190 },
          savings_usd: { low: -30, high: 140 },
        },
      ],
    },
  ],
  excluded: {
    requests: {
      unpriced: 300,
      failed: 120,
      prompt_too_large: 800,
      multi_turn: 5000,
      uses_tools: 1200,
      brand_new_reason: 3,
    },
    unknown_candidate_prices: ["mystery-1"],
  },
};

const withReport = (report: SavingsReport) =>
  routes([
    ["/analytics/savings", () => report],
    ["/virtual-keys", () => []],
    ["/models", () => [{ model: "chat", strategy: "weighted", targets: 1 }]],
  ]);

const meta = {
  title: "Screens/Savings",
  component: Savings,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Savings>;
export default meta;
type Story = StoryObj<typeof meta>;

/**
 * #3316: the limits come first, every projected cost and saving is a range
 * (the pessimistic saving can be negative), and a model with no price is named
 * with a link to the pricing editor.
 */
export const Populated: Story = {
  render: () => (
    <Harness fetchStub={withReport(REPORT)}>
      <Savings />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const limits = await canvas.findByTestId("savings-limits");
    await expect(limits).toHaveTextContent(/never a recommendation to switch/);
    await expect(limits).toHaveTextContent(en.pages.savings.limitsTitle);
    await expectListTable(canvasElement, "Spend by route");
    await expectListTable(canvasElement, "chat · gpt-4o");
    await expect(canvas.getByText("$9.75 to $147.50")).toBeVisible();
    await expect(canvas.getByText("-$30.00 to $140.00")).toBeVisible();
    await expect(canvas.getByTestId("savings-substitutable")).toHaveTextContent(
      "saving $12.50 to $98.25",
    );
    // a lone figure would read as a promise; only the recorded cost is one
    await expect(canvas.queryByText("$9.75")).toBeNull();
    await expect(canvas.getByText("Prompt over the threshold")).toBeVisible();
    await expect(canvas.getByText("Was part of a conversation")).toBeVisible();
    await expect(canvas.getByText("Used tools")).toBeVisible();
    // a reason this dashboard has no copy for is shown as the control plane names it
    await expect(canvas.getByText("brand_new_reason")).toBeVisible();
    const unknown = canvas.getByTestId("savings-unknown-prices");
    await expect(within(unknown).getByRole("link", { name: "mystery-1" })).toHaveAttribute(
      "href",
      "/pricing-overrides?model=mystery-1",
    );
  },
};

export const PopulatedAtMobile: Story = {
  ...atMobile,
  render: () => (
    <Harness fetchStub={withReport(REPORT)}>
      <Savings />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByTestId("savings-limits");
    await expectNoHorizontalOverflow();
  },
};

export const PopulatedInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={withReport(REPORT)}>
      <Savings />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(ru.pages.savings.limitsTitle)).toBeVisible();
    await expect(canvas.getByText(ru.pages.savings.reasons.prompt_too_large)).toBeVisible();
  },
};

const EMPTY: SavingsReport = {
  ...REPORT,
  spend: {
    ...REPORT.spend,
    requests: 0,
    cost_usd: 0,
    unpriced_requests: 0,
    by_route: [],
    by_model: [],
    by_caller: [],
  },
  substitutable: { requests: 0, current_cost_usd: 0, projected_savings_usd: { low: 0, high: 0 } },
  candidates: [],
  excluded: { requests: {}, unknown_candidate_prices: [] },
};

export const EmptyWindow: Story = {
  render: () => (
    <Harness fetchStub={withReport(EMPTY)}>
      <Savings />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No requests in this window/, /30 days/);
  },
};

export const NoCandidates: Story = {
  render: () => (
    <Harness
      fetchStub={withReport({
        ...REPORT,
        candidates: [],
        excluded: { ...REPORT.excluded, unknown_candidate_prices: [] },
      })}
    >
      <Savings />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("No cheaper model would have fit")).toBeVisible();
    await expect(canvas.queryByTestId("savings-unknown-prices")).toBeNull();
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <Savings />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    // the limits are the dashboard's own words, so they are there before the figures
    await expect(within(canvasElement).getByText(en.pages.savings.limitsLead)).toBeVisible();
  },
};

export const Error_: Story = {
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "boom" } }, 500))}>
      <Savings />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /savings report/i);
    await expectNoFalseEmpty(canvasElement, /no requests in this window/i);
  },
};

/** a deployment without ClickHouse is a supported shape, said calmly (#2016) */
export const NoAnalyticsStore: Story = {
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "no clickhouse_url" } }, 503))}>
      <Savings />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectAnalyticsUnavailable(
      canvasElement,
      en.pages.savings.noAnalytics.title,
      "no clickhouse_url",
    );
  },
};

/** a caller the `savings` capability refuses is told so, not shown a broken report */
export const Gated: Story = {
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "forbidden" } }, 403))}>
      <Savings />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

const asked = recording(withReport(REPORT));

/** the scope and thresholds in the address are what the read carries, over seven days unless told otherwise */
export const SendsTheScopeAndThresholds: Story = {
  render: () => (
    <Harness
      fetchStub={asked.stub}
      route="/savings?route=chat&project=project-1&max_prompt_tokens=2000&output_margin=0.5"
    >
      <Savings />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByTestId("savings-limits");
    await waitFor(() => {
      const call = asked.calls.find((c) => c.url.includes("/analytics/savings"));
      expect(call).toBeDefined();
      const url = new URL(call!.url, "http://x");
      expect(url.searchParams.get("route")).toBe("chat");
      expect(url.searchParams.get("project")).toBe("project-1");
      expect(url.searchParams.get("max_prompt_tokens")).toBe("2000");
      expect(url.searchParams.get("output_margin")).toBe("0.5");
      const days = (Date.now() - Date.parse(url.searchParams.get("since")!)) / 86_400_000;
      expect(Math.round(days)).toBe(7);
    });
  },
};

const recalculating = recording(withReport(REPORT));

/** the thresholds apply together, and a bad one is said before anything is sent */
export const RecalculatesWithNewThresholds: Story = {
  render: () => (
    <Harness fetchStub={recalculating.stub}>
      <Savings />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByTestId("savings-limits");
    const prompt = canvas.getByLabelText("Longest prompt (tokens)");
    await userEvent.type(prompt, "abc");
    await userEvent.click(canvas.getByRole("button", { name: "Recalculate" }));
    await expect(await canvas.findByText(/Enter a whole number of 1 or more/)).toBeVisible();
    await userEvent.clear(prompt);
    await userEvent.type(prompt, "2000");
    await userEvent.type(canvas.getByLabelText("Output margin"), "0.5");
    await userEvent.click(canvas.getByRole("button", { name: "Recalculate" }));
    await waitFor(() =>
      expect(
        recalculating.calls.some(
          (c) => c.url.includes("max_prompt_tokens=2000") && c.url.includes("output_margin=0.5"),
        ),
      ).toBe(true),
    );
  },
};

const windowed = recording(withReport(REPORT));

export const PickingAWindowReadsItAgain: Story = {
  render: () => (
    <Harness fetchStub={windowed.stub}>
      <Savings />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByTestId("savings-limits");
    await expect(canvas.getByRole("combobox", { name: "Time window" })).toHaveValue("Last 7 days");
    await pickOption(canvas.getByRole("combobox", { name: "Time window" }), "Last 30 days");
    await waitFor(() =>
      expect(
        windowed.calls.some((c) => {
          const since = new URL(c.url, "http://x").searchParams.get("since");
          return (
            c.url.includes("/analytics/savings") &&
            since !== null &&
            Math.round((Date.now() - Date.parse(since)) / 86_400_000) === 30
          );
        }),
      ).toBe(true),
    );
  },
};

const ranged = recording(withReport(REPORT));

/**
 * The advisor reads a custom range of days from the address (#3407): the read
 * is sent the start of the first day and the start of the day after the last,
 * and the picker shows the days.
 */
export const ACustomRangeInTheAddressIsRead: Story = {
  render: () => (
    <Harness
      fetchStub={ranged.stub}
      route="/savings?window=custom&since=2026-10-01&until=2026-10-07"
    >
      <Savings />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByTestId("savings-limits");
    await expect(canvas.getByRole("combobox", { name: "Time window" })).toHaveValue(
      "Custom range…",
    );
    await expect(canvas.getByLabelText("From")).toHaveValue("2026-10-01");
    await expect(canvas.getByLabelText("To")).toHaveValue("2026-10-07");
    await waitFor(() =>
      expect(
        ranged.calls.some((c) => {
          const q = new URL(c.url, "http://x").searchParams;
          return (
            c.url.includes("/analytics/savings") &&
            q.get("since") === new Date(2026, 9, 1).toISOString() &&
            q.get("until") === new Date(2026, 9, 8).toISOString()
          );
        }),
      ).toBe(true),
    );
  },
};

const zoned = recording(withReport(REPORT));

/** puts the chart zone back to the browser's when the story unmounts */
function RestoresChartZone({ children }: { children: React.ReactNode }) {
  React.useEffect(() => () => setChartTimeZone(null), []);
  return <>{children}</>;
}

// fourteen hours ahead of UTC with no daylight saving: never the zone a browser
// running the stories is in
const FAR_ZONE = "Pacific/Kiritimati";
const FAR_OFFSET = 14 * 3_600_000;

/**
 * #3519: the month so far starts at the chart zone's midnight, so a change of
 * **Time zone** in Preferences with the month on screen is a new read whose
 * `since` is the new zone's first of the month, not the cached answer read
 * over the old zone's.
 */
export const ChangingTheChartZoneReadsTheMonthAgain: Story = {
  render: () => (
    <RestoresChartZone>
      <Harness fetchStub={zoned.stub} route="/savings?window=mtd">
        <Savings />
      </Harness>
    </RestoresChartZone>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByTestId("savings-limits");
    await expect(canvas.getByRole("combobox", { name: "Time window" })).toHaveValue(
      "Month to date",
    );
    const reads = () =>
      zoned.calls
        .filter((c) => c.url.includes("/analytics/savings"))
        .map((c) => new URL(c.url, "http://x").searchParams.get("since"));
    await waitFor(() => expect(reads().length).toBeGreaterThan(0));
    const before = reads().length;

    setChartTimeZone(FAR_ZONE);
    // the first of this month on the zone's own calendar, at its midnight
    const there = new Date(Date.now() + FAR_OFFSET);
    const midnight = new Date(
      Date.UTC(there.getUTCFullYear(), there.getUTCMonth(), 1) - FAR_OFFSET,
    ).toISOString();
    await waitFor(() => expect(reads().length).toBeGreaterThan(before));
    await expect(reads()[reads().length - 1]).toBe(midnight);
    await expect(reads()[0]).not.toBe(midnight);
  },
};

const NAMED_CALLERS: SavingsReport = {
  ...REPORT,
  spend: {
    ...REPORT.spend,
    by_caller: [
      {
        project_id: "project-1",
        virtual_key_id: "vk-1",
        requests: 9000,
        cost_usd: 400,
        unpriced_requests: 0,
      },
      // a project and a key the account's lists do not hold: deleted, or not readable
      {
        project_id: "project-gone",
        virtual_key_id: "vk-gone",
        requests: 3000,
        cost_usd: 80.25,
        unpriced_requests: 300,
      },
    ],
  },
};

/**
 * The spend-by-caller table names the project and the key from the lists the
 * pickers already load (#3408), and a caller those lists do not hold stays an
 * id, so a deleted key or a project the account cannot read is still shown.
 */
export const SpendByCallerNamesTheProjectAndKey: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/analytics/savings", () => NAMED_CALLERS],
        [
          "/virtual-keys",
          () => [
            {
              id: "vk-1",
              project_id: "project-1",
              name: "ci-runner",
              key_prefix: "rk_live_ab12",
              models: [],
              providers: [],
              disabled: false,
            },
          ],
        ],
        ["/models", () => [{ model: "chat", strategy: "weighted", targets: 1 }]],
      ])}
    >
      <Savings />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByTestId("savings-limits");
    await expectListTable(canvasElement, "Spend by caller (50 costliest)");
    const table = canvas.getByRole("table", { name: "Spend by caller (50 costliest)" });
    const rows = within(table).getAllByRole("row");
    // header, then one row per caller
    const [named, unnamed] = [rows[1]!, rows[2]!];
    // the names arrive with the pickers' lists, a step after the report
    await expect(await within(named).findByRole("cell", { name: "Gateway" })).toBeVisible();
    await expect(
      await within(named).findByRole("cell", { name: "ci-runner (rk_live_ab12…)" }),
    ).toBeVisible();
    await expect(within(named).queryByText("project-1")).toBeNull();
    await expect(within(named).queryByText("vk-1")).toBeNull();
    // the fallback is the id as the report sent it
    await expect(within(unnamed).getByRole("cell", { name: "project-gone" })).toBeVisible();
    await expect(within(unnamed).getByRole("cell", { name: "vk-gone" })).toBeVisible();
  },
};

const TWO_PROJECT_CALLERS: SavingsReport = {
  ...REPORT,
  spend: {
    ...REPORT.spend,
    by_caller: [
      {
        project_id: "project-1",
        virtual_key_id: "vk-1",
        key_name: "ci-runner",
        requests: 9000,
        cost_usd: 400,
        unpriced_requests: 0,
      },
      // a project the key picker does not list, since it reads one project's keys
      {
        project_id: "project-2",
        virtual_key_id: "vk-2",
        key_name: "batch-jobs",
        requests: 4000,
        cost_usd: 120,
        unpriced_requests: 0,
      },
      // a key since deleted: the report has no name for it either
      {
        project_id: "project-2",
        virtual_key_id: "vk-gone",
        key_name: null,
        requests: 10,
        cost_usd: 1,
        unpriced_requests: 0,
      },
    ],
  },
};

/**
 * #3499: with no project picked, the table holds callers of several projects.
 * Their keys are named by the report itself, from one read over all of them,
 * so the key of a project the key picker does not list is named too; the
 * picker reads only the scope's project. A key that is gone stays an id.
 */
export const SpendByCallerNamesTheKeysOfEveryProject: Story = {
  render: () => {
    const rest = routes([
      ["/analytics/savings", () => TWO_PROJECT_CALLERS],
      // the one project the picker reads holds only the first key
      [
        "/virtual-keys",
        () => [
          {
            id: "vk-1",
            project_id: "project-1",
            name: "ci-runner",
            key_prefix: "rk_live_ab12",
            models: [],
            providers: [],
            disabled: false,
          },
        ],
      ],
      ["/models", () => [{ model: "chat", strategy: "weighted", targets: 1 }]],
    ]);
    const stub: FetchStub = async (input, init) => {
      const path = new URL(String(input), "http://localhost").pathname;
      // the org holds two projects, so both callers' projects are named
      if (/^\/api\/v1\/orgs\/[^/]+\/projects$/.test(path)) {
        return json([
          { ...PROJECT, team_name: TEAM.name },
          { ...PROJECT, id: "project-2", name: "Batch", team_name: TEAM.name },
        ]);
      }
      return rest(input, init);
    };
    return (
      <Harness fetchStub={stub}>
        <Savings />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByTestId("savings-limits");
    const table = canvas.getByRole("table", { name: "Spend by caller (50 costliest)" });
    const [, first, second, gone] = within(table).getAllByRole("row");
    await expect(within(first!).getByRole("cell", { name: "ci-runner" })).toBeVisible();
    await expect(within(second!).getByRole("cell", { name: "batch-jobs" })).toBeVisible();
    await expect(await within(second!).findByRole("cell", { name: "Batch" })).toBeVisible();
    await expect(within(second!).queryByText("vk-2")).toBeNull();
    await expect(within(gone!).getByRole("cell", { name: "vk-gone" })).toBeVisible();
  },
};

/** a route name is unique per project only: the picker offers it once, not once per project */
export const ARouteNameSharedByTwoProjectsIsOfferedOnce: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/analytics/savings", () => REPORT],
        ["/virtual-keys", () => []],
        [
          "/models",
          () => [
            { model: "chat", strategy: "weighted", targets: 1, project_id: "project-1" },
            { model: "chat", strategy: "weighted", targets: 1, project_id: "project-2" },
          ],
        ],
      ])}
    >
      <Savings />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByTestId("savings-limits");
    const listbox = await openOptions(await canvas.findByLabelText("Route"));
    await expect(within(listbox).getAllByRole("option", { name: "chat" })).toHaveLength(1);
  },
};
