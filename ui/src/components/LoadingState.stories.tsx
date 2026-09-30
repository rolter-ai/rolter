import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, within } from "storybook/test";

import {
  CardGridSkeleton,
  FormSkeleton,
  ListSkeleton,
  PanelSkeleton,
  StatGridSkeleton,
  TableSkeleton,
} from "./LoadingState";
import { STAT_GRID, StatCard } from "./ui/stat-card";
import en from "@/lib/i18n/locales/en.json";
import { atMobile, atTablet } from "@/lib/story-viewport";

// asserted against the catalog rather than a repeated string: rewording the
// label must not leave these stories checking copy the dashboard dropped
const LOADING = en.common.loading;

const meta = {
  title: "Feedback/LoadingState",
  component: ListSkeleton,
  parameters: { layout: "padded" },
} satisfies Meta<typeof ListSkeleton>;

export default meta;
type Story = StoryObj<typeof meta>;

export const List: Story = {
  render: () => <ListSkeleton rows={5} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("status")).toHaveAttribute("aria-busy", "true");
  },
};

export const CardGrid: Story = {
  render: () => <CardGridSkeleton cards={3} height={186} min={380} />,
};

export const Form: Story = { render: () => <FormSkeleton fields={5} /> };

export const Panels: Story = { render: () => <PanelSkeleton panels={2} height={160} /> };

export const TableRows: Story = { render: () => <TableSkeleton rows={6} /> };

export const StatGrid: Story = { render: () => <StatGridSkeleton /> };

/**
 * One announcement per shape, not one per bar. A screen reader hearing
 * "loading" four times for four placeholder rows learns nothing it did not
 * learn the first time.
 */
export const AnnouncesOnce: Story = {
  render: () => <ListSkeleton rows={6} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getAllByLabelText(LOADING)).toHaveLength(1);
  },
};

/**
 * Every shape carries the same label, which is what lets a screen story assert
 * "this screen is busy" without reaching for a class name (`expectSkeleton` in
 * `story-harness.tsx` is exactly this query).
 */
export const EveryShapeIsLabelled: Story = {
  render: () => (
    <div className="space-y-6">
      <ListSkeleton rows={2} />
      <CardGridSkeleton cards={2} />
      <FormSkeleton fields={2} />
      <PanelSkeleton panels={1} />
      <TableSkeleton rows={2} />
      <StatGridSkeleton cards={2} />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getAllByLabelText(LOADING)).toHaveLength(6);
  },
};

/** a strip of four figures in the grid a screen lays them in, under its skeleton */
const StripAndSkeleton = () => (
  <div className="space-y-6">
    <div data-testid="strip" className={STAT_GRID}>
      {["Requests", "Spend", "Avg latency", "Error rate"].map((label) => (
        <StatCard key={label} label={label} value="132" />
      ))}
    </div>
    <StatGridSkeleton cards={4} />
  </div>
);

/** the tracks of the strip and of its skeleton, as laid out at this width */
async function expectSameTracks(canvasElement: HTMLElement) {
  const canvas = within(canvasElement);
  const strip = getComputedStyle(canvas.getByTestId("strip")).gridTemplateColumns;
  const skeleton = getComputedStyle(canvas.getByRole("status")).gridTemplateColumns;
  await expect(skeleton).toBe(strip);
}

/**
 * #1994: the skeleton laid its cards out in `auto-fill` tracks of 220px while
 * the strip was one, two or four columns by breakpoint, so the tiles changed
 * columns, and the page moved, the moment the figures landed. Both are the same
 * grid now, at every width.
 */
export const StatGridKeepsTheColumnsOfTheLoadedStrip: Story = {
  render: () => <StripAndSkeleton />,
  play: async ({ canvasElement }) => expectSameTracks(canvasElement),
};

export const StatGridKeepsTheColumnsOfTheLoadedStripOnATablet: Story = {
  ...atTablet,
  render: () => <StripAndSkeleton />,
  play: async ({ canvasElement }) => {
    await expectSameTracks(canvasElement);
    // two across at this width: auto-fill would have made it three
    const canvas = within(canvasElement);
    await expect(
      getComputedStyle(canvas.getByRole("status")).gridTemplateColumns.split(" "),
    ).toHaveLength(2);
  },
};

export const StatGridKeepsTheColumnsOfTheLoadedStripOnAPhone: Story = {
  ...atMobile,
  render: () => <StripAndSkeleton />,
  play: async ({ canvasElement }) => expectSameTracks(canvasElement),
};
