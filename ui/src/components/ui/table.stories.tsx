import type { Meta, StoryObj } from "@storybook/react";
import { expect, within } from "storybook/test";

import { Button } from "./button";
import { EmptyState } from "./empty-state";
import { Table, type TableColumn } from "./table";
import { ANSWERED, type ReadState } from "@/lib/read-state";
import { atMobile, expectInFrame } from "@/lib/story-viewport";
import { expectTableStateInFrame } from "@/pages/story-harness";

interface Row extends Record<string, unknown> {
  id: string;
  model: string;
  provider: string;
  requests: number;
  p95: string;
}

const ROWS: Row[] = [
  { id: "r1", model: "gpt-4o", provider: "openai-prod", requests: 18422, p95: "812 ms" },
  { id: "r2", model: "claude-sonnet", provider: "anthropic-prod", requests: 9310, p95: "1.2 s" },
  { id: "r3", model: "llama-3.1-70b", provider: "vllm-cluster", requests: 4180, p95: "340 ms" },
];

const COLUMNS: TableColumn<Row>[] = [
  { key: "model", header: "Model", mono: true },
  { key: "provider", header: "Provider" },
  { key: "requests", header: "Requests", align: "right" },
  { key: "p95", header: "p95", align: "right", mono: true },
];

const meta = {
  title: "Display/Table",
  component: Table,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Table>;

export default meta;

// `Table` is generic in its row type, so `StoryObj<typeof meta>` would type
// `args` against the `Record<string, unknown>` constraint rather than against
// `Row` and reject the fixture below. Every story renders its own table, so the
// story type only has to allow a bare `render`.
type Story = StoryObj;

export const Default: Story = {
  render: () => <Table columns={COLUMNS} data={ROWS} rowKey="id" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // header cells are real column headers, not styled divs: that is what lets
    // a screen reader say which column a value is in
    await expect(canvas.getByRole("columnheader", { name: "Model" })).toBeVisible();
    await expect(canvas.getAllByRole("row")).toHaveLength(ROWS.length + 1);
    await expect(canvas.getByText("llama-3.1-70b")).toBeVisible();
  },
};

/**
 * A `render` per column, for cells that are more than the raw value — this is
 * how the dashboard puts badges, links and copy buttons inside a table without
 * every screen re-deriving the markup.
 */
export const RenderedCells: Story = {
  render: () => (
    <Table
      columns={[
        ...COLUMNS.slice(0, 2),
        {
          key: "requests",
          header: "Requests",
          align: "right",
          render: (value) => (
            <span className="font-mono text-xs">{(value as number).toLocaleString("en-US")}</span>
          ),
        },
      ]}
      data={ROWS}
      rowKey="id"
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("18,422")).toBeVisible();
  },
};

/**
 * Loaded and empty (#1180). The placeholder is rendered in one full-width cell
 * so it sits inside the table's border with the column headers still above it:
 * a header row over nothing reads as a screen that is still loading.
 */
export const Empty: Story = {
  render: () => (
    <Table
      columns={COLUMNS}
      data={[]}
      read={ANSWERED}
      empty={
        <EmptyState
          title="No traffic in this window"
          description="Widen the time range, or send a request through the gateway to see it here."
        />
      }
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("No traffic in this window")).toBeVisible();
    // the columns survive the empty state — they say what a row would carry
    await expect(canvas.getByRole("columnheader", { name: "Provider" })).toBeVisible();
  },
};

// a table whose columns need more than a phone's card gives them, so it scrolls
// sideways inside its frame. headers do not wrap, so their text is the floor
// each column is held to: `width` alone is a preference a narrow table gives up
const WIDE_COLUMNS: TableColumn<Row>[] = [
  { key: "model", header: "Model identifier", mono: true },
  { key: "provider", header: "Provider account" },
  { key: "requests", header: "Requests in the last hour", align: "right" },
  { key: "p95", header: "p95 latency in milliseconds", align: "right", mono: true },
];

const NoTraffic = () => (
  <EmptyState
    title="No traffic in this window"
    description="Widen the time range, or send a request through the gateway to see it here."
    actions={<Button>Send a request</Button>}
  />
);

const EmptyWide = ({ columns = WIDE_COLUMNS }: { columns?: TableColumn<Row>[] }) => (
  <Table columns={columns} data={[]} read={ANSWERED} empty={<NoTraffic />} />
);

/** The frame, the box the placeholder sits in, and the first header cell, read off the title. */
function parts(canvasElement: HTMLElement) {
  const title = within(canvasElement).getByText("No traffic in this window");
  const table = title.closest("table")!;
  return {
    title,
    table,
    frame: table.parentElement!,
    state: title.closest("td")!.firstElementChild as HTMLElement,
    header: table.querySelector("th")!,
  };
}

