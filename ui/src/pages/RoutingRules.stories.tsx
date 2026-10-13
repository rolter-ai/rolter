import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import RoutingRules from "./RoutingRules";
import {
  Harness,
  Toasted,
  cancelConfirmation,
  confirmDestructive,
  expectAllowed,
  expectRefused,
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
  expectAnalyticsUnavailable,
  expectNoFalseEmpty,
  openOptions,
  stubClipboard,
} from "./story-harness";
import { atMobile } from "@/lib/story-viewport";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import type {
  AnalyticsByModelRow,
  ClusterNodeRow,
  LabelRow,
  ProviderRow,
  RouteRow,
  RouteTargetRow,
  TimelineRow,
  UptimeRow,
} from "@/lib/api";

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
    strategy: "fastest",
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

// a route's name is also the upstream model a target without one forwards, and
// the targets are on the card from the first paint now (#3031), so the name is
// found by its own span
const ROUTE_NAME = { selector: "span.font-semibold" };

const trafficRow = (model: string, requests: number, errors: number): AnalyticsByModelRow => ({
  model,
  requests,
  tokens: requests * 900,
  cost_usd: requests * 0.004,
  unpriced_requests: 0,
  errors,
  p50_latency_ms: 420,
  p95_latency_ms: 1900,
});

// gpt-4o took traffic over the last day; claude-sonnet and qwen-coder took none
const TRAFFIC: AnalyticsByModelRow[] = [trafficRow("gpt-4o", 1240, 31)];

const node = (id: string, over: Partial<ClusterNodeRow> = {}): ClusterNodeRow => ({
  id,
  role: "gateway",
  build_version: "0.9.0",
  config_version: 42,
  desired_state: "active",
  state_changed_at: "2026-10-01T00:00:00Z",
  first_seen_at: "2026-09-01T00:00:00Z",
  last_seen_at: "2026-10-01T00:00:00Z",
  live: true,
  converged: true,
  ...over,
});

const answer = (routes: RouteRow[], status = 200, labels: LabelRow[] = []): FetchStub =>
  scoped(async (input) => {
    const url = String(input);
    if (url.includes("/providers")) return json(PROVIDERS);
    if (url.includes("/analytics/by-model")) return json({ data: TRAFFIC });
    if (url.includes("/cluster/nodes")) return json([node("gw-1"), node("gw-2")]);
    // the label list is its own endpoint; without this branch it would be
    // answered with the route list and the chips would be nonsense (#1329)
    if (url.includes("/labels")) {
      const subject = new URL(url, "http://localhost").searchParams.get("subject_id");
      return json(subject ? labels.filter((l) => l.subject_id === subject) : labels);
    }
    const targets = /\/routes\/([^/]+)\/targets/.exec(url);
    if (targets) return json(TARGETS[targets[1]] ?? []);
    // the route list embeds each route's targets when it is asked to, which
    // is how the screen reads them (#3031)
    if (status === 200 && url.includes("include=targets")) {
      return json(routes.map((r) => ({ ...r, targets: TARGETS[r.id] ?? [] })));
    }
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
    await expect(await canvas.findByText("gpt-4o", ROUTE_NAME)).toBeInTheDocument();
    // gpt-4o is `weighted`, so its weights are shares of the route's total and
    // 80/100 is 80%
    const targets = within(await canvas.findByRole("list", { name: "Targets of gpt-4o" }));
    await expect(targets.getByText("80%")).toBeInTheDocument();
    await expect(targets.getByText("20%")).toBeInTheDocument();
    await expect(targets.getAllByTestId("target-share")).toHaveLength(2);
    await expect(canvas.getByText("disabled")).toBeInTheDocument();
    // the count arrives with the list it counts
    await expect(canvas.getByText(/^2 routes/)).toBeInTheDocument();
    // a route is named verbatim and never in capitals (#2139)
    await expect(canvas.getByText("weighted")).toBeVisible();
    await expect(canvas.queryByText("WEIGHTED")).toBeNull();
    // the target bars take the categorical tokens, not the ornament red
    const fills = within(
      await canvas.findByRole("list", { name: "Targets of gpt-4o" }),
    ).getAllByTestId("target-share");
    await expect(fills[0].firstElementChild?.getAttribute("style")).toContain("var(--chart-1)");
    await expect(fills[1].firstElementChild?.getAttribute("style")).toContain("var(--chart-2)");
  },
};

// ------------------------------------------- traffic, playground link, config (#2138)

