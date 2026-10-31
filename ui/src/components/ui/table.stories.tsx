import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { Button } from "./button";
import { EmptyState } from "./empty-state";
import { Table, type TableColumn } from "./table";
import { cn } from "@/lib/utils";
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
 * A table is named by its caption, and a column can be the row's header, so a
 * screen reader reads each value under both the column and the row it is in.
 */
export const NamedWithRowHeaders: Story = {
  render: () => (
    <Table
      caption="Models by usage"
      columns={[{ ...COLUMNS[0]!, rowHeader: true }, ...COLUMNS.slice(1)]}
      data={ROWS}
      rowKey="id"
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("table", { name: "Models by usage" })).toBeVisible();
    await expect(canvas.getAllByRole("rowheader")).toHaveLength(ROWS.length);
    await expect(canvas.getByRole("rowheader", { name: ROWS[0]!.model as string })).toBeVisible();
    // the last row draws no rule under any of its cells, the row header included
    const last = canvas.getByRole("rowheader", { name: ROWS[ROWS.length - 1]!.model as string });
    for (const cell of last.closest("tr")!.children) {
      await expect(getComputedStyle(cell).borderBottomWidth).toBe("0px");
    }
  },
};

/**
 * A row's detail opens in a full-width row directly under it (#2129), so the
 * reader never has to match a block below the table to the row it came from.
 */
