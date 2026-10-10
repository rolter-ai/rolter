import type { Meta, StoryObj } from "@storybook/react-vite";
import { focusManager } from "@tanstack/react-query";
import { MemoryRouter, useLocation } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Logs from "./Logs";
import {
  Harness,
  LOADING_LABEL,
  expectEmptyState,
  expectGateAnswered,
  expectLoadError,
  expectNoUxEvent,
  expectSkeleton,
  expectUxEvent,
  json,
  pending,
  recordUxEvents,
  recording,
  routes,
  scoped,
  uxEvents,
  type FetchStub,
  type Recorder,
} from "./story-harness";
import type { BusinessUnitRow, CustomerRow, InvocationRow, VirtualKeyRow } from "@/lib/api";
import { formattersFor } from "@/lib/i18n/format";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import {
  atLaptop,
  atMobile,
  atNarrow,
  atTablet,
  expectInFrame,
  expectInViewport,
  expectNoHorizontalOverflow,
  phoneFits,
} from "@/lib/story-viewport";
import { UxScreenProvider } from "@/lib/ux-react";

// the formatter the screen itself uses, so a story asserts the house format
// rather than a second copy of it
const fmt = formattersFor("en");

// every fixture row gets its own stamp, 350ms after the one before, so the
// time column can never look right while it renders one value for every row.
// the #1202 audit read exactly that off a constant here and filed it as a
// backend bug (#1344, #1393)
const BASE_TS = Date.parse("2026-10-05T12:34:56.789Z");
const STEP_MS = 350;
let seq = 0;
const nextTs = () => new Date(BASE_TS + STEP_MS * seq++).toISOString();

// the upstream answered with the status the caller got, on the first try, unless a
// row says otherwise: a connection that never got a status line reads 0 (#2837)
const row = (over: Partial<InvocationRow>): InvocationRow => ({
  ts: nextTs(),
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
  upstream_status: Number(over.status ?? 200),
  attempts: 1,
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

// the models the rail's model filter offers
const MODELS = [{ model: "gpt-4o" }, { model: "internal-llama" }];

// the control plane's status classes, as `status_predicate` in
// crates/rolter-control/src/analytics.rs writes them
const inStatusClass = (status: number, wanted: string) =>
  wanted === "error" ? status >= 400 : wanted === "success" ? status > 0 && status < 400 : true;

/**
 * A stub that filters the way the control plane does (#1247): the attribution
 * dimensions arrive as comma-separated sets on the query string and narrow the
 * rows *before* the page is cut. A stub that ignored them would let a story
 * pass while the screen quietly filtered the page itself again. The status
 * class, the one exact model and the unpriced flag (#1986) narrow the same way.
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
      const model = url.searchParams.get("model");
      const status = url.searchParams.get("status") ?? "all";
      const onlyUnpriced = url.searchParams.get("unpriced") === "true";
      const data = rows.filter(
        (r) =>
          (!units || units.includes(r.business_unit_id)) &&
          (!customers || customers.includes(r.customer_id)) &&
          (!model || r.model === model) &&
          (!onlyUnpriced || Number(r.unpriced ?? 0) === 1) &&
          inStatusClass(Number(r.status), status),
      );
      return json({ data });
    }
    if (url.pathname === "/api/v1/currency") return json({ base, codes: [base], rates: {} });
    if (url.pathname === "/api/v1/models") return json(MODELS);
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

/**
 * A polling cadence a play can watch several intervals of (#1984). The screen
 * polls every 5s, and the story tests give a whole story 15s, so a play that
 * waited out two real intervals after a retried load would have no budget left.
 * The cadence is the only thing that changes: the policy under test is the same.
 */
const FAST_POLL_MS = 400;
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** how many times the screen has asked for a page of the log */
const logReads = (recorder: Recorder) =>
  recorder.calls.filter((c) => c.url.includes("/analytics/invocations")).length;

const refusing = (): FetchStub =>
  scoped(async () => json({ error: { message: "clickhouse refused" } }, 500));

/**
 * The columns the table is drawing, and whether it has to scroll sideways to
 * draw them (#1986). The narrower columns give way to the width the table has
 * rather than scrolling out of it, so a read of the headers and of a row's
 * cells says what a reader can see without touching the scrollbar.
 */
const tableShape = (canvasElement: HTMLElement) => {
  const table = canvasElement.querySelector("table") as HTMLTableElement;
  const scroller = table.parentElement as HTMLElement;
  const drawn = (el: Element) => getComputedStyle(el).display !== "none";
  return {
    scroller,
    columns: Array.from(table.querySelectorAll("thead th"))
      .filter(drawn)
      .map((th) => th.textContent ?? ""),
    cells: (row: number) =>
      Array.from(table.querySelectorAll("tbody tr")[row].querySelectorAll("td")).filter(drawn),
    scrolls: scroller.scrollWidth > scroller.clientWidth,
  };
};

/** Every cell of the row is inside the scroll area's own frame, not past its edge. */
async function expectRowInFrame(shape: ReturnType<typeof tableShape>, row: number) {
  for (const cell of shape.cells(row)) await expectInFrame(cell, shape.scroller);
}

const meta = {
  title: "Screens/Logs",
  component: Logs,
  parameters: { layout: "fullscreen" },
  // the payload drawer links to the log settings through react-router, and the
  // filters live in the address (#1985), so every story supplies a router the
  // way main.tsx does. `parameters.address` is where a story's router starts
  decorators: [
    (Story, { parameters }) => (
      <MemoryRouter initialEntries={parameters.address ? [parameters.address] : undefined}>
        <Story />
      </MemoryRouter>
    ),
  ],
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
    // one house stamp per row for the timestamp column, milliseconds included,
    // and one grouped number for tokens — neither follows the browser locale (#1182)
    for (const r of ROWS)
      await expect(await canvas.findAllByText(fmt.timeMs(r.ts))).toHaveLength(1);
    await expect(await canvas.findAllByText(fmt.number(12345))).toHaveLength(2);
    await expect(await canvas.findByText(fmt.currency(0.0123, "USD"))).toBeInTheDocument();
    // a fetch that succeeded is the one state the toolbar may call live (#1984)
    await expect(canvas.getByText("Streaming · 2 requests")).toBeVisible();
  },
};

// a row's chevron is a 15px glyph; its button must still be a 24px target (WCAG 2.5.8, #2573)
export const RowChevronHasA24pxHitArea: Story = {
  render: () => (
    <Harness fetchStub={withLogs(ROWS)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const [chevron] = await canvas.findAllByRole("button", {
      name: en.analytics.openDetails.replace("{{model}}", "gpt-4o"),
    });
    const box = chevron.getBoundingClientRect();
    await expect(box.width).toBeGreaterThanOrEqual(24);
    await expect(box.height).toBeGreaterThanOrEqual(24);
  },
};

// a burst: three requests a few hundred ms apart, then two that landed inside
// the same millisecond. the tie is real traffic, not a fixture mistake
const BURST_AT = Date.parse("2026-10-05T00:02:22.061Z");
const burstTs = (ms: number) => new Date(BURST_AT + ms).toISOString();
const BURST: InvocationRow[] = [
  row({ request_id: "req-b1", ts: burstTs(0) }),
  row({ request_id: "req-b2", ts: burstTs(250) }),
  row({ request_id: "req-b3", ts: burstTs(500) }),
  row({ request_id: "req-b4", ts: burstTs(750) }),
  row({ request_id: "req-b5", ts: burstTs(750) }),
];

/**
 * #1393: every row renders the time it was served, not one shared value. A
 * regression that collapsed the column to a single stamp would otherwise look
 * plausible on a fixture that only ever had one.
 */
export const EveryRowShowsItsOwnTimestamp: Story = {
  render: () => (
    <Harness fetchStub={withLogs(BURST)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText(fmt.timeMs(BURST[0].ts));
    const stamps = Array.from(
      canvasElement.querySelectorAll("tbody tr"),
      (tr) => tr.querySelector("td")?.textContent ?? "",
    );
    await expect(stamps).toEqual(BURST.map((r) => fmt.timeMs(r.ts)));
    // four distinct instants, milliseconds included, and the tie kept both rows
    await expect(new Set(stamps).size).toBe(4);
    await expect(canvas.getAllByText(fmt.timeMs(burstTs(750)))).toHaveLength(2);

    // the cell is the clock and nothing else: the date is not repeated on every
    // row of a day's log, and the full stamp is one hover away (#1986)
    for (const stamp of stamps) await expect(stamp).toMatch(/^\d{2}:\d{2}:\d{2}\.\d{3}$/);
    const time = canvasElement.querySelector("tbody tr time");
    await expect(time).toHaveAttribute("datetime", BURST[0].ts);
    await expect(time).toHaveAttribute("title", fmt.dateTimeMs(BURST[0].ts));
  },
};

/**
 * #969/#1182: a request against a model with no price used to read `$0.0000`,
 * which claims the request was free. It then read a dim dash, the same glyph as
 * a missing provider, with the reason in a tooltip a keyboard or a touch never
 * reaches. The cell now says `unpriced` in words, in mono, and keeps the
 * explanation on hover (#1986).
 */
export const UnpricedRequestsAreNotFree: Story = {
  render: () => (
    <Harness fetchStub={withLogs(ROWS)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const label = await canvas.findByText("unpriced");
    await expect(label).toBeVisible();
    await expect(label.closest("td")).toHaveTextContent(/^unpriced$/);
    await expect(getComputedStyle(label).fontFamily).toMatch(/mono/i);
    await expect(label.getAttribute("title")).toMatch(/no price is configured/i);
    // only the row with no price carries it: the other one shows its money
    await expect(canvas.getAllByText("unpriced")).toHaveLength(1);
    await expect(canvas.getByText(fmt.currency(0.0123, "USD"))).toBeVisible();
    // and neither the false zero nor a dash standing in for it is on the screen
    await expect(canvas.queryByText(fmt.currency(0, "USD"))).toBeNull();
    await expect(label.closest("td")).not.toHaveTextContent("—");
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
    <Harness fetchStub={withLogs([row({ request_id: "req-free", cost_usd: 0, unpriced: 0 })])}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(fmt.currency(0, "USD"))).toBeInTheDocument();
    await expect(canvas.queryByTitle(/no price is configured/i)).toBeNull();
    await expect(canvas.queryByText("unpriced")).toBeNull();
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
    // and the toolbar does not claim a stream it has not received yet (#1984)
    const canvas = within(canvasElement);
    await expect(canvas.getByText("Loading requests")).toBeVisible();
    await expect(canvas.queryByText(/Streaming/)).toBeNull();
  },
};

const failing = recording(refusing());

/**
 * #1984: a first load that fails stays failed until someone retries it. The
 * feed used to go on polling, and a query that never held data goes back to
 * pending on every refetch, so the alert and a skeleton took turns — and a
 * screen reader heard the alert again each cycle — while the toolbar pulsed
 * green and said "Streaming".
 */
export const Failed: Story = {
  render: () => (
    <Harness fetchStub={failing.stub}>
      <Logs pollMs={FAST_POLL_MS} />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /failed to return request logs/i);
    const alert = canvas
      .getAllByRole("alert")
      .find((a) => /failed to return request logs/i.test(a.textContent ?? ""));
    const reads = logReads(failing);
    // the toolbar reports the failure and when it happened, not a stream
    await expect(canvas.getByText(/^Load failed at /)).toBeVisible();
    await expect(canvas.queryByText(/Streaming/)).toBeNull();

    // three polling intervals later it is the same alert node: a poll that sent
    // the query back to pending would have unmounted it, leaving the play
    // holding a detached one. no skeleton stood in for it, and no request left
    await sleep(FAST_POLL_MS * 3);
    await expect(alert?.isConnected).toBe(true);
    await expect(canvas.queryAllByLabelText(LOADING_LABEL)).toHaveLength(0);
    await expect(logReads(failing)).toBe(reads);
    await expect(canvas.queryByText(/Streaming/)).toBeNull();

    // the retry the alert offers still asks again
    await userEvent.click(within(alert!).getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(logReads(failing)).toBeGreaterThan(reads));
  },
};

// fails until the play says otherwise, so a story can watch a retry recover
// or a live feed start failing under it
let upstream: "ok" | "failing" = "ok";
const flaky = recording(
  scoped(async (input) => {
    const url = new URL(String(input), "http://localhost");
    if (url.pathname === "/api/v1/analytics/invocations") {
      return upstream === "ok"
        ? json({ data: ROWS })
        : json({ error: { message: "clickhouse refused" } }, 500);
    }
    if (url.pathname === "/api/v1/currency")
      return json({ base: "USD", codes: ["USD"], rates: {} });
    return json([]);
  }),
);

/** #1984: stopping the poll must not strand the screen — a retry that lands resumes the feed. */
export const ARetryResumesTheFeed: Story = {
  beforeEach: () => {
    upstream = "failing";
  },
  render: () => (
    <Harness fetchStub={flaky.stub}>
      <Logs pollMs={FAST_POLL_MS} />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /failed to return request logs/i);
    upstream = "ok";
    await userEvent.click(canvas.getByRole("button", { name: "Try again" }));
    await canvas.findByText(fmt.timeMs(ROWS[0].ts));
    await expect(canvas.getByText("Streaming · 2 requests")).toBeVisible();
    await expect(canvas.queryAllByRole("alert")).toHaveLength(0);
    // and it polls again
    const reads = logReads(flaky);
    await waitFor(() => expect(logReads(flaky)).toBeGreaterThan(reads));
  },
};

/** every `since` the screen has sent for the log, oldest first */
const logSinces = (recorder: Recorder): number[] =>
  recorder.calls
    .filter((c) => c.url.includes("/analytics/invocations"))
    .map((c) => Date.parse(new URL(c.url, "http://localhost").searchParams.get("since") ?? ""));

const windowed = recording(withLogs(ROWS));

/**
 * #2315: "the last 24 hours" is the 24 hours before each read. `since` used to
 * be fixed when the screen mounted, and every poll reused it, so a tab left
 * open for an afternoon read the last 24 hours plus the afternoon. Each poll's
 * `since` is later than the one before, and the query key does not churn with
 * it: a key that changed on every render would fetch on every render.
 */
export const TheWindowRollsForwardWithEveryPoll: Story = {
  render: () => (
    <Harness fetchStub={windowed.stub}>
      <Logs pollMs={FAST_POLL_MS} />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByText(fmt.timeMs(ROWS[0].ts));
    await waitFor(() => expect(logSinces(windowed).length).toBeGreaterThan(2));
    const [first, second, third] = logSinces(windowed);
    await expect(second).toBeGreaterThan(first);
    await expect(third).toBeGreaterThan(second);
    // 24 hours behind the moment it was sent, not behind when the screen opened
    const sent = logSinces(windowed);
    const behind = Date.now() - sent[sent.length - 1];
    await expect(behind).toBeGreaterThanOrEqual(24 * 3_600_000);
    await expect(behind).toBeLessThan(24 * 3_600_000 + 10_000);
    // one request per poll: the key held still while `since` moved
    await expect(logSinces(windowed).length).toBeLessThan(12);
  },
};

/**
 * #1984: a refresh that fails with rows already on screen keeps them, keeps its
 * error through the next attempt, and goes on polling — so the toolbar says the
 * refresh failed, when, and that it is retrying. Paused, it stops promising a
 * retry it will not make.
 */
export const ARefreshFailureKeepsTheRows: Story = {
  beforeEach: () => {
    upstream = "ok";
  },
  render: () => (
    <Harness fetchStub={flaky.stub}>
      <Logs pollMs={FAST_POLL_MS} />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await failARefresh(canvasElement);
    await expect(canvas.getByText(fmt.timeMs(ROWS[0].ts))).toBeVisible();
    await expectLoadError(canvasElement, /failed to return request logs/i);
    await expect(canvas.queryByText(/Streaming/)).toBeNull();

    await userEvent.click(canvas.getByRole("button", { name: "Pause" }));
    await expect(canvas.getByText(/^Refresh failed at [^,]+$/)).toBeVisible();
  },
};

/** Load the rows, then fail every read after them until the toolbar says so. */
async function failARefresh(canvasElement: HTMLElement): Promise<void> {
  const canvas = within(canvasElement);
  await canvas.findByText(fmt.timeMs(ROWS[0].ts));
  upstream = "failing";
  // the next poll, plus the screen's own two retries (1s, then 2s), which
  // outlast the shared 5s budget on a busy runner
  await canvas.findByText(/^Refresh failed at .+, retrying$/, {}, { timeout: 8000 });
}

/** The longest thing the toolbar says, at 375px: it wraps rather than pushing the page sideways. */
export const ARefreshFailureFitsOnMobile: Story = {
  ...atMobile,
  beforeEach: () => {
    upstream = "ok";
  },
  render: () => (
    <Harness fetchStub={flaky.stub}>
      <Logs pollMs={FAST_POLL_MS} />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await failARefresh(canvasElement);
    await expectNoHorizontalOverflow();
  },
};

const parked = recording(refusing());

/**
 * #1984: in a hidden tab react-query parks a failed first load's retry until
 * focus returns. The query is then pending and not fetching — no error yet,
 * and no data — and an empty state gated on "not loading and no error" said
 * "Nothing logged yet" about a load that had failed. Only a successful answer
 * can be empty.
 */
export const AFailedLoadInAHiddenTabIsNotEmpty: Story = {
  beforeEach: () => {
    focusManager.setFocused(false);
    return () => focusManager.setFocused(undefined);
  },
  render: () => (
    <Harness fetchStub={parked.stub}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(logReads(parked)).toBeGreaterThan(0));
    const reads = logReads(parked);
    // the first retry waits 1s, then parks behind the hidden tab rather than
    // asking again
    await sleep(1500);
    await expect(logReads(parked)).toBe(reads);
    await expect(canvas.queryByText(/Nothing logged yet/)).toBeNull();
    await expectSkeleton(canvasElement);
    await expect(canvas.getByText("Loading requests")).toBeVisible();
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

// the log read is held until the play lets it go, so a story can watch what
// the screen reports while the skeleton is still up
let releaseLog: () => void = () => {};
let logGate: Promise<void> = Promise.resolve();
const holdTheLog = () => {
  logGate = new Promise<void>((resolve) => {
    releaseLog = resolve;
  });
};

const answersModels = recording(
  scoped(async (input) => {
    const url = new URL(String(input), "http://localhost");
    if (url.pathname === "/api/v1/analytics/invocations") {
      await logGate;
      return json({ data: ROWS });
    }
    if (url.pathname === "/api/v1/currency")
      return json({ base: "USD", codes: ["USD"], rates: {} });
    if (url.pathname === "/api/v1/models") return json(MODELS);
    return json([]);
  }),
);

/**
 * #2017: `screen_ready` followed the model list, which only feeds the rail's
 * picker. It fired as soon as that answered, over a log table that was still a
 * skeleton. It now waits for the log read and fires once, when it lands.
 */
export const TheScreenIsNotReadyWhileTheLogIsOut: Story = {
  beforeEach: () => {
    holdTheLog();
    return recordUxEvents();
  },
  render: () => (
    <UxScreenProvider screen="logs">
      <Harness fetchStub={answersModels.stub}>
        <Logs />
      </Harness>
    </UxScreenProvider>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectSkeleton(canvasElement);
    // the model list has answered, and the screen is still not interactive
    await waitFor(() =>
      expect(answersModels.calls.some((c) => c.url.includes("/api/v1/models"))).toBe(true),
    );
    await sleep(300);
    expectNoUxEvent("time_to_interactive");

    releaseLog();
    await canvas.findByText(fmt.timeMs(ROWS[0].ts));
    const ready = await expectUxEvent("time_to_interactive");
    await expect(ready.screen).toBe("logs");
    await expect(typeof ready.duration_ms).toBe("number");
    await expect(uxEvents().filter((e) => e.action === "time_to_interactive")).toHaveLength(1);
  },
};

// the model list never answers; the log does
const noModelList = recording(
  scoped(async (input) => {
    const url = new URL(String(input), "http://localhost");
    if (url.pathname === "/api/v1/analytics/invocations") return json({ data: ROWS });
    if (url.pathname === "/api/v1/currency")
      return json({ base: "USD", codes: ["USD"], rates: {} });
    if (url.pathname === "/api/v1/models") return new Promise<Response>(() => {});
    return json([]);
  }),
);

/** #2017: a model list that never comes back does not hold the screen back from being ready. */
export const TheScreenIsReadyWithoutTheModelList: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <UxScreenProvider screen="logs">
      <Harness fetchStub={noModelList.stub}>
        <Logs />
      </Harness>
    </UxScreenProvider>
  ),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByText(fmt.timeMs(ROWS[0].ts));
    const ready = await expectUxEvent("time_to_interactive");
    await expect(ready.screen).toBe("logs");
  },
};

