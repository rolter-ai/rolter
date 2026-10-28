import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Models from "./Models";
import {
  Harness,
  cancelConfirmation,
  confirmDestructive,
  expectEmptyState,
  expectLoadError,
  expectAllowed,
  expectNoUxEvent,
  expectRefused,
  expectSheetClosed,
  expectListTable,
  expectNoFalseEmpty,
  expectSkeleton,
  json,
  NEEDS_SUPERADMIN,
  pending,
  recording,
  routes,
  scoped,
  Toasted,
  expectToast,
  expectUxEvent,
  recordUxEvents,
  uxEvents,
  type Recorder,
} from "./story-harness";
import type {
  BreakerStateDto,
  BreakerTargetDto,
  EffectiveModelDto,
  LabelRow,
  ProviderCacheVerdict,
  RouteRow,
  TimelineRow,
  UptimeRow,
} from "@/lib/api";
import { UxScreenProvider } from "@/lib/ux-react";

// each entry carries its own targets (#2210): provider names, not row ids
const MODELS: EffectiveModelDto[] = [
  {
    model: "gpt-4o",
    strategy: "weighted",
    targets: 2,
    route_targets: [
      { provider: "sim-a", model: null, weight: 3 },
      { provider: "sim-b", model: null, weight: 1 },
    ],
    org_id: "org-1",
    project_id: "project-1",
    source: "db",
  },
  {
    model: "claude-sonnet",
    strategy: "fastest",
    targets: 1,
    route_targets: [{ provider: "anthropic", model: "claude-sonnet-4-20250514", weight: 1 }],
    source: "config",
  },
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

const FANOUT_PROVIDERS = [
  { id: "prov-a", name: "sim-a" },
  { id: "prov-b", name: "sim-b" },
];

const loaded = routes([
  // longest first: `/models/prices` would otherwise be answered by `/models`
  ["/model-prices", () => []],
  ["/currency", () => ({ base: "USD", rates: {} })],
  ["/health/uptime", () => ({ data: [] })],
  ["/models", () => MODELS],
  ["/providers", () => []],
  ["/routes", () => []],
]);

const meta = {
  title: "Screens/Models",
  component: Models,
  parameters: { layout: "fullscreen" },
  beforeEach: recordUxEvents,
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
    await expectListTable(canvasElement, "Model Catalog");
    // a config-file route names its provider and its targets, which it used to
    // leave as dashes (#1979)
    await waitFor(() => expect(canvas.getByText("anthropic")).toBeVisible());
    await expect(canvas.getByRole("button", { name: "1 target for claude-sonnet" })).toBeVisible();
    // the strategy column sorts, like the columns beside it
    const strategy = canvas.getByRole("columnheader", { name: "Strategy" });
    await userEvent.click(within(strategy).getByRole("button"));
    await expect(strategy).toHaveAttribute("aria-sort", "ascending");
    const rows = canvas.getAllByRole("row").slice(1);
    await expect(rows[0]).toHaveTextContent("claude-sonnet");
    await expect(rows[1]).toHaveTextContent("gpt-4o");

    // origin filter using Segmented radiogroup
    const originFilter = canvas.getByRole("radiogroup", { name: "Filter by origin" });
    await expect(originFilter).toBeInTheDocument();
    const configRadio = within(originFilter).getByRole("radio", { name: "Config 1" });
    await userEvent.click(configRadio);
    await expect(canvas.queryByText("gpt-4o")).toBeNull();
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
    await expectNoFalseEmpty(canvasElement, /No models yet/);
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
    await expectEmptyState(canvasElement, /No models yet/, /Add route/);
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
    await expect(canvas.getByRole("button", { name: /Clear filters/i })).toBeInTheDocument();
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
    await expectNoFalseEmpty(canvasElement, /No models yet/);
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
    await expectNoFalseEmpty(canvasElement, /No models yet/);
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
        ["/health/uptime", () => ({ data: [] })],
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

// ------------------------------------------------ strategy and targets (#1979)

const FLEET_MODEL: EffectiveModelDto = {
  model: "llama-70b",
  strategy: "cache_aware",
  targets: 3,
  route_targets: [
    { provider: "vllm-a", model: "meta-llama/Llama-3.1-70B", weight: 1 },
    { provider: "vllm-b", model: "meta-llama/Llama-3.1-70B", weight: 1 },
    { provider: "vllm-c", model: "meta-llama/Llama-3.1-70B", weight: 2 },
  ],
  source: "db",
};

const uptimeRow = (patch: Partial<UptimeRow>): UptimeRow => ({
  provider: "vllm-a",
  target_id: "meta-llama/Llama-3.1-70B",
  grain: "target",
  sources: ["passive"],
  events: 2000,
  ok: 1999,
  errors: 1,
  timeouts: 0,
  uptime: 0.9995,
  failure_rate: 0.0005,
  error_budget_burn: 0.05,
  sla_breached: 0,
  last_event: "2026-09-29T08:00:00Z",
  ...patch,
});

// one replica healthy, one under its SLA, and one nothing has observed yet
const FLEET_UPTIME: UptimeRow[] = [
  uptimeRow({}),
  uptimeRow({ provider: "vllm-b", ok: 1940, errors: 60, uptime: 0.97, sla_breached: 1 }),
];

// #2256: vllm-b meets the SLA over the window, but its last two hours failed
// at 3x what the SLA allows
const AT_RISK_UPTIME: UptimeRow[] = [
  uptimeRow({}),
  uptimeRow({ provider: "vllm-b", ok: 1994, errors: 6, uptime: 0.997 }),
];

const AT_RISK_TIMELINE: TimelineRow[] = [
  ["vllm-a", 0, 0],
  ["vllm-a", 1, 0],
  ["vllm-b", 0, 0],
  ["vllm-b", 1, 3],
  ["vllm-b", 2, 3],
].map(([provider, hour, errors]) => ({
  bucket: `2026-09-29T${String(hour).padStart(2, "0")}:00:00Z`,
  provider: provider as string,
  target_id: "meta-llama/Llama-3.1-70B",
  grain: "target" as const,
  events: 100,
  ok: 100 - (errors as number),
  errors: errors as number,
  timeouts: 0,
}));

/**
 * A target that meets the SLA over seven days but is failing faster than it
 * allows right now reads "SLA at risk" in words, in the row and in the list.
 */
export const TargetSlaAtRisk: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/model-prices", () => []],
        ["/currency", () => ({ base: "USD", rates: {} })],
        ["/health/uptime", () => ({ data: AT_RISK_UPTIME })],
        ["/health/timeline", () => ({ data: AT_RISK_TIMELINE })],
        ["/models", () => [FLEET_MODEL]],
        ["/providers", () => []],
        ["/routes", () => []],
      ])}
    >
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("1 SLA at risk")).toBeVisible());
    await expect(canvas.queryByText(/below SLA/)).toBeNull();
    await userEvent.click(await canvas.findByRole("button", { name: "3 targets for llama-70b" }));
    const list = await canvas.findByRole("list", { name: "Targets of llama-70b" });
    const lines = within(list).getAllByRole("listitem");
    await expect(lines[0]).toHaveTextContent(/99\.95% uptime$/);
    await expect(lines[1]).toHaveTextContent(/vllm-b.*99\.70% uptime, SLA at risk/);
  },
};

