import type { Meta, StoryObj } from "@storybook/react";
import { expect, waitFor, within } from "storybook/test";

import ComplexityRouter from "./ComplexityRouter";
import {
  Harness,
  expectEmptyState,
  expectLoadError,
  expectRefused,
  expectSkeleton,
  json,
  pending,
  routes,
  scoped,
} from "./story-harness";
import type { RouteRow } from "@/lib/api";

const route = (over: Partial<RouteRow> = {}): RouteRow => ({
  id: "r-1",
  project_id: "project-1",
  model: "gpt-4o",
  strategy: "weighted",
  enabled: true,
  advanced: {},
  params: {},
  param_policy: {},
  created_at: "2026-02-01T00:00:00Z",
  ...over,
});

const ROUTES = [route(), route({ id: "r-2", model: "claude-sonnet" })];

// `/complexity` is listed first: it is a suffix of the route path, and `routes`
// matches in order, so the shorter fragment would otherwise swallow it
const loaded = routes([
  ["/complexity", () => ({ tiers: [{ name: "small", max_input_bytes: 4096, route: "gpt-4o-mini" }] })],
  ["/routes", () => ROUTES],
]);

const meta = {
  title: "Screens/ComplexityRouter",
  component: ComplexityRouter,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof ComplexityRouter>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("gpt-4o").length).toBeGreaterThan(0));
    await waitFor(() =>
      expect(canvas.getAllByText("small").length).toBeGreaterThan(0),
    );
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

// a complexity policy hangs off a route, so with no routes there is nothing
// this screen can create — the CTA points at the screen that can
export const Empty: Story = {
  render: () => (
    <Harness fetchStub={routes([["/routes", () => []]])}>
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectEmptyState(canvasElement, /No routes to give a policy/);
    await expect(
      canvas.getByRole("link", { name: /Open routing rules/ }),
    ).toBeInTheDocument();
  },
};

// the screen had no error state at all before #1180: a failed route read left
// the page looking like a project with no routes
export const Error_: Story = {
  name: "Error",
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "boom" } }, 500))}>
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return routes/i);
  },
};

export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "forbidden" } }, 403))}>
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /You do not have access to routes/);
  },
};

// a route with no policy at all, so the "add a policy" entry point renders
const unconfigured = routes([
  ["/complexity", () => ({ tiers: [] })],
  ["/routes", () => ROUTES],
]);

// A complexity policy is stored on its route, so both entry points are
// `route:update` — admin (#1606).
export const RefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="viewer">
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Edit the complexity policy for gpt-4o");
  },
};

// the second entry point is a bare button rather than a `GatedButton`, so it
// carries the gate by hand and can lose it without the first one noticing
export const RefusedToAMemberWithNoPolicyYet: Story = {
  render: () => (
    <Harness fetchStub={unconfigured} role="member">
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Add a complexity policy for gpt-4o");
  },
};