// what a ClickHouse outage looks like: the analytics reads fail, and the
// catalogue the rail's picker reads answers as usual
const failedRead = recording(
  scoped(async (input) => {
    const url = new URL(String(input), "http://localhost");
    if (url.pathname.startsWith("/api/v1/analytics/"))
      return json({ error: { message: "clickhouse refused" } }, 500);
    if (url.pathname === "/api/v1/currency")
      return json({ base: "USD", codes: ["USD"], rates: {} });
    if (url.pathname === "/api/v1/models") return json(MODELS);
    return json([]);
  }),
);

/**
 * #2017: a ClickHouse outage fails every log read and recorded nothing, because
 * the `error_state` row followed the model list. It follows the log read now,
 * once for the failure and not once per retry or per poll, in the region the
 * empty state names so the two pair up in the dead-states query.
 */
export const AFailedLogReadIsAnErrorState: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <UxScreenProvider screen="logs">
      <Harness fetchStub={failedRead.stub}>
        <Logs pollMs={FAST_POLL_MS} />
      </Harness>
    </UxScreenProvider>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return request logs/i);
    const failed = await expectUxEvent("error_state", "request-logs");
    await expect(failed.screen).toBe("logs");
    await expect(failed.outcome).toBe("error");
    // the feed stopped polling on the failure, so nothing adds a second row
    await sleep(FAST_POLL_MS * 3);
    await expect(uxEvents().filter((e) => e.action === "error_state")).toHaveLength(1);
  },
};

// the log answers and the model list fails
const failedModelList = recording(
  scoped(async (input) => {
    const url = new URL(String(input), "http://localhost");
    if (url.pathname === "/api/v1/analytics/invocations") return json({ data: ROWS });
    if (url.pathname === "/api/v1/currency")
      return json({ base: "USD", codes: ["USD"], rates: {} });
    if (url.pathname === "/api/v1/models")
      return json({ error: { message: "catalogue down" } }, 500);
    return json([]);
  }),
);

/** #2017: the model list failing is the rail's problem, not the screen's: no `error_state` for it. */
export const AFailedModelListIsNotTheLogsError: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <UxScreenProvider screen="logs">
      <Harness fetchStub={failedModelList.stub}>
        <Logs />
      </Harness>
    </UxScreenProvider>
  ),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByText(fmt.timeMs(ROWS[0].ts));
    await waitFor(() =>
      expect(failedModelList.calls.some((c) => c.url.includes("/api/v1/models"))).toBe(true),
    );
    await sleep(300);
    expectNoUxEvent("error_state");
    await expectUxEvent("time_to_interactive");
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
    await canvas.findByText(fmt.timeMs(ROWS[0].ts));
    await expectNoHorizontalOverflow();

    // #1986: the table kept an 880px floor and scrolled inside its card, so at
    // 375px only Time and Model were in view and Cost was off the edge. Now the
    // columns that matter share the width and the rest give way; the provider,
    // latency and tokens are all in the drawer
    const shape = tableShape(canvasElement);
    await expect(shape.columns).toEqual(["Time", "Model", "Status", "Cost", "Details"]);
    await expect(shape.scrolls).toBe(false);
    for (const row of [0, 1]) {
      await expectRowInFrame(shape, row);
      for (const cell of shape.cells(row)) await expectInViewport(cell);
    }
    // the unpriced row says so in the cost column that is on screen
    await expect(within(shape.cells(1)[3]).getByText("unpriced")).toBeVisible();
    await expect(within(shape.cells(0)[3]).getByText(fmt.currency(0.0123, "USD"))).toBeVisible();
    await expectStackedRow(canvasElement, 0, ROWS[0].model);
  },
};

/**
 * #2446: below 480px a row stacks. The model is on the first line with the
 * status, time and cost on the second, the model is cut with an ellipsis and
 * not wrapped, and its full name is in the row's accessible name.
 */
async function expectStackedRow(canvasElement: HTMLElement, rowIndex: number, model: string) {
  const shape = tableShape(canvasElement);
  const [time, modelCell, status, cost] = [
    shape.cells(rowIndex)[0],
    shape.cells(rowIndex)[1],
    shape.cells(rowIndex)[2],
    shape.cells(rowIndex)[3],
  ];
  await expect(modelCell).toHaveTextContent(model);
  // one line: no taller than the line it is set in, and cut rather than wrapped
  const style = getComputedStyle(modelCell);
  await expect(style.whiteSpace).toBe("nowrap");
  await expect(style.textOverflow).toBe("ellipsis");
  await expect(modelCell.getBoundingClientRect().height).toBeLessThan(
    parseFloat(style.lineHeight) * 1.5,
  );
  // the model and the status share the first line, the time and the cost the second
  const top = (el: Element) => Math.round(el.getBoundingClientRect().top);
  const bottom = (el: Element) => el.getBoundingClientRect().bottom;
  await expect(Math.abs(top(modelCell) - top(status))).toBeLessThanOrEqual(6);
  await expect(Math.abs(top(time) - top(cost))).toBeLessThanOrEqual(2);
  await expect(top(time)).toBeGreaterThanOrEqual(bottom(modelCell) - 1);
  await expect(cost.getBoundingClientRect().left).toBeGreaterThan(
    time.getBoundingClientRect().left,
  );
  // the chevron is the row's own button, named by the full model
  const button = within(shape.cells(rowIndex)[4]).getByRole("button");
  await expect(button).toHaveAccessibleName(new RegExp(model));
  await expectInViewport(button);
}

// a model name long enough to be cut in the narrowest row it gets, and a row
// with no price, so the cost column holds both of the things it can hold
const LONG_MODEL = row({
  request_id: "req-long-model",
  model: "claude-sonnet-4-5-20250929",
  provider: "anthropic",
  cost_usd: 0,
  unpriced: 1,
});

/**
 * #1986 in Russian, the longer copy: Time, Status and Cost all stay in frame
 * at 375px with a model name that has to be cut, and the page does not scroll
 * sideways. #2446: the long model stays on one line in every row.
 */
export const TimeStatusAndCostStayInFrameInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => (
    <Harness fetchStub={withLogs([...ROWS, LONG_MODEL])}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const clock = formattersFor("ru").timeMs(ROWS[0].ts);
    await canvas.findByText(clock);
    await expectNoHorizontalOverflow();

    const shape = tableShape(canvasElement);
    await expect(shape.columns).toEqual([
      ru.pages.logs.time,
      ru.pages.logs.model,
      ru.pages.logs.status,
      ru.pages.logs.cost,
      ru.analytics.details,
    ]);
    await expect(shape.scrolls).toBe(false);
    for (const row of [0, 1, 2]) {
      await expectRowInFrame(shape, row);
      for (const cell of shape.cells(row)) await expectInViewport(cell);
    }
    // the label is the Russian one, and it is in the cost column of the row
    await expect(within(shape.cells(2)[3]).getByText(ru.analytics.unpriced)).toBeVisible();
    // the clock is the locale's own: a comma before the milliseconds
    await expect(shape.cells(0)[0]).toHaveTextContent(/^\d{2}:\d{2}:\d{2},\d{3}$/);
    for (const [index, model] of [ROWS[0].model, ROWS[1].model, LONG_MODEL.model].entries())
      await expectStackedRow(canvasElement, index, model);
  },
};

/**
 * #1986: the table is narrower than the window once the filter rail and the
 * detail drawer are open beside it, and it gives up columns to the width it
 * actually has, widest need first, never Status or Cost. With both panels open
 * at 1280px it is 652px wide and keeps six columns without scrolling.
 */