/**
 * A `cache_aware` route over three vLLM replicas (#1979).
 *
 * The strategy is the identifier an operator writes in `rolter.toml`, in mono
 * and in its own case. The targets cell counts the replicas and names the one
 * below its SLA; the weights open on demand, one line per target, with health
 * where the rollup has it. `cache_aware` never reads weights, so the list says
 * so and states no traffic split.
 */
export const CacheAwareMultiTargetRoute: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/model-prices", () => []],
        ["/currency", () => ({ base: "USD", rates: {} })],
        ["/health/uptime", () => ({ data: FLEET_UPTIME })],
        ["/models", () => [FLEET_MODEL]],
        ["/providers", () => []],
        ["/routes", () => []],
      ])}
    >
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const strategy = await canvas.findByText("cache_aware");
    await expect(strategy).toHaveTextContent(/^cache_aware$/);
    // verbatim: not the uppercase pill that read `CACHE_AWARE`
    await expect(getComputedStyle(strategy).textTransform).toBe("none");
    await expect(strategy.closest("span.font-mono")).not.toBeNull();

    const toggle = await canvas.findByRole("button", { name: "3 targets for llama-70b" });
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await waitFor(() => expect(canvas.getByText("1 below SLA")).toBeVisible());
    // the weight column is gone: the first target's weight stood in for three
    await expect(canvas.queryByRole("columnheader", { name: "Weight" })).toBeNull();

    await userEvent.click(toggle);
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
    const list = await canvas.findByRole("list", { name: "Targets of llama-70b" });
    // the disclosure points at the region it opened
    const region = document.getElementById(toggle.getAttribute("aria-controls") ?? "");
    await expect(region).toContainElement(list);
    const lines = within(list).getAllByRole("listitem");
    await expect(lines).toHaveLength(3);
    await expect(lines[0]).toHaveTextContent(/vllm-a.*Llama-3\.1-70B.*weight 1.*99\.95% uptime/);
    await expect(lines[1]).toHaveTextContent(/vllm-b.*97\.00% uptime, below SLA/);
    await expect(lines[2]).toHaveTextContent(/vllm-c.*weight 2.*no health data/);
    await expect(canvas.getByText(/does not read weights/)).toHaveTextContent(/^cache_aware/);
    await expect(canvas.queryByText(/of traffic/)).toBeNull();
    // the opened row is a row of the table, with one cell across it
    await expectListTable(canvasElement, "Model Catalog");

    await userEvent.click(toggle);
    await waitFor(() => expect(canvas.queryByRole("list", { name: /Targets of/ })).toBeNull());
  },
};