const card = async (canvas: ReturnType<typeof within>, model: string) =>
  within((await canvas.findByText(model, ROUTE_NAME)).closest("div.flex-col") as HTMLElement);

/**
 * A route with traffic says how many requests it served over the last day and
 * what share failed; one that took none says so in words, since the rollup
 * answered. Each enabled route links the Playground with its model selected.
 */
export const RouteTraffic: Story = {
  render: () => (
    <Harness fetchStub={answer(ROUTES)}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const gpt = await card(canvas, "gpt-4o");
    await expect(await gpt.findByTestId("route-traffic")).toHaveTextContent(
      "1,240 requests for this model name, org-wide, in the last 24 h · 2.5% errors",
    );
    const claude = await card(canvas, "claude-sonnet");
    await expect(await claude.findByTestId("route-traffic")).toHaveTextContent(
      "No requests for this model name, org-wide, in the last 24 h",
    );
    const tryIt = gpt.getByRole("link", { name: "Try gpt-4o in the Playground" });
    await expect(tryIt).toHaveAttribute("href", "/playground?model=gpt-4o");
    // a disabled route is not served, so there is nothing to try
    await expect(claude.queryByRole("link", { name: /Try claude-sonnet/ })).toBeNull();
  },
};

/** The rollup still out: each card holds a skeleton, not a zero. */
export const TrafficLoading: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        String(input).includes("/analytics/by-model")
          ? new Promise<Response>(() => {})
          : answer(ROUTES)(input, init),
      )}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("gpt-4o", ROUTE_NAME)).toBeInTheDocument();
    await expect((await canvas.findAllByTestId("route-traffic-loading")).length).toBe(2);
    await expect(canvas.queryByTestId("route-traffic")).toBeNull();
    await expect(canvas.queryByText(/No requests for this model name/)).toBeNull();
  },
};

/**
 * A deployment without ClickHouse answers the rollup with a 503: the screen
 * says so once, calmly, and no card claims "no requests" about traffic nobody
 * could count.
 */
export const TrafficNeedsClickHouse: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        String(input).includes("/analytics/by-model")
          ? json({ error: { message: "analytics store not configured" } }, 503)
          : answer(ROUTES)(input, init),
      )}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectAnalyticsUnavailable(
      canvasElement,
      en.pages.routing.noAnalytics.title,
      "analytics store not configured",
    );
    await expect(canvas.queryAllByTestId("route-traffic")).toHaveLength(0);
    await expect(canvas.queryByText(/No requests for this model name/)).toBeNull();
    // the cards still render, and the playground link does not need analytics
    await expect(
      canvas.getByRole("link", { name: "Try gpt-4o in the Playground" }),
    ).toBeInTheDocument();
  },
};

/** Any other failure of the rollup is stated once, in the control plane's words. */
export const TrafficReadFails: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        String(input).includes("/analytics/by-model")
          ? json({ error: { message: "clickhouse: timeout" } }, 500)
          : answer(ROUTES)(input, init),
      )}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const note = await canvas.findByTestId("traffic-unavailable");
    await expect(note).toHaveTextContent(/Request counts are unavailable/);
    await expect(note).toHaveTextContent(/The control plane hit an error/);
    await expect(canvas.queryAllByTestId("route-traffic")).toHaveLength(0);
  },
};

/** Every live gateway on the latest config: the screen says so and links Cluster. */
export const GatewaysCurrent: Story = {
  render: () => (
    <Harness fetchStub={answer(ROUTES)}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const note = await within(canvasElement).findByTestId("gateway-config");
    await expect(note).toHaveTextContent("All 2 live gateways run the latest config.");
    await expect(within(note).getByRole("link", { name: "Open Cluster" })).toHaveAttribute(
      "href",
      "/cluster",
    );
  },
};

/** A gateway that has not reached the latest config is counted, not hidden. */
export const GatewayBehind: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        String(input).includes("/cluster/nodes")
          ? json([
              node("gw-1"),
              node("gw-2", { converged: false, config_version: 41 }),
              // a drained, a dead and a control-plane node are not gateways to wait for
              node("gw-3", { converged: false, desired_state: "draining" }),
              node("gw-4", { converged: false, live: false }),
              node("cp-1", { role: "control", converged: false }),
            ])
          : answer(ROUTES)(input, init),
      )}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const note = await within(canvasElement).findByTestId("gateway-config");
    await expect(note).toHaveTextContent(
      "1 of 2 live gateways has not picked up the latest config yet.",
    );
  },
};