export const TheColumnsFollowTheWidthTheTableHas: Story = {
  render: () => (
    <Harness fetchStub={withLogs(ROWS)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText(fmt.timeMs(ROWS[0].ts));
    const all = ["Time", "Model", "Provider", "Status", "Latency", "Tokens", "Cost", "Details"];
    await expect(tableShape(canvasElement).columns).toEqual(all);

    // the rail takes 248px and the table still has room for all eight
    await userEvent.click(canvas.getByRole("button", { name: /Filters/ }));
    await canvas.findByRole("button", { name: "Hide filters" });
    await waitFor(() => expect(tableShape(canvasElement).columns).toEqual(all));

    // the drawer takes 380px more: the provider and the tokens go, in that order
    await userEvent.click(canvas.getByRole("button", { name: /Open request details for gpt-4o/i }));
    await canvas.findByRole("complementary", { name: "Details" });
    await waitFor(() =>
      expect(tableShape(canvasElement).columns).toEqual([
        "Time",
        "Model",
        "Status",
        "Latency",
        "Cost",
        "Details",
      ]),
    );
    const shape = tableShape(canvasElement);
    await expect(shape.scrolls).toBe(false);
    await expectRowInFrame(shape, 0);
    await expectRowInFrame(shape, 1);
    // Status and Cost are both in the row that was opened
    await expect(within(shape.cells(0)[2]).getByText("200")).toBeVisible();
    await expect(within(shape.cells(0)[4]).getByText(fmt.currency(0.0123, "USD"))).toBeVisible();

    // closing the drawer gives the columns back
    await userEvent.click(canvas.getByRole("button", { name: "Close details" }));
    await waitFor(() => expect(tableShape(canvasElement).columns).toEqual(all));
  },
};

/**
 * #1986: beside the 232px sidebar the 380px drawer left the table 412px at the
 * `lg` breakpoint, and 164px with the rail open too. It overlays the table as a
 * sheet below `xl`, the same panel out of the flow, so the table keeps the
 * whole window.
 */
export const TheDrawerOverlaysTheTableBelowXl: Story = {
  ...atLaptop,
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
    const dialog = await within(document.body).findByRole("dialog");
    await waitFor(() => expect(within(dialog).getByText("200")).toBeVisible());
    // not the inline panel beside the table, and the table has not been squeezed
    await expect(canvas.queryByRole("complementary", { name: "Details" })).toBeNull();
    const shape = tableShape(canvasElement);
    await expect(shape.columns).toHaveLength(8);
    await expect(shape.scrolls).toBe(false);
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
    await canvas.findByText(fmt.timeMs(ROWS[0].ts));
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
    await userEvent.click(await canvas.findByRole("button", { name: "Business unit" }));
    await userEvent.click(await canvas.findByRole("checkbox", { name: "Platform Engineering" }));

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
    await userEvent.click(await canvas.findByRole("button", { name: "Business unit" }));
    await userEvent.click(await canvas.findByRole("checkbox", { name: "Platform Engineering" }));

    await waitFor(() => {
      const asked = filtered.calls.find((c) => c.url.includes("business_unit=unit-1"));
      expect(asked).toBeDefined();
    });

    // and the warning that the old client-side filter needed is gone, because
    // the answer is no longer partial
    await expect(canvas.queryByText(/searched by model, key and status only/)).toBeNull();
  },
};

/**
 * The router's search string, published so a play can read what the screen
 * wrote to the address (#1985). It rides on a data attribute with no text, so
 * no `getByText` can match it.
 */
function AddressProbe() {
  const { search } = useLocation();
  return <span data-testid="address" data-search={search} hidden />;
}

const addressOf = (canvasElement: HTMLElement) =>
  new URLSearchParams(within(canvasElement).getByTestId("address").dataset.search ?? "");

/** the query string of the screen's latest read of the log */
const lastLogQuery = (recorder: Recorder) => {
  const reads = recorder.calls.filter((c) => c.url.includes("/analytics/invocations"));
  return new URL(reads[reads.length - 1]?.url ?? "", "http://localhost").searchParams;
};

/** the model cell of every row in the table, in order */
const modelsOnScreen = (canvasElement: HTMLElement) =>
  Array.from(
    canvasElement.querySelectorAll("tbody tr"),
    (tr) => tr.querySelectorAll("td")[1]?.textContent ?? "",
  );

// one request that succeeded and one that failed, on two models, so the status
// and the model filter each have a row to remove
const MIXED: InvocationRow[] = [
  row({ request_id: "req-ok", model: "gpt-4o" }),
  row({
    request_id: "req-failed",
    model: "internal-llama",
    provider: "vllm",
    status: 502,
    upstream_status: 0,
    error: "upstream reset the connection",
  }),
];

const byStatus = recording(serverFiltered(MIXED));

/**
 * #1985: status was a pair of checkboxes, and ticking both collapsed to "all"
 * and cleared both ticks, undoing the reader's click without a word. It is one
 * choice of three now, each pick is the class the server is asked for, and the
 * address carries it.
 */
export const StatusIsOneChoiceOfThree: Story = {
  render: () => (
    <Harness fetchStub={byStatus.stub}>
      <Logs />
      <AddressProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("internal-llama");
    await userEvent.click(canvas.getByRole("button", { name: /Filters/ }));
    const group = within(await canvas.findByRole("radiogroup", { name: "Status" }));
    const radio = (name: string) => group.getByRole("radio", { name });
    await expect(radio("All")).toHaveAttribute("aria-checked", "true");
    // the label promises what the server matches, an answer below 400, and
    // not the 2xx it used to claim
    await expect(
      canvas.getByText("OK is any request answered with a status below 400."),
    ).toBeVisible();
    await expect(canvas.queryByText(/2xx/)).toBeNull();

    await userEvent.click(radio("Errors"));
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toEqual(["internal-llama"]));
    await expect(lastLogQuery(byStatus).get("status")).toBe("error");
    await expect(addressOf(canvasElement).get("status")).toBe("error");

    // picking the other class replaces the first rather than cancelling both
    await userEvent.click(radio("OK"));
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toEqual(["gpt-4o"]));
    await expect(radio("OK")).toHaveAttribute("aria-checked", "true");
    await expect(radio("Errors")).toHaveAttribute("aria-checked", "false");
    await expect(lastLogQuery(byStatus).get("status")).toBe("success");
    await expect(addressOf(canvasElement).get("status")).toBe("success");
    await expect(canvas.getByRole("button", { name: "Filters · 1" })).toBeVisible();

    // "All" takes the parameter out of the address rather than writing a default
    await userEvent.click(radio("All"));
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toHaveLength(2));
    await expect(addressOf(canvasElement).has("status")).toBe(false);
  },
};

const byModel = recording(serverFiltered(MIXED));

/**
 * #1985: the model list was checkboxes that kept only the last tick, so a
 * second model silently unticked the first. The control plane filters on one
 * exact model, so the rail picks one, a second pick visibly replaces it, and
 * the pick holds through the feed's refreshes.
 */
export const AModelPickSticks: Story = {
  render: () => (
    <Harness fetchStub={byModel.stub}>
      <Logs pollMs={FAST_POLL_MS} />
      <AddressProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("internal-llama");
    await userEvent.click(canvas.getByRole("button", { name: /Filters/ }));
    const picker = await canvas.findByRole("combobox", { name: "Model" });
    await expect(picker).toHaveAttribute("placeholder", "All models");

    await userEvent.click(picker);
    await userEvent.click(await canvas.findByRole("option", { name: "internal-llama" }));
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toEqual(["internal-llama"]));
    await expect(picker).toHaveValue("internal-llama");
    await expect(addressOf(canvasElement).get("model")).toBe("internal-llama");

    await userEvent.click(picker);
    await userEvent.click(await canvas.findByRole("option", { name: "gpt-4o" }));
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toEqual(["gpt-4o"]));
    await expect(picker).toHaveValue("gpt-4o");
    await expect(addressOf(canvasElement).get("model")).toBe("gpt-4o");

    // two more reads of the feed, and the pick is still the one on screen
    const reads = logReads(byModel);
    await waitFor(() => expect(logReads(byModel)).toBeGreaterThan(reads + 1));
    await expect(lastLogQuery(byModel).get("model")).toBe("gpt-4o");
    await expect(picker).toHaveValue("gpt-4o");
    await expect(modelsOnScreen(canvasElement)).toEqual(["gpt-4o"]);
  },
};

const clearing = recording(serverFiltered(MIXED));

/**
 * #1985: the only way to drop every filter was the no-match empty state, under
 * a button that said "Clear search". The rail carries its own control, which
 * waits disabled until there is a filter to clear.
 */
export const ClearFiltersResetsTheRail: Story = {
  render: () => (
    <Harness fetchStub={clearing.stub}>
      <Logs />
      <AddressProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("internal-llama");
    await userEvent.click(canvas.getByRole("button", { name: /Filters/ }));
    const clear = await canvas.findByRole("button", { name: "Clear filters" });
    await expect(clear).toBeDisabled();

    const group = within(canvas.getByRole("radiogroup", { name: "Status" }));
    await userEvent.click(group.getByRole("radio", { name: "Errors" }));
    const picker = canvas.getByRole("combobox", { name: "Model" });
    await userEvent.click(picker);
    await userEvent.click(await canvas.findByRole("option", { name: "internal-llama" }));
    await waitFor(() => expect(canvas.getByRole("button", { name: "Filters · 2" })).toBeVisible());
    await expect(clear).toBeEnabled();

    await userEvent.click(clear);
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toHaveLength(2));
    await expect(group.getByRole("radio", { name: "All" })).toHaveAttribute("aria-checked", "true");
    await expect(picker).toHaveValue("");
    await expect(clear).toBeDisabled();
    await expect(canvas.getByRole("button", { name: "Filters" })).toBeVisible();
    const address = addressOf(canvasElement);
    await expect(address.has("status")).toBe(false);
    await expect(address.has("model")).toBe(false);
    const sent = lastLogQuery(clearing);
    await expect(sent.get("status")).toBe("all");
    await expect(sent.has("model")).toBe(false);
  },
};

const fromAddress = recording(serverFiltered(MIXED));

/**
 * #1985: a filtered view could not be shared, because the filters lived in the
 * component and a reload dropped them. Opened from an address that names
 * them, the screen reads them before its first request, so there is never an
 * unfiltered page first, and the rail shows what is applied.
 */
export const FiltersComeBackFromTheAddress: Story = {
  parameters: { address: "/logs?status=error&model=internal-llama" },
  render: () => (
    <Harness fetchStub={fromAddress.stub}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toEqual(["internal-llama"]));
    const reads = fromAddress.calls
      .filter((c) => c.url.includes("/analytics/invocations"))
      .map((c) => new URL(c.url, "http://localhost").searchParams);
    await expect(reads.length).toBeGreaterThan(0);
    for (const sent of reads) {
      await expect(sent.get("status")).toBe("error");
      await expect(sent.get("model")).toBe("internal-llama");
    }

    await userEvent.click(canvas.getByRole("button", { name: "Filters · 2" }));
    const group = within(await canvas.findByRole("radiogroup", { name: "Status" }));
    await expect(group.getByRole("radio", { name: "Errors" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    await waitFor(() =>
      expect(canvas.getByRole("combobox", { name: "Model" })).toHaveValue("internal-llama"),
    );
    await expect(canvas.getByRole("button", { name: "Clear filters" })).toBeEnabled();
  },
};

const staleAddress = recording(serverFiltered(MIXED));

/**
 * An address outlives what it names. A status the control plane does not know
 * would be a 400, so it reads as no status filter; a model since removed from
 * the catalogue still filters the log, so the picker still shows it rather
 * than reading as unset over a narrowed table. The empty state's way out now
 * says what it does.
 */
export const AStaleAddressStillReadsTrue: Story = {
  parameters: { address: "/logs?status=2xx&model=gpt-3.5-legacy" },
  render: () => (
    <Harness fetchStub={staleAddress.stub}>
      <Logs />
      <AddressProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectEmptyState(canvasElement, /No requests match these filters/, /Clear filters/);
    const sent = lastLogQuery(staleAddress);
    await expect(sent.get("status")).toBe("all");
    await expect(sent.get("model")).toBe("gpt-3.5-legacy");

    await userEvent.click(canvas.getByRole("button", { name: "Filters · 1" }));
    const group = within(await canvas.findByRole("radiogroup", { name: "Status" }));
    await expect(group.getByRole("radio", { name: "All" })).toHaveAttribute("aria-checked", "true");
    await waitFor(() =>
      expect(canvas.getByRole("combobox", { name: "Model" })).toHaveValue("gpt-3.5-legacy"),
    );
    await userEvent.click(canvas.getByRole("button", { name: "Hide filters" }));

    await userEvent.click(canvas.getByRole("button", { name: "Clear filters" }));
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toHaveLength(2));
    await expect(addressOf(canvasElement).toString()).toBe("");
  },
};

const onlyUnpriced = recording(serverFiltered(ROWS));

/**
 * #1986: the log could not be narrowed to the requests with no price, which is
 * the list a FinOps reader wants once the spend total says it is incomplete.
 * The rail's Cost section carries an "Unpriced only" check, the server is asked
 * for it before the page is cut like every other filter here, the address holds
 * it, and Clear filters takes it back out.
 */
export const TheUnpricedFilterIsSentToTheServer: Story = {
  render: () => (
    <Harness fetchStub={onlyUnpriced.stub}>
      <Logs />
      <AddressProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("internal-llama");
    await expect(modelsOnScreen(canvasElement)).toEqual(["gpt-4o", "internal-llama"]);
    await userEvent.click(canvas.getByRole("button", { name: /Filters/ }));
    const only = await canvas.findByRole("checkbox", { name: "Unpriced only" });
    await expect(only).toHaveAttribute("aria-checked", "false");

    await userEvent.click(only);
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toEqual(["internal-llama"]));
    await expect(only).toHaveAttribute("aria-checked", "true");
    await expect(lastLogQuery(onlyUnpriced).get("unpriced")).toBe("true");
    await expect(addressOf(canvasElement).get("unpriced")).toBe("true");
    await expect(canvas.getByRole("button", { name: "Filters · 1" })).toBeVisible();

    // unticked, or cleared with the rest of the rail, it leaves the address and the request
    await userEvent.click(only);
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toHaveLength(2));
    await expect(addressOf(canvasElement).has("unpriced")).toBe(false);
    await expect(lastLogQuery(onlyUnpriced).has("unpriced")).toBe(false);

    await userEvent.click(only);
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toEqual(["internal-llama"]));
    await userEvent.click(canvas.getByRole("button", { name: "Clear filters" }));
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toHaveLength(2));
    await expect(addressOf(canvasElement).toString()).toBe("");
    await expect(canvas.getByRole("checkbox", { name: "Unpriced only" })).toHaveAttribute(
      "aria-checked",
      "false",
    );
  },
};

const unpricedLink = recording(serverFiltered(ROWS));

