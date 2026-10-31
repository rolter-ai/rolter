import type { Meta, StoryObj } from "@storybook/react";
import { MemoryRouter } from "react-router";
import { expect, within } from "storybook/test";

import {
  Harness,
  expectAnalyticsUnavailable,
  expectEmptyState,
  expectListTable,
  expectLoadError,
  expectNoFalseEmpty,
  expectSkeleton,
  json,
  pending,
  routes,
  scoped,
} from "@/pages/story-harness";
import type { ParamAdjustmentRow } from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";

import { ParamAdjustmentsPanel } from "./ParamAdjustmentsPanel";

const A = en.pages.compatibility.adjustments;

const ROWS: ParamAdjustmentRow[] = [
  {
    route: "claude-sonnet",
    provider: "anthropic",
    disposition: "dropped",
    param: "presence_penalty",
    requests: "40",
    route_requests: "100",
    share: "0.4",
  },
  {
    route: "gemini-flash",
    provider: "gemini",
    disposition: "translated",
    param: "tool_choice",
    requests: 3,
    route_requests: 12,
    share: 0.25,
  },
];

const answering = (body: unknown) => routes([["/api/v1/analytics/param-adjustments", () => body]]);

const meta = {
  title: "Components/ParamAdjustmentsPanel",
  component: ParamAdjustmentsPanel,
} satisfies Meta<typeof ParamAdjustmentsPanel>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * #3323: the share of each route's traffic that had a parameter dropped,
 * clamped or approximated, so an operator sees it once instead of each caller
 * finding out alone.
 */
export const ListsEachParameterWithItsShare: Story = {
  render: () => (
    <MemoryRouter>
      <Harness fetchStub={answering({ data: ROWS, recorded: true })}>
        <ParamAdjustmentsPanel />
      </Harness>
    </MemoryRouter>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("presence_penalty");
    await expectListTable(canvasElement, A.title);
    await expect(canvas.getByText("claude-sonnet")).toBeVisible();
    await expect(canvas.getByText(en.analytics.paramAdjustments.dropped)).toBeVisible();
    await expect(canvas.getByText("40.0% (40 of 100)")).toBeVisible();
    // a disposition is worded, never the raw name for a known one
    await expect(canvas.getByText(en.analytics.paramAdjustments.translated)).toBeVisible();
    await expect(canvas.getByText("25.0% (3 of 12)")).toBeVisible();
  },
};

export const EveryParameterWentThrough: Story = {
  render: () => (
    <MemoryRouter>
      <Harness fetchStub={answering({ data: [], recorded: true })}>
        <ParamAdjustmentsPanel />
      </Harness>
    </MemoryRouter>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, new RegExp(A.emptyTitle));
  },
};

/**
 * Without `clickhouse/022_param_adjustments.sql` an empty list would read as
 * "nothing was dropped", which is not known: the panel says they are not kept.
 */
export const NotRecordedIsNotNothingDropped: Story = {
  render: () => (
    <MemoryRouter>
      <Harness fetchStub={answering({ data: [], recorded: false })}>
        <ParamAdjustmentsPanel />
      </Harness>
    </MemoryRouter>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectEmptyState(canvasElement, new RegExp(A.notRecordedTitle));
    await expect(canvas.getByText("clickhouse/022_param_adjustments.sql").tagName).toBe("CODE");
    await expectNoFalseEmpty(canvasElement, new RegExp(A.emptyTitle));
  },
};

export const Loading: Story = {
  render: () => (
    <MemoryRouter>
      <Harness fetchStub={pending}>
        <ParamAdjustmentsPanel />
      </Harness>
    </MemoryRouter>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, new RegExp(A.emptyTitle));
  },
};

export const ReadFailed: Story = {
  render: () => (
    <MemoryRouter>
      <Harness
        fetchStub={scoped(async () => json({ error: { message: "clickhouse timed out" } }, 502))}
      >
        <ParamAdjustmentsPanel />
      </Harness>
    </MemoryRouter>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, new RegExp(en.errors.resources.paramAdjustments));
    await expectNoFalseEmpty(canvasElement, new RegExp(A.emptyTitle));
  },
};

export const NoAnalyticsStore: Story = {
  render: () => (
    <MemoryRouter>
      <Harness
        fetchStub={scoped(async () =>
          json({ error: { message: "analytics is not configured" } }, 503),
        )}
      >
        <ParamAdjustmentsPanel />
      </Harness>
    </MemoryRouter>
  ),
  play: async ({ canvasElement }) => {
    await expectAnalyticsUnavailable(
      canvasElement,
      A.noAnalytics.title,
      "analytics is not configured",
    );
  },
};