/**
 * Below its columns' width the table scrolls sideways inside its card, and the
 * placeholder's cell spans the whole table (#2420). Centred on it, the title
 * and the button sat past the card's right edge, or off to one side of it. The
 * placeholder is as wide as the frame the reader sees instead, so the title,
 * the description and the button are inside it and centred in it.
 */
export const EmptyFitsThePhone: Story = {
  ...atMobile,
  render: () => <EmptyWide />,
  play: async ({ canvasElement }) => {
    await expectTableStateInFrame(canvasElement, {
      says: /No traffic in this window/,
      body: /Widen the time range/,
      cta: /Send a request/,
    });
    const { frame } = parts(canvasElement);
    // the premise: this table really is wider than the frame that shows it
    await expect(frame.scrollWidth).toBeGreaterThan(frame.clientWidth);
  },
};

/**
 * The header and the columns keep the width their content needs, so they scroll
 * under the frame; the placeholder is stuck to the frame's left edge and stays
 * in front of a reader who scrolls the table to its end.
 */
export const EmptyStaysInFrameWhenTheTableScrolls: Story = {
  ...atMobile,
  render: () => <EmptyWide />,
  play: async ({ canvasElement }) => {
    const { title, frame, state, header } = parts(canvasElement);
    const edge = frame.getBoundingClientRect().left + frame.clientLeft;
    await expect(state.getBoundingClientRect().width).toBeCloseTo(frame.clientWidth, 0);
    const before = header.getBoundingClientRect().left;

    frame.scrollLeft = frame.scrollWidth;
    await expect(frame.scrollLeft).toBeGreaterThan(0);
    // the columns have moved under the frame and the placeholder has not
    await expect(header.getBoundingClientRect().left).toBeLessThan(before);
    await expect(state.getBoundingClientRect().left).toBeCloseTo(edge, 0);
    const range = document.createRange();
    range.selectNodeContents(title);
    await expectInFrame(range, frame);
  },
};

/**
 * Where the columns fit nothing scrolls, and the placeholder is the width of the
 * table and of its header row: one left edge, one right edge, so it stays
 * centred under the columns it stands in for.
 */
export const EmptySpansTheTableAtDesktopWidth: Story = {
  render: () => <EmptyWide columns={COLUMNS} />,
  play: async ({ canvasElement }) => {
    const { table, frame, state, header } = parts(canvasElement);
    await expect(frame.scrollWidth).toBe(frame.clientWidth);
    const box = table.getBoundingClientRect();
    await expect(state.getBoundingClientRect().left).toBeCloseTo(box.left, 0);
    await expect(state.getBoundingClientRect().right).toBeCloseTo(box.right, 0);
    await expect(header.closest("tr")!.getBoundingClientRect().width).toBeCloseTo(box.width, 0);
  },
};

// the two reads that hold no rows without having answered "none"
const FAILED: ReadState = { isPending: false, isSuccess: false, fetchStatus: "idle" };
const IN_FLIGHT: ReadState = { isPending: true, isSuccess: false, fetchStatus: "fetching" };

/**
 * The placeholder waits for the read to succeed (#2211). A read that failed, or
 * one still in flight, holds no rows either — and "No traffic in this window"
 * under a load error states an outage as a quiet day. The screen's own
 * `LoadError` or skeleton says what is going on; the table says nothing.
 */
export const NoPlaceholderUntilTheReadSucceeds: Story = {
  render: () => (
    <div className="flex flex-col gap-4">
      {[FAILED, IN_FLIGHT].map((read, i) => (
        <Table
          key={i}
          aria-label={i === 0 ? "Failed read" : "Read in flight"}
          columns={COLUMNS}
          data={[]}
          read={read}
          empty={<EmptyState title="No traffic in this window" />}
        />
      ))}
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.queryByText("No traffic in this window")).not.toBeInTheDocument();
    // both tables keep their header row and add nothing under it
    await expect(canvas.getAllByRole("row")).toHaveLength(2);
  },
};

/**
 * With no `empty` prop and no rows the table is a bare header, which is the
 * shape #1180 was filed against. Kept as a story so the difference from
 * `Empty` above is visible side by side rather than argued about.
 */
export const EmptyWithoutPlaceholder: Story = {
  render: () => <Table columns={COLUMNS} data={[]} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getAllByRole("row")).toHaveLength(1);
  },
};

/**
 * The scroll container is focusable: a table wider than its panel would
 * otherwise hide its right-hand columns from anyone not using a mouse (#1181).
 */
export const ScrollsFromTheKeyboard: Story = {
  render: () => (
    <div className="max-w-[320px]">
      <Table columns={COLUMNS} data={ROWS} rowKey="id" />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const scroller = canvasElement.querySelector<HTMLElement>("[tabindex='0']");
    await expect(scroller).not.toBeNull();
    scroller?.focus();
    await expect(scroller).toHaveFocus();
  },
};