/** #1986: a link to the unpriced view opens it, asked for before the first page, with the rail showing it. */
export const AnUnpricedLinkComesBackFiltered: Story = {
  parameters: { address: "/logs?unpriced=true" },
  render: () => (
    <Harness fetchStub={unpricedLink.stub}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toEqual(["internal-llama"]));
    const reads = unpricedLink.calls
      .filter((c) => c.url.includes("/analytics/invocations"))
      .map((c) => new URL(c.url, "http://localhost").searchParams);
    for (const sent of reads) await expect(sent.get("unpriced")).toBe("true");

    await userEvent.click(canvas.getByRole("button", { name: "Filters · 1" }));
    await expect(await canvas.findByRole("checkbox", { name: "Unpriced only" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
  },
};

const staleUnpriced = recording(serverFiltered(ROWS));

/** An address that says `unpriced=yes` is not the filter: only `true` is, as with an unknown status. */
export const AnUnpricedValueItDoesNotKnowReadsAsOff: Story = {
  parameters: { address: "/logs?unpriced=yes" },
  render: () => (
    <Harness fetchStub={staleUnpriced.stub}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toHaveLength(2));
    await expect(lastLogQuery(staleUnpriced).has("unpriced")).toBe(false);
    await expect(canvas.getByRole("button", { name: "Filters" })).toBeVisible();
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
    // no key in the project's list carries this id, so the id itself stands in
    await expect(drawer.getByText("vk-1")).toBeVisible();
  },
};

// every fixture above answered 200, and a failed row is the one the drawer is
// opened for most (#1983). the upstream reset the connection mid-stream, so
// nothing was billed and no token arrived
const FAILED = row({
  request_id: "req-failed-7f3a",
  trace_id: "4bf92f3577b34da6a3ce929d0e0e4736",
  status: 502,
  upstream_status: 0,
  stream: 1,
  prompt_tokens: 0,
  completion_tokens: 0,
  total_tokens: 0,
  cost_usd: 0,
  latency_ms: 30012,
  ttft_ms: 0,
  error: "upstream openai reset the connection before the response completed",
});
const SERVED = row({
  request_id: "req-served",
  model: "claude-sonnet",
  provider: "anthropic",
  target: "anthropic/claude-sonnet",
});

/** The drawer's group titles, top to bottom: the order it is read in. */
const groupTitles = (panel: HTMLElement) =>
  within(panel)
    .getAllByRole("heading", { level: 3 })
    .map((h) => h.textContent);

/** The value a drawer row pairs with `label`, read off the `dt` it follows. */
const valueOf = (region: HTMLElement, label: string) =>
  within(region).getByText(label, { selector: "dt" }).nextElementSibling?.textContent;

/**
 * #1983: a failed request opens on its verdict — the status, when it ran and
 * the ids to quote, each copyable — and the error is the first thing after it,
 * ahead of routing, usage and attribution. The row the drawer belongs to says
 * it is the open one.
 */
export const AFailedRequestLeadsWithItsError: Story = {
  render: () => (
    <Harness fetchStub={withLogs([FAILED, SERVED])}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open request details for gpt-4o/i }),
    );
    const panel = await canvas.findByRole("complementary", { name: "Details" });
    const drawer = within(panel);

    // the verdict line
    await expect(drawer.getByText("502")).toBeVisible();
    await expect(drawer.getByText(fmt.dateTimeMs(FAILED.ts))).toBeVisible();
    await expect(
      drawer.getByRole("button", { name: `Copy request ID: ${FAILED.request_id}` }),
    ).toBeVisible();
    // the hop into the tracing backend (engineer journey E5.3)
    await expect(
      drawer.getByRole("button", { name: `Copy trace ID: ${FAILED.trace_id}` }),
    ).toBeVisible();

    // the error, before everything the drawer groups under it
    await expect(groupTitles(panel)).toEqual([
      "Error",
      "Routing",
      "Usage and cost",
      "Attribution",
      // neither body was stored, so the two share one note and one heading
      "Request and response",
    ]);
    const error = drawer.getByRole("region", { name: /^Error — / });
    await expect(error).toHaveTextContent(FAILED.error);

    // and the row it describes is the one marked open
    const [failedRow, servedRow] = canvasElement.querySelectorAll("tbody tr");
    await expect(failedRow).toHaveAttribute("aria-selected", "true");
    await expect(servedRow).toHaveAttribute("aria-selected", "false");
  },
};

const CI_KEY: VirtualKeyRow = {
  id: "vk-ci",
  project_id: "project-1",
  key_hash: "",
  key_prefix: "rk_live_ab12",
  name: "ci-runner",
  models: [],
  providers: [],
  disabled: false,
  created_by: null,
  business_unit_id: "unit-1",
  customer_id: "cust-1",
  created_at: "2026-03-05T10:00:00Z",
};

// a streamed canary call rolter answered from its own response cache, billed
// through a named key. the caller sent no traceparent
const ROUTED = row({
  request_id: "req-routed",
  trace_id: "",
  virtual_key_id: "vk-ci",
  business_unit_id: "unit-1",
  customer_id: "cust-1",
  variant: "canary",
  cache_hit: 1,
  // answered from rolter's own cache, so no upstream was reached
  upstream_status: 0,
  attempts: 0,
  stream: 1,
  ttft_ms: 120,
  cache_read_tokens: 2048,
  cache_write_tokens: 512,
});

/**
 * #1983: the fields that explain cache-aware routing were missing from the
 * drawer, and the rest sat in one flat grid. Each group now holds its own —
 * routing, usage and cost, attribution — with numbers in the house format and
 * the key by its name and prefix rather than its id.
 */
export const TheDrawerGroupsRoutingUsageAndAttribution: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/api/v1/analytics/invocations", () => ({ data: [ROUTED] })],
        ["/api/v1/currency", () => ({ base: "USD", codes: ["USD"], rates: {} })],
        ["/virtual-keys", () => [CI_KEY]],
        ["/business-units", () => [UNIT]],
        ["/customers", () => [CUSTOMER]],
      ])}
    >
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open request details for gpt-4o/i }),
    );
    const panel = await canvas.findByRole("complementary", { name: "Details" });
    const drawer = within(panel);

    await expect(drawer.getByText("200")).toBeVisible();
    // a request that failed nothing has no error group
    await expect(drawer.queryByRole("heading", { name: "Error" })).toBeNull();
    // and a caller that sent no trace is told so, with nothing to copy
    await expect(valueOf(panel, "Trace ID")).toBe("Not sent by the caller");
    await expect(drawer.queryByRole("button", { name: /Copy trace ID/ })).toBeNull();

    const routing = drawer.getByRole("region", { name: "Routing" });
    await expect(valueOf(routing, "Provider → target")).toBe("openai → openai/gpt-4o");
    await expect(valueOf(routing, "Variant")).toBe("canary");
    await expect(valueOf(routing, "Response cache")).toBe("Hit");
    await expect(valueOf(routing, "Stream")).toBe("Streamed");
    await expect(valueOf(routing, "Time to first token")).toBe(`${fmt.number(120)} ms`);
    await expect(valueOf(routing, "Latency")).toBe(`${fmt.number(842)} ms`);

    const usage = drawer.getByRole("region", { name: "Usage and cost" });
    await expect(valueOf(usage, "Tokens")).toBe(`${fmt.number(8000)} in · ${fmt.number(4345)} out`);
    await expect(valueOf(usage, "Prompt cache")).toBe(
      `${fmt.number(2048)} read · ${fmt.number(512)} written`,
    );
    // a row with no 1 hour share, as an older control plane sends it, has no split to state
    await expect(within(usage).queryByText("Cache write by lifetime")).toBeNull();
    await expect(valueOf(usage, "Cost")).toBe(fmt.currency(0.0123, "USD"));

    const attribution = drawer.getByRole("region", { name: "Attribution" });
    // the key list answers after the drawer opens
    await waitFor(() => expect(within(attribution).getByText("ci-runner")).toBeVisible());
    await expect(within(attribution).getByText("rk_live_ab12…")).toBeVisible();
    await expect(within(attribution).queryByText("vk-ci")).toBeNull();
    await expect(valueOf(attribution, "Business unit")).toBe("Platform Engineering");
    await expect(valueOf(attribution, "Customer")).toBe("Acme Corp");
  },
};

/**
 * #2903: a request that wrote to Anthropic's 1 hour cache says how its cache
 * write divided, beside the read and written totals it divides. The 5 minute
 * part is what is left of the write, so the two parts add up to the total.
 */
export const TheDrawerSplitsACacheWriteBetweenItsTwoCaches: Story = {
  render: () => (
    <Harness
      fetchStub={withLogs([
        row({ cache_read_tokens: 2048, cache_write_tokens: 1512, cache_write_1h_tokens: 1000 }),
      ])}
    >
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open request details for gpt-4o/i }),
    );
    const panel = await canvas.findByRole("complementary", { name: "Details" });
    const usage = within(panel).getByRole("region", { name: "Usage and cost" });

    await expect(valueOf(usage, "Prompt cache")).toBe(
      `${fmt.number(2048)} read · ${fmt.number(1512)} written`,
    );
    await expect(valueOf(usage, "Cache write by lifetime")).toBe(
      `${fmt.number(512)} to the 5 minute cache · ${fmt.number(1000)} to the 1 hour cache`,
    );
  },
};

/**
 * #2903: a 1 hour share of 0 is what a provider with no split, or a row older
 * than the column, reads, so it states nothing. A share larger than the write
 * (a row the two columns disagree on) never prints a negative 5 minute part.
 */
export const TheDrawerStatesNoSplitForAZeroShare: Story = {
  render: () => (
    <Harness
      fetchStub={withLogs([
        row({
          request_id: "req-no-split",
          cache_read_tokens: 100,
          cache_write_tokens: 300,
          cache_write_1h_tokens: 0,
        }),
        row({
          request_id: "req-odd",
          cache_write_tokens: 300,
          cache_write_1h_tokens: 400,
        }),
      ])}
    >
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const [none, odd] = await canvas.findAllByRole("button", {
      name: /Open request details for gpt-4o/i,
    });

    await userEvent.click(none);
    let panel = await canvas.findByRole("complementary", { name: "Details" });
    let usage = within(panel).getByRole("region", { name: "Usage and cost" });
    await expect(valueOf(usage, "Prompt cache")).toBe(
      `${fmt.number(100)} read · ${fmt.number(300)} written`,
    );
    await expect(within(usage).queryByText("Cache write by lifetime")).toBeNull();

    await userEvent.click(odd);
    await waitFor(() => {
      panel = canvas.getByRole("complementary", { name: "Details" });
      usage = within(panel).getByRole("region", { name: "Usage and cost" });
      expect(within(usage).getByText("Cache write by lifetime")).toBeVisible();
    });
    await expect(valueOf(usage, "Cache write by lifetime")).toBe(
      `${fmt.number(0)} to the 5 minute cache · ${fmt.number(400)} to the 1 hour cache`,
    );
  },
};

/**
 * #1983: the drawer sits beside the table with nothing tying it to its row, so
 * the open row carries `aria-selected` and a surface of its own. The mark moves
 * with the selection and goes when the drawer closes.
 */
export const TheOpenRowIsMarkedSelected: Story = {
  render: () => (
    <Harness fetchStub={withLogs(ROWS)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText(fmt.timeMs(ROWS[0].ts));
    const [first, second] = Array.from(canvasElement.querySelectorAll("tbody tr"));
    const surface = (el: Element) => getComputedStyle(el).backgroundColor;
    await expect(first).toHaveAttribute("aria-selected", "false");
    await expect(second).toHaveAttribute("aria-selected", "false");
    const resting = surface(first);

    await userEvent.click(
      canvas.getByRole("button", { name: /Open request details for internal-llama/i }),
    );
    await canvas.findByRole("complementary", { name: "Details" });
    await expect(second).toHaveAttribute("aria-selected", "true");
    await expect(first).toHaveAttribute("aria-selected", "false");
    // the row eases between surfaces, so the colour is read once it has settled
    await waitFor(() => expect(surface(second)).not.toBe(resting));
    await expect(surface(first)).toBe(resting);

    // opening another row moves the mark rather than adding a second one
    await userEvent.click(canvas.getByRole("button", { name: /Open request details for gpt-4o/i }));
    await waitFor(() => expect(first).toHaveAttribute("aria-selected", "true"));
    await expect(second).toHaveAttribute("aria-selected", "false");

    await userEvent.click(canvas.getByRole("button", { name: "Close details" }));
    await waitFor(() => expect(first).toHaveAttribute("aria-selected", "false"));
    await waitFor(() => expect(surface(first)).toBe(resting));
  },
};

// the gateway never got an answer: the upstream timed out, so there is no
// http status to show, only the error it recorded
const TIMED_OUT = row({
  request_id: "req-timeout-19c2",
  trace_id: "0af7651916cd43dd8448eb211c80319c",
  status: 0,
  latency_ms: 30000,
  ttft_ms: 0,
  prompt_tokens: 0,
  completion_tokens: 0,
  total_tokens: 0,
  cost_usd: 0,
  error: "upstream request timed out after 30s",
});

/**
 * The same drawer below `lg`, where it opens as a sheet over the table: it
 * still leads with the verdict and the error, and nothing in it pushes the page
 * sideways. A status of 0 reads as "No response" rather than a bare zero.
 */
export const AFailedRequestInTheSheet: Story = {
  ...atTablet,
  render: () => (
    <Harness fetchStub={withLogs([TIMED_OUT])}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open request details for gpt-4o/i }),
    );
    const dialog = await within(document.body).findByRole("dialog");
    const sheet = within(dialog);
    await expect(sheet.getByText("No response")).toBeVisible();
    await expect(
      sheet.getByRole("button", { name: `Copy request ID: ${TIMED_OUT.request_id}` }),
    ).toBeVisible();
    await expect(
      sheet.getByRole("button", { name: `Copy trace ID: ${TIMED_OUT.trace_id}` }),
    ).toBeVisible();
    await expect(groupTitles(dialog)[0]).toBe("Error");
    await expect(sheet.getByRole("region", { name: /^Error — / })).toHaveTextContent(
      TIMED_OUT.error,
    );
    await expectNoHorizontalOverflow();
  },
};

// what the caller was told and what the upstream said are two columns (#2837).
// the provider answered 429, the next target 500, and the gateway ran out of
// targets: the caller got a 503 that names neither
const EXHAUSTED = row({
  request_id: "req-exhausted-5c1e",
  model: "gemini-2.5-flash",
  provider: "gemini",
  target: "gemini/gemini-2.5-flash",
  status: 503,
  upstream_status: 500,
  attempts: 3,
  latency_ms: 2210,
  ttft_ms: 0,
  prompt_tokens: 0,
  completion_tokens: 0,
  total_tokens: 0,
  cost_usd: 0,
  error: "upstream returned 500 after 3 attempts: internal error encountered",
});
// a rate limit that outlasted every target reaches the caller as the 429 it was,
// so the two columns agree and only the attempts say it was tried twice
const RATE_LIMITED = row({
  request_id: "req-rate-limited-2b90",
  model: "gpt-4o-mini",
  status: 429,
  upstream_status: 429,
  attempts: 2,
  latency_ms: 1180,
  ttft_ms: 0,
  prompt_tokens: 0,
  completion_tokens: 0,
  total_tokens: 0,
  cost_usd: 0,
  error: "upstream returned 429 after 2 attempts: gpt-4o-mini is temporarily rate-limited upstream",
});
// the upstream's own error, handed to the caller as it came
const PASSED_THROUGH = row({
  request_id: "req-passed-through-a07d",
  model: "claude-sonnet",
  provider: "anthropic",
  target: "anthropic/claude-sonnet",
  status: 400,
  upstream_status: 400,
  attempts: 1,
  latency_ms: 310,
  ttft_ms: 0,
  prompt_tokens: 0,
  completion_tokens: 0,
  total_tokens: 0,
  cost_usd: 0,
  error: "upstream returned 400 after 1 attempt: max_tokens: 99999 > 64000, the maximum",
});
// refused at the gateway's door: no provider or target was chosen and no
// upstream was called
const REFUSED = row({
  request_id: "req-refused-e3f4",
  model: "gpt-4.1",
  provider: "",
  target: "",
  status: 403,
  upstream_status: 0,
  attempts: 0,
  latency_ms: 4,
  ttft_ms: 0,
  prompt_tokens: 0,
  completion_tokens: 0,
  total_tokens: 0,
  cost_usd: 0,
  error: "this key is not allowed to use model gpt-4.1",
});
// failed over and then answered: nothing is wrong, but it took two tries
const RETRIED = row({
  request_id: "req-retried-9d12",
  model: "gemini-2.5-pro",
  provider: "gemini",
  target: "gemini/gemini-2.5-pro",
  attempts: 2,
});
// written before the columns existed, or by a ClickHouse without them: the
// fields are absent, not zero
const OLD = row({
  request_id: "req-old-41aa",
  model: "legacy-model",
  provider: "vllm",
  target: "vllm/legacy-model",
  status: 502,
  error: "upstream reset the connection",
});
delete OLD.upstream_status;
delete OLD.attempts;
// answered from rolter's response cache: no attempts, and nothing wrong
const CACHED = row({
  request_id: "req-cached-77b3",
  model: "cached-model",
  cache_hit: 1,
  upstream_status: 0,
  attempts: 0,
});
const UPSTREAM_ROWS = [EXHAUSTED, RATE_LIMITED, PASSED_THROUGH, REFUSED, RETRIED, OLD, CACHED];