/**
 * A weighted route states the split its weights make. Without the health
 * rollup — no ClickHouse here — the targets carry no health at all rather than
 * a column of "no data".
 */
export const WeightedRouteSplitsByWeight: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/health/uptime")) {
          return json({ error: { message: "analytics store not configured" } }, 503);
        }
        return oneRoutedModel(input);
      })}
    >
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "2 targets for gpt-4o" }));
    const list = within(await canvas.findByRole("list", { name: "Targets of gpt-4o" }));
    const lines = list.getAllByRole("listitem");
    await expect(lines[0]).toHaveTextContent(/sim-a.*weight 3.*75% of traffic/);
    await expect(lines[1]).toHaveTextContent(/sim-b.*weight 1.*25% of traffic/);
    await expect(canvas.queryByText(/uptime|no health data|below SLA/)).toBeNull();
  },
};

/**
 * A strategy with a caveat carries it in the row: `precise_cache_aware`
 * quietly falls back to least-load without a telemetry source. This one is
 * shipped in `rolter.toml`, and its targets come from there too.
 */
export const ConfigRouteCarriesItsCaveat: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/model-prices", () => []],
        // the sheet this story opens reads the deployment's currency table
        ["/currency", () => ({ settlement: "USD", codes: ["USD"] })],
        ["/health/uptime", () => ({ data: [] })],
        ["/models", () => [{ ...FLEET_MODEL, strategy: "precise_cache_aware", source: "config" }]],
        ["/providers", () => []],
        ["/routes", () => []],
      ])}
    >
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // written in the row, where the table's scroll frame cannot clip it
    const caveat = await canvas.findByText("needs telemetry");
    await expect(caveat).toBeVisible();
    await expect(caveat).toHaveAttribute(
      "title",
      expect.stringMatching(/falls back to least-load/),
    );
    await expect(await canvas.findByTitle("vllm-a, vllm-b, vllm-c")).toHaveTextContent(
      "vllm-a +2 more",
    );
    // and the whole sentence opens with the targets
    await userEvent.click(canvas.getByRole("button", { name: "3 targets for llama-70b" }));
    await expect(await canvas.findByRole("note")).toHaveTextContent(/falls back to least-load/);

    // the read-only sheet lists the same targets instead of a blank draft
    await userEvent.click(canvas.getByRole("button", { name: "View llama-70b" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    const lines = await within(
      dialog.getByRole("list", { name: "Targets of llama-70b" }),
    ).findAllByRole("listitem");
    await expect(lines).toHaveLength(3);
    await expect(dialog.getByLabelText("Strategy")).toHaveValue("precise_cache_aware");
  },
};

/**
 * A control plane that predates the embedded targets sends the count alone, so
 * the count is plain text with nothing to open rather than a list of none.
 */
export const TargetsFromAnOlderControlPlane: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/models")) return json([{ ...FLEET_MODEL, route_targets: undefined }]);
        if (url.includes("/currency")) return json({ base: "USD", rates: {} });
        if (url.includes("/health/uptime")) return json({ data: [] });
        return json([]);
      })}
    >
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("3 targets")).toBeVisible());
    await expect(canvas.queryByRole("button", { name: /targets for/ })).toBeNull();
  },
};

