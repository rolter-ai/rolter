import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import ComplexityRouter from "./ComplexityRouter";
import {
  Harness,
  expectEmptyState,
  expectInStatusRegion,
  expectLoadError,
  expectRefused,
  expectSkeleton,
  json,
  pending,
  routes,
  scoped,
  scopeResponse,
} from "./story-harness";
import type { FetchStub } from "./story-harness";
import en from "@/lib/i18n/locales/en.json";
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

/** `LoadError`'s retry, read out of the catalog rather than written out again. */
const RETRY = en.errors.load.retry;

// `/complexity` is listed first: it is a suffix of the route path, and `routes`
// matches in order, so the shorter fragment would otherwise swallow it
const loaded = routes([
  [
    "/complexity",
    () => ({ tiers: [{ name: "small", max_input_bytes: 4096, route: "gpt-4o-mini" }] }),
  ],
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
    await waitFor(() => expect(canvas.getAllByText("small").length).toBeGreaterThan(0));
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
    await expect(canvas.getByRole("link", { name: /Open routing rules/ })).toBeInTheDocument();
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

// ── per-route policy states (#1461) ───────────────────────────────────────────
//
// The list makes one policy request per route, and `?? []` used to collapse
// every non-answer into "no policy yet": a delayed read, a 500 and the 403 a
// read-only caller gets all rendered as a route the operator had simply never
// configured. These four stories pin the four states apart.

/** The policy read answers for `r-1` and fails for `r-2`. */
const oneFailedPolicy: FetchStub = async (input) => {
  const url = String(input);
  const scope = scopeResponse(url);
  if (scope) return scope;
  if (url.includes("/complexity")) {
    return url.includes("r-2")
      ? json({ error: { message: "policy read blew up" } }, 500)
      : json({ tiers: [{ name: "small", max_input_bytes: 4096, route: "gpt-4o-mini" }] });
  }
  if (url.includes("/routes")) return json(ROUTES);
  return json([]);
};

// the half of the screen the issue is about: `claude-sonnet`'s policy never
// arrived, so it must not appear under "No policy yet" — the group that offers
// to create one
export const OnePolicyFailedToLoad: Story = {
  render: () => (
    <Harness fetchStub={oneFailedPolicy}>
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /complexity policies/i);
    // the summary counts the route that answered, not the one that did not
    await waitFor(() =>
      expect(canvas.getByText(/1 of 1 route has a complexity policy/)).toBeVisible(),
    );
    await expect(canvas.getByText(/1 route's policy could not be read/)).toBeVisible();
    // and the unread route offers no way in: an editor seeded from a failed
    // read would save a fresh draft over contents nobody has seen
    await expect(
      canvas.queryByRole("button", { name: "Add a complexity policy for claude-sonnet" }),
    ).not.toBeInTheDocument();
    await expect(
      canvas.queryByRole("button", { name: "Edit the complexity policy for claude-sonnet" }),
    ).not.toBeInTheDocument();
  },
};

/** Routes are in, the policy reads are not — the state that read as "empty". */
const routesWithoutPolicies: FetchStub = async (input) => {
  const url = String(input);
  const scope = scopeResponse(url);
  if (scope) return scope;
  if (url.includes("/complexity")) return new Promise<Response>(() => {});
  if (url.includes("/routes")) return json(ROUTES);
  return json([]);
};

export const PoliciesStillLoading: Story = {
  render: () => (
    <Harness fetchStub={routesWithoutPolicies}>
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the routes resolved, so the page-level skeleton is gone and these are the
    // per-route policy placeholders — named, so a stray skeleton cannot pass
    await expectInStatusRegion(canvasElement, "complexity-policies-loading");
    await expect(canvas.getByText(/Still reading the policy for 2 routes/)).toBeVisible();
    await expect(canvas.queryByText("No policy yet")).not.toBeInTheDocument();
  },
};

/**
 * `GET /routes/{id}/complexity` takes `route:read` server-side (#1666), so a
 * viewer is served the policy — but a caller the control plane refuses the read
 * to entirely still exists (a custom role denied `route:read`, or a control
 * plane older than #1666). The screen says so rather than reporting the routes
 * as unconfigured.
 */
const forbiddenPolicies: FetchStub = async (input) => {
  const url = String(input);
  const scope = scopeResponse(url);
  if (scope) return scope;
  if (url.includes("/complexity")) return json({ error: { message: "forbidden" } }, 403);
  if (url.includes("/routes")) return json(ROUTES);
  return json([]);
};

export const PolicyReadForbidden: Story = {
  render: () => (
    <Harness fetchStub={forbiddenPolicies} role="viewer">
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /You do not have access to complexity policies/);
    await expect(canvas.queryByText("No policy yet")).not.toBeInTheDocument();
    // a permission is not a transient failure, so no retry is offered
    await expect(canvas.queryByRole("button", { name: RETRY })).not.toBeInTheDocument();
  },
};

/** The retry on the group alert re-runs the reads that failed. */
const failsOnce = (): FetchStub => {
  let attempts = 0;
  return async (input) => {
    const url = String(input);
    const scope = scopeResponse(url);
    if (scope) return scope;
    if (url.includes("/complexity")) {
      attempts += 1;
      return attempts <= ROUTES.length
        ? json({ error: { message: "policy read blew up" } }, 500)
        : json({ tiers: [{ name: "small", max_input_bytes: 4096, route: "gpt-4o-mini" }] });
    }
    if (url.includes("/routes")) return json(ROUTES);
    return json([]);
  };
};

export const RetryRecoversThePolicies: Story = {
  render: () => (
    <Harness fetchStub={failsOnce()}>
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /complexity policies/i);
    await userEvent.click(canvas.getByRole("button", { name: RETRY }));
    await waitFor(() => expect(canvas.getAllByText("small").length).toBe(ROUTES.length));
    await expect(canvas.queryByRole("alert")).not.toBeInTheDocument();
  },
};

/**
 * The editor re-reads the policy on open, so it holds nothing in the same two
 * ways the list does. It used to seed the default two-tier draft either way and
 * offer a save that would have replaced a policy nobody had seen.
 */
const secondReadFails = (): FetchStub => {
  const seen = new Set<string>();
  return async (input) => {
    const url = String(input);
    const scope = scopeResponse(url);
    if (scope) return scope;
    if (url.includes("/complexity")) {
      // the list's read answers; the editor's re-read of the same route does not
      if (seen.has(url)) return json({ error: { message: "policy read blew up" } }, 500);
      seen.add(url);
      return json({ tiers: [{ name: "small", max_input_bytes: 4096, route: "gpt-4o-mini" }] });
    }
    if (url.includes("/routes")) return json(ROUTES);
    return json([]);
  };
};

export const EditorPolicyReadFails: Story = {
  render: () => (
    <Harness fetchStub={secondReadFails()}>
      <ComplexityRouter />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Edit the complexity policy for gpt-4o" }),
    );
    const dialog = within(await within(document.body).findByRole("dialog"));
    await waitFor(() =>
      expect(
        dialog.getAllByRole("alert").some((a) => /complexity policy/i.test(a.textContent ?? "")),
      ).toBe(true),
    );
    // no tier rows to edit and nothing to save: the draft would have gone over
    // contents the read never returned
    await expect(dialog.queryByDisplayValue("small")).not.toBeInTheDocument();
    await expect(dialog.getByRole("button", { name: "Save" })).toBeDisabled();
  },
};