/** The status cell of table row `index`, whatever columns the width is drawing. */
const statusCellOf = (canvasElement: HTMLElement, index: number) =>
  canvasElement.querySelectorAll("tbody tr")[index].querySelectorAll("td")[3];

/** Opens `model`'s drawer and returns its panel. */
async function openDetails(canvasElement: HTMLElement, model: string) {
  const canvas = within(canvasElement);
  await userEvent.click(
    await canvas.findByRole("button", { name: new RegExp(`Open request details for ${model}`) }),
  );
  return canvas.findByRole("complementary", { name: "Details" });
}

/**
 * #2837: a 503 the gateway made after the provider's 500 says so beside its
 * status, in the table, where the row is scanned. The hint is the upstream's
 * number with the sentence as its title and its text for a screen reader. It is
 * drawn only where the two statuses differ: a pass-through error, a rate limit
 * the caller got as it was, a refusal, an old row and a cache hit have nothing
 * to add, and a failover that succeeded is no different from any 200.
 */
export const TheStatusCellHintsAtTheUpstreamStatusWhenItDiffers: Story = {
  render: () => (
    <Harness fetchStub={withLogs(UPSTREAM_ROWS)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText(fmt.timeMs(EXHAUSTED.ts));

    const hinted = within(statusCellOf(canvasElement, 0));
    await expect(hinted.getByText("503")).toBeVisible();
    await expect(hinted.getByText("500")).toBeVisible();
    await expect(hinted.getByTitle("Upstream returned 500 after 3 attempts")).toBeVisible();
    // a screen reader hears the sentence, not a bare second number
    await expect(statusCellOf(canvasElement, 0)).toHaveTextContent(
      "Upstream returned 500 after 3 attempts",
    );

    for (const index of [1, 2, 3, 4, 5, 6]) {
      await expect(statusCellOf(canvasElement, index).querySelector("[title]")).toBeNull();
      await expect(statusCellOf(canvasElement, index)).not.toHaveTextContent(/Upstream/);
    }
    // the hint takes its room from the column, not from the row's other cells
    const shape = tableShape(canvasElement);
    await expect(shape.scrolls).toBe(false);
    await expectRowInFrame(shape, 0);
  },
};

/**
 * #2837: a failed-over request's drawer says what the upstream answered and how
 * many tries it took, under the verdict and ahead of the ids, so the status the
 * gateway made up is explained where it is read. A rate limit that ran out of
 * targets and a request that was retried and then succeeded say the same thing
 * in the same words.
 */
export const AFailedOverRequestSaysWhatTheUpstreamAnswered: Story = {
  render: () => (
    <Harness fetchStub={withLogs(UPSTREAM_ROWS)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const panel = await openDetails(canvasElement, "gemini-2.5-flash");
    const drawer = within(panel);

    await waitFor(() =>
      expect(drawer.getByText("Upstream returned 500 after 3 attempts")).toBeVisible(),
    );
    await expect(drawer.getByText("503")).toBeVisible();
    // the sentence leads the ids, and the upstream's reason is still its own group
    const sentence = drawer.getByText("Upstream returned 500 after 3 attempts");
    const requestId = drawer.getByText("Request ID", { selector: "dt" });
    await expect(sentence.compareDocumentPosition(requestId)).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
    await expect(drawer.getByRole("region", { name: /^Error — / })).toHaveTextContent(
      EXHAUSTED.error,
    );

    // the rate limit the caller got as a 429: the same two numbers, two tries
    await userEvent.click(
      within(canvasElement).getByRole("button", { name: /Open request details for gpt-4o-mini/ }),
    );
    await waitFor(() =>
      expect(drawer.getByText("Upstream returned 429 after 2 attempts")).toBeVisible(),
    );
    await expect(drawer.queryByText(/after 3 attempts/)).toBeNull();

    // a request that failed over and then answered is worth knowing about
    await userEvent.click(
      within(canvasElement).getByRole("button", {
        name: /Open request details for gemini-2.5-pro/,
      }),
    );
    await waitFor(() =>
      expect(drawer.getByText("Upstream returned 200 after 2 attempts")).toBeVisible(),
    );
    await expect(drawer.queryByRole("heading", { name: "Error" })).toBeNull();
  },
};

/**
 * #2837: an error the upstream made and the gateway handed over as it came is
 * one attempt with the same status on both sides. It has no hint in the table,
 * and the drawer says the status is the upstream's own, in the singular.
 */
export const APassThroughUpstreamErrorIsOneAttempt: Story = {
  render: () => (
    <Harness fetchStub={withLogs([PASSED_THROUGH, row({})])}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByText(fmt.timeMs(PASSED_THROUGH.ts));
    await expect(statusCellOf(canvasElement, 0)).toHaveTextContent(/^400$/);
    await expect(statusCellOf(canvasElement, 0).querySelector("[title]")).toBeNull();

    const drawer = within(await openDetails(canvasElement, "claude-sonnet"));
    await waitFor(() =>
      expect(drawer.getByText("Upstream returned 400 after 1 attempt")).toBeVisible(),
    );
    await expect(drawer.getByRole("region", { name: /^Error — / })).toHaveTextContent(
      PASSED_THROUGH.error,
    );
  },
};

/**
 * #2837: a request refused at the gateway's door has no provider, no target and
 * no attempts, and its error is the message the caller got. The drawer says it
 * never reached an upstream rather than leaving the empty routing to be
 * guessed at, and the table has no upstream status to hint at.
 */
export const ARefusalSaysItNeverReachedAnUpstream: Story = {
  render: () => (
    <Harness fetchStub={withLogs([REFUSED, row({})])}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByText(fmt.timeMs(REFUSED.ts));
    const cells = canvasElement.querySelectorAll("tbody tr")[0].querySelectorAll("td");
    await expect(cells[2]).toHaveTextContent("—");
    await expect(statusCellOf(canvasElement, 0)).toHaveTextContent(/^403$/);

    const panel = await openDetails(canvasElement, "gpt-4.1");
    const drawer = within(panel);
    await waitFor(() =>
      expect(drawer.getByText("Refused before reaching an upstream")).toBeVisible(),
    );
    await expect(drawer.getByRole("region", { name: /^Error — / })).toHaveTextContent(
      REFUSED.error,
    );
    await expect(
      valueOf(drawer.getByRole("region", { name: "Routing" }), "Provider → target"),
    ).toBe("—");
    await expect(drawer.queryByText(/Upstream returned/)).toBeNull();
  },
};

/**
 * #2837: a row written before the columns existed, or read from a ClickHouse
 * without them, carries neither field. A missing field is zero attempts and no
 * upstream status, and a failed row that names its provider must not be called
 * a refusal on that evidence: the screen says nothing extra, as does a cache
 * hit, which also made no attempts.
 */
export const ARowWithoutTheUpstreamFieldsSaysNothingExtra: Story = {
  render: () => (
    <Harness fetchStub={withLogs([OLD, CACHED])}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expect("attempts" in OLD || "upstream_status" in OLD).toBe(false);
    await within(canvasElement).findByText(fmt.timeMs(OLD.ts));
    for (const index of [0, 1]) {
      await expect(statusCellOf(canvasElement, index).querySelector("[title]")).toBeNull();
    }

    const old = within(await openDetails(canvasElement, "legacy-model"));
    await waitFor(() => expect(old.getByRole("region", { name: /^Error — / })).toBeVisible());
    await expect(old.queryByText(/Upstream|Refused before/)).toBeNull();

    await userEvent.click(
      within(canvasElement).getByRole("button", { name: /Open request details for cached-model/ }),
    );
    await waitFor(() => expect(old.getByText("Hit")).toBeVisible());
    await expect(old.queryByText(/Upstream|Refused before/)).toBeNull();
  },
};

// the counts the Russian plural rules tell apart: 1 and 21 are "one", 2 to 4
// "few", 5 to 20 "many"
const COUNTED = [1, 2, 5, 21].map((attempts) =>
  row({
    request_id: `req-counted-${attempts}`,
    model: `counted-${attempts}`,
    status: 503,
    upstream_status: 500,
    attempts,
  }),
);

/**
 * #2837 in Russian: the sentence follows the language's plural rules for the
 * number of attempts, and the status it names is not grouped like a count.
 */
export const TheUpstreamSentenceIsPluralisedInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={withLogs(COUNTED)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const ruSentence = (key: "answered_one" | "answered_few" | "answered_many", attempts: number) =>
      ru.pages.logs.detail.upstream[key]
        .replace("{{status}}", "500")
        .replace("{{count}}", String(attempts));
    const canvas = within(canvasElement);
    const expected: [number, "answered_one" | "answered_few" | "answered_many"][] = [
      [1, "answered_one"],
      [2, "answered_few"],
      [5, "answered_many"],
      [21, "answered_one"],
    ];
    for (const [attempts, key] of expected) {
      await userEvent.click(
        await canvas.findByRole("button", {
          name: ru.analytics.openDetails.replace("{{model}}", `counted-${attempts}`),
        }),
      );
      const drawer = within(
        await canvas.findByRole("complementary", { name: ru.analytics.details }),
      );
      await waitFor(() => expect(drawer.getByText(ruSentence(key, attempts))).toBeVisible());
    }
  },
};

/**
 * #2837 on a phone, in the longer Russian copy: the upstream's number joins the
 * status on the row's first line, the model is cut to make room for it, and
 * nothing leaves the frame or scrolls the page sideways.
 */
export const TheUpstreamHintFitsAtMobileInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => (
    <Harness fetchStub={withLogs([EXHAUSTED, ...ROWS])}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByText(formattersFor("ru").timeMs(EXHAUSTED.ts));
    await expectNoHorizontalOverflow();

    const shape = tableShape(canvasElement);
    await expect(shape.scrolls).toBe(false);
    await expectRowInFrame(shape, 0);
    for (const cell of shape.cells(0)) await expectInViewport(cell);
    const sentence = ru.pages.logs.detail.upstream.answered_few
      .replace("{{status}}", "500")
      .replace("{{count}}", "3");
    await expect(within(shape.cells(0)[2]).getByTitle(sentence)).toBeVisible();
    await expectStackedRow(canvasElement, 0, EXHAUSTED.model);
  },
};

/**
 * #2837 where the table is just wide enough to draw its columns and no wider:
 * the status column has room for the badge alone, so the upstream's number
 * wraps under it instead of pushing the cost out of frame.
 */
export const TheUpstreamHintWrapsUnderTheBadgeInANarrowTable: Story = {
  ...atNarrow,
  render: () => (
    <Harness fetchStub={withLogs([EXHAUSTED, ...ROWS])}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByText(fmt.timeMs(EXHAUSTED.ts));
    await expectNoHorizontalOverflow();
    const shape = tableShape(canvasElement);
    await expect(shape.scrolls).toBe(false);
    await expectRowInFrame(shape, 0);

    const cell = within(shape.cells(0)[2]);
    const badge = cell.getByText("503");
    const hint = cell.getByText("500");
    await expect(hint).toBeVisible();
    await expect(hint.getBoundingClientRect().top).toBeGreaterThanOrEqual(
      badge.getBoundingClientRect().bottom - 1,
    );
  },
};

/**
 * #1986: the drawer had the same dash-and-tooltip for a request with no price.
 * It has room to say it, so the cost row names the state and the sentence that
 * explains it is text on the screen, not a title attribute.
 */
export const AnUnpricedRequestSaysSoInTheDrawer: Story = {
  render: () => (
    <Harness fetchStub={withLogs(ROWS)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open request details for internal-llama/i }),
    );
    const panel = await canvas.findByRole("complementary", { name: "Details" });
    const usage = within(panel).getByRole("region", { name: "Usage and cost" });
    const cost = within(usage).getByText("Cost", { selector: "dt" }).nextElementSibling;
    await expect(within(cost as HTMLElement).getByText("unpriced")).toBeVisible();
    // the reason is a sentence in the row, read by anyone who reads the drawer
    const reason = within(cost as HTMLElement).getByText(/no price is configured/i);
    await expect(reason).toBeVisible();
    await expect(reason.getAttribute("title")).toBeNull();
    await expect(cost).not.toHaveTextContent(fmt.currency(0, "USD"));
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
    // the two bodies are missing for the same reason, so it is said once, under
    // a heading that names both (#2131)
    await expect(drawer.getAllByText(/retention window has already passed/i)).toHaveLength(1);
    await expect(drawer.getByRole("heading", { name: "Request and response" })).toBeVisible();
    await expect(drawer.queryByRole("heading", { name: "Request" })).toBeNull();
    await expect(drawer.queryByRole("heading", { name: "Response" })).toBeNull();
    // and it is not the old claim, which asserted a reason it could not know
    await expect(drawer.queryByText("payload logging is off")).not.toBeInTheDocument();
    const links = drawer.getAllByRole("link", { name: "Open log settings" });
    await expect(links).toHaveLength(1);
    await expect(links[0]).toHaveAttribute("href", "/logs-settings");
  },
};

/**
 * #1984: the log settings are a superadmin-only screen, and the link to them
 * rendered for everyone, so a member who followed it landed on a refusal. The
 * link now follows `logging_settings:read`; a caller the gate refuses reads who
 * owns the setting instead, and the explanation above it is unchanged.
 */
export const AMemberIsNotSentToLogSettings: Story = {
  render: () => (
    <Harness fetchStub={withLogs(ROWS)} role="member">
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open request details for gpt-4o/i }),
    );
    const drawer = within(await canvas.findByRole("complementary", { name: "Details" }));
    // absent is also what the link looks like before the gate has spoken
    await expectGateAnswered();
    await expect(drawer.getAllByText(/retention window has already passed/i)).toHaveLength(1);
    await expect(drawer.queryByRole("link", { name: "Open log settings" })).toBeNull();
    await expect(drawer.getAllByText(/set by a superadmin/i)).toHaveLength(1);
  },
};

