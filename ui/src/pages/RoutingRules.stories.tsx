import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import RoutingRules from "./RoutingRules";
import {
  Harness,
  Toasted,
  cancelConfirmation,
  confirmDestructive,
  expectRefused,
  expectInStatusRegion,
  expectLoadError,
  expectSkeleton,
  expectToast,
  json,
  pending,
  pickOption,
  PROJECT,
  recording,
  scoped,
  type FetchStub,
  type Recorder,
  expectEmptyState,
  expectForbidden,
} from "./story-harness";
import type { LabelRow, ProviderRow, RouteRow, RouteTargetRow } from "@/lib/api";

const PROVIDERS: ProviderRow[] = [
  {
    id: "prov-1",
    org_id: "org-1",
    name: "openai-prod",
    slug: "openai-prod",
    kind: "openai",
    api_base: "https://api.openai.com/v1",
    egress_proxies: [],
    created_at: "2026-05-01T00:00:00Z",
  },
  {
    id: "prov-2",
    org_id: "org-1",
    name: "azure-west",
    slug: "azure-west",
    kind: "azure_openai",
    api_base: "https://west.openai.azure.com",
    egress_proxies: [],
    created_at: "2026-05-01T00:00:00Z",
  },
];

const ROUTES: RouteRow[] = [
  {
    id: "route-1",
    project_id: "project-1",
    model: "gpt-4o",
    strategy: "weighted",
    enabled: true,
    params: {},
    param_policy: {},
    advanced: {},
    created_at: "2026-05-01T00:00:00Z",
  },
  // a disabled route still renders: a route nobody can see is a route nobody
  // remembers to turn back on
  {
    id: "route-2",
    project_id: "project-1",
    model: "claude-sonnet",
    strategy: "least_load",
    enabled: false,
    params: {},
    param_policy: {},
    advanced: {},
    created_at: "2026-05-02T00:00:00Z",
  },
];

const TARGETS: Record<string, RouteTargetRow[]> = {
  "route-1": [
    {
      id: "rt-1",
      route_id: "route-1",
      provider_id: "prov-1",
      upstream_model: "gpt-4o-2024-08-06",
      weight: 80,
      created_at: "2026-05-01T00:00:00Z",
    },
    {
      id: "rt-2",
      route_id: "route-1",
      provider_id: "prov-2",
      upstream_model: null,
      weight: 20,
      created_at: "2026-05-01T00:00:00Z",
    },
  ],
  "route-2": [],
  // the same 80/20 weights as route-1, on a strategy that never reads them
  "route-3": [
    {
      id: "rt-3",
      route_id: "route-3",
      provider_id: "prov-1",
      upstream_model: "qwen-coder-32b",
      weight: 80,
      created_at: "2026-05-03T00:00:00Z",
    },
    {
      id: "rt-4",
      route_id: "route-3",
      provider_id: "prov-2",
      upstream_model: null,
      weight: 20,
      created_at: "2026-05-03T00:00:00Z",
    },
  ],
};

const CACHE_AWARE: RouteRow = {
  id: "route-3",
  project_id: "project-1",
  model: "qwen-coder",
  strategy: "cache_aware",
  enabled: true,
  params: {},
  param_policy: {},
  advanced: {},
  created_at: "2026-05-03T00:00:00Z",
};

const answer = (routes: RouteRow[], status = 200, labels: LabelRow[] = []): FetchStub =>
  scoped(async (input) => {
    const url = String(input);
    if (url.includes("/providers")) return json(PROVIDERS);
    // the label list is its own endpoint; without this branch it would be
    // answered with the route list and the chips would be nonsense (#1329)
    if (url.includes("/labels")) {
      const subject = new URL(url, "http://localhost").searchParams.get("subject_id");
      return json(subject ? labels.filter((l) => l.subject_id === subject) : labels);
    }
    const targets = /\/routes\/([^/]+)\/targets/.exec(url);
    if (targets) return json(TARGETS[targets[1]] ?? []);
    return json(status === 200 ? routes : { error: { message: "forbidden" } }, status);
  });

const meta = {
  title: "Screens/RoutingRules",
  component: RoutingRules,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof RoutingRules>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={answer(ROUTES)}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("gpt-4o")).toBeInTheDocument();
    // gpt-4o is `weighted`, so its weights are shares of the route's total and
    // 80/100 is 80%
    const targets = within(await canvas.findByRole("list", { name: "Targets of gpt-4o" }));
    await expect(targets.getByText("80%")).toBeInTheDocument();
    await expect(targets.getByText("20%")).toBeInTheDocument();
    await expect(targets.getAllByTestId("target-share")).toHaveLength(2);
    await expect(canvas.getByText("disabled")).toBeInTheDocument();
    // the count arrives with the list it counts
    await expect(canvas.getByText(/^2 routes/)).toBeInTheDocument();
  },
};