/**
 * Two organizations can each serve a `gpt-4o` (#2210). The catalog has two
 * entries, each with its own targets, and neither borrows the other's: the
 * first match by name used to stand in for both, and the shared name was also
 * the row key.
 */
export const SameNameInTwoOrganizations: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/model-prices", () => []],
        ["/currency", () => ({ base: "USD", rates: {} })],
        ["/health/uptime", () => ({ data: [] })],
        [
          "/models",
          () => [
            { ...MODELS[0], org_id: "org-1" },
            {
              ...MODELS[0],
              org_id: "org-2",
              targets: 1,
              route_targets: [{ provider: "other-edge", model: null, weight: 1 }],
            },
          ],
        ],
        ["/providers", () => []],
        ["/routes", () => []],
      ])}
    >
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("gpt-4o")).toHaveLength(2));
    await expect(canvas.getByTitle("sim-a, sim-b")).toBeVisible();
    await expect(canvas.getByText("other-edge")).toBeVisible();
    // opening one row opens that row only
    await userEvent.click(canvas.getByRole("button", { name: "1 target for gpt-4o" }));
    await expect(await canvas.findAllByRole("list", { name: "Targets of gpt-4o" })).toHaveLength(1);
  },
};

/**
 * Two organizations share a model name (#1857): each row deletes its own
 * organization's model, never every route of the name, and only the scope's
 * own project's row is matched to the scope's route.
 */
let sharedNameDeletes: Recorder;
export const SameNameDeletesOnlyThatOrganizationsModel: Story = {
  render: () => {
    const sharedName = routes([
      ["/model-prices", () => []],
      ["/currency", () => ({ base: "USD", rates: {} })],
      ["/health/uptime", () => ({ data: [] })],
      [
        "/models",
        () => [
          MODELS[0],
          {
            ...MODELS[0],
            org_id: "org-2",
            project_id: "project-9",
            targets: 1,
            route_targets: [{ provider: "other-edge", model: null, weight: 1 }],
          },
        ],
      ],
      ["/providers", () => FANOUT_PROVIDERS],
      ["/routes", () => [FANOUT_ROUTE]],
    ]);
    sharedNameDeletes = recording(
      scoped(async (input, init) =>
        init?.method === "DELETE" ? json(null, 204) : sharedName(input, init),
      ),
    );
    return (
      <Harness fetchStub={sharedNameDeletes.stub}>
        <Models />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("gpt-4o")).toHaveLength(2));
    const other = canvas.getByText("other-edge").closest('[role="row"]') as HTMLElement;
    await userEvent.click(within(other).getByRole("button", { name: "Delete model gpt-4o" }));
    await confirmDestructive(/every route and target/i, "Delete model");
    await sharedNameDeletes.expectSent("DELETE", "/models/gpt-4o?org_id=org-2");
    sharedNameDeletes.expectNotSent("DELETE", "org_id=org-1");
  },
};

/** A route with no target has nowhere to send a request, and the row says so. */
export const RouteWithNoTargets: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/model-prices", () => []],
        ["/currency", () => ({ base: "USD", rates: {} })],
        ["/health/uptime", () => ({ data: [] })],
        ["/models", () => [{ ...FLEET_MODEL, targets: 0, route_targets: [] }]],
        ["/providers", () => []],
        ["/routes", () => []],
      ])}
    >
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("no targets")).toBeVisible());
    await expect(canvas.queryByRole("button", { name: /targets for/ })).toBeNull();
  },
};