/** The same drawer for a caller who can read the settings: the link, and the specific reason. */
export const ASuperadminIsSentToLogSettings: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/api/v1/analytics/invocations", () => ({ data: ROWS })],
        ["/api/v1/currency", () => ({ base: "USD", codes: ["USD"], rates: {} })],
        [
          "/api/v1/logging-settings",
          () => ({ payload_capture_enabled: true, payload_retention_hours: 24 }),
        ],
      ])}
      role="superadmin"
    >
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open request details for gpt-4o/i }),
    );
    const drawer = within(await canvas.findByRole("complementary", { name: "Details" }));
    await expectGateAnswered();
    // the settings were read, so the reason names the retention window's length
    await waitFor(() => expect(drawer.getAllByText(/24h retention window/i)).toHaveLength(1));
    const links = drawer.getAllByRole("link", { name: "Open log settings" });
    await expect(links).toHaveLength(1);
    await expect(links[0]).toHaveAttribute("href", "/logs-settings");
    await expect(drawer.queryByText(/set by a superadmin/i)).toBeNull();
  },
};

/**
 * #1820: a body the caller's role may not read comes back blanked with
 * `payload_withheld` set. That is the one absence the screen can name for
 * certain, so it says so — and does not send the reader to the deployment's
 * log settings, which cannot change a role.
 */
export const AWithheldPayloadSaysItIsTheRole: Story = {
  render: () => (
    <Harness fetchStub={withLogs([row({ request_id: "req-withheld", payload_withheld: 1 })])}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open request details for gpt-4o/i }),
    );
    const drawer = within(await canvas.findByRole("complementary", { name: "Details" }));
    await expect(drawer.getAllByText(/hidden for your role/i)).toHaveLength(1);
    await expect(drawer.getByRole("heading", { name: "Request and response" })).toBeVisible();
    await expect(drawer.queryByText(/retention window/i)).not.toBeInTheDocument();
    await expect(drawer.queryByRole("link", { name: "Open log settings" })).not.toBeInTheDocument();
  },
};

const REQUEST_BODY = '{"model":"gpt-4o","messages":[{"role":"user","content":"ping"}]}';
const RESPONSE_BODY = '{"id":"chatcmpl-1","choices":[{"message":{"content":"pong"}}]}';

/** Both bodies stored: two code blocks under their own headings, and nothing to explain. */
export const BothBodiesAreShownWithNoNote: Story = {
  render: () => (
    <Harness
      fetchStub={withLogs([
        row({ request_payload: REQUEST_BODY, response_payload: RESPONSE_BODY }),
      ])}
    >
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open request details for gpt-4o/i }),
    );
    const drawer = within(await canvas.findByRole("complementary", { name: "Details" }));
    await expect(drawer.getByRole("region", { name: /^Request — / })).toHaveTextContent("ping");
    await expect(drawer.getByRole("region", { name: /^Response — / })).toHaveTextContent("pong");
    await expect(drawer.queryByRole("heading", { name: "Request and response" })).toBeNull();
    await expect(drawer.queryByText(/No (request|response) body was stored/)).toBeNull();
    await expect(drawer.queryByText(/retention window/i)).toBeNull();
    await expect(drawer.queryByRole("link", { name: "Open log settings" })).toBeNull();
  },
};

/**
 * #2131: a request with one body keeps the explanation for the missing one
 * only. The gateway stores a request's two bodies together, so one of them
 * being there means capture was on, the request passed the allow-list and the
 * retention window is open; none of the three guesses applies, and the log
 * settings cannot fill a body that was empty when it was logged. The note says
 * that and offers no settings link.
 */
export const ARequestWithOneBodyExplainsTheMissingOneOnly: Story = {
  render: () => (
    <Harness fetchStub={withLogs([row({ request_payload: REQUEST_BODY, response_payload: "" })])}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open request details for gpt-4o/i }),
    );
    const drawer = within(await canvas.findByRole("complementary", { name: "Details" }));
    await expect(drawer.getByRole("region", { name: /^Request — / })).toHaveTextContent("ping");
    await expect(drawer.getByRole("heading", { name: "Response" })).toBeVisible();
    await expect(drawer.getAllByText(/No response body was stored/)).toHaveLength(1);
    await expect(drawer.queryByText(/No request body was stored/)).toBeNull();
    await expect(drawer.queryByRole("heading", { name: "Request and response" })).toBeNull();
    // the three guesses are for a request with neither body
    await expect(drawer.queryByText(/retention window|capture/i)).toBeNull();
    await expect(drawer.queryByRole("link", { name: "Open log settings" })).toBeNull();
  },
};

/** The other half: a stored response with no request body. */
export const AResponseWithNoRequestBodyExplainsTheRequestOnly: Story = {
  render: () => (
    <Harness fetchStub={withLogs([row({ request_payload: "", response_payload: RESPONSE_BODY })])}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open request details for gpt-4o/i }),
    );
    const drawer = within(await canvas.findByRole("complementary", { name: "Details" }));
    await expect(drawer.getByRole("region", { name: /^Response — / })).toHaveTextContent("pong");
    await expect(drawer.getByRole("heading", { name: "Request" })).toBeVisible();
    await expect(drawer.getAllByText(/No request body was stored/)).toHaveLength(1);
    await expect(drawer.queryByText(/No response body was stored/)).toBeNull();
    await expect(drawer.queryByRole("link", { name: "Open log settings" })).toBeNull();
  },
};

/**
 * #2131 in Russian, the copy that ran to five lines twice: one note, one
 * heading that names both bodies and one link, in a drawer that holds them
 * without pushing the page sideways.
 */
export const TheAbsentPayloadNoteIsOnceInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => (
    <Harness fetchStub={withLogs(ROWS)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", {
        name: ru.analytics.openDetails.replace("{{model}}", "gpt-4o"),
      }),
    );
    const dialog = within(await within(document.body).findByRole("dialog"));
    await waitFor(() =>
      expect(dialog.getByRole("heading", { name: ru.pages.logs.requestAndResponse })).toBeVisible(),
    );
    await expect(dialog.getAllByText(/срок хранения тела уже истёк/)).toHaveLength(1);
    await expect(dialog.queryByRole("heading", { name: ru.pages.logs.request })).toBeNull();
    await expect(
      dialog.getAllByRole("link", { name: ru.pages.logs.payloadSettingsLink }),
    ).toHaveLength(1);
    await expectNoHorizontalOverflow();
  },
};

const unconfigured = recording(
  scoped(async (input) =>
    String(input).includes("/analytics/")
      ? json({ error: { message: "analytics is not configured" } }, 503)
      : json([]),
  ),
);

/**
 * A control plane with no clickhouse_url answers the analytics routes 503, and
 * one too old to have them answers 404. Both used to render an untranslated
 * grey paragraph of this screen's own (#1236), then the red alert a 500 gets.
 * It is a deployment shape rolter supports, so since #1984 it is a calm
 * `status` panel: nothing on the screen is an alert, and nothing polls.
 */
export const NoAnalyticsStore: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <UxScreenProvider screen="logs">
      <Harness fetchStub={unconfigured.stub}>
        <Logs pollMs={FAST_POLL_MS} />
      </Harness>
    </UxScreenProvider>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const title = await canvas.findByText("Request logs need ClickHouse");
    const panel = title.closest('[role="status"]') as HTMLElement | null;
    await expect(panel).not.toBeNull();
    await expect(canvas.queryAllByRole("alert")).toHaveLength(0);
    // the setting to change is named in monospace, twice: what is missing and
    // what to set
    const names = within(panel!).getAllByText("CLICKHOUSE_URL");
    await expect(names).toHaveLength(2);
    for (const name of names) await expect(name.tagName).toBe("CODE");
    // the control plane's own words stay under it (#962)
    await expect(within(panel!).getByText("analytics is not configured")).toBeVisible();
    // and the retry that cannot help is withheld
    await expect(canvas.queryByRole("button", { name: /try again/i })).toBeNull();

    // the panel holds still across polling intervals rather than flipping back
    // to the table and its skeleton on each one
    const reads = logReads(unconfigured);
    await sleep(FAST_POLL_MS * 3);
    await expect(panel!.isConnected).toBe(true);
    await expect(logReads(unconfigured)).toBe(reads);

    // it is an answer, so the screen is ready, and a supported deployment, so it
    // is not an error state (#2017)
    await expectUxEvent("time_to_interactive");
    expectNoUxEvent("error_state");
  },
};

// newest first, as the control plane sorts them, each with its own id so a
// cursor names exactly one row
const logPage = (count: number): InvocationRow[] =>
  Array.from({ length: count }, (_, i) =>
    row({
      request_id: `req-p${String(i + 1).padStart(3, "0")}`,
      ts: new Date(BASE_TS - i * 1000).toISOString(),
    }),
  );

/**
 * A stub that pages the way `GET /api/v1/analytics/invocations` does since
 * #1410: `cursor` is the `ts|request_id` of the last row already served, the
 * page resumes strictly after it, and `next_cursor` names the page's own last
 * row. An `offset` is ignored, exactly as the server ignores it — a screen that
 * still sent one would be handed page one again.
 */
const cursorPaged = (rows: InvocationRow[], failAfterFirst = false): FetchStub =>
  scoped(async (input) => {
    const url = new URL(String(input), "http://localhost");
    if (url.pathname === "/api/v1/analytics/invocations") {
      const cursor = url.searchParams.get("cursor");
      if (cursor && failAfterFirst) return json({ error: { message: "clickhouse refused" } }, 500);
      const limit = Number(url.searchParams.get("limit") ?? 50);
      const from = cursor ? rows.findIndex((r) => `${r.ts}|${r.request_id}` === cursor) + 1 : 0;
      const data = rows.slice(from, from + limit);
      const last = data[data.length - 1];
      return json({ data, next_cursor: last ? `${last.ts}|${last.request_id}` : null });
    }
    if (url.pathname === "/api/v1/currency")
      return json({ base: "USD", codes: ["USD"], rates: {} });
    if (url.pathname === "/api/v1/models") return json([]);
    return json([]);
  });

const SIXTY = logPage(60);
const paged = recording(cursorPaged(SIXTY));

/**
 * #1411: "next page" hands the server the cursor it returned rather than a row
 * offset, and "previous page" goes back to the cursor it came from. With an
 * offset the second page was page one again, silently.
 */
export const PagesOnTheCursor: Story = {
  render: () => (
    <Harness fetchStub={paged.stub}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText(fmt.timeMs(SIXTY[0].ts));
    await expect(canvasElement.querySelectorAll("tbody tr")).toHaveLength(50);
    await expect(canvas.getByRole("button", { name: "Previous page" })).toBeDisabled();

    await userEvent.click(canvas.getByRole("button", { name: "Next page" }));
    // the second page starts at the 51st row and holds the ten that are left
    await canvas.findByText(fmt.timeMs(SIXTY[50].ts));
    await waitFor(() => expect(canvasElement.querySelectorAll("tbody tr")).toHaveLength(10));
    const sent = paged.calls.filter((c) => c.url.includes("/analytics/invocations"));
    const cursor = `${SIXTY[49].ts}|${SIXTY[49].request_id}`;
    await expect(
      sent.some((c) => new URL(c.url, "http://localhost").searchParams.get("cursor") === cursor),
    ).toBe(true);
    await expect(sent.some((c) => c.url.includes("offset="))).toBe(false);
    await expect(canvas.getByText("p2")).toBeInTheDocument();
    // a short page is the last one, even though it carries a cursor
    await expect(canvas.getByRole("button", { name: "Next page" })).toBeDisabled();

    await userEvent.click(canvas.getByRole("button", { name: "Previous page" }));
    await canvas.findByText(fmt.timeMs(SIXTY[0].ts));
    await expect(canvas.getByText("p1")).toBeInTheDocument();
    await expect(canvas.queryByText(fmt.timeMs(SIXTY[50].ts))).toBeNull();
  },
};

const FIFTY = logPage(50);

/**
 * A full page can be the last one, so the page after it comes back empty. That
 * is the end of the log, not "nothing logged yet", and it offers the way back.
 */
