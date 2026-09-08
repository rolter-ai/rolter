import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Logs from "./Logs";
import {
  Harness,
  expectEmptyState,
  expectLoadError,
  expectSkeleton,
  json,
  pending,
  recording,
  routes,
  scoped,
  type FetchStub,
} from "./story-harness";
import type {
  BusinessUnitRow,
  CustomerRow,
  InvocationRow,
} from "@/lib/api";
import { formattersFor } from "@/lib/i18n/format";
import { atMobile, atTablet, expectNoHorizontalOverflow } from "@/lib/story-viewport";

// the formatter the screen itself uses, so a story asserts the house format
// rather than a second copy of it
const fmt = formattersFor("en");

const PRICED_AT = "2026-10-05T12:34:56.789Z";

const row = (over: Partial<InvocationRow>): InvocationRow => ({
  ts: PRICED_AT,
  request_id: "req-1",
  trace_id: "trace-1",
  org_id: "org-1",
  team_id: "team-1",
  project_id: "project-1",
  virtual_key_id: "vk-1",
  business_unit_id: "",
  customer_id: "",
  model: "gpt-4o",
  provider: "openai",
  target: "openai/gpt-4o",
  variant: "",
  status: 200,
  stream: 0,
  cache_hit: 0,
  cache_read_tokens: 0,
  cache_write_tokens: 0,
  prompt_tokens: 8000,
  completion_tokens: 4345,
  total_tokens: 12345,
  cost_usd: 0.0123,
  unpriced: 0,
  latency_ms: 842,
  ttft_ms: 120,
  error: "",
  ...over,
});

const ROWS: InvocationRow[] = [
  row({}),
  // served against a model with no price row: the zero is the absence of a
  // cost, not a cost of zero (#969). the gateway said so on the row itself
  row({
    request_id: "req-2",
    model: "internal-llama",
    provider: "vllm",
    cost_usd: 0,
    unpriced: 1,
  }),
];

const UNIT: BusinessUnitRow = {
  id: "unit-1",
  org_id: "org-1",
  name: "Platform Engineering",
  slug: "platform-engineering",
  retired_at: null,
  created_at: "2026-01-05T10:00:00Z",
};

const CUSTOMER: CustomerRow = {
  id: "cust-1",
  org_id: "org-1",
  business_unit_id: "unit-1",
  name: "Acme Corp",
  slug: "acme-corp",
  retired_at: null,
  created_at: "2026-02-05T10:00:00Z",
};

/**
 * A stub that filters the way the control plane does (#1247): the attribution
 * dimensions arrive as comma-separated sets on the query string and narrow the
 * rows *before* the page is cut. A stub that ignored them would let a story
 * pass while the screen quietly filtered the page itself again.
 */
const serverFiltered = (rows: InvocationRow[], base = "USD"): FetchStub =>
  scoped(async (input) => {
    const url = new URL(String(input), "http://localhost");
    if (url.pathname === "/api/v1/analytics/invocations") {
      const set = (name: string) => {
        const raw = url.searchParams.get(name);
        return raw ? raw.split(",") : null;
      };
      const units = set("business_unit");
      const customers = set("customer");
      const data = rows.filter(
        (r) =>
          (!units || units.includes(r.business_unit_id)) &&
          (!customers || customers.includes(r.customer_id)),
      );
      return json({ data });
    }
    if (url.pathname === "/api/v1/currency")
      return json({ base, codes: [base], rates: {} });
    if (url.pathname === "/api/v1/models") return json([]);
    if (url.pathname.includes("/business-units")) return json([UNIT]);
    if (url.pathname.includes("/customers")) return json([CUSTOMER]);
    return json([]);
  });

const withLogs = (rows: InvocationRow[], base = "USD"): FetchStub =>
  routes([
    ["/api/v1/analytics/invocations", () => ({ data: rows })],
    ["/api/v1/currency", () => ({ base, codes: [base], rates: {} })],
    ["/api/v1/models", () => []],
    ["/business-units", () => [UNIT]],
    ["/customers", () => [CUSTOMER]],
  ]);

// one attributed request and one that never named a unit, so a filter has
// something to actually remove
const ATTRIBUTED: InvocationRow[] = [
  row({ request_id: "req-attributed", business_unit_id: "unit-1", customer_id: "cust-1" }),
  row({ request_id: "req-orphan", model: "internal-llama", provider: "vllm" }),
];

