import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Models from "./Models";
import {
  Harness,
  expectEmptyState,
  expectLoadError,
  expectRefused,
  expectSkeleton,
  json,
  NEEDS_SUPERADMIN,
  pending,
  routes,
  scoped,
  Toasted,
  expectToast,
} from "./story-harness";
import type { EffectiveModelDto, LabelRow, RouteRow, RouteTargetRow } from "@/lib/api";

const MODELS: EffectiveModelDto[] = [
  { model: "gpt-4o", strategy: "weighted", targets: 2, source: "db" },
  { model: "claude-sonnet", strategy: "least_load", targets: 1, source: "config" },
];

// one route fanned out over two providers, which is the shape the provider
// column has to summarise without losing the second name (#1202)
const FANOUT_ROUTE = {
  id: "route-1",
  project_id: "p1",
  model: "gpt-4o",
  strategy: "weighted",
  enabled: true,
  params: {},
  param_policy: {},
} as RouteRow;

const FANOUT_TARGETS = [
  { id: "t1", route_id: "route-1", provider_id: "prov-a", weight: 3, created_at: "" },
  { id: "t2", route_id: "route-1", provider_id: "prov-b", weight: 1, created_at: "" },
] as RouteTargetRow[];

const FANOUT_PROVIDERS = [
  { id: "prov-a", name: "sim-a" },
  { id: "prov-b", name: "sim-b" },
];

const loaded = routes([
  // longest first: `/models/prices` would otherwise be answered by `/models`
  ["/model-prices", () => []],
  ["/currency", () => ({ base: "USD", rates: {} })],
  ["/models", () => MODELS],
  ["/providers", () => []],
  ["/routes", () => []],
]);

const meta = {
  title: "Screens/Models",
  component: Models,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Models>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("gpt-4o")).toBeVisible());
    await expect(canvas.getByText("claude-sonnet")).toBeVisible();
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

// nothing configured at all: the CTA opens the sheet that makes the first one
export const Empty: Story = {
  render: () => (
    <Harness fetchStub={routes([["/models", () => []]])}>
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No models yet/, /Add model/);
  },
};

// filters on, nothing through them: a different sentence and a different button
export const NoFilterMatch: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("gpt-4o")).toBeVisible());
    await userEvent.type(canvas.getByLabelText("Search models"), "nonexistent");
    await waitFor(() => expect(canvas.getByText(/No models match/)).toBeVisible());
    await expect(canvas.getByRole("button", { name: /Clear search/i })).toBeInTheDocument();
  },
};

export const Error_: Story = {
  name: "Error",
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "boom" } }, 500))}>
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return models/i);
  },
};

export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "forbidden" } }, 403))}>
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /You do not have access to models/);
  },
};

// the column has room for one name, so the rest live in a tooltip rather than
// silently vanishing behind the first target (#1202)
export const MultiProviderRoute: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/model-prices", () => []],
        ["/currency", () => ({ base: "USD", rates: {} })],
        // longest first: `/routes/:id/targets` would otherwise match `/routes`
        ["/targets", () => FANOUT_TARGETS],
        ["/models", () => [MODELS[0]]],
        ["/providers", () => FANOUT_PROVIDERS],
        ["/routes", () => [FANOUT_ROUTE]],
      ])}
    >
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("gpt-4o")).toBeVisible());
    await waitFor(() => expect(canvas.getByTitle("sim-a, sim-b")).toBeVisible());
    await expect(canvas.getByTitle("sim-a, sim-b")).toHaveTextContent("sim-a +1 more");
  },
};

// a catalog with one db-backed row that has a route behind it, so the edit
// control is enabled for anyone the gate allows
const oneRoutedModel = routes([
  ["/model-prices", () => []],
  ["/currency", () => ({ base: "USD", rates: {} })],
  ["/targets", () => FANOUT_TARGETS],
  ["/models", () => [MODELS[0]]],
  ["/providers", () => FANOUT_PROVIDERS],
  ["/routes", () => [FANOUT_ROUTE]],
]);

/**
 * The delete is refused (#1607).
 *
 * Deleting a model takes a route and its targets with it, so the dialog has to
 * stay open on a refusal rather than closing over a model that is still
 * serving. The refusal is reported twice — the toast queue and the line inside
 * the dialog — and both are asserted.
 */
export const DeleteRejectedByTheServer: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        init?.method === "DELETE"
          ? json({ error: { message: "gpt-4o is still referenced by 2 virtual keys" } }, 409)
          : oneRoutedModel(input, init),
      )}
    >
      <Toasted>
        <Models />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Delete model gpt-4o" }),
    );
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.click(dialog.getByRole("button", { name: "Delete" }));

    await expectToast(canvasElement, /referenced by 2 virtual keys/, "error");
    await waitFor(() =>
      expect(dialog.getByText(/referenced by 2 virtual keys/)).toBeVisible(),
    );
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
  },
};

// The two gates on this screen take different authorities (#1606).
//
// Adding a model creates a *route*, which is admin, while deleting one is the
// deployment-wide `model:delete` a superadmin alone holds — so an admin is
// refused the delete and offered the create, and a story that asserted one
// sentence for both would pass against either gate keyed to the other.
export const RefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={oneRoutedModel} role="viewer">
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, /add model/i);
    await expectRefused(canvasElement, "Edit gpt-4o");
    await expectRefused(canvasElement, "Delete model gpt-4o", NEEDS_SUPERADMIN);
  },
};