export const AnEmptyLaterPageIsTheEnd: Story = {
  render: () => (
    <Harness fetchStub={cursorPaged(FIFTY)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText(fmt.timeMs(FIFTY[0].ts));
    await userEvent.click(canvas.getByRole("button", { name: "Next page" }));
    await expectEmptyState(canvasElement, /reached the end/);
    await expect(canvas.queryByText(/Nothing logged yet/)).toBeNull();

    await userEvent.click(canvas.getByRole("button", { name: "Back to newest" }));
    await canvas.findByText(fmt.timeMs(FIFTY[0].ts));
    await expect(canvas.getByText("p1")).toBeInTheDocument();
  },
};

/** A later page that fails reads as a load error with a retry, like the first. */
export const ALaterPageFails: Story = {
  render: () => (
    <Harness fetchStub={cursorPaged(SIXTY, true)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText(fmt.timeMs(SIXTY[0].ts));
    await userEvent.click(canvas.getByRole("button", { name: "Next page" }));
    await expectLoadError(canvasElement, /failed to return request logs/i);
    // the way back is still there
    await expect(canvas.getByRole("button", { name: "Previous page" })).toBeEnabled();
  },
};

// the lookup's copy, read out of the catalog so rewording it cannot leave a
// story asserting a sentence the screen no longer says
const lookupCopy = en.pages.logs.lookup;

// a recent request the feed shows, a request from long before the feed's 24
// hours, and three requests that share one trace. stamped against now rather
// than against a fixed date, so the feed's window keeps holding them
const nowMinus = (ms: number) => new Date(Date.now() - ms).toISOString();
const FEED: InvocationRow[] = [
  row({ request_id: "req-feed-1", ts: nowMinus(60_000) }),
  row({
    request_id: "req-feed-2",
    ts: nowMinus(90_000),
    model: "internal-llama",
    provider: "vllm",
  }),
];
const ARCHIVED = row({
  request_id: "req-archived-7d41",
  trace_id: "",
  ts: "2025-01-15T09:30:00.000Z",
  status: 502,
  upstream_status: 0,
  error: "upstream reset the connection",
});
const TRACE = "0af7651916cd43dd8448eb211c80319c";
const TRACED: InvocationRow[] = [
  row({ request_id: "req-trace-a", trace_id: TRACE, ts: nowMinus(200_000) }),
  row({
    request_id: "req-trace-b",
    trace_id: TRACE,
    ts: nowMinus(190_000),
    model: "internal-llama",
    provider: "vllm",
    status: 500,
    error: "replica 3 ran out of memory",
  }),
  row({ request_id: "req-trace-c", trace_id: TRACE, ts: nowMinus(180_000) }),
];

/**
 * The control plane as a lookup meets it: `request_id` and `trace_id` match
 * exactly, `since` bounds the read when it is sent, and the status class and
 * the model narrow it like any other filter. A stub that ignored `since` would
 * let a screen that kept the 24 hour window find the old request anyway.
 */
const archive = (rows: InvocationRow[]): FetchStub =>
  scoped(async (input) => {
    const url = new URL(String(input), "http://localhost");
    if (url.pathname === "/api/v1/analytics/invocations") {
      const q = url.searchParams;
      const since = q.get("since");
      const data = rows.filter(
        (r) =>
          (!q.get("request_id") || r.request_id === q.get("request_id")) &&
          (!q.get("trace_id") || r.trace_id === q.get("trace_id")) &&
          (!since || Date.parse(r.ts) >= Date.parse(since)) &&
          (!q.get("model") || r.model === q.get("model")) &&
          inStatusClass(Number(r.status), q.get("status") ?? "all"),
      );
      return json({ data });
    }
    if (url.pathname === "/api/v1/currency")
      return json({ base: "USD", codes: ["USD"], rates: {} });
    if (url.pathname === "/api/v1/models") return json(MODELS);
    return json([]);
  });

/** every read of the log the screen sent, as the parameters it carried */
const logReadsOf = (recorder: Recorder) =>
  recorder.calls
    .filter((c) => c.url.includes("/analytics/invocations"))
    .map((c) => new URL(c.url, "http://localhost").searchParams);

const lookupField = (canvas: ReturnType<typeof within>, name: string = lookupCopy.label) =>
  canvas.getByRole("textbox", { name });

const pasteLookup = async (canvasElement: HTMLElement, text: string) => {
  const canvas = within(canvasElement);
  await userEvent.click(await canvas.findByRole("textbox", { name: lookupCopy.label }));
  await userEvent.paste(text);
  await userEvent.keyboard("{Enter}");
};

const pasted = recording(archive([...FEED, ARCHIVED, ...TRACED]));

/**
 * #1861: a request id pasted into the field finds its row wherever it sits in
 * the log and opens its drawer. The request carries the id and no window, so
 * a row from long before the feed's 24 hours is found rather than answered with
 * an empty page; the drawer opens because exactly one row came back. The feed
 * stopped for the lookup, since nothing in an answer streams.
 */
export const APastedRequestIdOpensItsRow: Story = {
  render: () => (
    <Harness fetchStub={pasted.stub}>
      <Logs pollMs={FAST_POLL_MS} />
      <AddressProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText(fmt.timeMs(FEED[0].ts));
    await expect(canvas.getByRole("button", { name: "Pause" })).toBeVisible();
    // a pasted id keeps what the clipboard put around it out of the request
    await pasteLookup(canvasElement, `  ${ARCHIVED.request_id}\n`);

    const panel = await canvas.findByRole("complementary", { name: "Details" });
    await waitFor(() => expect(within(panel).getByText(ARCHIVED.request_id)).toBeVisible());
    await expect(within(panel).getByText("502")).toBeVisible();
    await expect(modelsOnScreen(canvasElement)).toEqual([ARCHIVED.model]);
    await expect(canvasElement.querySelector("tbody tr")).toHaveAttribute("aria-selected", "true");

    const sent = lastLogQuery(pasted);
    await expect(sent.get("request_id")).toBe(ARCHIVED.request_id);
    await expect(sent.has("trace_id")).toBe(false);
    await expect(sent.has("since")).toBe(false);
    await expect(sent.has("until")).toBe(false);
    await expect(addressOf(canvasElement).get("request_id")).toBe(ARCHIVED.request_id);
    await expect(lookupField(canvas)).toHaveValue(ARCHIVED.request_id);

    // the lookup is stated as what the list is, and there is no feed to pause
    await expect(canvas.getByText(`${lookupCopy.feedRequest} · 1 request`)).toBeVisible();
    await expect(canvas.queryByText(/Streaming/)).toBeNull();
    await expect(canvas.queryByRole("button", { name: "Pause" })).toBeNull();

    // and it does not poll: the control plane reads every retained row for it
    const reads = logReads(pasted);
    await sleep(FAST_POLL_MS * 3);
    await expect(logReads(pasted)).toBe(reads);
  },
};

const missing = recording(archive([...FEED, ARCHIVED]));

/**
 * #1861: an id nothing matches says so in one message, as an empty result and
 * not as an alert. The control plane filters by what the caller may read inside
 * the query, so an id from a project the caller cannot see comes back the same
 * way and reads the same; the message names both causes. Clearing the lookup
 * returns to the feed, with its window again.
 */
export const AnIdThatMatchesNothingSaysSo: Story = {
  render: () => (
    <Harness fetchStub={missing.stub}>
      <Logs />
      <AddressProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText(fmt.timeMs(FEED[0].ts));
    await pasteLookup(canvasElement, "req-unknown-5150");

    await expectEmptyState(canvasElement, new RegExp(lookupCopy.missTitle), /Clear lookup/);
    await expect(canvas.getByText(lookupCopy.missBody)).toBeVisible();
    await expect(canvas.queryAllByRole("alert")).toHaveLength(0);
    await expect(canvas.queryByRole("complementary", { name: "Details" })).toBeNull();
    await expect(canvas.queryByText(/Nothing logged yet/)).toBeNull();
    await expect(canvas.getByText(`${lookupCopy.feedRequest} · 0 requests`)).toBeVisible();
    await expect(lastLogQuery(missing).get("request_id")).toBe("req-unknown-5150");

    await userEvent.click(canvas.getByRole("button", { name: lookupCopy.clear }));
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toHaveLength(FEED.length));
    await expect(canvas.queryByText(lookupCopy.missTitle)).toBeNull();
    await expect(lookupField(canvas)).toHaveValue("");
    await waitFor(() => expect(lookupField(canvas)).toHaveFocus());
    await expect(addressOf(canvasElement).has("request_id")).toBe(false);
    const back = lastLogQuery(missing);
    await expect(back.has("since")).toBe(true);
    await expect(back.has("request_id")).toBe(false);
    await expect(canvas.getByText("Streaming · 2 requests")).toBeVisible();
  },
};

const traced = recording(archive([...FEED, ...TRACED]));

/**
 * #1861: a trace id is told from a request id by its shape, and a `traceparent`
 * header pasted whole is reduced to the trace id inside it. Several requests
 * can share a trace, so they come back as the list and no drawer opens; the
 * lookup is stated as what the list is, and the field's clear control returns
 * to the feed.
 */
export const ATraceIdListsEveryRequestOnIt: Story = {
  render: () => (
    <Harness fetchStub={traced.stub}>
      <Logs />
      <AddressProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText(fmt.timeMs(FEED[0].ts));
    await pasteLookup(canvasElement, `00-${TRACE}-b7ad6b7169203331-01`);

    await waitFor(() => expect(modelsOnScreen(canvasElement)).toHaveLength(TRACED.length));
    await expect(lookupField(canvas)).toHaveValue(TRACE);
    await expect(canvas.getByText(`${lookupCopy.feedTrace} · 3 requests`)).toBeVisible();
    // more than one row, so none is the one that was asked for
    await expect(canvas.queryByRole("complementary", { name: "Details" })).toBeNull();
    await expect(canvasElement.querySelectorAll('tbody tr[aria-selected="true"]')).toHaveLength(0);
    const sent = lastLogQuery(traced);
    await expect(sent.get("trace_id")).toBe(TRACE);
    await expect(sent.has("request_id")).toBe(false);
    await expect(sent.has("since")).toBe(false);
    await expect(addressOf(canvasElement).get("trace_id")).toBe(TRACE);

    // a row of the trace still opens on a click, like any other
    await userEvent.click(
      canvas.getByRole("button", { name: /Open request details for internal-llama/i }),
    );
    const panel = await canvas.findByRole("complementary", { name: "Details" });
    await waitFor(() => expect(within(panel).getByText("req-trace-b")).toBeVisible());

    // the feed holds the trace's requests too, since they are recent
    await userEvent.click(canvas.getByRole("button", { name: lookupCopy.clearField }));
    await waitFor(() =>
      expect(modelsOnScreen(canvasElement)).toHaveLength(FEED.length + TRACED.length),
    );
    await expect(lookupField(canvas)).toHaveValue("");
    await expect(canvas.queryByRole("complementary", { name: "Details" })).toBeNull();
    await expect(addressOf(canvasElement).has("trace_id")).toBe(false);
    await expect(lastLogQuery(traced).has("since")).toBe(true);
    await expect(canvas.getByRole("button", { name: "Pause" })).toBeVisible();
  },
};

const linked = recording(archive([...FEED, ARCHIVED]));

/**
 * #1861: `/logs?request_id=…` opens that request. The screen reads the address
 * before its first request, so no unfiltered feed page is fetched first, and
 * the field shows the id the link carried. This is also the address the command
 * palette navigates to.
 */
export const ALinkOpensTheRequest: Story = {
  parameters: { address: `/logs?request_id=${ARCHIVED.request_id}` },
  render: () => (
    <Harness fetchStub={linked.stub}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const panel = await canvas.findByRole("complementary", { name: "Details" });
    await waitFor(() => expect(within(panel).getByText(ARCHIVED.request_id)).toBeVisible());
    await expect(lookupField(canvas)).toHaveValue(ARCHIVED.request_id);
    await expect(modelsOnScreen(canvasElement)).toEqual([ARCHIVED.model]);
    const reads = logReadsOf(linked);
    await expect(reads.length).toBeGreaterThan(0);
    for (const sent of reads) {
      await expect(sent.get("request_id")).toBe(ARCHIVED.request_id);
      await expect(sent.has("since")).toBe(false);
    }

    // the drawer closes and stays closed: the row was opened once, not kept open
    await userEvent.click(canvas.getByRole("button", { name: "Close details" }));
    await waitFor(() =>
      expect(canvas.queryByRole("complementary", { name: "Details" })).toBeNull(),
    );
    await expect(modelsOnScreen(canvasElement)).toEqual([ARCHIVED.model]);
  },
};

const held = recording(archive([...FEED, ARCHIVED]));

/**
 * #1861: an id names one request, so the picks the rail holds do not narrow
 * it. The view was filtered to OK requests on one model and the request asked
 * for was a 502 on another; a lookup that kept the picks would have answered
 * "not found" for an id that exists. Starting one drops them from the address
 * and from the request.
 */
export const AnIdLookupStartsFromTheWholeLog: Story = {
  parameters: { address: "/logs?status=success&model=internal-llama" },
  render: () => (
    <Harness fetchStub={held.stub}>
      <Logs />
      <AddressProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toEqual(["internal-llama"]));
    await expect(canvas.getByRole("button", { name: "Filters · 2" })).toBeVisible();
    await pasteLookup(canvasElement, ARCHIVED.request_id);

    await canvas.findByRole("complementary", { name: "Details" });
    await expect(modelsOnScreen(canvasElement)).toEqual([ARCHIVED.model]);
    const sent = lastLogQuery(held);
    await expect(sent.get("request_id")).toBe(ARCHIVED.request_id);
    await expect(sent.get("status")).toBe("all");
    await expect(sent.has("model")).toBe(false);
    const address = addressOf(canvasElement);
    await expect(address.has("status")).toBe(false);
    await expect(address.has("model")).toBe(false);
    await expect(canvas.getByRole("button", { name: "Filters" })).toBeVisible();
  },
};

const lookupFailing = recording(refusing());

/**
 * #1861: a lookup the control plane could not answer is a failure, not a miss.
 * A 5xx shows the load error with its retry; "no request found" is reserved for
 * an answer that came back empty.
 */
export const ALookupThatFailsIsAnError: Story = {
  parameters: { address: "/logs?request_id=req-5xx-0001" },
  render: () => (
    <Harness fetchStub={lookupFailing.stub}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /failed to return request logs/i);
    await expect(canvas.queryByText(lookupCopy.missTitle)).toBeNull();
    await expect(canvas.queryByText(lookupCopy.missBody)).toBeNull();
    await expect(lookupField(canvas)).toHaveValue("req-5xx-0001");
  },
};

const russian = recording(archive([...FEED, ARCHIVED]));
const ruLookup = ru.pages.logs.lookup;

/**
 * The lookup at 375px in Russian, the longest copy it carries: the field and
 * its button share a row inside the viewport, a long id fits in the field, and
 * the miss reads in Russian without pushing the page sideways.
 */
export const TheLookupFitsAtMobileInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => (
    <Harness fetchStub={russian.stub}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const field = await canvas.findByRole("textbox", { name: ruLookup.label });
    await expectInViewport(field);
    await expectInViewport(canvas.getByRole("button", { name: ruLookup.find }));
    await expectNoHorizontalOverflow();

    await userEvent.click(field);
    await userEvent.paste("3f2c9a1e-7b4d-4f10-9c2e-0a1b2c3d4e5f-retry-0042");
    await userEvent.keyboard("{Enter}");
    await expectEmptyState(canvasElement, new RegExp(ruLookup.missTitle), /Сбросить поиск/);
    await expect(canvas.getByText(ruLookup.missBody)).toBeVisible();
    await expectInViewport(canvas.getByRole("button", { name: ruLookup.clearField }));
    await expectInViewport(canvas.getByRole("button", { name: ruLookup.find }));
    await expectNoHorizontalOverflow();
  },
};

// the sheet portals onto the body
const screen = () => within(document.body);

const KEY_ID = "22222222-2222-4222-8222-222222222222";
const GONE_CUSTOMER = "44444444-4444-4444-8444-444444444444";
const SAVED_VIEW = {
  id: "11111111-1111-4111-8111-111111111111",
  surface: "llm_logs",
  name: "Platform errors",
  // the stored set still names a customer the account can no longer read
  filters: {
    window: "7d",
    status: "error",
    model: "internal-llama",
    key: KEY_ID,
    business_unit: ["unit-1"],
    customer: ["cust-1", GONE_CUSTOMER],
  },
  effective_filters: {
    window: "7d",
    status: "error",
    model: "internal-llama",
    key: KEY_ID,
    business_unit: ["unit-1"],
    customer: ["cust-1"],
  },
  unavailable: [{ filter: "customer", id: GONE_CUSTOMER }],
  created_at: "2026-09-01T09:00:00Z",
  updated_at: "2026-09-01T09:00:00Z",
};

const withSavedView = recording(
  scoped(async (input, init) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (path === "/api/v1/me/saved-views") return json([SAVED_VIEW]);
    return serverFiltered(MIXED)(input, init);
  }),
);

/**
 * #2452: applying a saved view writes its `effective_filters` into the address,
 * so the screen reads them like any link, and the id it could not apply is
 * counted, not named. The log is then read with the view's window, key and
 * attribution, and with no cursor or request id.
 */