/**
 * "Add route" opens the model sheet, the same one Model Catalog opens (#1979).
 * The two screens used to create the same route through two forms that asked
 * for different things; a route now gets its strategy, its targets and the
 * rest of a model's settings in one place, whichever screen it starts from.
 */
let creates: Recorder;
export const AddRouteOpensTheModelSheet: Story = {
  render: () => {
    creates = recording(
      scoped(async (input, init) => {
        const url = String(input);
        if (init?.method === "POST" && url.endsWith("/routes")) {
          return json({ ...ROUTES[0], id: "route-new", model: "llama-70b" });
        }
        if (init?.method === "POST") return json({ id: "rt-new" });
        if (url.includes("/models")) return json([]);
        if (url.includes("/currency")) return json({ settlement: "USD", codes: ["USD"] });
        return answer(ROUTES)(input, init);
      }),
    );
    return (
      <Harness fetchStub={creates.stub}>
        <RoutingRules />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the button waits for the scope to name a project to create the route in
    const add = await canvas.findByRole("button", { name: /Add route/ });
    await waitFor(() => expect(add).toBeEnabled());
    await userEvent.click(add);
    const dialog = within(await within(document.body).findByRole("dialog"));
    await expect(dialog.getByRole("heading", { name: "Add model" })).toBeVisible();
    await waitFor(() =>
      expect(dialog.getByLabelText("Target 1 provider")).toHaveValue("openai-prod"),
    );
    await userEvent.type(dialog.getByLabelText("Model name"), "llama-70b");
    await pickOption(dialog.getByLabelText("Strategy"), "power_of_two");
    await userEvent.click(dialog.getByRole("button", { name: "Add model" }));
    await expect(await creates.expectSentBody("POST", `/projects/${PROJECT.id}/routes`)).toEqual({
      model: "llama-70b",
      strategy: "power_of_two",
    });
    await creates.expectSent("POST", "/routes/route-new/targets");
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    // no count before there is anything to count: "0 routes" here stated an
    // empty project while the list was still out (#2133)
    await expect(within(canvasElement).queryByText(/routes? ·/)).toBeNull();
  },
};

export const Empty: Story = {
  render: () => (
    <Harness fetchStub={answer([])}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No routes yet/, /Add route/);
  },
};

export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={answer([], 403)}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
    // a refusal is not an empty project: no zero count, no "No routes yet"
    const canvas = within(canvasElement);
    await expect(canvas.queryByText(/routes? ·/)).toBeNull();
    await expect(canvas.queryByText(/No routes yet/)).toBeNull();
  },
};

// --------------------------------------------------- per-route target reads (#2133)

/**
 * A target read still in flight holds a skeleton in its card. Folded into an
 * empty list it read "No targets yet." and "0 targets" about targets nobody
 * had seen; claude-sonnet really has none, so that copy appears exactly once.
 */
export const TargetsLoading: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        if (String(input).includes("/routes/route-1/targets"))
          return new Promise<Response>(() => {});
        return answer(ROUTES)(input, init);
      })}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("gpt-4o")).toBeInTheDocument();
    await expectInStatusRegion(canvasElement, "route-targets-loading");
    await expect(await canvas.findByText("No targets yet.")).toBeInTheDocument();
    await expect(canvas.getAllByText("No targets yet.")).toHaveLength(1);
    await expect(canvas.getAllByText("0 targets")).toHaveLength(1);
  },
};

// the first read of gpt-4o's targets fails and the retry succeeds; reset on
// every render so a remount starts from the failure again
let targetsDown = true;
const targetsFail = scoped(async (input, init) => {
  if (targetsDown && String(input).includes("/routes/route-1/targets")) {
    return json({ error: { message: "route_targets: connection refused" } }, 500);
  }
  return answer(ROUTES)(input, init);
});

/**
 * A failed target read says so in its card, with the control plane's own words
 * and a retry that re-reads that route alone — never "No targets yet.".
 */
