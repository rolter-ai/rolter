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
import en from "@/lib/i18n/locales/en.json";

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