/** No live gateway, or an inventory that cannot be read, says nothing. */
export const NoGatewayNote: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        String(input).includes("/cluster/nodes")
          ? json({ error: { message: "forbidden" } }, 403)
          : answer(ROUTES)(input, init),
      )}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("gpt-4o", ROUTE_NAME)).toBeInTheDocument();
    await expect((await canvas.findAllByTestId("route-traffic")).length).toBeGreaterThan(0);
    await expect(canvas.queryByTestId("gateway-config")).toBeNull();
  },
};

/** The public model name copies in one click. */
export const CopiesTheModelName: Story = {
  beforeEach: () => {
    copied = [];
    return stubClipboard(async (value) => {
      copied.push(value);
    })();
  },
  render: () => (
    <Harness fetchStub={answer(ROUTES)}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /Copy model name.*gpt-4o/ }));
    await waitFor(() => expect(copied).toEqual(["gpt-4o"]));
  },
};
let copied: string[] = [];

/**
 * At 375 px in Russian the strategy and the labels wrap under the route name
 * instead of squeezing it to an ellipsis (#2139).
 */
export const NarrowRussianKeepsTheNameWhole: Story = {
  ...atMobile,
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={answer(ROUTES)}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const name = await canvas.findByText("claude-sonnet");
    await waitFor(() => expect(canvas.getByText(ru.pages.routing.disabledBadge)).toBeVisible());
    await expect(name.scrollWidth).toBeLessThanOrEqual(name.clientWidth);
    await expect(canvas.getByRole("button", { name: ru.pages.routing.add })).toBeInTheDocument();
  },
};

/** The route sheet's picker offers adaptive, with descriptions, from a route. */
export const EditSheetOffersAdaptive: Story = {
  render: () => (
    <Harness fetchStub={answer(ROUTES)} role="admin">
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectAllowed(canvasElement, "Edit route gpt-4o");
    await userEvent.click(await canvas.findByRole("button", { name: "Edit route gpt-4o" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    const picker = await dialog.findByLabelText("Strategy");
    const listbox = within(await openOptions(picker));
    await expect(listbox.getByRole("option", { name: /^adaptive\b/ })).toHaveTextContent(
      en.strategies.descriptions.adaptive,
    );
    await userEvent.click(listbox.getByRole("option", { name: /^adaptive\b/ }));
    await waitFor(() => expect(picker).toHaveValue("adaptive"));
    await expect(
      await dialog.findByRole("link", { name: "Adaptive Routing settings" }),
    ).toHaveAttribute("href", "/adaptive-settings");
  },
};

/**
 * "Add route" opens the route sheet, the same one Model Catalog opens (#1979).
 * The two screens used to create the same route through two forms that asked
 * for different things; a route now gets its strategy, its targets and the
 * rest of its settings in one place, whichever screen it starts from. The
 * sheet is called what the button was: a route (#2810).
 */
let creates: Recorder;
export const AddRouteOpensTheRouteSheet: Story = {
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
    await waitFor(() => expect(dialog.getByRole("heading", { name: "Add route" })).toBeVisible());
    // two providers to choose between, so the target starts on neither: the
    // first one alphabetically is not where this route's traffic should go
    await expect(await dialog.findByLabelText("Target 1 provider")).toHaveValue("");
    await userEvent.type(dialog.getByLabelText("Route name"), "llama-70b");
    await pickOption(dialog.getByLabelText("Target 1 provider"), "azure-west");
    await pickOption(dialog.getByLabelText("Strategy"), /^power_of_two\b/);
    await userEvent.click(dialog.getByRole("button", { name: "Add route" }));
    await expect(await creates.expectSentBody("POST", `/projects/${PROJECT.id}/routes`)).toEqual({
      model: "llama-70b",
      strategy: "power_of_two",
    });
    await expect(await creates.expectSentBody("POST", "/routes/route-new/targets")).toMatchObject({
      provider_id: "prov-2",
    });
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

/**
 * A failing route list says so with the control plane's own words and a retry
 * that works; no zero count and no "No routes yet" stands in for it (#2139).
 */
let routesDown = true;
export const ServerError: Story = {
  render: () => {
    routesDown = true;
    return (
      <Harness
        fetchStub={scoped(async (input, init) => {
          const url = String(input);
          if (routesDown && /\/projects\/[^/]+\/routes/.test(url)) {
            return json({ error: { message: "routes: connection refused" } }, 500);
          }
          return answer(ROUTES)(input, init);
        })}
      >
        <RoutingRules />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /routes/);
    await expect(canvas.getByText(/connection refused/)).toBeInTheDocument();
    await expectNoFalseEmpty(canvasElement, /No routes yet/);
    routesDown = false;
    await userEvent.click(canvas.getByRole("button", { name: /Try again/ }));
    await expect(await canvas.findByText("gpt-4o", ROUTE_NAME)).toBeInTheDocument();
    await waitFor(() => expect(canvas.queryByRole("alert")).toBeNull());
  },
};

// --------------------------------------------------- embedded targets (#2133, #3031)

let reads: Recorder;

/**
 * The targets come with the route list, so a project of N routes costs one
 * request and none per route. Each route's targets are the ones its own row
 * carried; claude-sonnet really has none, so that copy appears exactly once.
 */
export const TargetsComeWithTheRoutes: Story = {
  render: () => {
    reads = recording(answer(ROUTES));
    return (
      <Harness fetchStub={reads.stub}>
        <RoutingRules />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("gpt-4o", ROUTE_NAME)).toBeInTheDocument();
    await expect(await canvas.findByText("No targets yet.")).toBeInTheDocument();
    await expect(canvas.getAllByText("No targets yet.")).toHaveLength(1);
    await expect(canvas.getAllByText("0 targets")).toHaveLength(1);
    await expect(canvas.getByText("2 targets")).toBeInTheDocument();
    // one list read asks for the targets, and no route is read for them again
    const listReads = reads.calls.filter((c) => /\/projects\/[^/]+\/routes/.test(c.url));
    await expect(listReads).toHaveLength(1);
    await expect(listReads[0].url).toContain("include=targets");
    await expect(reads.calls.filter((c) => /\/routes\/[^/?]+\/targets/.test(c.url))).toHaveLength(
      0,
    );
  },
};

/**
 * A control plane that predates the embedded targets returns routes without
 * them. That is not a route with no targets, so the card says it could not
 * read them rather than "No targets yet." or "0 targets" (#2133).
 */
export const TargetsUnreadFromAnOlderControlPlane: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        const url = String(input);
        if (/\/projects\/[^/]+\/routes/.test(url)) return json(ROUTES);
        return answer(ROUTES)(input, init);
      })}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("gpt-4o", ROUTE_NAME)).toBeInTheDocument();
    await expect(canvas.getAllByText("Targets could not be read.")).toHaveLength(2);
    await expect(canvas.queryByText("No targets yet.")).toBeNull();
    await expect(canvas.queryByText("0 targets")).toBeNull();
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