// a catalog with one db-backed row that has a route behind it, so the edit
// control is enabled for anyone the gate allows
const oneRoutedModel = routes([
  ["/model-prices", () => []],
  ["/currency", () => ({ base: "USD", rates: {} })],
  ["/health/uptime", () => ({ data: [] })],
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
      <UxScreenProvider screen="model-catalog">
        <Toasted>
          <Models />
        </Toasted>
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Delete model gpt-4o" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.click(dialog.getByRole("button", { name: "Delete model" }));
    // the stub refuses in the same tick, which ConfirmDialog used to miss
    // (#1761): the refusal is a row of its own beside the press
    await waitFor(() =>
      expect(
        uxEvents()
          .filter((e) => e.action === "form_submit" && e.target === "model-delete")
          .map((e) => e.outcome),
      ).toEqual(["ok", "error"]),
    );
    expectNoUxEvent("save_confirmed", "model-delete");

    await expectToast(canvasElement, /referenced by 2 virtual keys/, "error");
    await waitFor(() => expect(dialog.getByText(/referenced by 2 virtual keys/)).toBeVisible());
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
  },
};

/**
 * The delete confirms through the shared `ConfirmDialog` (#1760): the title
 * names the model, a cancel sends nothing and is an abandon, and a confirm
 * sends the DELETE and lands as `save_confirmed`.
 */
let modelDeletes: Recorder;
export const DeleteIsConfirmedAndReported: Story = {
  render: () => {
    modelDeletes = recording(
      scoped(async (input, init) =>
        init?.method === "DELETE" ? json(null, 204) : oneRoutedModel(input, init),
      ),
    );
    return (
      <Harness fetchStub={modelDeletes.stub}>
        <UxScreenProvider screen="model-catalog">
          <Models />
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const remove = await canvas.findByRole("button", { name: "Delete model gpt-4o" });
    await userEvent.click(remove);
    await expect(
      await within(document.body).findByRole("heading", { name: "Delete model gpt-4o?" }),
    ).toBeInTheDocument();
    await cancelConfirmation();
    modelDeletes.expectNotSent("DELETE", "/models/");
    const abandon = await expectUxEvent("form_abandon", "model-delete");
    await expect(abandon.screen).toBe("model-catalog");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "model-delete");

    await userEvent.click(remove);
    await confirmDestructive(/every route and target/i, "Delete model");
    await modelDeletes.expectSent("DELETE", "/models/gpt-4o");
    await expectSheetClosed();
    const submit = await expectUxEvent("form_submit", "model-delete");
    await expect(submit.outcome).toBe("ok");
    await expectUxEvent("save_confirmed", "model-delete");
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
    await expectRefused(canvasElement, /add route/i);
    await expectRefused(canvasElement, "Edit gpt-4o");
    await expectRefused(canvasElement, "Delete model gpt-4o", NEEDS_SUPERADMIN);
  },
};

// the row's edit and delete used to be disabled from the screen's own
// `useGate`, which disabled them silently: a viewer reaching for either left no
// trace in the UX stream. Through the gated primitives each reach is recorded,
// under a slug that says which of the two it was (#1759)
export const RefusedRowRecordsTheReach: Story = {
  render: () => (
    <Harness fetchStub={oneRoutedModel} role="viewer">
      <UxScreenProvider screen="models">
        <Models />
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectRefused(canvasElement, "Delete model gpt-4o", NEEDS_SUPERADMIN);
    await userEvent.click(canvas.getByRole("button", { name: "Delete model gpt-4o" }), {
      pointerEventsCheck: 0,
    });
    await expectUxEvent("refused_click", "model-delete:model:delete");

    await expectRefused(canvasElement, "Edit gpt-4o");
    await userEvent.click(canvas.getByRole("button", { name: "Edit gpt-4o" }), {
      pointerEventsCheck: 0,
    });
    await expectUxEvent("refused_click", "model-edit:route:update");
  },
};