export const DetailUnderItsRow: Story = {
  render: () => (
    <Table
      columns={COLUMNS}
      data={ROWS}
      rowKey="id"
      detailRow={(row) => (row.id === "r2" ? <p>Detail for {row.model}</p> : null)}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const detail = canvas.getByText("Detail for claude-sonnet");
    await expect(detail).toBeVisible();
    const rows = canvas.getAllByRole("row");
    // header, three data rows, one detail row
    await expect(rows).toHaveLength(5);
    await expect(rows[2]).toHaveTextContent("claude-sonnet");
    await expect(rows[3]).toContainElement(detail);
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

// a dense table the way a log draws it: its own width decides what shows, so it
// sits in an `@container` of a width the story picks
const DENSE: TableColumn<Row>[] = [
  { key: "model", header: "Model", colClassName: "w-1/2", className: "truncate" },
  {
    key: "provider",
    header: "Provider",
    colClassName: "hidden w-1/4 @min-[600px]:table-column",
    className: "hidden @min-[600px]:table-cell",
  },
  { key: "requests", header: "Requests", align: "right", colClassName: "w-1/4" },
];

function Dense({ width }: { width: number }) {
  return (
    <div className="@container max-h-48 overflow-auto" style={{ width }}>
      <Table
        bare
        compact
        columns={DENSE}
        data={ROWS}
        rowKey="id"
        caption="Requests"
        tableClassName="table-fixed"
      />
    </div>
  );
}

/**
 * A column hides by the table's own width: `colClassName` sizes (and hides) its
 * `<col>`, `className` its header and cells. Narrow, the provider is gone from
 * the table; wide, it is there.
 */
export const ColumnsGiveWayByTheTablesWidth: Story = {
  render: () => (
    <div className="space-y-4">
      <div data-testid="narrow">
        <Dense width={400} />
      </div>
      <div data-testid="wide">
        <Dense width={700} />
      </div>
    </div>
  ),
  play: async ({ canvasElement }) => {
    const narrow = within(within(canvasElement).getByTestId("narrow"));
    const wide = within(within(canvasElement).getByTestId("wide"));
    await expect(narrow.getByRole("columnheader", { name: "Model" })).toBeVisible();
    await expect(narrow.queryByRole("columnheader", { name: "Provider" })).toBeNull();
    await expect(wide.getByRole("columnheader", { name: "Provider" })).toBeVisible();
    // a table that is named is found by that name
    await expect(narrow.getByRole("table", { name: "Requests" })).toBeInTheDocument();
  },
};

/**
 * `rowProps` hands a row its attributes: the selected one says so with
 * `aria-selected`, and a click selects it.
 */
export const RowsCanBeSelected: Story = {
  render: () => {
    function Selectable() {
      const [selected, setSelected] = React.useState<string | null>(null);
      return (
        <Table
          columns={COLUMNS}
          data={ROWS}
          rowKey="id"
          hover={false}
          rowProps={(r) => ({
            "aria-selected": selected === r.id,
            onClick: () => setSelected(r.id),
            className: cn("cursor-pointer", selected === r.id && "bg-muted"),
          })}
        />
      );
    }
    return <Selectable />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const row = canvas.getByText("claude-sonnet").closest("tr") as HTMLElement;
    await expect(row).toHaveAttribute("aria-selected", "false");
    await userEvent.click(row);
    await expect(row).toHaveAttribute("aria-selected", "true");
    await expect(canvas.getByText("gpt-4o").closest("tr")).toHaveAttribute(
      "aria-selected",
      "false",
    );
  },
};

/** A cell's own attributes come from `cellProps`, here the full name of a clipped value. */
export const CellsTakeTheirOwnAttributes: Story = {
  render: () => (
    <Table
      columns={[
        { key: "model", header: "Model", cellProps: (r: Row) => ({ title: r.model }) },
        { key: "provider", header: "Provider" },
      ]}
      data={ROWS}
      rowKey="id"
    />
  ),
  play: async ({ canvasElement }) => {
    await expect(within(canvasElement).getByText("gpt-4o")).toHaveAttribute("title", "gpt-4o");
  },
};

/**
 * The header of a table without a frame sticks to the top of the scroll area
 * that holds it, so it stays in view while the rows move under it.
 */
export const StickyHeaderStaysWhileRowsScroll: Story = {
  render: () => (
    <div
      className="max-h-24 overflow-auto"
      data-testid="scroller"
      role="region"
      aria-label="Rows"
      tabIndex={0}
    >
      <Table
        bare
        compact
        stickyHeader
        columns={COLUMNS}
        data={Array.from({ length: 30 }, (_, i) => ({ ...ROWS[i % 3]!, id: `r${i}` }))}
        rowKey="id"
      />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const scroller = canvas.getByTestId("scroller");
    const header = canvas.getByRole("columnheader", { name: "Model" });
    scroller.scrollTop = 200;
    await waitFor(() => expect(scroller.scrollTop).toBeGreaterThan(100));
    await expect(header.getBoundingClientRect().top).toBeGreaterThanOrEqual(
      scroller.getBoundingClientRect().top - 1,
    );
    await expect(header).toBeVisible();
  },
};

const STACKABLE: TableColumn<Row>[] = [
  {
    key: "model",
    header: "Model",
    stackedClassName: "@max-[479px]:col-start-1 @max-[479px]:row-start-1",
  },
  {
    key: "requests",
    header: "Requests",
    align: "right",
    stackedClassName: "@max-[479px]:col-start-2 @max-[479px]:row-start-1",
  },
  {
    key: "p95",
    header: "p95",
    align: "right",
    stackedClassName: "@max-[479px]:col-start-1 @max-[479px]:row-start-2",
  },
];

/**
 * Below 480px of the table's own width each row restacks into a grid and the
 * header leaves the screen but not the accessibility tree: the cells keep
 * their column names.
 */
export const StackedRowsKeepTheirHeaders: Story = {
  render: () => (
    <div className="@container" style={{ width: 360 }}>
      <Table bare compact stacked columns={STACKABLE} data={ROWS} rowKey="id" caption="Requests" />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const row = canvas.getByText("gpt-4o").closest("tr") as HTMLElement;
    await expect(getComputedStyle(row).display).toBe("grid");
    // the header is out of sight, yet still the cells' names
    const header = canvas.getByRole("columnheader", { name: "Requests" });
    await expect(header.closest("thead")).toHaveClass("@max-[479px]:sr-only");
    await expect(canvas.getAllByRole("columnheader")).toHaveLength(3);
    await expect(within(row).getByText("18422")).toBeVisible();
  },
};

/** At 480px and wider the same table is a table again. */
export const StackedRowsAreATableWhenWide: Story = {
  render: () => (
    <div className="@container" style={{ width: 640 }}>
      <Table bare compact stacked columns={STACKABLE} data={ROWS} rowKey="id" caption="Requests" />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const row = within(canvasElement).getByText("gpt-4o").closest("tr") as HTMLElement;
    await expect(getComputedStyle(row).display).toBe("table-row");
  },
};
