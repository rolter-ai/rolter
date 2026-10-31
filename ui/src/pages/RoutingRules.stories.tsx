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
import { atMobile, expectNoHorizontalOverflow } from "@/lib/story-viewport";
import { onlyRestrictedDialled } from "@/lib/failure-classes";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import type {
  AnalyticsByModelRow,
  BreakerStateDto,
  BreakerTargetDto,
  ClusterNodeRow,
  LabelRow,
  ProviderRow,
  RouteRow,
  RouteTargetRow,
  RouteVariantInput,
  RouteVariantRow,
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

const answer = (
  routes: RouteRow[],
  status = 200,
  labels: LabelRow[] = [],
  // the variants each route embeds when the list asks for them (#3343)
  variants: Record<string, RouteVariantRow[]> = {},
): FetchStub =>
  scoped(async (input) => {
    const url = String(input);
    if (url.includes("/providers")) return json(PROVIDERS);
    if (url.includes("/analytics/by-model")) return json({ data: TRAFFIC });
    // the route sheet reads what each saved variant served (#3342); none, unless a story says
    if (url.includes("/analytics/by-variant")) return json({ data: [] });
    if (url.includes("/cluster/nodes")) return json([node("gw-1"), node("gw-2")]);
    // the label list is its own endpoint; without this branch it would be
    // answered with the route list and the chips would be nonsense (#1329)
    if (url.includes("/labels")) {
      const subject = new URL(url, "http://localhost").searchParams.get("subject_id");
      return json(subject ? labels.filter((l) => l.subject_id === subject) : labels);
    }
    const targets = /\/routes\/([^/]+)\/targets/.exec(url);
    if (targets) return json(TARGETS[targets[1]] ?? []);
    // the route sheet reads the variants beside the targets (#391)
    if (/\/routes\/[^/]+\/variants/.test(url)) return json([]);
    // the route list embeds each route's targets when it is asked to, which
    // is how the screen reads them (#3031)
    if (status === 200 && url.includes("include=targets")) {
      return json(
        routes.map((r) => ({
          ...r,
          targets: TARGETS[r.id] ?? [],
          ...(url.includes("variants") ? { variants: variants[r.id] ?? [] } : {}),
        })),
      );
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

let trafficReads: Recorder;

const card = async (canvas: ReturnType<typeof within>, model: string) =>
  within((await canvas.findByText(model, ROUTE_NAME)).closest("div.flex-col") as HTMLElement);

/**
 * A route with traffic says how many requests it served over the last day and
 * what share failed; one that took none says so in words, since the rollup
 * answered. Each enabled route links the Playground with its model selected.
 */
export const RouteTraffic: Story = {
  render: () => {
    trafficReads = recording(answer(ROUTES));
    return (
      <Harness fetchStub={trafficReads.stub}>
        <RoutingRules />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the rollup is read for the route's project, not the whole org (#3101)
    await trafficReads.expectSent("GET", "/analytics/by-model");
    const read = trafficReads.calls.find((c) => c.url.includes("/analytics/by-model"))!;
    await expect(new URL(read.url, "http://localhost").searchParams.get("project")).toBe(
      PROJECT.id,
    );
    const gpt = await card(canvas, "gpt-4o");
    await expect(await gpt.findByTestId("route-traffic")).toHaveTextContent(
      "1,240 requests for this model name in this project in the last 24 h · 2.5% errors",
    );
    const claude = await card(canvas, "claude-sonnet");
    await expect(await claude.findByTestId("route-traffic")).toHaveTextContent(
      "No requests for this model name in this project in the last 24 h",
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
        { provider_id: "prov-1", upstream_model: "gpt-4o-2024-08-06", weight: 80, takes_over: [] },
        { provider_id: "prov-2", upstream_model: null, weight: 30, takes_over: [] },
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
      takes_over: [],
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
        { provider_id: "prov-2", upstream_model: null, weight: 20, takes_over: [] },
        { provider_id: "prov-1", upstream_model: null, weight: 1, takes_over: [] },
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

// ------------------------------------------------ route variants (#391)

const variantRow = (
  name: string,
  weight: number,
  targets: {
    provider_id: string;
    upstream_model?: string | null;
    weight: number;
    takes_over?: string[];
  }[],
  position: number,
): RouteVariantRow => ({
  id: `var-${name}`,
  route_id: "route-1",
  name,
  weight,
  position,
  params: {},
  created_at: "2026-05-01T00:00:00Z",
  targets: targets.map((tg, i) => ({
    id: `vt-${name}-${i}`,
    variant_id: `var-${name}`,
    upstream_model: null,
    ...tg,
    position: i,
  })),
});

const SPLIT: RouteVariantRow[] = [
  variantRow("stable", 9, [{ provider_id: "prov-1", upstream_model: "gpt-4o", weight: 1 }], 0),
  variantRow(
    "canary",
    1,
    [
      { provider_id: "prov-2", weight: 1 },
      { provider_id: "prov-1", weight: 1 },
    ],
    1,
  ),
];

/**
 * route-1 with `stored` as its variants; `put` answers the variants PUT, which
 * by default echoes what was sent
 */
const withVariants = (stored: RouteVariantRow[], put?: () => Response) =>
  recording(
    scoped(async (input, init) => {
      const url = String(input);
      if (/\/routes\/route-1\/variants$/.test(url)) {
        if (init?.method === "PUT") return put ? put() : json([]);
        return json(stored);
      }
      return answer(ROUTES)(input, init);
    }),
  );

async function openRouteSheet(canvasElement: HTMLElement) {
  const canvas = within(canvasElement);
  await expectAllowed(canvasElement, "Edit route gpt-4o");
  await userEvent.click(await canvas.findByRole("button", { name: "Edit route gpt-4o" }));
  const dialog = within(await within(document.body).findByRole("dialog"));
  await waitFor(() => expect(dialog.getByRole("heading", { name: "Edit route" })).toBeVisible());
  return dialog;
}

/** the open sheet's own element, for an assertion that must not read the cards behind it */
const sheetElement = () => within(document.body).getByRole("dialog");

/**
 * A stored 9:1 split is previewed as the gateway applies it, a weight edit
 * moves the preview, and reordering moves the fallback chain. Only the
 * variants changed, so only the variants PUT is sent, carrying the whole set
 * in its new order.
 */
let splits: Recorder;
export const EditsAVariantSplit: Story = {
  render: () => {
    splits = withVariants(SPLIT);
    return (
      <Harness fetchStub={splits.stub} role="admin">
        <Toasted>
          <RoutingRules />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    const preview = within(await dialog.findByRole("list", { name: "Traffic split" }));
    await expect(preview.getByText("90%")).toBeInTheDocument();
    await expect(preview.getByText("10%")).toBeInTheDocument();
    await expect(dialog.getByText(/its targets above are not used/)).toBeVisible();
    // the second variant's targets are in the order they are tried
    await expect(dialog.getByLabelText("Variant 2 target 1 provider")).toHaveValue("azure-west");
    // story-wait-allow: the first line has nowhere to move up to, a prop from its first paint
    await expect(dialog.getByLabelText("Move variant 2 target 1 up")).toBeDisabled();

    const weight = dialog.getByLabelText("Variant 2 weight");
    await userEvent.clear(weight);
    await userEvent.type(weight, "3");
    // the list is redrawn while the field is briefly empty, so it is found again
    const redrawn = async () => within(await dialog.findByRole("list", { name: "Traffic split" }));
    await waitFor(async () => expect((await redrawn()).getByText("75%")).toBeInTheDocument());
    await expect((await redrawn()).getByText("25%")).toBeInTheDocument();

    await userEvent.click(dialog.getByRole("button", { name: "Move variant 2 up" }));
    await expect(dialog.getByLabelText("Variant 1 name")).toHaveValue("canary");
    await userEvent.click(dialog.getByRole("button", { name: "Move variant 1 target 2 up" }));
    await expect(dialog.getByLabelText("Variant 1 target 1 provider")).toHaveValue("openai-prod");

    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await expect(
      await splits.expectSentBody<{ variants: RouteVariantInput[] }>(
        "PUT",
        "/api/v1/routes/route-1/variants",
      ),
    ).toEqual({
      variants: [
        {
          name: "canary",
          weight: 3,
          params: {},
          targets: [
            { provider_id: "prov-1", upstream_model: null, weight: 1, takes_over: [] },
            { provider_id: "prov-2", upstream_model: null, weight: 1, takes_over: [] },
          ],
        },
        {
          name: "stable",
          weight: 9,
          params: {},
          targets: [{ provider_id: "prov-1", upstream_model: "gpt-4o", weight: 1, takes_over: [] }],
        },
      ],
    });
    await expectToast(canvasElement, /gpt-4o updated/);
    // the route itself did not change, so it was not written
    splits.expectNotSent("PUT", "/api/v1/routes/route-1?");
    await expect(
      splits.calls.filter((c) => c.method === "PUT" && /\/routes\/route-1$/.test(c.url)),
    ).toHaveLength(0);
  },
};

/**
 * A variant weight of 0 is a fallback-only variant (#3427): the sheet accepts
 * it, the preview shows the share the gateway applies, and the note says what
 * the 0 means.
 */
export const AcceptsAZeroVariantWeight: Story = {
  render: () => {
    splits = withVariants(SPLIT);
    return (
      <Harness fetchStub={splits.stub} role="admin">
        <Toasted>
          <RoutingRules />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    const weight = dialog.getByLabelText("Variant 2 weight");
    await userEvent.clear(weight);
    await userEvent.type(weight, "0");
    await waitFor(async () =>
      expect(
        within(await dialog.findByRole("list", { name: "Traffic split" })).getByText("100%"),
      ).toBeInTheDocument(),
    );
    await expect(dialog.getByText(/gets no first attempts while another variant/)).toBeVisible();
    await expect(dialog.queryByText(/variant weight is a whole number/)).toBeNull();

    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const sent = await splits.expectSentBody<{ variants: RouteVariantInput[] }>(
      "PUT",
      "/api/v1/routes/route-1/variants",
    );
    await expect(sent.variants.map((v) => v.weight)).toEqual([9, 0]);
  },
};

/**
 * A route that routes through variants shows them on its card, in fallback
 * order with the split their weights give, and sets its own targets aside
 * (#3343); a route with targets only is drawn as it always was, beside it.
 */
const variantRoutes = recording(
  answer(ROUTES, 200, [], {
    "route-1": [
      variantRow(
        "canary",
        1,
        [{ provider_id: "prov-2", upstream_model: "gpt-4o-next", weight: 1 }],
        1,
      ),
      variantRow("stable", 9, [{ provider_id: "prov-1", upstream_model: "gpt-4o", weight: 1 }], 0),
    ],
    // route-2 has no targets of its own, and routes only through variants
    "route-2": [variantRow("only", 1, [{ provider_id: "prov-1", weight: 1 }], 0)],
  }),
);
export const CardsShowTheVariantsTheyRouteThrough: Story = {
  render: () => (
    <Harness fetchStub={variantRoutes.stub}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("gpt-4o", ROUTE_NAME)).toBeInTheDocument();
    const list = within(
      await canvas.findByRole("list", { name: "Variants of gpt-4o, in fallback order" }),
    );
    // declared order is the fallback order, whatever the order the API listed them in
    const names = list.getAllByRole("listitem").map((li) => li.textContent);
    await expect(names[0]).toContain("stable");
    await expect(names[0]).toContain("90%");
    await expect(names[0]).toContain("openai-prod → gpt-4o");
    await expect(names[1]).toContain("canary");
    await expect(names[1]).toContain("10%");
    await expect(canvas.getByText(/its own 2 targets are not used/)).toBeVisible();
    // the unused target split is not drawn as if the gateway applied it
    await expect(canvas.queryByRole("list", { name: "Targets of gpt-4o" })).toBeNull();
    await expect(canvas.getByText("2 variants")).toBeVisible();
    await expect(canvas.queryByText("2 targets")).toBeNull();
    // a variants-only route never reads as having no targets
    await expect(canvas.queryByText("No targets yet.")).toBeNull();
    await expect(canvas.queryByText("0 targets")).toBeNull();
    await expect(canvas.getByText("1 variant")).toBeVisible();
    await variantRoutes.expectSent("GET", "include=targets,variants");
  },
};

/**
 * The sheet shows what each saved variant served beside the split its weights
 * configure (#3342): the observed share is of the requests the variants served,
 * a variant no longer configured still counts, and traffic served without a
 * variant is said apart rather than diluting the shares.
 */
const observing = (data: unknown[], status = 200) =>
  recording(
    scoped(async (input, init) => {
      const url = String(input);
      if (url.includes("/analytics/by-variant")) {
        return status === 200
          ? json({ data })
          : json({ error: { message: "no clickhouse_url" } }, status);
      }
      if (/\/routes\/route-1\/variants$/.test(url)) return json(SPLIT);
      return answer(ROUTES)(input, init);
    }),
  );
const variantTraffic = (variant: string, requests: number, errors: number, p95 = 300) => ({
  org_id: "org-1",
  model: "gpt-4o",
  variant,
  requests,
  tokens: requests * 10,
  cost_usd: requests / 100,
  unpriced_requests: 0,
  errors,
  p50_latency_ms: p95 / 3,
  p95_latency_ms: p95,
});
const observed = observing([
  variantTraffic("stable", 880, 8, 420),
  variantTraffic("canary", 120, 24, 900),
  variantTraffic("old-experiment", 40, 0),
  variantTraffic("", 15, 0),
]);
export const ShowsTheObservedShareBesideTheConfiguredSplit: Story = {
  render: () => (
    <Harness fetchStub={observed.stub} role="admin">
      <Toasted>
        <RoutingRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    const table = within(await dialog.findByRole("table", { name: "Observed traffic" }));
    await observed.expectSent("GET", "/analytics/by-variant?");
    const row = (name: string) => table.getByText(name).closest('[role="row"]') as HTMLElement;
    // 9:1 configured; 880 of the 1040 requests variants served observed
    await waitFor(() => expect(within(row("stable")).getByText("90%")).toBeVisible());
    await expect(within(row("stable")).getByText("85%")).toBeVisible();
    await expect(within(row("stable")).getByText("0.9%")).toBeVisible();
    await expect(within(row("canary")).getByText("10%")).toBeVisible();
    await expect(within(row("canary")).getByText("20.0%")).toBeVisible();
    await expect(within(row("canary")).getByText("300 / 900 ms")).toBeVisible();
    // a variant nobody configures any more has no configured share
    await expect(within(row("old-experiment")).getByText("—")).toBeVisible();
    await expect(
      dialog.getByText(/15 requests in this window were served without a variant/),
    ).toBeVisible();
    const url = new URL(
      observed.calls.find((c) => c.url.includes("/analytics/by-variant"))!.url,
      "http://x",
    );
    await expect(url.searchParams.get("model")).toBe("gpt-4o");
    // route names are unique per project only, so the read is narrowed to the route's
    await expect(url.searchParams.get("project")).toBe("project-1");
  },
};

/**
 * The log records the variant that answered, so a variant others fail over to
 * reads high in Observed (#3409). The first-attempt column counts the requests
 * it answered with no failover, the closer comparison with the weights: here
 * the canary is configured at 10%, answered 22% of the requests because of
 * failovers, and took 10% of the first attempts.
 */
const failingOver = observing([
  { ...variantTraffic("stable", 780, 8, 420), first_attempt_requests: 900 },
  { ...variantTraffic("canary", 220, 24, 900), first_attempt_requests: 100 },
]);
export const FirstAttemptsAreComparedWithTheConfiguredSplit: Story = {
  render: () => (
    <Harness fetchStub={failingOver.stub} role="admin">
      <Toasted>
        <RoutingRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    const table = within(await dialog.findByRole("table", { name: "Observed traffic" }));
    await expect(await table.findByRole("columnheader", { name: "First attempt" })).toBeVisible();
    const row = (name: string) => table.getByText(name).closest('[role="row"]') as HTMLElement;
    // configured 90 / 10; answered 78% / 22%; first attempts 90% / 10%
    const cells = (name: string) =>
      Array.from(row(name).querySelectorAll('[role="cell"]')).map((c) => c.textContent);
    await waitFor(() =>
      expect(cells("stable").slice(0, 4)).toEqual(["stable", "90%", "78%", "90%"]),
    );
    await expect(cells("canary").slice(0, 4)).toEqual(["canary", "10%", "22%", "10%"]);
    await expect(
      dialog.getByText(/share of requests answered on the first attempt, with no failover/),
    ).toBeVisible();
  },
};

/**
 * A ClickHouse that has not applied the fallback-reason file cannot split
 * failovers from first attempts, and the control plane says so with `null`:
 * the column is left out rather than drawn as a guess.
 */
const unsplit = observing([
  {
    ...variantTraffic("stable", 880, 8, 420),
    first_attempt_requests: null,
    assigned_requests: null,
  },
  {
    ...variantTraffic("canary", 120, 24, 900),
    first_attempt_requests: null,
    assigned_requests: null,
  },
]);
export const NoFirstAttemptColumnWhenTheSplitIsUnknown: Story = {
  render: () => (
    <Harness fetchStub={unsplit.stub} role="admin">
      <Toasted>
        <RoutingRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    const table = within(await dialog.findByRole("table", { name: "Observed traffic" }));
    await waitFor(() => expect(table.getAllByRole("columnheader")).toHaveLength(6));
    await expect(table.queryByRole("columnheader", { name: "First attempt" })).toBeNull();
    await expect(dialog.getByText(/counting failovers as well as first attempts/)).toBeVisible();
  },
};

/**
 * The assigned column counts the requests each variant was dialled to first,
 * failed over or not (#3505): here 40 of the canary's 100 requests failed over
 * to stable, which reads high in Observed and First attempt and at 90 / 10 in
 * Assigned.
 */
const assigned = observing([
  {
    ...variantTraffic("stable", 940, 8, 420),
    first_attempt_requests: 900,
    assigned_requests: 900,
  },
  { ...variantTraffic("canary", 60, 24, 900), first_attempt_requests: 60, assigned_requests: 100 },
]);
export const AssignedSharesAreComparedWithTheConfiguredSplit: Story = {
  render: () => (
    <Harness fetchStub={assigned.stub} role="admin">
      <Toasted>
        <RoutingRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    const table = within(await dialog.findByRole("table", { name: "Observed traffic" }));
    await expect(await table.findByRole("columnheader", { name: "Assigned" })).toBeVisible();
    await expect(table.getAllByRole("columnheader")).toHaveLength(8);
    const row = (name: string) => table.getByText(name).closest('[role="row"]') as HTMLElement;
    const cells = (name: string) =>
      Array.from(row(name).querySelectorAll('[role="cell"]')).map((c) => c.textContent);
    // configured, assigned, observed, first attempt
    await waitFor(() =>
      expect(cells("stable").slice(0, 5)).toEqual(["stable", "90%", "90%", "94%", "94%"]),
    );
    await expect(cells("canary").slice(0, 5)).toEqual(["canary", "10%", "10%", "6%", "6%"]);
    await expect(dialog.getByText(/share of the requests it was dialled to first/)).toBeVisible();
  },
};

/**
 * A variant whose every assigned request failed over answered nothing, and
 * still has its line: its assigned share is there, its error rate and
 * latency are not.
 */
const answeredNothing = observing([
  {
    ...variantTraffic("stable", 1000, 8, 420),
    first_attempt_requests: 900,
    assigned_requests: 900,
  },
  { ...variantTraffic("canary", 0, 0, 0), first_attempt_requests: 0, assigned_requests: 100 },
]);
export const AVariantThatAnsweredNothingKeepsItsAssignedShare: Story = {
  render: () => (
    <Harness fetchStub={answeredNothing.stub} role="admin">
      <Toasted>
        <RoutingRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    const table = within(await dialog.findByRole("table", { name: "Observed traffic" }));
    const row = (name: string) => table.getByText(name).closest('[role="row"]') as HTMLElement;
    const cells = (name: string) =>
      Array.from(row(name).querySelectorAll('[role="cell"]')).map((c) => c.textContent);
    await waitFor(() =>
      // no latency either: the zeros the read carries for it are not a measurement
      expect(cells("canary")).toEqual(["canary", "10%", "10%", "0%", "0%", "0", "—", "—"]),
    );
  },
};

/**
 * The sampled column counts the requests the weighted split picked each
 * variant for, before the gateway skipped one (#3547): here the canary's
 * breaker was open, so it was sampled for its 10% and dialled first for 1%,
 * and the gap is the traffic routed around it.
 */
const routedAround = observing([
  {
    ...variantTraffic("stable", 990, 8, 420),
    first_attempt_requests: 990,
    assigned_requests: 990,
    sampled_requests: 900,
  },
  {
    ...variantTraffic("canary", 10, 0, 900),
    first_attempt_requests: 10,
    assigned_requests: 10,
    sampled_requests: 100,
  },
]);
export const SampledSharesShowTheTrafficRoutedAroundAVariant: Story = {
  render: () => (
    <Harness fetchStub={routedAround.stub} role="admin">
      <Toasted>
        <RoutingRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    const table = within(await dialog.findByRole("table", { name: "Observed traffic" }));
    await expect(await table.findByRole("columnheader", { name: "Sampled" })).toBeVisible();
    await expect(table.getAllByRole("columnheader")).toHaveLength(9);
    // the widest grid fits the table's own floor: no row runs past it
    const tableEl = dialog.getByRole("table", { name: "Observed traffic" });
    for (const group of Array.from(tableEl.children) as HTMLElement[]) {
      await expect(group.scrollWidth).toBeLessThanOrEqual(group.clientWidth);
    }
    const row = (name: string) => table.getByText(name).closest('[role="row"]') as HTMLElement;
    const cells = (name: string) =>
      Array.from(row(name).querySelectorAll('[role="cell"]')).map((c) => c.textContent);
    // configured, sampled, assigned, observed
    await waitFor(() =>
      expect(cells("stable").slice(0, 5)).toEqual(["stable", "90%", "90%", "99%", "99%"]),
    );
    await expect(cells("canary").slice(0, 5)).toEqual(["canary", "10%", "10%", "1%", "1%"]);
    await expect(dialog.getByText(/the gateway routed requests around the variant/)).toBeVisible();
  },
};

/**
 * A ClickHouse without the sampled-variant file answers `null`, and the
 * column is left out rather than drawn as a guess.
 */
const unsampled = observing([
  { ...variantTraffic("stable", 900, 8, 420), assigned_requests: 900, sampled_requests: null },
  { ...variantTraffic("canary", 100, 24, 900), assigned_requests: 100, sampled_requests: null },
]);
export const NoSampledColumnWithoutTheSampledVariant: Story = {
  render: () => (
    <Harness fetchStub={unsampled.stub} role="admin">
      <Toasted>
        <RoutingRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    const table = within(await dialog.findByRole("table", { name: "Observed traffic" }));
    await expect(await table.findByRole("columnheader", { name: "Assigned" })).toBeVisible();
    await expect(table.queryByRole("columnheader", { name: "Sampled" })).toBeNull();
    await expect(dialog.queryByText(/routed requests around the variant/)).toBeNull();
  },
};

/**
 * Rows are keyed on the organization too (#3548): two organizations may each
 * have a route and variants of one name. The read is narrowed to the route's
 * project, so another organization's rows of the same names never join this
 * route's, here neither in the shares nor as lines of their own.
 */
const twoOrgs = observing([
  { ...variantTraffic("stable", 900, 8, 420), assigned_requests: 900 },
  { ...variantTraffic("canary", 100, 24, 900), assigned_requests: 100 },
  { ...variantTraffic("stable", 50, 0), org_id: "org-2", assigned_requests: 50 },
  { ...variantTraffic("canary", 50, 0), org_id: "org-2", assigned_requests: 50 },
  { ...variantTraffic("other-org-only", 20, 0), org_id: "org-2", assigned_requests: 20 },
]);
export const AnotherOrganizationsVariantsOfOneNameStayApart: Story = {
  render: () => (
    <Harness fetchStub={twoOrgs.stub} role="admin">
      <Toasted>
        <RoutingRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    const table = within(await dialog.findByRole("table", { name: "Observed traffic" }));
    const row = (name: string) => table.getByText(name).closest('[role="row"]') as HTMLElement;
    const cells = (name: string) =>
      Array.from(row(name).querySelectorAll('[role="cell"]')).map((c) => c.textContent);
    // 900 / 100 of this organization's requests, not 950 / 150 of both
    await waitFor(() =>
      expect(cells("stable").slice(0, 5)).toEqual(["stable", "90%", "90%", "90%", "900"]),
    );
    await expect(cells("canary").slice(0, 5)).toEqual(["canary", "10%", "10%", "10%", "100"]);
    await expect(table.queryByText("other-org-only")).toBeNull();
  },
};

export const ObservedTrafficIsEmptyBeforeTheRouteIsCalled: Story = {
  render: () => (
    <Harness fetchStub={observing([]).stub} role="admin">
      <Toasted>
        <RoutingRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    await expect(
      await dialog.findByText(/No variant served a request in the Last 7 days/),
    ).toBeVisible();
    await expect(dialog.queryByRole("table", { name: "Observed traffic" })).toBeNull();
  },
};

export const ObservedTrafficNeedsClickHouse: Story = {
  render: () => (
    <Harness fetchStub={observing([], 503).stub} role="admin">
      <Toasted>
        <RoutingRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    await expect(await dialog.findByText("Observed traffic needs ClickHouse")).toBeVisible();
    // not an alert: a deployment without ClickHouse is a supported one
    await expect(dialog.queryByRole("alert")).toBeNull();
  },
};

export const ObservedTrafficReadFails: Story = {
  render: () => (
    <Harness fetchStub={observing([], 500).stub} role="admin">
      <Toasted>
        <RoutingRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    await expectLoadError(sheetElement(), /observed traffic/);
    await expect(dialog.queryByRole("table", { name: "Observed traffic" })).toBeNull();
  },
};

/** A route with no variants says so, and the first one is added and named in place. */
let adds: Recorder;
export const AddsAVariant: Story = {
  render: () => {
    adds = withVariants([]);
    return (
      <Harness fetchStub={adds.stub} role="admin">
        <RoutingRules />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    await expect(await dialog.findByText(/^No variants\./)).toBeVisible();
    await userEvent.click(dialog.getByRole("button", { name: "Add variant" }));
    const name = dialog.getByLabelText("Variant 1 name");
    await expect(name).toHaveValue("variant-1");
    await userEvent.clear(name);
    await userEvent.type(name, "canary");
    await userEvent.type(dialog.getByLabelText("Variant 1 target 1 upstream model"), "gpt-4o-mini");
    const preview = within(dialog.getByRole("list", { name: "Traffic split" }));
    await expect(preview.getByText("100%")).toBeInTheDocument();
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await expect(await adds.expectSentBody("PUT", "/api/v1/routes/route-1/variants")).toEqual({
      variants: [
        {
          name: "canary",
          weight: 1,
          params: {},
          targets: [
            { provider_id: "prov-1", upstream_model: "gpt-4o-mini", weight: 1, takes_over: [] },
          ],
        },
      ],
    });
  },
};

/**
 * A fractional variant weight, a repeated name and a variant left without a
 * target are refused in the sheet, each at its field, and nothing is sent. A 0
 * is accepted and previewed as the share the gateway gives it (#3427).
 */
let refusesSplit: Recorder;
export const RefusesASplitTheGatewayCannotRoute: Story = {
  render: () => {
    refusesSplit = withVariants(SPLIT);
    return (
      <Harness fetchStub={refusesSplit.stub} role="admin">
        <RoutingRules />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    const second = await dialog.findByLabelText("Variant 2 weight");
    await userEvent.clear(second);
    await userEvent.type(second, "0");
    // a 0 takes no first attempts while another weight is positive (#3341): the
    // preview shows that and says so
    await expect(
      await dialog.findByText(/gets no first attempts while another variant/),
    ).toBeVisible();
    const split = within(dialog.getByRole("list", { name: "Traffic split" }));
    await expect(split.getByText("100%")).toBeInTheDocument();
    await expect(split.getByText("0%")).toBeInTheDocument();
    await userEvent.clear(second);
    await userEvent.type(second, "1.5");
    const name = dialog.getByLabelText("Variant 2 name");
    await userEvent.clear(name);
    await userEvent.type(name, "stable");
    await userEvent.click(dialog.getByRole("button", { name: "Remove variant 2 target 2" }));
    await userEvent.click(dialog.getByRole("button", { name: "Remove variant 2 target 1" }));
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));

    await waitFor(() =>
      expect(dialog.getByText("A variant weight is a whole number, 0 or more.")).toBeVisible(),
    );
    await expect(dialog.getByText(/Another variant of this route has this name/)).toBeVisible();
    await expect(dialog.getByText("A variant needs at least one target.")).toBeVisible();
    await expect(dialog.getByLabelText("Variant 2 name")).toBeInvalid();
    await expect(dialog.getByLabelText("Variant 2 weight")).toBeInvalid();
    await expect(dialog.getByLabelText("Variant 1 weight")).toBeValid();
    await expect(dialog.getByText(/3 fields need attention/)).toBeVisible();
    refusesSplit.expectNotSent("PUT", "/variants");
  },
};

/**
 * The control plane refuses one variant's weight. The refusal is said on that
 * field and the sheet keeps the draft; a stored route edit is not repeated.
 */
export const VariantRefusedByTheControlPlane: Story = {
  render: () => (
    <Harness
      role="admin"
      fetchStub={
        withVariants(SPLIT, () =>
          json(
            {
              error: {
                code: "invalid_field",
                message: "variants[1].weight: weight must be 0 or more",
                field: "variants[1].weight",
              },
            },
            400,
          ),
        ).stub
      }
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    const weight = await dialog.findByLabelText("Variant 2 weight");
    await userEvent.clear(weight);
    await userEvent.type(weight, "4");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await waitFor(() => expect(dialog.getByLabelText("Variant 2 weight")).toBeInvalid());
    await expect(dialog.getByLabelText("Variant 1 weight")).toBeValid();
    await expect(dialog.getByText(/The control plane refused this field/)).toBeVisible();
    // editing the field clears the refusal
    await userEvent.type(dialog.getByLabelText("Variant 2 weight"), "0");
    await waitFor(() => expect(dialog.getByLabelText("Variant 2 weight")).toBeValid());
  },
};

/**
 * A refusal of the set as a whole names only `variants`, which covers more than
 * one rule, so the control plane's own sentence is shown, and focus goes to the
 * section's add button.
 */
export const VariantSetRefusedByTheControlPlane: Story = {
  render: () => (
    <Harness
      role="admin"
      fetchStub={
        withVariants(SPLIT, () =>
          json(
            {
              error: {
                code: "invalid_field",
                message: "a route takes at most 32 variants",
                field: "variants",
              },
            },
            400,
          ),
        ).stub
      }
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    const weight = await dialog.findByLabelText("Variant 2 weight");
    await userEvent.clear(weight);
    await userEvent.type(weight, "2");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await waitFor(() =>
      expect(dialog.getByText("a route takes at most 32 variants")).toBeVisible(),
    );
    await waitFor(() => expect(dialog.getByRole("button", { name: "Add variant" })).toHaveFocus());
  },
};

/**
 * With no provider the route's project may use, no variant or target can be
 * added, and the sheet says where to make one rather than leaving the buttons
 * disabled without a word.
 */
export const VariantsWithoutProviders: Story = {
  render: () => (
    <Harness
      role="admin"
      fetchStub={scoped(async (input, init) => {
        if (String(input).includes("/providers")) return json([]);
        return answer(ROUTES)(input, init);
      })}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    await expect(await dialog.findByText(/^No variants\./)).toBeVisible();
    // story-wait-allow: disabled by the empty provider list from the sheet's first paint
    await expect(dialog.getByRole("button", { name: "Add variant" })).toBeDisabled();
    await expect(dialog.getAllByRole("link", { name: /providers/i }).length).toBeGreaterThan(0);
  },
};

/**
 * Under a strategy that never reads weights, a variant's target weights do not
 * split its traffic, and the section says so as the route's own target list does.
 */
export const SaysWhenVariantTargetWeightsAreIgnored: Story = {
  render: () => (
    <Harness role="admin" fetchStub={withVariants(SPLIT).stub}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    await dialog.findByLabelText("Variant 2 weight");
    await expect(dialog.queryByText(/target weights do not split traffic/)).toBeNull();
    await pickOption(dialog.getByLabelText("Strategy"), /^round_robin\b/);
    await expect(await dialog.findByText(/target weights do not split traffic/)).toBeVisible();
  },
};

/** The sheet waits for the variants as well as the targets before it offers a draft. */
export const VariantsLoading: Story = {
  render: () => (
    <Harness
      role="admin"
      fetchStub={scoped(async (input, init) => {
        if (/\/routes\/route-1\/variants/.test(String(input))) return pending(input, init);
        return answer(ROUTES)(input, init);
      })}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    await expectSkeleton(sheetElement());
    await expectNoFalseEmpty(sheetElement(), /^No variants\./);
    // story-wait-allow: the save button is disabled by the sheet's own read, from the first paint
    await expect(dialog.getByRole("button", { name: "Save route" })).toBeDisabled();
  },
};

/** A failed variants read is an error with a retry, never an empty split. */
export const VariantsLoadFailed: Story = {
  render: () => (
    <Harness
      role="admin"
      fetchStub={scoped(async (input, init) => {
        if (/\/routes\/route-1\/variants/.test(String(input))) {
          return json({ error: { message: "boom" } }, 500);
        }
        return answer(ROUTES)(input, init);
      })}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await openRouteSheet(canvasElement);
    await expectLoadError(sheetElement(), /route variants/);
    await expectNoFalseEmpty(sheetElement(), /^No variants\./);
  },
};

/**
 * Editing variants is editing the route: a viewer is refused the sheet, and so
 * never reads or writes a variant.
 */
let viewerReads: Recorder;
export const VariantEditorRefusedToAViewer: Story = {
  render: () => {
    viewerReads = withVariants(SPLIT);
    return (
      <Harness fetchStub={viewerReads.stub} role="viewer">
        <RoutingRules />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Edit route gpt-4o");
    viewerReads.expectNotSent("GET", "/variants");
    viewerReads.expectNotSent("PUT", "/variants");
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

// ------------------------------------------------ live circuit breakers (#2254)

const breakerTarget = (patch: Partial<BreakerTargetDto>): BreakerTargetDto => ({
  model: "gpt-4o",
  target_index: 0,
  provider: "openai-prod",
  upstream_model: "gpt-4o-2024-08-06",
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

const withBreakers = (answerBreakers: () => Response | Promise<Response>) =>
  scoped(async (input, init) => {
    if (String(input).includes("/breaker-state")) return answerBreakers();
    return answer(ROUTES)(input, init);
  });

/**
 * A route's target shows its live breaker beside its health: open on a number
 * of gateways, or probing. The second target is not listed, so it is closed
 * everywhere and its line stays as it was.
 */
export const TargetBreakerOpen: Story = {
  render: () => (
    <Harness
      fetchStub={withBreakers(() =>
        json(
          breakerState([
            breakerTarget({ target_index: 0, nodes_open: 2 }),
            breakerTarget({
              model: "claude-sonnet",
              target_index: 0,
              state: "closed",
              nodes_open: 0,
            }),
          ]),
        ),
      )}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const list = within(await canvas.findByRole("list", { name: "Targets of gpt-4o" }));
    const badge = await list.findByText("Breaker open on 2 gateways");
    await expect(badge).toBeVisible();
    await expect(list.getAllByTestId("breaker-badge")).toHaveLength(1);
    await expect(badge.closest("li")).toHaveTextContent(/openai-prod/);
  },
};

export const TargetBreakerProbing: Story = {
  render: () => (
    <Harness
      fetchStub={withBreakers(() =>
        json(
          breakerState([
            breakerTarget({
              target_index: 1,
              state: "half_open",
              nodes_open: 0,
              nodes_half_open: 1,
            }),
          ]),
        ),
      )}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const list = within(await canvas.findByRole("list", { name: "Targets of gpt-4o" }));
    const badge = await list.findByText("Probing on 1 gateway");
    await expect(badge.closest("li")).toHaveTextContent(/azure-west/);
  },
};

/**
 * #3460: another organization also has a `gpt-4o` route through a provider
 * called `openai-prod`. Its breaker is open and its provider is below SLA, but
 * this organization's card shows its own route's breaker and its own
 * provider's health, never the namesake's.
 */
export const TwoOrgsShareARouteName: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        const url = String(input);
        if (url.includes("/breaker-state"))
          return json(
            breakerState([
              breakerTarget({ org_id: "org-2", target_index: 0, state: "open" }),
              breakerTarget({
                org_id: "org-1",
                target_index: 0,
                state: "half_open",
                nodes_open: 0,
                nodes_half_open: 1,
              }),
            ]),
          );
        if (url.includes("/health/uptime"))
          return json({
            data: [
              uptimeRow({ org_id: "org-1" }),
              uptimeRow({
                org_id: "org-2",
                ok: 1000,
                errors: 1000,
                uptime: 0.5,
                sla_breached: 1,
              }),
            ],
          });
        if (url.includes("/health/timeline")) return json({ data: [] });
        return answer(ROUTES)(input, init);
      })}
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const list = within(await canvas.findByRole("list", { name: "Targets of gpt-4o" }));
    const badge = await list.findByText("Probing on 1 gateway");
    await expect(badge.closest("li")).toHaveTextContent(/openai-prod/);
    await expect(list.queryByText("Breaker open on 2 gateways")).toBeNull();
    await expect(await list.findByText("99.95% uptime")).toBeInTheDocument();
    await expect(list.queryByText(/50\.00% uptime/)).toBeNull();
  },
};

/** a failed breaker read leaves the cards exactly as they were, with no error */
export const BreakerStateUnavailable: Story = {
  render: () => (
    <Harness fetchStub={withBreakers(() => json({ error: { message: "no store" } }, 404))}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole("list", { name: "Targets of gpt-4o" });
    await expect(canvas.queryByTestId("breaker-badge")).toBeNull();
    await expect(canvas.queryByRole("alert")).toBeNull();
  },
};

// ------------------------------------------- fallback targets (#1584, #3319)

/** route-1 with its second target a fallback for throttling */
const WITH_FALLBACK: RouteTargetRow[] = [
  TARGETS["route-1"][0],
  { ...TARGETS["route-1"][1], takes_over: ["throttled"] },
];

/**
 * route-1 read with `stored` as its targets; `put` answers the route PUT, which
 * by default echoes the route
 */
const withFallback = (stored: RouteTargetRow[] = WITH_FALLBACK, put?: () => Response) =>
  recording(
    scoped(async (input, init) => {
      const url = String(input);
      if (init?.method === "PUT" && /\/routes\/route-1$/.test(url)) {
        return put ? put() : json(ROUTES[0]);
      }
      if (/\/routes\/route-1\/targets/.test(url)) return json(stored);
      if (url.includes("include=targets")) {
        return json(
          ROUTES.map((r) => ({
            ...r,
            targets: r.id === "route-1" ? stored : (TARGETS[r.id] ?? []),
          })),
        );
      }
      return answer(ROUTES)(input, init);
    }),
  );

/** the fallback set of one target line in the open sheet */
const fallbackOf = (dialog: ReturnType<typeof within>, name: string) =>
  within(dialog.getByRole("group", { name }));

/**
 * A fallback takes no ordinary traffic, so the card marks it and states no
 * share for it: the ordinary target carries the whole route, and only it has
 * a share bar.
 */
export const FallbackTargetHasNoShare: Story = {
  render: () => (
    <Harness fetchStub={withFallback().stub}>
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const targets = within(await canvas.findByRole("list", { name: "Targets of gpt-4o" }));
    await expect(targets.getByText("100%")).toBeInTheDocument();
    await expect(targets.queryByText("20%")).toBeNull();
    await expect(targets.getAllByTestId("target-share")).toHaveLength(1);
    const badge = targets.getByTestId("fallback-badge");
    await expect(badge).toHaveTextContent("fallback");
    await expect(badge.getAttribute("title")).toContain("throttled");
  },
};

/**
 * A stored set opens unfolded with the target marked; a class toggled on and a
 * refusal category typed in go out with the whole list, the ordinary target's
 * empty set included, since an entry without one clears what is stored.
 */
let fallbackEdits: Recorder;
export const EditsAFallbackSet: Story = {
  render: () => {
    fallbackEdits = withFallback();
    return (
      <Harness fetchStub={fallbackEdits.stub} role="admin">
        <Toasted>
          <RoutingRules />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    await dialog.findByLabelText("Target 2 weight");
    const first = fallbackOf(dialog, "Fallback classes of target 1");
    await expect(first.getByRole("button", { name: "Ordinary target" })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
    const second = fallbackOf(dialog, "Fallback classes of target 2");
    await expect(second.getByRole("button", { name: "Fallback for throttled" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    await expect(second.getByRole("button", { name: "throttled" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await userEvent.click(second.getByRole("button", { name: "timeout" }));
    await userEvent.type(second.getByLabelText("Refusal category"), "violence{Enter}");
    await expect(second.getByRole("button", { name: "refusal:violence" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const body = await fallbackEdits.expectSentBody<RouteUpdate>("PUT", "/api/v1/routes/route-1");
    await expect(body.targets).toEqual([
      { provider_id: "prov-1", upstream_model: "gpt-4o-2024-08-06", weight: 80, takes_over: [] },
      {
        provider_id: "prov-2",
        upstream_model: null,
        weight: 20,
        takes_over: ["throttled", "timeout", "refusal:violence"],
      },
    ]);
    await expectToast(canvasElement, /gpt-4o updated/);
  },
};

/**
 * Making the last ordinary target a fallback would leave the route nothing it
 * may dial first. The sheet says so at the target list and sends nothing.
 */
let onlyFallbacks: Recorder;
export const RefusesARouteOfOnlyFallbacks: Story = {
  render: () => {
    onlyFallbacks = withFallback();
    return (
      <Harness fetchStub={onlyFallbacks.stub} role="admin">
        <RoutingRules />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    await dialog.findByLabelText("Target 1 weight");
    const first = fallbackOf(dialog, "Fallback classes of target 1");
    await userEvent.click(first.getByRole("button", { name: "Ordinary target" }));
    await userEvent.click(first.getByRole("button", { name: "refusal" }));
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await waitFor(() => expect(dialog.getByText(/Every target is a fallback/)).toBeVisible());
    onlyFallbacks.expectNotSent("PUT", "/api/v1/routes/route-1");
  },
};

/**
 * A set the control plane refuses is said under that target's own set, which
 * opens to show it, and not as a generic failure.
 */
export const FallbackSetRefusedByTheControlPlane: Story = {
  render: () => (
    <Harness
      role="admin"
      fetchStub={
        withFallback(WITH_FALLBACK, () =>
          json(
            {
              error: {
                code: "invalid_field",
                message: "takes_over lists 'throttled' twice",
                field: "targets[1].takes_over",
              },
            },
            400,
          ),
        ).stub
      }
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    await dialog.findByLabelText("Target 2 weight");
    await userEvent.clear(dialog.getByLabelText("Target 2 weight"));
    await userEvent.type(dialog.getByLabelText("Target 2 weight"), "25");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const second = fallbackOf(dialog, "Fallback classes of target 2");
    await waitFor(() => expect(second.getByText(/control plane refused this set/)).toBeVisible());
    await expect(second.getByRole("button", { name: "Fallback for throttled" })).toBeInvalid();
    await expect(dialog.queryByText(/control plane refused this set/i)).toBeVisible();
  },
};

/**
 * A refusal category the control plane would refuse is caught as it is typed;
 * a valid one at the 64-character limit is added as a chip of its own.
 */
export const RejectsABadRefusalCategory: Story = {
  render: () => (
    <Harness fetchStub={withFallback().stub} role="admin">
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    await dialog.findByLabelText("Target 2 weight");
    const second = fallbackOf(dialog, "Fallback classes of target 2");
    const category = second.getByLabelText("Refusal category");
    await userEvent.type(category, "Self Harm");
    await userEvent.click(second.getByRole("button", { name: "Add" }));
    await expect(second.getByText(/1 to 64 lowercase letters/)).toBeVisible();
    await expect(category).toBeInvalid();
    const long = `x${"_content_policy".repeat(4)}`.padEnd(64, "y");
    await userEvent.clear(category);
    await userEvent.type(category, long);
    await userEvent.click(second.getByRole("button", { name: "Add" }));
    await expect(second.getByRole("button", { name: `refusal:${long}` })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(category).toHaveValue("");
  },
};

/** On a phone a long category wraps inside the sheet instead of widening the page. */
export const FallbackEditorOnMobile: Story = {
  ...atMobile,
  render: () => (
    <Harness
      fetchStub={
        withFallback([
          TARGETS["route-1"][0],
          {
            ...TARGETS["route-1"][1],
            takes_over: ["throttled", `refusal:${"content_policy_violation_".repeat(2)}sexual`],
          },
        ]).stub
      }
      role="admin"
    >
      <RoutingRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    await dialog.findByLabelText("Target 2 weight");
    const second = fallbackOf(dialog, "Fallback classes of target 2");
    await expect(
      second.getByRole("button", {
        name: `refusal:${"content_policy_violation_".repeat(2)}sexual`,
      }),
    ).toBeVisible();
    await expectNoHorizontalOverflow();
  },
};

/**
 * A variant target takes over failures like a route target (#3373), and its
 * set goes out with the variants.
 */
let variantFallback: Recorder;
export const EditsAVariantTargetsFallbackSet: Story = {
  render: () => {
    variantFallback = withVariants(SPLIT);
    return (
      <Harness fetchStub={variantFallback.stub} role="admin">
        <Toasted>
          <RoutingRules />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    await dialog.findByLabelText("Variant 2 target 2 provider");
    const set = fallbackOf(dialog, "Fallback classes of target 2 of variant 2");
    await userEvent.click(set.getByRole("button", { name: "Ordinary target" }));
    await userEvent.click(set.getByRole("button", { name: "server_error" }));
    await expect(set.getByTestId("fallback-badge")).toBeVisible();
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const body = await variantFallback.expectSentBody<{ variants: RouteVariantInput[] }>(
      "PUT",
      "/api/v1/routes/route-1/variants",
    );
    await expect(body.variants[1].targets.map((tg) => tg.takes_over)).toEqual([
      [],
      ["server_error"],
    ]);
  },
};

/**
 * A route with variants dials only the variants' targets (#3487), so variants
 * that hold only fallbacks strand the route even over an ordinary target list.
 * The sheet says so under the variants, once, and sends nothing.
 */
let fallbackVariants: Recorder;
export const RefusesFallbackOnlyVariantsOverAnOrdinaryList: Story = {
  render: () => {
    fallbackVariants = withVariants(SPLIT);
    return (
      <Harness fetchStub={fallbackVariants.stub} role="admin">
        <RoutingRules />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    await dialog.findByLabelText("Variant 2 target 2 provider");
    for (const name of [
      "Fallback classes of target 1 of variant 1",
      "Fallback classes of target 1 of variant 2",
      "Fallback classes of target 2 of variant 2",
    ]) {
      const set = fallbackOf(dialog, name);
      await userEvent.click(set.getByRole("button", { name: "Ordinary target" }));
      await userEvent.click(set.getByRole("button", { name: "throttled" }));
    }
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await waitFor(() => expect(dialog.getByText(/Every target is a fallback/)).toBeVisible());
    await expect(dialog.getAllByText(/Every target is a fallback/)).toHaveLength(1);
    fallbackVariants.expectNotSent("PUT", "/api/v1/routes/route-1/variants");
    fallbackVariants.expectNotSent("PUT", "/api/v1/routes/route-1");
  },
};

/**
 * The converse: while a variant has an ordinary target, the route's own list
 * is not dialled, so it may hold only fallbacks and the save goes out.
 */
let fallbackList: Recorder;
export const AcceptsAFallbackOnlyListUnderOrdinaryVariants: Story = {
  render: () => {
    fallbackList = withVariants(SPLIT);
    return (
      <Harness fetchStub={fallbackList.stub} role="admin">
        <Toasted>
          <RoutingRules />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    await dialog.findByLabelText("Variant 2 target 2 provider");
    for (const name of ["Fallback classes of target 1", "Fallback classes of target 2"]) {
      const set = fallbackOf(dialog, name);
      await userEvent.click(set.getByRole("button", { name: "Ordinary target" }));
      await userEvent.click(set.getByRole("button", { name: "timeout" }));
    }
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const body = await fallbackList.expectSentBody<{ targets: { takes_over: string[] }[] }>(
      "PUT",
      "/api/v1/routes/route-1",
    );
    await expect(body.targets.map((tg) => tg.takes_over)).toEqual([["timeout"], ["timeout"]]);
    await expect(dialog.queryByText(/Every target is a fallback/)).toBeNull();
  },
};

/**
 * route-1 as the API could store it before #3487: variants whose every target
 * is a fallback, over an ordinary pool, so the snapshot drops the route
 */
const STRANDED: RouteVariantRow[] = [
  variantRow("stable", 9, [{ provider_id: "prov-1", weight: 1, takes_over: ["throttled"] }], 0),
  variantRow("canary", 1, [{ provider_id: "prov-2", weight: 1, takes_over: ["timeout"] }], 1),
];

/** the refusal the control plane answers a write that would strand the route */
const strands = (field: string) =>
  json(
    {
      error: {
        code: "invalid_field",
        message: "route gpt-4o: all_targets_restricted",
        field,
      },
    },
    400,
  );

/**
 * The stranded route with the control plane's rule applied to each write, as
 * `crud/routes.rs` applies it (#3487): a route with variants dials only them,
 * one without dials its pool, and each PUT is judged against what is stored at
 * that moment, the other half included. A write that would leave only
 * fallbacks to dial is refused and stores nothing.
 */
const strandedRoute = () => {
  let pool: (string[] | undefined)[] = TARGETS["route-1"].map((tg) => tg.takes_over);
  let variants: (string[] | undefined)[][] = STRANDED.map((v) =>
    v.targets.map((tg) => tg.takes_over),
  );
  return recording(
    scoped(async (input, init) => {
      const url = String(input);
      if (/\/routes\/route-1\/variants$/.test(url)) {
        if (init?.method === "PUT") {
          const sent = JSON.parse(String(init.body)) as { variants: RouteVariantInput[] };
          const next = sent.variants.map((v) => v.targets.map((tg) => tg.takes_over));
          if (onlyRestrictedDialled(pool, next)) return strands("variants");
          variants = next;
          return json([]);
        }
        return json(STRANDED);
      }
      if (init?.method === "PUT" && /\/routes\/route-1$/.test(url)) {
        const sent = JSON.parse(String(init.body)) as { targets?: { takes_over?: string[] }[] };
        if (sent.targets) {
          const next = sent.targets.map((tg) => tg.takes_over);
          if (onlyRestrictedDialled(next, variants)) return strands("targets");
          pool = next;
        }
        return json(ROUTES[0]);
      }
      return answer(ROUTES)(input, init);
    }),
  );
};

/**
 * One save repairs a stored route whose variants hold only fallbacks (#3510):
 * the edit gives a variant an ordinary target and touches the pool, and the
 * sheet stores the variants first, so the pool PUT is judged against variants
 * that can be dialled and both writes land.
 */
let repair: Recorder;
export const RepairsFallbackOnlyVariantsInOneSave: Story = {
  render: () => {
    repair = strandedRoute();
    return (
      <Harness fetchStub={repair.stub} role="admin">
        <Toasted>
          <RoutingRules />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    await dialog.findByLabelText("Variant 2 target 1 provider");
    const set = fallbackOf(dialog, "Fallback classes of target 1 of variant 1");
    await userEvent.click(set.getByRole("button", { name: "throttled" }));
    const weight = dialog.getByLabelText("Target 1 weight");
    await userEvent.clear(weight);
    await userEvent.type(weight, "2");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await expectToast(canvasElement, /gpt-4o updated/);
    const puts = repair.calls
      .filter((c) => c.method === "PUT")
      .map((c) => new URL(c.url, "http://x").pathname);
    await expect(puts).toEqual(["/api/v1/routes/route-1/variants", "/api/v1/routes/route-1"]);
    const sent = await repair.expectSentBody<{ variants: RouteVariantInput[] }>(
      "PUT",
      "/api/v1/routes/route-1/variants",
    );
    await expect(sent.variants[0].targets[0].takes_over).toEqual([]);
  },
};

/**
 * The other repair (#3510): the person removes every fallback-only variant and
 * edits the pool in the same save. The pool is judged against the variants
 * still stored, so they go first here, and the pool after, judged against
 * none.
 */
let removal: Recorder;
export const RepairsByRemovingFallbackOnlyVariantsInOneSave: Story = {
  render: () => {
    removal = strandedRoute();
    return (
      <Harness fetchStub={removal.stub} role="admin">
        <Toasted>
          <RoutingRules />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    await dialog.findByLabelText("Variant 2 target 1 provider");
    await userEvent.click(dialog.getByRole("button", { name: "Remove variant 2" }));
    await userEvent.click(dialog.getByRole("button", { name: "Remove variant 1" }));
    const weight = dialog.getByLabelText("Target 1 weight");
    await userEvent.clear(weight);
    await userEvent.type(weight, "2");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    await expectToast(canvasElement, /gpt-4o updated/);
    const puts = removal.calls
      .filter((c) => c.method === "PUT")
      .map((c) => new URL(c.url, "http://x").pathname);
    await expect(puts).toEqual(["/api/v1/routes/route-1/variants", "/api/v1/routes/route-1"]);
    const sent = await removal.expectSentBody<{ variants: RouteVariantInput[] }>(
      "PUT",
      "/api/v1/routes/route-1/variants",
    );
    await expect(sent.variants).toEqual([]);
  },
};

/**
 * A save refused for the variants as a whole moves focus to the variants'
 * own control, and the stranded message is said there rather than under the
 * route's targets, which are not what strands it (#3510).
 */
let strandedFocus: Recorder;
export const FocusMovesToTheStrandedVariants: Story = {
  render: () => {
    strandedFocus = strandedRoute();
    return (
      <Harness fetchStub={strandedFocus.stub} role="admin">
        <RoutingRules />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openRouteSheet(canvasElement);
    await dialog.findByLabelText("Variant 2 target 1 provider");
    await userEvent.click(dialog.getByRole("button", { name: "Save route" }));
    const add = dialog.getByRole("button", { name: "Add variant" });
    await waitFor(() => expect(add).toHaveFocus());
    await expect(add).toHaveAccessibleDescription(/a route with variants dials only/);
    await expect(dialog.getAllByText(/Every target is a fallback/)).toHaveLength(1);
    strandedFocus.expectNotSent("PUT", "/api/v1/routes/route-1");
  },
};