export const DeleteRefusedToAnAdmin: Story = {
  render: () => (
    <Harness fetchStub={oneRoutedModel} role="admin">
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Delete model gpt-4o", NEEDS_SUPERADMIN);
    // the route half of the screen is still theirs, read after the gate has
    // answered rather than before it (#1707)
    await expectAllowed(canvasElement, "Edit gpt-4o");
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
    await userEvent.click(canvas.getByRole("button", { name: /Clear filters/i }));
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
    const panel = await within(document.body).findByRole("dialog");
    await waitFor(() => expect(within(panel).getByText(/a price row exists/)).toBeVisible());
    // `expectRefused` rather than a bare `toBeDisabled`, on both counts. The
    // sheet fires its own label query as it opens and the effective-permissions
    // answer is a request behind that again, so a one-shot assertion here is
    // only ever reading whichever of the two happened to land first (#1689) —
    // and the `title` half is what distinguishes a refusal from `Add label`
    // being disabled for its own reason, an empty key, which is what made that
    // line pass while the gate was still in flight
    await expectRefused(panel, "Add label", NEEDS_SUPERADMIN);
    await expectRefused(panel, "Remove tier=flagship", NEEDS_SUPERADMIN);
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
    const dialog = await within(document.body).findByRole("dialog");
    const panel = within(dialog);
    // the sheet reads its own labels, so the chips arrive a request after the
    // dialog does — until then the body is a skeleton and there is no remove
    // control to assert on at all (#1689) — and the gate is a request behind
    // that again, which a bare `toBeEnabled` would never notice (#1707)
    await expectAllowed(dialog, "Remove tier=flagship");
    // still no way to touch the observation, asked once the list is on screen —
    // before that the absence would be the skeleton's, not the rule's
    await expect(panel.queryByRole("button", { name: "Remove tier=priced" })).toBeNull();
  },
};

// ------------------------------------------------ live circuit breakers (#2254)

const breakerTarget = (patch: Partial<BreakerTargetDto>): BreakerTargetDto => ({
  model: "llama-70b",
  target_index: 0,
  provider: "vllm-a",
  upstream_model: "meta-llama/Llama-3.1-70B",
  state: "open",
  nodes_open: 2,
  nodes_half_open: 0,
  nodes: [],
  ...patch,
});

const breakerState = (targets: BreakerTargetDto[], gateways_live = 3): BreakerStateDto => ({
  generated_at: "2026-09-29T08:00:00Z",
  fresh_window_secs: 40,
  gateways_live,
  targets,
});

const withBreakers = (answer: () => Response | Promise<Response>) =>
  scoped(async (input, init) => {
    if (String(input).includes("/breaker-state")) return answer();
    return routes([
      ["/model-prices", () => []],
      ["/currency", () => ({ base: "USD", rates: {} })],
      ["/health/uptime", () => ({ data: FLEET_UPTIME })],
      ["/models", () => [FLEET_MODEL]],
      ["/providers", () => []],
      ["/routes", () => []],
    ])(input, init);
  });

/**
 * A target whose breaker is open on two gateways says so beside its health, and
 * one letting a trial request through says it is probing. A target the
 * gateways list nowhere is closed everywhere and shows nothing new; the model's
 * row names the worst breaker without opening the list.
 */
export const BreakerOpenAndProbing: Story = {
  render: () => (
    <Harness
      fetchStub={withBreakers(() =>
        json(
          breakerState([
            breakerTarget({ target_index: 0, state: "open", nodes_open: 2 }),
            breakerTarget({
              target_index: 1,
              provider: "vllm-b",
              state: "half_open",
              nodes_open: 0,
              nodes_half_open: 1,
            }),
          ]),
        ),
      )}
    >
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const toggle = await canvas.findByRole("button", { name: "3 targets for llama-70b" });
    await waitFor(() => expect(canvas.getByText("Breaker open on 2 gateways")).toBeVisible());
    await userEvent.click(toggle);
    const list = await canvas.findByRole("list", { name: "Targets of llama-70b" });
    const lines = within(list).getAllByRole("listitem");
    await expect(lines[0]).toHaveTextContent(/vllm-a.*Breaker open on 2 gateways/);
    await expect(lines[1]).toHaveTextContent(/vllm-b.*Probing on 1 gateway/);
    // the third target is closed on every gateway: nothing is added to its line
    await expect(lines[2]).not.toHaveTextContent(/Breaker|Probing/);
  },
};