export const DeleteRefusedToAnAdmin: Story = {
  render: () => (
    <Harness fetchStub={oneRoutedModel} role="admin">
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectRefused(canvasElement, "Delete model gpt-4o", NEEDS_SUPERADMIN);
    // the route half of the screen is still theirs
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: "Edit gpt-4o" })).toBeEnabled(),
    );
  },
};

// ------------------------------------------------------------- labels (#1329)

// a model label is addressed by the model's name, because the catalog is
// deployment-wide and has no row of its own to point at
const MODEL_LABELS: LabelRow[] = [
  {
    id: "ml-1",
    subject_type: "model",
    subject_id: "gpt-4o",
    key: "tier",
    value: "flagship",
    source: "custom",
    created_at: "2026-04-01T00:00:00Z",
    updated_at: "2026-04-01T00:00:00Z",
  },
  {
    id: "ml-2",
    subject_type: "model",
    subject_id: "gpt-4o",
    key: "tier",
    value: "priced",
    source: "auto",
    observed_at: "2026-04-02T07:00:00Z",
    observation: "a price row exists for this model",
    created_at: "2026-04-02T07:00:00Z",
    updated_at: "2026-04-02T07:00:00Z",
  },
];

const withLabels = (labels = MODEL_LABELS) =>
  scoped(async (input) => {
    const url = new URL(String(input), "http://localhost");
    if (url.pathname.endsWith("/model-labels")) {
      const subject = url.searchParams.get("subject_id");
      return json(subject ? labels.filter((l) => l.subject_id === subject) : labels);
    }
    if (url.pathname.endsWith("/models")) return json(MODELS);
    if (url.pathname.endsWith("/currency")) return json({ base: "USD", rates: {} });
    return json([]);
  });

/** both sources on one key, told apart by name rather than by colour */
export const Labelled: Story = {
  render: () => (
    <Harness fetchStub={withLabels()}>
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByLabelText("tier=flagship, your label")).toBeVisible());
    await expect(canvas.getByLabelText("tier=priced, automatic label")).toBeVisible();
  },
};

/** the filter narrows the catalog, and counts as a filter for the empty copy */
export const FilteredByLabel: Story = {
  render: () => (
    <Harness fetchStub={withLabels()}>
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("claude-sonnet")).toBeVisible());
    await userEvent.click(canvas.getByRole("combobox", { name: /Filter by label/i }));
    await userEvent.click(
      await within(document.body).findByRole("option", { name: "tier=flagship" }),
    );
    await waitFor(() => expect(canvas.queryByText("claude-sonnet")).toBeNull());
    await expect(canvas.getByText("gpt-4o")).toBeVisible();
  },
};

/** a label nothing in the catalog carries reads as a filter, not an empty catalog */
export const NoLabelMatch: Story = {
  render: () => (
    <Harness
      fetchStub={withLabels([
        { ...MODEL_LABELS[0], id: "ml-9", subject_id: "gone-model", value: "legacy" },
      ])}
    >
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("gpt-4o")).toBeVisible());
    await userEvent.click(canvas.getByRole("combobox", { name: /Filter by label/i }));
    await userEvent.click(
      await within(document.body).findByRole("option", { name: "tier=legacy" }),
    );
    await waitFor(() => expect(canvas.getByText(/No models match/i)).toBeVisible());
    await userEvent.click(canvas.getByRole("button", { name: /Clear search/i }));
    await waitFor(() => expect(canvas.getByText("gpt-4o")).toBeVisible());
  },
};

/**
 * Labelling a model is a superadmin's act — the catalog is deployment-wide, the
 * way `model_price` is — so an admin sees the panel and its contents with the
 * write controls disabled rather than a form that answers 403.
 */
export const AdminCannotWriteAModelLabel: Story = {
  render: () => (
    <Harness fetchStub={withLabels()} role="admin">
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("gpt-4o")).toBeVisible());
    await userEvent.click(canvas.getByRole("button", { name: "Labels on gpt-4o" }));
    const panel = within(await within(document.body).findByRole("dialog"));
    await expect(await panel.findByText(/a price row exists/)).toBeVisible();
    await expect(panel.getByRole("button", { name: "Add label" })).toBeDisabled();
    await expect(panel.getByRole("button", { name: "Remove tier=flagship" })).toBeDisabled();
  },
};

/** and a superadmin gets both */
export const SuperadminCanWriteAModelLabel: Story = {
  render: () => (
    <Harness fetchStub={withLabels()} role="superadmin">
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("gpt-4o")).toBeVisible());
    await userEvent.click(canvas.getByRole("button", { name: "Labels on gpt-4o" }));
    const panel = within(await within(document.body).findByRole("dialog"));
    await expect(panel.getByRole("button", { name: "Remove tier=flagship" })).toBeEnabled();
    // still no way to touch the observation
    await expect(panel.queryByRole("button", { name: "Remove tier=priced" })).toBeNull();
  },
};