const meta = {
  title: "Screens/Logs",
  component: Logs,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Logs>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={withLogs(ROWS)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // one house stamp for the timestamp column, milliseconds included, and one
    // grouped number for tokens — neither follows the browser locale (#1182)
    await expect(await canvas.findAllByText(fmt.dateTimeMs(PRICED_AT))).toHaveLength(2);
    await expect(await canvas.findAllByText(fmt.number(12345))).toHaveLength(2);
    await expect(await canvas.findByText(fmt.currency(0.0123, "USD"))).toBeInTheDocument();
  },
};

/**
 * #969/#1182: a request against a model with no price used to read `$0.0000`,
 * which claims the request was free. The cell says "unknown" instead.
 */
export const UnpricedRequestsAreNotFree: Story = {
  render: () => (
    <Harness fetchStub={withLogs(ROWS)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const marker = await canvas.findByTitle(/no price is configured/i);
    await expect(marker).toBeInTheDocument();
    // and the false zero is nowhere on the screen
    await expect(canvas.queryByText(fmt.currency(0, "USD"))).toBeNull();
  },
};

/**
 * #1226: the marker keys off the `unpriced` flag the gateway recorded, not off
 * the live price catalogue. A request that genuinely cost nothing — a cache
 * hit, a zero-token completion — carries `unpriced: 0` and must read as free,
 * even though its cost is also zero and the screen no longer fetches prices.
 */
export const AZeroCostThatIsNotUnpricedReadsAsFree: Story = {
  render: () => (
    <Harness
      fetchStub={withLogs([
        row({ request_id: "req-free", cost_usd: 0, unpriced: 0 }),
      ])}
    >
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(fmt.currency(0, "USD"))).toBeInTheDocument();
    await expect(canvas.queryByTitle(/no price is configured/i)).toBeNull();
  },
};

/** Spend is denominated in the deployment's settlement currency, not in dollars. */
export const AmountsFollowTheDeploymentCurrency: Story = {
  render: () => (
    <Harness fetchStub={withLogs([row({})], "EUR")}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(fmt.currency(0.0123, "EUR"))).toBeInTheDocument();
    await expect(canvas.queryByText(fmt.currency(0.0123, "USD"))).toBeNull();
  },
};

export const Empty: Story = {
  render: () => (
    <Harness fetchStub={withLogs([])}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    // no filter is set, so this is "the gateway has served nothing yet" rather
    // than "your filters excluded everything" (#1180)
    await expectEmptyState(canvasElement, /Nothing logged yet/);
  },
};

// the screen had no loading indicator at all: a slow ClickHouse read was
// indistinguishable from a deployment that had served nothing (#1180)
export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

export const Failed: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async () => json({ error: { message: "clickhouse refused" } }, 500))}
    >
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return request logs/i);
  },
};

// a 403 is not a 500: retrying cannot fix a permission, so no retry is offered
export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "forbidden" } }, 403))}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /You do not have access to request logs/);
  },
};

/**
 * #1203: the 248px filter rail and the 380px detail drawer both sat in the
 * flow, so at 375px the table had 127px and the page scrolled sideways. Both
 * are overlays at this width, and the table scrolls inside its own container.
 */
export const Mobile: Story = {
  ...atMobile,
  render: () => (
    <Harness fetchStub={withLogs(ROWS)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.dateTimeMs(PRICED_AT));
    await expectNoHorizontalOverflow();
  },
};

export const Tablet: Story = {
  ...atTablet,
  render: () => (
    <Harness fetchStub={withLogs(ROWS)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.dateTimeMs(PRICED_AT));
    await expectNoHorizontalOverflow();
  },
};

/**
 * #1193: a request log that records which business unit paid for a call, and
 * then cannot be read by it, is a column nobody can use. The rail filters on
 * both governance dimensions.
 */