export const TargetsFail: Story = {
  render: () => {
    targetsDown = true;
    return (
      <Harness fetchStub={targetsFail}>
        <RoutingRules />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /this route's targets/);
    await expect(canvas.getByText(/connection refused/)).toBeInTheDocument();
    await expect(await canvas.findByText("No targets yet.")).toBeInTheDocument();
    await expect(canvas.getAllByText("No targets yet.")).toHaveLength(1);
    await expect(canvas.getAllByText("0 targets")).toHaveLength(1);

    targetsDown = false;
    await userEvent.click(canvas.getByRole("button", { name: /Try again/ }));
    const targets = within(await canvas.findByRole("list", { name: "Targets of gpt-4o" }));
    await expect(targets.getByText("80%")).toBeInTheDocument();
    await waitFor(() => expect(canvas.queryByRole("alert")).toBeNull());
    await expect(canvas.getByText("2 targets")).toBeInTheDocument();
  },
};

/**
 * `cache_aware` builds its balancer from the target count and never reads a
 * weight, so the same 80/20 split that `weighted` honours draws no share and
 * no bar there; the card says the weights are unused instead.
 */
export const CacheAwareDrawsNoShare: Story = {
  render: () => (
    <Harness fetchStub={answer([ROUTES[0], CACHE_AWARE])}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const cacheAware = within(await canvas.findByRole("list", { name: "Targets of qwen-coder" }));
    await expect(cacheAware.getByText("qwen-coder-32b")).toBeInTheDocument();
    await expect(cacheAware.getByText("azure-west")).toBeInTheDocument();
    await expect(cacheAware.queryByText(/%/)).toBeNull();
    await expect(cacheAware.queryAllByTestId("target-share")).toHaveLength(0);
    await expect(canvas.getByText(/does not read weights/)).toHaveTextContent(/cache_aware/);

    // the weighted route beside it still draws its split
    const weighted = within(canvas.getByRole("list", { name: "Targets of gpt-4o" }));
    await expect(weighted.getByText("80%")).toBeInTheDocument();
    await expect(weighted.getAllByTestId("target-share")).toHaveLength(2);
  },
};

// deleting a route silently breaks every client calling that public model name,
// so it asks by name first (#1179)
const deletes = recording(
  scoped(async (input, init) => {
    if (init?.method === "DELETE") return json({}, 204);
    return answer(ROUTES)(input, init);
  }),
);

export const ConfirmsBeforeDeletingARoute: Story = {
  render: () => (
    <Harness fetchStub={deletes.stub}>
      <Toasted>
        <RoutingRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("gpt-4o")).toBeInTheDocument();
    // by name, not by index: every row control names the route it acts on,
    // so the story asserts on the control it means (#1214)
    const button = canvas.getByRole("button", { name: "Delete route gpt-4o" });

    // cancelling must leave the route alone — the half a manual click-through
    // never checks
    await userEvent.click(button);
    await cancelConfirmation();
    deletes.expectNotSent("DELETE", "/api/v1/routes/route-1");

    await userEvent.click(button);
    await confirmDestructive(/gpt-4o/, /delete route/i);
    await deletes.expectSent("DELETE", "/api/v1/routes/route-1");
    // the confirmation closes on success, so the outcome is announced where it
    // outlives the dialog (#1197)
    await expectToast(canvasElement, /gpt-4o deleted/);
  },
};

// the request is on the wire and the button says so, rather than looking like
// the click was dropped
const hangs = scoped(async (input, init) => {
  if (init?.method === "DELETE") return new Promise<Response>(() => {});
  return answer(ROUTES)(input, init);
});

export const DeletingARoute: Story = {
  render: () => (
    <Harness fetchStub={hangs}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("gpt-4o")).toBeInTheDocument();
    const trigger = canvas.getByRole("button", { name: "Delete route gpt-4o" });
    await userEvent.click(trigger);
    await confirmDestructive(/gpt-4o/, /delete route/i);
    const dialog = within(document.body).getByRole("dialog");
    await waitFor(() =>
      expect(within(dialog).getByRole("button", { name: /delete route/i })).toBeDisabled(),
    );
    // the pressed button going disabled takes focus with it; the panel picks
    // it up, so the next Tab stays out of the table behind the scrim (#1998)
    await waitFor(() => expect(dialog).toHaveFocus());
    await userEvent.tab();
    await expect(dialog).toContainElement(document.activeElement as HTMLElement);
    await expect(trigger.closest("[inert]")).not.toBeNull();
  },
};