// --------------------------------------------------- target health (#2246)

const uptimeRow = (patch: Partial<UptimeRow>): UptimeRow => ({
  provider: "openai-prod",
  target_id: "gpt-4o-2024-08-06",
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

// openai-prod is healthy, azure-west is under its SLA, and nothing has observed
// the qwen-coder targets of route-3
const HEALTH_UPTIME: UptimeRow[] = [
  uptimeRow({}),
  uptimeRow({
    provider: "azure-west",
    target_id: "gpt-4o",
    ok: 1940,
    errors: 60,
    uptime: 0.97,
    sla_breached: 1,
  }),
];

const withHealth = (uptime: () => Response | Promise<Response>, routes = ROUTES) =>
  scoped(async (input, init) => {
    const url = String(input);
    if (url.includes("/health/uptime")) return uptime();
    if (url.includes("/health/timeline")) return json({ data: [] });
    return answer(routes)(input, init);
  });

/**
 * Each target says where it stands against the SLA in words beside its dot,
 * from the rollup Model Catalog and Health read. A target nothing has observed
 * says "no health data" rather than borrowing a colour.
 */
export const TargetHealthStates: Story = {
  render: () => (
    <Harness fetchStub={withHealth(() => json({ data: HEALTH_UPTIME }), [ROUTES[0], CACHE_AWARE])}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const gpt = within(await canvas.findByRole("list", { name: "Targets of gpt-4o" }));
    await expect(await gpt.findByText("99.95% uptime")).toBeInTheDocument();
    await expect(gpt.getByText("97.00% uptime, below SLA")).toBeInTheDocument();
    const qwen = within(canvas.getByRole("list", { name: "Targets of qwen-coder" }));
    await expect(qwen.getAllByText("no health data")).toHaveLength(2);
    await expect(canvas.queryByTestId("health-unavailable")).toBeNull();
  },
};

/**
 * A target that meets the SLA over the week but whose last two hours failed
 * faster than it allows reads "SLA at risk", the same state Models draws.
 */
export const TargetHealthAtRisk: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        const url = String(input);
        if (url.includes("/health/uptime"))
          return json({ data: [uptimeRow({ ok: 1994, errors: 6, uptime: 0.997 })] });
        if (url.includes("/health/timeline")) {
          const rows: TimelineRow[] = [0, 1, 2].map((hour) => ({
            bucket: `2026-09-29T0${hour}:00:00Z`,
            provider: "openai-prod",
            target_id: "gpt-4o-2024-08-06",
            grain: "target",
            events: 100,
            ok: hour === 0 ? 100 : 97,
            errors: hour === 0 ? 0 : 3,
            timeouts: 0,
          }));
          return json({ data: rows });
        }
        return answer([ROUTES[0]])(input, init);
      })}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("99.70% uptime, SLA at risk")).toBeInTheDocument();
  },
};