export const ApplyingASavedViewSetsTheAddress: Story = {
  parameters: { address: "/logs?request_id=req-ok&unpriced=true" },
  render: () => (
    <Harness fetchStub={withSavedView.stub}>
      <Logs />
      <AddressProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Saved views" }));
    await userEvent.click(await screen().findByRole("button", { name: "Apply Platform errors" }));
    await waitFor(() => expect(addressOf(canvasElement).get("status")).toBe("error"));
    const address = addressOf(canvasElement);
    await expect(address.get("window")).toBe("7d");
    await expect(address.get("model")).toBe("internal-llama");
    await expect(address.get("key")).toBe(KEY_ID);
    await expect(address.get("business_unit")).toBe("unit-1");
    await expect(address.get("customer")).toBe("cust-1");
    // the lookup would have masked the filters, so it is gone; the unpriced
    // flag is not part of a view and stays
    await expect(address.has("request_id")).toBe(false);
    await expect(address.get("unpriced")).toBe("true");
    await expect(await canvas.findByRole("status")).toHaveTextContent(
      "Applied without: 1 customer.",
    );
    await waitFor(() => {
      const sent = lastLogQuery(withSavedView);
      expect(sent.get("key")).toBe(KEY_ID);
      expect(sent.get("customer")).toBe("cust-1");
      expect(sent.get("status")).toBe("error");
      expect(sent.has("request_id")).toBe(false);
      const span = Date.now() - Date.parse(sent.get("since") ?? "");
      expect(span).toBeGreaterThan(6.9 * 24 * 3600_000);
    });
  },
};

const savingFromLogs = recording(
  scoped(async (input, init) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (path === "/api/v1/me/saved-views") {
      return init?.method === "POST" ? json(SAVED_VIEW, 201) : json([]);
    }
    return serverFiltered(MIXED)(input, init);
  }),
);

/**
 * Saving keeps the filters applied now: an `all` status and empty values are
 * left out, and neither the lookup id, the cursor nor the limit is sent.
 */
export const SavingKeepsOnlyTheAppliedFilters: Story = {
  parameters: { address: "/logs?status=error&model=gpt-4o&window=30d&trace_id=abc" },
  render: () => (
    <Harness fetchStub={savingFromLogs.stub}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Saved views" }));
    const sheet = within(await screen().findByRole("dialog", { name: "Saved views" }));
    await userEvent.type(await sheet.findByLabelText("Save the current filters as"), "Mine");
    await userEvent.click(sheet.getByRole("button", { name: "Save view" }));
    const body = await savingFromLogs.expectSentBody<{ filters: Record<string, unknown> }>(
      "POST",
      "/api/v1/me/saved-views",
    );
    await expect(body.filters).toEqual({ window: "30d", status: "error", model: "gpt-4o" });
  },
};

const KEY_ROWS: InvocationRow[] = [
  row({ request_id: "req-ci", model: "gpt-4o", virtual_key_id: "vk-ci" }),
  row({ request_id: "req-other", model: "internal-llama", virtual_key_id: "vk-other" }),
];

// the control plane narrows on the one exact key id before the page is cut
const keyFiltered = recording(
  scoped(async (input) => {
    const url = new URL(String(input), "http://localhost");
    if (url.pathname === "/api/v1/analytics/invocations") {
      const key = url.searchParams.get("key");
      return json({ data: KEY_ROWS.filter((r) => !key || r.virtual_key_id === key) });
    }
    if (url.pathname === "/api/v1/currency")
      return json({ base: "USD", codes: ["USD"], rates: {} });
    if (url.pathname.endsWith("/virtual-keys")) {
      return json([
        CI_KEY,
        { ...CI_KEY, id: "vk-other", name: "batch-worker", key_prefix: "rk_live_cd34" },
      ]);
    }
    return json([]);
  }),
);

/**
 * #2516: a key filter could only arrive from a saved view or a pasted link.
 * The rail now picks one by name, writes `?key=`, counts toward "Filters · N",
 * and Clear filters takes it out again.
 */
export const AKeyCanBePickedByName: Story = {
  render: () => (
    <Harness fetchStub={keyFiltered.stub}>
      <Logs />
      <AddressProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toHaveLength(2));
    await userEvent.click(canvas.getByRole("button", { name: /Filters/ }));
    const picker = await canvas.findByRole("combobox", { name: "Virtual key" });
    await expect(picker).toHaveAttribute("placeholder", "All keys");

    await userEvent.click(picker);
    await expect(await canvas.findByRole("option", { name: /batch-worker/ })).toBeVisible();
    await userEvent.click(await canvas.findByRole("option", { name: /ci-runner/ }));
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toEqual(["gpt-4o"]));
    await expect(addressOf(canvasElement).get("key")).toBe("vk-ci");
    await expect(lastLogQuery(keyFiltered).get("key")).toBe("vk-ci");
    await expect(canvas.getByRole("button", { name: "Filters · 1" })).toBeVisible();

    await userEvent.click(canvas.getByRole("button", { name: "Clear filters" }));
    await waitFor(() => expect(modelsOnScreen(canvasElement)).toHaveLength(2));
    await expect(addressOf(canvasElement).has("key")).toBe(false);
    await expect(picker).toHaveValue("");
  },
};

// a call on a stored response (#2836, #2865): the model is the one the response
// was created for, the call generates nothing, so there are no tokens and no
// cost, and `lifecycle_operation` says which call it was
const lifecycleRow = (operation: string, over: Partial<InvocationRow> = {}) =>
  row({
    request_id: `req-lifecycle-${operation}`,
    model: "gpt-4o",
    lifecycle_operation: operation,
    prompt_tokens: 0,
    completion_tokens: 0,
    total_tokens: 0,
    cost_usd: 0,
    ttft_ms: 0,
    latency_ms: 38,
    ...over,
  });
const RETRIEVED = lifecycleRow("retrieve");
// a request that ran a model: the column is there and empty
const MODEL_RUN = row({
  request_id: "req-model-run",
  model: "claude-haiku",
  provider: "anthropic",
  lifecycle_operation: "",
});
// written before the column, or by a control plane that predates it: the field
// is absent, not empty
const BEFORE_THE_COLUMN = row({ request_id: "req-before-the-column", model: "legacy-model" });

/** The model cell of table row `index`, whatever columns the width is drawing. */
const modelCellOf = (canvasElement: HTMLElement, index: number) =>
  canvasElement.querySelectorAll("tbody tr")[index].querySelectorAll("td")[1];

/**
 * #2865: a call on a stored response names its operation beside the model, and
 * only that row does. A request that ran a model, and a row from before the
 * column or from an older control plane, have nothing to add, so the model cell
 * is the model and nothing else. The badge's title says what kind of call it
 * is, and the drawer carries the same words as the first row of Routing.
 */
export const ALifecycleCallNamesItsOperationBesideTheModel: Story = {
  render: () => (
    <Harness fetchStub={withLogs([RETRIEVED, MODEL_RUN, BEFORE_THE_COLUMN])}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    // the fixture is only an old row while it carries no field at all
    await expect("lifecycle_operation" in BEFORE_THE_COLUMN).toBe(false);
    await within(canvasElement).findByText(fmt.timeMs(MODEL_RUN.ts));

    const called = within(modelCellOf(canvasElement, 0));
    await expect(called.getByText("gpt-4o")).toBeVisible();
    const badge = called.getByText("Retrieve");
    await expect(badge).toBeVisible();
    await expect(badge).toHaveAttribute("title", "Call on a stored response: Retrieve");

    for (const [index, model] of [
      [1, "claude-haiku"],
      [2, "legacy-model"],
    ] as const) {
      const cell = modelCellOf(canvasElement, index);
      await expect(cell).toHaveTextContent(new RegExp(`^${model}$`));
      await expect(cell.querySelector("[title]")).toBeNull();
    }
    await expect(canvasElement.querySelectorAll("tbody tr")).toHaveLength(3);

    const lifecycle = within(await openDetails(canvasElement, "gpt-4o"));
    const routing = await lifecycle.findByRole("region", { name: "Routing" });
    await waitFor(() => expect(within(routing).getByText("Stored response call")).toBeVisible());
    await expect(valueOf(routing, "Stored response call")).toBe("Retrieve");
    // the first row of the group, ahead of where the call was routed to
    await expect(routing.querySelector("dt")).toHaveTextContent("Stored response call");

    for (const model of ["claude-haiku", "legacy-model"]) {
      await userEvent.click(
        within(canvasElement).getByRole("button", {
          name: new RegExp(`Open request details for ${model}`),
        }),
      );
      await waitFor(() =>
        expect(within(canvasElement).getByRole("heading", { name: model, level: 2 })).toBeVisible(),
      );
      await expect(lifecycle.queryByText("Stored response call")).toBeNull();
      await expect(lifecycle.getByRole("region", { name: "Routing" })).toBeVisible();
    }
  },
};

const OPERATIONS = [
  "retrieve",
  "delete",
  "cancel",
  "input_items",
  "compact",
  "input_tokens",
] as const;
const FUTURE_CALL = lifecycleRow("future_call", { model: "resp-future" });
// built once: every row is stamped as it is made, so a second call would be
// six other rows that the screen is not showing
const EVERY_OPERATION = OPERATIONS.map((operation) =>
  lifecycleRow(operation, { model: `resp-${operation.replace("_", "-")}` }),
);

/**
 * #2865: each operation the gateway records has its own words, and one this
 * build has no words for is printed as the control plane sent it rather than
 * dropped, so a newer gateway's call is still readable.
 */
export const EveryLifecycleOperationHasItsOwnWords: Story = {
  render: () => (
    <Harness fetchStub={withLogs([...EVERY_OPERATION, FUTURE_CALL])}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByText(fmt.timeMs(EVERY_OPERATION[0].ts));
    const words = en.pages.logs.detail.lifecycle.operations;
    await expect(words).toEqual({
      retrieve: "Retrieve",
      delete: "Delete",
      cancel: "Cancel",
      input_items: "Input items",
      compact: "Compact",
      input_tokens: "Count input tokens",
    });
    for (const [index, operation] of OPERATIONS.entries()) {
      await expect(modelCellOf(canvasElement, index)).toHaveTextContent(
        new RegExp(`^resp-${operation.replace("_", "-")}${words[operation]}$`),
      );
    }
    await expect(modelCellOf(canvasElement, OPERATIONS.length)).toHaveTextContent(
      "resp-futurefuture_call",
    );
  },
};

/**
 * #2865 in Russian: the same six words, translated, in the table and under the
 * drawer's own label.
 */
export const TheLifecycleOperationIsTranslated: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={withLogs(EVERY_OPERATION)}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByText(formattersFor("ru").timeMs(EVERY_OPERATION[0].ts));
    const words = ru.pages.logs.detail.lifecycle.operations;
    for (const [index, operation] of OPERATIONS.entries()) {
      await expect(modelCellOf(canvasElement, index)).toHaveTextContent(words[operation]);
    }
    const badge = within(modelCellOf(canvasElement, 0)).getByText(words.retrieve);
    await expect(badge).toHaveAttribute(
      "title",
      ru.pages.logs.detail.lifecycle.title.replace("{{operation}}", words.retrieve),
    );

    await userEvent.click(
      within(canvasElement).getByRole("button", {
        name: ru.analytics.openDetails.replace("{{model}}", "resp-input-items"),
      }),
    );
    const drawer = within(
      await within(canvasElement).findByRole("complementary", { name: ru.analytics.details }),
    );
    const routing = await drawer.findByRole("region", { name: ru.pages.logs.detail.routing });
    await waitFor(() =>
      expect(within(routing).getByText(ru.pages.logs.detail.lifecycle.label)).toBeVisible(),
    );
    await expect(valueOf(routing, ru.pages.logs.detail.lifecycle.label)).toBe(words.input_items);
  },
};

// an id that does not exist, has expired or belongs to another tenant is a 404
// with no stored response to take a model from
const MISSING_RESPONSE = lifecycleRow("retrieve", {
  request_id: "req-missing-response",
  model: "",
  provider: "",
  target: "",
  status: 404,
  upstream_status: 0,
  attempts: 0,
  error: "no response resp_1 for this key",
});

/**
 * #2865: a call on a response that is not there has no model, so the row is
 * named by the call instead of by nothing: the badge stands alone in the model
 * cell, the chevron's name and the drawer's title say what was asked.
 */
export const ACallWithNoModelIsNamedByTheCall: Story = {
  render: () => (
    <Harness fetchStub={withLogs([MISSING_RESPONSE, MODEL_RUN])}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText(fmt.timeMs(MODEL_RUN.ts));
    await expect(modelCellOf(canvasElement, 0)).toHaveTextContent(/^Retrieve$/);

    await userEvent.click(
      canvas.getByRole("button", { name: "Open request details for Retrieve" }),
    );
    const panel = await canvas.findByRole("complementary", { name: "Details" });
    await waitFor(() =>
      expect(within(panel).getByRole("heading", { name: "Retrieve", level: 2 })).toBeVisible(),
    );
    await expect(
      valueOf(within(panel).getByRole("region", { name: "Routing" }), "Stored response call"),
    ).toBe("Retrieve");
  },
};

const LONG_MODEL_CALL = lifecycleRow("input_tokens", {
  request_id: "req-long-model-call",
  model: "claude-sonnet-4-5-20250929",
});

/**
 * #2865 on a phone, in the longer Russian copy: the model is the part that is
 * cut, never the operation, which keeps its room on the first line beside the
 * status. Nothing leaves the frame or scrolls the page sideways.
 */
export const TheOperationSurvivesALongModelAtMobileInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => (
    <Harness fetchStub={withLogs([LONG_MODEL_CALL, ...ROWS])}>
      <Logs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByText(formattersFor("ru").timeMs(LONG_MODEL_CALL.ts));
    await expectNoHorizontalOverflow();
    const shape = tableShape(canvasElement);
    await expect(shape.scrolls).toBe(false);
    await expectRowInFrame(shape, 0);

    const words = ru.pages.logs.detail.lifecycle.operations;
    const cell = within(shape.cells(0)[1]);
    const badge = cell.getByText(words.input_tokens);
    await expectInViewport(badge);
    await expect(badge.getBoundingClientRect().right).toBeLessThanOrEqual(
      shape.cells(0)[1].getBoundingClientRect().right + 1,
    );
    // the model gave way to it
    const model = cell.getByText(LONG_MODEL_CALL.model);
    await expect(model.scrollWidth).toBeGreaterThan(model.clientWidth);
    await expectStackedRow(canvasElement, 0, LONG_MODEL_CALL.model);
  },
};

// the same screen at a phone's width in both languages: Russian runs a third
// longer than English and overflowed twice as many screens (#2004)
const logsFit = phoneFits({
  render: () => (
    <Harness fetchStub={withLogs(ROWS)}>
      <Logs />
    </Harness>
  ),
  ready: (canvas, locale) => canvas.findByText(formattersFor(locale).timeMs(ROWS[0].ts)),
});
// the feed's pager sat 47px past the edge at 320px, in English
export const SmallPhone: Story = logsFit("small", "en");
export const SmallPhoneInRussian: Story = logsFit("small", "ru");