// a delete the server refuses leaves the dialog open with the reason, instead
// of closing on an action that did not happen
export const DeleteFails: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        if (init?.method === "DELETE") {
          return json({ error: { message: "route is referenced by 2 virtual keys" } }, 409);
        }
        return answer(ROUTES)(input, init);
      })}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("gpt-4o")).toBeInTheDocument();
    await userEvent.click(canvas.getByRole("button", { name: "Delete route gpt-4o" }));
    await confirmDestructive(/gpt-4o/, /delete route/i);

    const dialog = within(document.body).getByRole("dialog");
    await waitFor(() =>
      expect(within(dialog).getByRole("alert")).toHaveTextContent(/referenced by 2 virtual keys/),
    );
  },
};

// `route` is admin at create and delete (#1606). The delete is a bare button
// reading `deleteGate`, so it is gated by hand and can lose the gate while the
// toolbar keeps it.
export const RefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={answer(ROUTES)} role="viewer">
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, /add route/i);
    await expectRefused(canvasElement, "Delete route gpt-4o");
  },
};

export const RefusedToAMember: Story = {
  render: () => (
    <Harness fetchStub={answer(ROUTES)} role="member">
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, /add route/i);
    await expectRefused(canvasElement, "Delete route claude-sonnet");
  },
};

// ------------------------------------------------------------- labels (#1329)

const ROUTE_LABELS: LabelRow[] = [
  {
    id: "rl-1",
    subject_type: "route",
    subject_id: "route-1",
    key: "tier",
    value: "gold",
    source: "custom",
    created_at: "2026-05-01T00:00:00Z",
    updated_at: "2026-05-01T00:00:00Z",
  },
  {
    id: "rl-2",
    subject_type: "route",
    subject_id: "route-1",
    key: "tier",
    value: "observed-gold",
    source: "auto",
    observed_at: "2026-05-03T08:00:00Z",
    observation: "priced on every target",
    created_at: "2026-05-03T08:00:00Z",
    updated_at: "2026-05-03T08:00:00Z",
  },
];

const labelled = answer(ROUTES, 200, ROUTE_LABELS);

/** the chips ride on the route card, both sources distinguishable */
export const Labelled: Story = {
  render: () => (
    <Harness fetchStub={labelled}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByLabelText("tier=gold, your label")).toBeVisible());
    await expect(canvas.getByLabelText("tier=observed-gold, automatic label")).toBeVisible();
  },
};

/** filtering narrows the grid to the routes carrying the label */
export const FilteredByLabel: Story = {
  render: () => (
    <Harness fetchStub={labelled}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("claude-sonnet").length).toBeGreaterThan(0));
    await userEvent.click(canvas.getByRole("combobox", { name: /Filter by label/i }));
    await userEvent.click(await within(document.body).findByRole("option", { name: "tier=gold" }));
    await waitFor(() => expect(canvas.queryByText("claude-sonnet")).toBeNull());
    await expect(canvas.getAllByText("gpt-4o").length).toBeGreaterThan(0);
  },
};

/**
 * A filter that matches nothing is not a project with no routes: the copy
 * blames the narrowing and offers to clear it rather than offering to add the
 * first route to a project that already has two.
 */
export const NoLabelMatch: Story = {
  render: () => (
    <Harness
      fetchStub={answer(ROUTES, 200, [
        { ...ROUTE_LABELS[0], id: "rl-3", subject_id: "route-missing", value: "silver" },
      ])}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("gpt-4o").length).toBeGreaterThan(0));
    await userEvent.click(canvas.getByRole("combobox", { name: /Filter by label/i }));
    await userEvent.click(
      await within(document.body).findByRole("option", { name: "tier=silver" }),
    );
    await waitFor(() => expect(canvas.getByText(/No routes match/i)).toBeVisible());
    await userEvent.click(canvas.getByRole("button", { name: /Clear filters/i }));
    await waitFor(() => expect(canvas.getAllByText("gpt-4o").length).toBeGreaterThan(0));
  },
};

/** the panel names the route it was opened from, and the observation is read-only */
export const LabelPanel: Story = {
  render: () => (
    <Harness fetchStub={labelled}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("gpt-4o").length).toBeGreaterThan(0));
    await userEvent.click(canvas.getByRole("button", { name: "Labels on gpt-4o" }));
    const panel = within(await within(document.body).findByRole("dialog"));
    await expect(await panel.findByText(/priced on every target/)).toBeVisible();
    await expect(panel.getByRole("button", { name: "Remove tier=gold" })).toBeVisible();
    await expect(panel.queryByRole("button", { name: "Remove tier=observed-gold" })).toBeNull();
  },
};