/**
 * A deployment with no analytics store answers the rollup with a 503. The
 * screen says so once, the cards still render, and no row claims "no health
 * data" about a target nobody asked after.
 */
export const TargetHealthUnavailable: Story = {
  render: () => (
    <Harness
      fetchStub={withHealth(() =>
        json({ error: { message: "analytics store not configured" } }, 503),
      )}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const note = await canvas.findByTestId("health-unavailable");
    await expect(note).toHaveTextContent(/Target health is unavailable/);
    await expect(note).toHaveTextContent(/The control plane hit an error/);
    await expect(canvas.getAllByTestId("health-unavailable")).toHaveLength(1);
    const targets = within(await canvas.findByRole("list", { name: "Targets of gpt-4o" }));
    await expect(targets.getByText("80%")).toBeInTheDocument();
    await expect(canvas.queryByText(/no health data|uptime/)).toBeNull();
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
    await expect(await canvas.findByText("gpt-4o", ROUTE_NAME)).toBeInTheDocument();
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
    await expect(await canvas.findByText("gpt-4o", ROUTE_NAME)).toBeInTheDocument();
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
    await expect(await canvas.findByText("gpt-4o", ROUTE_NAME)).toBeInTheDocument();
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
    await expectRefused(canvasElement, "Edit route gpt-4o");
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
    await expectRefused(canvasElement, "Edit route claude-sonnet");
  },
};

// ------------------------------------------------------ route editor (#2134)

interface RouteUpdate {
  enabled?: boolean;
  strategy?: string;
  targets?: { provider_id: string; upstream_model: string | null; weight: number }[];
}

const updating = (stored: RouteTargetRow[] = TARGETS["route-1"]) =>
  recording(
    scoped(async (input, init) => {
      const url = String(input);
      if (init?.method === "PUT" && /\/routes\/route-1$/.test(url)) {
        return json({ ...ROUTES[0], strategy: "round_robin" });
      }
      const targets = /\/routes\/route-1\/targets/.exec(url);
      if (targets) return json(stored);
      return answer(ROUTES)(input, init);
    }),
  );

/**
 * A two-target route is edited in one sheet: the strategy and a weight change,
 * and the whole target list goes out in one PUT. The target with no upstream
 * model goes back as null, so the control plane matches it to the stored one
 * and keeps its id rather than replacing it.
 */