export const FiltersByBusinessUnit: Story = {
  render: () => (
    <Harness fetchStub={serverFiltered(ATTRIBUTED)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findAllByText("gpt-4o")).toHaveLength(1);
    await expect(await canvas.findByText("internal-llama")).toBeInTheDocument();

    await userEvent.click(canvas.getByRole("button", { name: /Filters/ }));
    await userEvent.click(
      await canvas.findByRole("button", { name: "Business unit" }),
    );
    await userEvent.click(
      await canvas.findByRole("checkbox", { name: "Platform Engineering" }),
    );

    // only the attributed row survives; the unattributed one is not "cheap",
    // it is charged to nobody, and a business-unit report must not include it
    await waitFor(() => expect(canvas.queryByText("internal-llama")).toBeNull());
    await expect(canvas.getAllByText("gpt-4o")).toHaveLength(1);
  },
};

/** the same rail, on the customer dimension */
export const FiltersByCustomer: Story = {
  render: () => (
    <Harness fetchStub={serverFiltered(ATTRIBUTED)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("internal-llama");
    await userEvent.click(canvas.getByRole("button", { name: /Filters/ }));
    await userEvent.click(await canvas.findByRole("button", { name: "Customer" }));
    await userEvent.click(await canvas.findByRole("checkbox", { name: "Acme Corp" }));
    await waitFor(() => expect(canvas.queryByText("internal-llama")).toBeNull());
  },
};

/**
 * #1247: the attribution filter is a query the server answers, not a pass over
 * the page it already returned. The request has to carry it — a screen that
 * filtered locally would look identical on a single page and be wrong on every
 * page after it.
 */
const filtered = recording(serverFiltered(ATTRIBUTED));

export const TheAttributionFilterIsSentToTheServer: Story = {
  render: () => (
    <Harness fetchStub={filtered.stub}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("internal-llama");
    await userEvent.click(canvas.getByRole("button", { name: /Filters/ }));
    await userEvent.click(
      await canvas.findByRole("button", { name: "Business unit" }),
    );
    await userEvent.click(
      await canvas.findByRole("checkbox", { name: "Platform Engineering" }),
    );

    await waitFor(() => {
      const asked = filtered.calls.find((c) =>
        c.url.includes("business_unit=unit-1"),
      );
      expect(asked).toBeDefined();
    });

    // and the warning that the old client-side filter needed is gone, because
    // the answer is no longer partial
    await expect(canvas.queryByText(/searched by model, key and status only/)).toBeNull();
  },
};

/** the detail drawer names the unit and the customer, not their uuids */
export const DetailDrawerNamesTheAttribution: Story = {
  render: () => (
    <Harness fetchStub={withLogs(ATTRIBUTED)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open request details for gpt-4o/i }),
    );
    const drawer = within(await canvas.findByRole("complementary", { name: "Details" }));
    await expect(drawer.getByText("Platform Engineering")).toBeVisible();
    await expect(drawer.getByText("Acme Corp")).toBeVisible();
  },
};

/**
 * #954: an absent payload used to read "payload logging is off", which is one
 * of three possible reasons and often the wrong one. A viewer cannot read the
 * logging settings — that route is superadmin-only — so the screen does not
 * ask, and the copy names every reason instead of asserting one. Either way it
 * says where the setting lives.
 */
export const AnAbsentPayloadSaysWhy: Story = {
  render: () => (
    <Harness fetchStub={withLogs(ROWS)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open request details for gpt-4o/i }),
    );
    const drawer = within(await canvas.findByRole("complementary", { name: "Details" }));
    // both the Request and the Response panel explain themselves
    await expect(drawer.getAllByText(/retention window has already passed/i)).toHaveLength(2);
    // and it is not the old claim, which asserted a reason it could not know
    await expect(drawer.queryByText("payload logging is off")).not.toBeInTheDocument();
    const link = drawer.getAllByRole("link", { name: "Open log settings" })[0];
    await expect(link).toHaveAttribute("href", "/logs-settings");
  },
};

// a control plane with no clickhouse_url answers the analytics routes 503, and
// one too old to have them answers 404. Both used to render an untranslated
// grey paragraph of this screen's own (#1236)
export const NoAnalyticsStore: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) =>
        String(input).includes("/analytics/")
          ? json({ error: { message: "analytics is not configured" } }, 503)
          : json([]),
      )}
    >
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /Analytics are not configured/i);
    // the setting to change is named, and the retry that cannot help is withheld
    await expect(canvas.getByText(/CLICKHOUSE_URL/)).toBeVisible();
    await expect(canvas.queryByRole("button", { name: /try again/i })).toBeNull();
  },
};
