import type { Meta, StoryObj } from "@storybook/react-vite";
import { Pencil, Trash2 } from "lucide-react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import {
  HEALTH_COLOR,
  ListHeader,
  ListRow,
  ListTable,
  PageBody,
  Pill,
  RowIconButton,
  SearchInput,
  SortLabel,
  StatusDot,
  useSort,
} from "./screen";
import { Harness, routes } from "@/pages/story-harness";

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

function ProviderList({ rows = ROWS }: { rows?: ProviderRow[] }) {
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
      <ListTable>
        <ListHeader grid={GRID}>
          <SortLabel label="Provider" col="name" sort={sort} onCycle={(c) => cycle(c as Col)} />
          <span>Kind</span>
          <SortLabel
            label="p95"
            col="latency"
            sort={sort}
            onCycle={(c) => cycle(c as Col)}
            justify="flex-end"
          />
          <span className="sr-only">Actions</span>
        </ListHeader>
        {sorted.map((row) => (
          <ListRow key={row.name} grid={GRID}>
            <span className="flex items-center gap-2 font-mono text-xs">
              <StatusDot color={HEALTH_COLOR[row.health]} />
              {row.name}
            </span>
            <Pill color="var(--text-secondary)" border="var(--border-subtle)">
              {row.kind}
            </Pill>
            <span className="text-right font-mono text-xs">{row.latency} ms</span>
            <span className="flex justify-end gap-1.5">
              <RowIconButton aria-label={`Edit ${row.name}`}>
                <Pencil className="h-3.5 w-3.5" />
              </RowIconButton>
              <RowIconButton danger aria-label={`Delete ${row.name}`}>
                <Trash2 className="h-3.5 w-3.5" />
              </RowIconButton>
            </span>
          </ListRow>
        ))}
      </ListTable>
    </PageBody>
  );
}

const meta = {
  title: "Display/ScreenPrimitives",
  component: ListTable,
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

/** Nothing to list: the table keeps its header and the caller fills the body. */
export const NoRows: Story = {
  render: () => <ProviderList rows={[]} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("Kind")).toBeVisible();
    await expect(canvas.queryByText("openai-prod")).not.toBeInTheDocument();
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
        <RowIconButton danger gate="provider:delete" aria-label="Delete openai-prod">
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
        <RowIconButton danger gate="provider:delete" aria-label="Delete openai-prod">
          <Trash2 className="h-3.5 w-3.5" />
        </RowIconButton>
      </PageBody>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const button = within(canvasElement).getByRole("button", { name: "Delete openai-prod" });
    await waitFor(() => expect(button).toBeEnabled());
    await expect(button).not.toHaveAttribute("title");
  },
};