let edits: Recorder;
export const EditsATwoTargetRoute: Story = {
  render: () => {
    edits = updating();
    return (
      <Harness fetchStub={edits.stub} role="admin">
        <Toasted>
          <RoutingRules />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectAllowed(canvasElement, "Edit route gpt-4o");
    await userEvent.click(await canvas.findByRole("button", { name: "Edit route gpt-4o" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await waitFor(() => expect(dialog.getByRole("heading", { name: "Edit route" })).toBeVisible());
    await expect(await dialog.findByLabelText("Target 1 upstream model")).toHaveValue(
      "gpt-4o-2024-08-06",
    );
    await expect(dialog.getByLabelText("Target 2 weight")).toHaveValue(20);
    await expect(dialog.getByLabelText("Strategy")).toHaveValue("weighted");
    await pickOption(dialog.getByLabelText("Strategy"), /^round_robin\b/);
    const weight = dialog.getByLabelText("Target 2 weight");
    await userEvent.clear(weight);
    await userEvent.type(weight, "30");
    await userEvent.click(dialog.getByRole("switch", { name: "Enabled" }));
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await expect(await edits.expectSentBody<RouteUpdate>("PUT", "/api/v1/routes/route-1")).toEqual({
      enabled: false,
      strategy: "round_robin",
      targets: [
        { provider_id: "prov-1", upstream_model: "gpt-4o-2024-08-06", weight: 80 },
        { provider_id: "prov-2", upstream_model: null, weight: 30 },
      ],
    });
    await expectToast(canvasElement, /gpt-4o updated/);
  },
};

/**
 * A stored upstream model that spells out the route's own name goes back as it
 * was read. Normalising it to "absent" would stop it matching the stored
 * target, which the control plane would then replace with a new id.
 */
let keeps: Recorder;
export const KeepsAnUpstreamModelEqualToTheRouteName: Story = {
  render: () => {
    keeps = updating([
      { ...TARGETS["route-1"][0], upstream_model: "gpt-4o" },
      TARGETS["route-1"][1],
    ]);
    return (
      <Harness fetchStub={keeps.stub} role="admin">
        <RoutingRules />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectAllowed(canvasElement, "Edit route gpt-4o");
    await userEvent.click(await canvas.findByRole("button", { name: "Edit route gpt-4o" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    const weight = await dialog.findByLabelText("Target 1 weight");
    await userEvent.clear(weight);
    await userEvent.type(weight, "70");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const body = await keeps.expectSentBody<RouteUpdate>("PUT", "/api/v1/routes/route-1");
    await expect(body.targets?.[0]).toEqual({
      provider_id: "prov-1",
      upstream_model: "gpt-4o",
      weight: 70,
    });
    await expect(body.strategy).toBeUndefined();
    await expect(body.enabled).toBeUndefined();
  },
};

/** A line can be added and one removed; the list sent is the whole list. */
let reshapes: Recorder;
export const AddsAndRemovesATarget: Story = {
  render: () => {
    reshapes = updating();
    return (
      <Harness fetchStub={reshapes.stub} role="admin">
        <RoutingRules />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectAllowed(canvasElement, "Edit route gpt-4o");
    await userEvent.click(await canvas.findByRole("button", { name: "Edit route gpt-4o" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.click(await dialog.findByRole("button", { name: "Remove target 1" }));
    await userEvent.click(dialog.getByRole("button", { name: "Add target" }));
    await pickOption(dialog.getByLabelText("Target 2 provider"), "openai-prod");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await expect(
      await reshapes.expectSentBody<RouteUpdate>("PUT", "/api/v1/routes/route-1"),
    ).toEqual({
      targets: [
        { provider_id: "prov-2", upstream_model: null, weight: 20 },
        { provider_id: "prov-1", upstream_model: null, weight: 1 },
      ],
    });
  },
};

/**
 * The control plane refuses the weight of the second target. The refusal is
 * said on that line, the sheet stays open with the draft, and nothing is toasted
 * as saved.
 */
export const RejectedSave: Story = {
  render: () => (
    <Harness
      role="admin"
      fetchStub={scoped(async (input, init) => {
        if (init?.method === "PUT") {
          return json(
            {
              error: {
                code: "invalid_field",
                message: "weight must be at least 1",
                field: "targets[1].weight",
              },
            },
            400,
          );
        }
        return answer(ROUTES)(input, init);
      })}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectAllowed(canvasElement, "Edit route gpt-4o");
    await userEvent.click(await canvas.findByRole("button", { name: "Edit route gpt-4o" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    const weight = await dialog.findByLabelText("Target 2 weight");
    await userEvent.clear(weight);
    await userEvent.type(weight, "5");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await waitFor(() => expect(dialog.getByLabelText("Target 2 weight")).toBeInvalid());
    await expect(dialog.getByLabelText("Target 1 weight")).toBeValid();
    await waitFor(() => expect(dialog.getByText(/refused the weight of target 2/)).toBeVisible());
    await expectAllowed(document.body, "Save route");
    // fixing the line clears the refusal
    await userEvent.type(dialog.getByLabelText("Target 2 weight"), "0");
    await waitFor(() => expect(dialog.getByLabelText("Target 2 weight")).toBeValid());
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
    await waitFor(() => expect(panel.getByText(/priced on every target/)).toBeVisible());
    await expect(panel.getByRole("button", { name: "Remove tier=gold" })).toBeVisible();
    await expect(panel.queryByRole("button", { name: "Remove tier=observed-gold" })).toBeNull();
  },
};
