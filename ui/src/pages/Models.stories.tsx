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
import type { EffectiveModelDto, RouteRow, RouteTargetRow } from "@/lib/api";

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
