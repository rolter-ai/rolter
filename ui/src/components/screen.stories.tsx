import type { Meta, StoryObj } from "@storybook/react-vite";
import { Pencil, Trash2 } from "lucide-react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import {
  HEALTH_COLOR,
  ListActionsHeader,
  ListCell,
  ListEmptyRow,
  ListHeader,
  ListHeaderCell,
  ListLoadingRow,
  ListRow,
  ListSummary,
  ListTable,
  PageBody,
  Pill,
  RowIconButton,
  SearchInput,
  SortLabel,
  StatusDot,
  useSort,
} from "./screen";
import { ListSkeleton } from "./LoadingState";
import { ANSWERED, type ReadState } from "@/lib/read-state";
import { expectAllowed, expectListTable, Harness, routes } from "@/pages/story-harness";

// the grid every list screen is assembled from: a template shared by the
// header and the rows, so a column cannot drift between the two
const GRID = "1.6fr 1fr 0.8fr 96px";

interface ProviderRow {
  name: string;
  kind: string;
  health: keyof typeof HEALTH_COLOR;
  latency: number;
}

const ROWS: ProviderRow[] = [
  { name: "openai-prod", kind: "openai", health: "ok", latency: 812 },
  { name: "anthropic-prod", kind: "anthropic", health: "degraded", latency: 1240 },
  { name: "vllm-cluster", kind: "openai_compatible", health: "down", latency: 340 },
];

type Col = "name" | "latency";

// the reads a list can be holding no rows under, besides one that answered
const IN_FLIGHT: ReadState = { isPending: true, isSuccess: false, fetchStatus: "fetching" };
const PARKED: ReadState = { isPending: true, isSuccess: false, fetchStatus: "paused" };
const FAILED: ReadState = { isPending: false, isSuccess: false, fetchStatus: "idle" };

function ProviderList({
  rows = ROWS,
  read = ANSWERED,
}: {
  rows?: ProviderRow[];
  read?: ReadState;
}) {
  const { sort, cycle, apply } = useSort<Col>();
  const [query, setQuery] = React.useState("");
  const filtered = rows.filter((r) => r.name.includes(query.trim()));
  const sorted = apply(filtered, { name: (r) => r.name, latency: (r) => r.latency });
  return (
    <PageBody>
      <SearchInput
        placeholder="Search providers"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />
      <ListTable label="Providers">
        <ListHeader grid={GRID}>
          <SortLabel label="Provider" col="name" sort={sort} onCycle={(c) => cycle(c as Col)} />
          <ListHeaderCell>Kind</ListHeaderCell>
          <SortLabel
            label="p95"
            col="latency"
            sort={sort}
            onCycle={(c) => cycle(c as Col)}
            justify="flex-end"
          />
          <ListActionsHeader />
        </ListHeader>
        <ListLoadingRow read={read}>
          <ListSkeleton rows={3} className="p-3" />
        </ListLoadingRow>
        {sorted.map((row) => (
          <ListRow key={row.name} grid={GRID}>
            <ListCell className="flex items-center gap-2 font-mono text-xs">
              <StatusDot color={HEALTH_COLOR[row.health]} />
              {row.name}
            </ListCell>
            {/* a cell around a component rather than in place of an element
                  passes `grid`, so the pill still spans its column */}
            <ListCell className="grid">
              <Pill color="var(--text-secondary)" border="var(--border-subtle)">
                {row.kind}
              </Pill>
            </ListCell>
            <ListCell className="text-right font-mono text-xs">{row.latency} ms</ListCell>
            <ListCell className="flex justify-end gap-1.5">
              <RowIconButton control="provider-edit" aria-label={`Edit ${row.name}`}>
                <Pencil className="h-3.5 w-3.5" />
              </RowIconButton>
              <RowIconButton control="provider-delete" danger aria-label={`Delete ${row.name}`}>
                <Trash2 className="h-3.5 w-3.5" />
              </RowIconButton>
            </ListCell>
          </ListRow>
        ))}
        <ListEmptyRow read={read} rows={sorted.length}>
          <p className="px-4 py-6 text-center text-sm text-muted-foreground">No providers yet</p>
        </ListEmptyRow>
      </ListTable>
    </PageBody>
  );
}

const meta = {
  title: "Display/ScreenPrimitives",
  component: ListTable,
  // a table has to be named, so the meta names the one every story renders
  args: { label: "Providers" },
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof ListTable>;

export default meta;
type Story = StoryObj<typeof meta>;

/** The whole kit assembled the way a list screen assembles it. */
export const ProviderListing: Story = {
  render: () => <ProviderList />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("openai-prod")).toBeVisible();
    await expect(canvas.getByRole("button", { name: "Delete vllm-cluster" })).toBeVisible();
  },
};

/**
 * The sorter is three-state: ascending, descending, then off. The third press
 * has to restore the source order — a sorter that cannot be turned off leaves
 * no way back to "as the server returned it".
 */