/** a closed breaker row (failing, not yet tripped) and an empty answer add nothing */
export const BreakerClosedShowsNothing: Story = {
  render: () => (
    <Harness
      fetchStub={withBreakers(() =>
        json(breakerState([breakerTarget({ state: "closed", nodes_open: 0 })])),
      )}
    >
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole("button", { name: "3 targets for llama-70b" });
    await waitFor(() => expect(canvas.getByText("1 below SLA")).toBeVisible());
    await expect(canvas.queryByTestId("breaker-badge")).toBeNull();
  },
};

/** no breaker state (a control plane without a store) leaves the catalog as it was */
export const BreakerStateUnavailable: Story = {
  render: () => (
    <Harness fetchStub={withBreakers(() => json({ error: { message: "no store" } }, 404))}>
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole("button", { name: "3 targets for llama-70b" });
    await waitFor(() => expect(canvas.getByText("1 below SLA")).toBeVisible());
    await expect(canvas.queryByTestId("breaker-badge")).toBeNull();
    await expect(canvas.queryByRole("alert")).toBeNull();
  },
};

// --- whether the providers behind a route report prompt-cache metrics (#984)

const CACHE_PROVIDERS = [
  { id: "prov-a", name: "sim-a" },
  { id: "prov-b", name: "sim-b" },
  { id: "prov-c", name: "anthropic" },
];

const verdict = (provider_id: string, reports: boolean): ProviderCacheVerdict => ({
  provider_id,
  reports,
  spelling: reports ? "cached_tokens" : null,
  model: "probe-model",
  probed_at: "2026-10-01T10:00:00Z",
});

const withCache = (read: () => unknown, status = 200) =>
  routes(
    [
      ["/provider-cache-probes", read],
      ["/model-prices", () => []],
      ["/currency", () => ({ base: "USD", rates: {} })],
      ["/health/uptime", () => ({ data: [] })],
      ["/models", () => MODELS],
      ["/providers", () => CACHE_PROVIDERS],
      ["/routes", () => []],
    ],
    status,
  );

const rowOf = async (canvas: ReturnType<typeof within>, name: string) =>
  within((await canvas.findByText(name)).closest('[role="row"]') as HTMLElement);

/**
 * A route reports only when every provider behind it does: gpt-4o's two
 * providers both do, claude-sonnet's has never been probed and so is "not
 * probed yet", not "no".
 */
export const CacheReportingNeedsEveryProviderToReport: Story = {
  render: () => (
    <Harness fetchStub={withCache(() => [verdict("prov-a", true), verdict("prov-b", true)])}>
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await (await rowOf(canvas, "gpt-4o")).findByText(/^Reports cache/)).toBeVisible();
    await expect(
      await (await rowOf(canvas, "claude-sonnet")).findByText("Not probed yet"),
    ).toBeVisible();
  },
};

/** One provider known not to report makes the route "no cache metrics", whatever its sibling says. */
export const CacheReportingOneSilentProviderMakesTheRouteSilent: Story = {
  render: () => (
    <Harness fetchStub={withCache(() => [verdict("prov-a", true), verdict("prov-b", false)])}>
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const badge = await (await rowOf(canvas, "gpt-4o")).findByText(/^No cache metrics/);
    await expect(badge).toBeVisible();
    await expect(badge).toHaveAttribute("title", expect.stringContaining("1 of 2 providers"));
  },
};

/** A reporting provider beside an unprobed one proves nothing about the route. */
export const CacheReportingPartlyProbedIsNotProbedYet: Story = {
  render: () => (
    <Harness fetchStub={withCache(() => [verdict("prov-a", true)])}>
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const row = await rowOf(canvas, "gpt-4o");
    await expect(await row.findByText("Not probed yet")).toBeVisible();
    await expect(row.queryByText(/^Reports cache/)).toBeNull();
  },
};

/** A failed read of the verdicts shows no badge on any route, and the catalog still loads. */
export const CacheReportingFailedReadShowsNothing: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/provider-cache-probes"))
          return json({ error: { message: "boom" } }, 500);
        return withCache(() => [])(input);
      })}
    >
      <Models />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("gpt-4o")).toBeVisible());
    await waitFor(() => expect(canvas.getByText("sim-a +1 more")).toBeVisible());
    await expect(canvas.queryByText(/Not probed yet|Reports cache|No cache metrics/)).toBeNull();
  },
};