export const SortCyclesThroughOff: Story = {
  render: () => <ProviderList />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const names = () =>
      canvas.getAllByText(/-(prod|cluster)$/).map((node) => node.textContent?.trim());
    await expect(names()).toEqual(["openai-prod", "anthropic-prod", "vllm-cluster"]);
    const header = canvas.getByRole("button", { name: /provider/i });
    await userEvent.click(header);
    await expect(names()).toEqual(["anthropic-prod", "openai-prod", "vllm-cluster"]);
    await userEvent.click(header);
    await expect(names()).toEqual(["vllm-cluster", "openai-prod", "anthropic-prod"]);
    await userEvent.click(header);
    await expect(names()).toEqual(["openai-prod", "anthropic-prod", "vllm-cluster"]);
  },
};

/**
 * The grid is a table to a screen reader too (#2000): a named table, a header
 * rowgroup and a body rowgroup, one row of column headers, and body rows whose
 * every child is a cell. The column with no visible heading — the row's
 * buttons — still has a name, or it is announced as an empty column.
 */
export const TableSemantics: Story = {
  render: () => <ProviderList />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectListTable(canvasElement, "Providers");
    const table = canvas.getByRole("table", { name: "Providers" });
    await expect(within(table).getAllByRole("rowgroup")).toHaveLength(2);
    const headers = within(table).getAllByRole("columnheader");
    await expect(headers.map((h) => h.textContent)).toEqual([
      "Provider",
      "Kind",
      "p95",
      "Row actions",
    ]);
    // the header row, then one row per provider
    await expect(within(table).getAllByRole("row")).toHaveLength(1 + ROWS.length);
    await expect(within(table).getAllByRole("cell")).toHaveLength(ROWS.length * headers.length);
    // focus lands on the table itself, since it is the sideways scroller
    table.focus();
    await expect(table).toHaveFocus();
  },
};

/**
 * Sort direction is `aria-sort` on the column header, where a screen reader
 * announces it with the column — the arrow shows the same thing to the eye and
 * is hidden, so the button's name stays the bare column label rather than
 * "Provider" plus an unlabelled image.
 */
export const SortIsAnnouncedOnTheHeader: Story = {
  render: () => <ProviderList />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const header = (name: string) =>
      canvas.getAllByRole("columnheader").find((h) => h.textContent === name);
    const button = canvas.getByRole("button", { name: "Provider" });
    // sortable and unsorted says so; a plain column carries no aria-sort at all
    await expect(header("Provider")).toHaveAttribute("aria-sort", "none");
    await expect(header("p95")).toHaveAttribute("aria-sort", "none");
    await expect(header("Kind")).not.toHaveAttribute("aria-sort");
    await expect(button.closest('[role="columnheader"]')).toBe(header("Provider"));

    await userEvent.click(button);
    await expect(header("Provider")).toHaveAttribute("aria-sort", "ascending");
    await expect(header("p95")).toHaveAttribute("aria-sort", "none");
    const arrow = button.querySelector("svg");
    await expect(arrow).toHaveAttribute("aria-hidden", "true");
    await expect(canvas.getByRole("button", { name: "Provider" })).toBe(button);

    await userEvent.click(button);
    await expect(header("Provider")).toHaveAttribute("aria-sort", "descending");
    await expect(button.querySelector("svg")).toHaveAttribute("aria-hidden", "true");

    await userEvent.click(button);
    await expect(header("Provider")).toHaveAttribute("aria-sort", "none");
    await expect(button.querySelector("svg")).toBeNull();
  },
};

/**
 * The loading skeleton sits in a row of its own with one cell across the
 * table: a `role="status"` placed straight in the body rowgroup is content no
 * row owns, which fails axe's `aria-required-children`.
 */
export const LoadingRowKeepsTheTableWhole: Story = {
  render: () => <ProviderList rows={[]} read={IN_FLIGHT} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectListTable(canvasElement, "Providers");
    const status = canvas.getByRole("status");
    await expect(status.parentElement).toHaveAttribute("role", "cell");
    await expect(status.parentElement?.parentElement).toHaveAttribute("role", "row");
  },
};

/**
 * A retry react-query parked — the tab is hidden, the browser is offline — is
 * still a read awaiting its answer, so it keeps the skeleton. `isLoading` is
 * false in that window, which is how a parked read once said "nothing logged
 * yet" (#1984).
 */
export const ParkedReadKeepsTheSkeleton: Story = {
  render: () => <ProviderList rows={[]} read={PARKED} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("status")).toBeVisible();
    await expect(canvas.queryByText("No providers yet")).not.toBeInTheDocument();
  },
};

/**
 * A read that failed holds no rows, and that is not the same as a list with
 * none in it (#2211). The empty row waits for a read that succeeded, so the
 * body stays empty and the screen's `LoadError` is the only thing that speaks.
 */
export const NoEmptyRowUntilTheReadSucceeds: Story = {
  render: () => <ProviderList rows={[]} read={FAILED} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectListTable(canvasElement, "Providers");
    await expect(canvas.queryByText("No providers yet")).not.toBeInTheDocument();
    await expect(canvas.queryByRole("status")).not.toBeInTheDocument();
    // the header row and nothing under it
    await expect(canvas.getAllByRole("row")).toHaveLength(1);
  },
};

/**
 * The count beside a list renders only while the data it counts is held
 * (#2211): no data, no "0 providers". A summary that also explains the screen
 * keeps the explanation through its `fallback`.
 */
export const SummaryWaitsForTheData: Story = {
  render: () => (
    <PageBody>
      <div data-testid="unread">
        <ListSummary data={undefined as ProviderRow[] | undefined}>
          {(rows) => `${rows.length} providers`}
        </ListSummary>
      </div>
      <div data-testid="unread-with-fallback">
        <ListSummary
          data={undefined as ProviderRow[] | undefined}
          fallback="upstreams the gateway routes to"
        >
          {(rows) => `${rows.length} providers · upstreams the gateway routes to`}
        </ListSummary>
      </div>
      <div data-testid="held">
        <ListSummary data={[] as ProviderRow[]}>{(rows) => `${rows.length} providers`}</ListSummary>
      </div>
    </PageBody>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByTestId("unread")).toBeEmptyDOMElement();
    await expect(canvas.getByTestId("unread-with-fallback")).toHaveTextContent(
      /^upstreams the gateway routes to$/,
    );
    // a list that answered with none is a real zero, and says so
    await expect(canvas.getByTestId("held")).toHaveTextContent("0 providers");
  },
};

/** The search box is a real labelled control, not a decorated div. */
export const SearchFilters: Story = {
  render: () => <ProviderList />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(canvas.getByLabelText("Search providers"), "vllm");
    await expect(canvas.getByText("vllm-cluster")).toBeVisible();
    await expect(canvas.queryByText("openai-prod")).not.toBeInTheDocument();
  },
};

/**
 * Columns have a width below which they stop being readable, so the table
 * scrolls sideways inside its own border rather than dragging the page with it
 * — and the scroll container is focusable, or everything past the right edge
 * is mouse-only (#1181, #1203).
 */
export const NarrowViewportScrollsSideways: Story = {
  render: () => (
    <div className="max-w-[420px]">
      <ProviderList />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const scroller = canvasElement.querySelector<HTMLElement>("[tabindex='0']");
    await expect(scroller).not.toBeNull();
    scroller?.focus();
    await expect(scroller).toHaveFocus();
    await expect(document.documentElement.scrollWidth).toBeLessThanOrEqual(
      document.documentElement.clientWidth,
    );
  },
};

/** The health colours on their own, dot and pill, in every state they carry. */
export const StatusVocabulary: Story = {
  render: () => (
    <PageBody>
      <div className="flex flex-col gap-2 text-sm">
        {(Object.keys(HEALTH_COLOR) as (keyof typeof HEALTH_COLOR)[]).map((health) => (
          <span key={health} className="flex items-center gap-2">
            <StatusDot color={HEALTH_COLOR[health]} />
            <Pill color="var(--text-secondary)" border="var(--border-subtle)">
              {health}
            </Pill>
          </span>
        ))}
      </div>
    </PageBody>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("degraded")).toBeVisible();
  },
};

/**
 * Nothing to list: the table keeps its header and the caller fills the body,
 * with the empty state in one row and one cell so the table stays whole.
 */
export const NoRows: Story = {
  render: () => <ProviderList rows={[]} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("Kind")).toBeVisible();
    await expect(canvas.queryByText("openai-prod")).not.toBeInTheDocument();
    await expectListTable(canvasElement, "Providers");
    await expect(canvas.getByText("No providers yet").closest('[role="cell"]')).not.toBeNull();
  },
};

/**
 * A refused row control: the icon button takes the same gate the labelled
 * controls do, and says what it would take in its `title` (#1258).
 *
 * An icon carries no text, so the tooltip is the only thing that can explain
 * the refusal — a viewer who sees a dimmed trash can and nothing else has been
 * told "no" without being told why.
 */
export const RowIconButtonRefused: Story = {
  render: () => (
    <Harness fetchStub={routes([])} role="viewer">
      <PageBody>
        <RowIconButton
          danger
          gate="provider:delete"
          control="provider-delete"
          aria-label="Delete openai-prod"
        >
          <Trash2 className="h-3.5 w-3.5" />
        </RowIconButton>
      </PageBody>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const button = within(canvasElement).getByRole("button", { name: "Delete openai-prod" });
    await waitFor(() => expect(button).toBeDisabled());
    await expect(button).toHaveAttribute("title", "Requires the Admin role");
  },
};

/** The same button for a caller who may press it: enabled, and no refusal text. */
export const RowIconButtonAllowed: Story = {
  render: () => (
    <Harness fetchStub={routes([])} role="admin">
      <PageBody>
        <RowIconButton
          danger
          gate="provider:delete"
          control="provider-delete"
          aria-label="Delete openai-prod"
        >
          <Trash2 className="h-3.5 w-3.5" />
        </RowIconButton>
      </PageBody>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    // `expectAllowed`, because an icon button is enabled before the gate has
    // answered too - waiting for the enabled state alone would pass at any
    // latency, including against a control plane that never answers (#1707)
    await expectAllowed(canvasElement, "Delete openai-prod");
    const button = within(canvasElement).getByRole("button", { name: "Delete openai-prod" });
    await expect(button).not.toHaveAttribute("title");
  },
};
