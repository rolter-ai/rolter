import type { Meta, StoryObj } from "@storybook/react-vite";
import { MemoryRouter } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Dashboard from "./Dashboard";
import { ScreenHeader } from "@/components/ScreenHeader";
import {
  Harness,
  LOADING_LABEL,
  expectGateAnswered,
  expectLoadError,
  expectNoFalseEmpty,
  expectSkeleton,
  json,
  pending,
  recording,
  routes,
  scoped,
  type FetchStub,
  type Recorder,
  type StoryRole,
} from "./story-harness";
import { formattersFor } from "@/lib/i18n/format";
import en from "@/lib/i18n/locales/en.json";
import { atMobile, atTablet, expectNoHorizontalOverflow } from "@/lib/story-viewport";
import { resolveColorToken } from "@/lib/story-tokens";

const fmt = formattersFor("en");

const SUMMARY = {
  requests: 132,
  tokens: 1_284_000,
  prompt_tokens: 900_000,
  completion_tokens: 384_000,
  cost_usd: 41.27,
  unpriced_requests: 0,
  unpriced_models: 0,
  errors: 7,
  avg_latency_ms: 214.6,
  p50_latency_ms: 210,
  p95_latency_ms: 980,
};

const SERIES = Array.from({ length: 6 }, (_, i) => ({
  bucket: `2026-10-05T0${i}:00:00Z`,
  requests: 10 + i * 3,
  tokens: 4000 + i * 500,
  cost_usd: 1.5 + i,
}));

const BY_MODEL = [
  {
    model: "gpt-4o",
    requests: 84,
    tokens: 800_000,
    cost_usd: 30.1,
    unpriced_requests: 0,
    errors: 4,
    p50_latency_ms: 190,
    p95_latency_ms: 820,
  },
  {
    model: "claude-sonnet-4",
    requests: 48,
    tokens: 484_000,
    cost_usd: 11.17,
    unpriced_requests: 0,
    errors: 3,
    p50_latency_ms: 240,
    p95_latency_ms: 1100,
  },
];

const RECENT = [
  {
    ts: "2026-10-05T12:34:56.789Z",
    request_id: "req-1",
    trace_id: "trace-1",
    org_id: "org-1",
    team_id: "team-1",
    project_id: "project-1",
    virtual_key_id: "vk-1",
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
    latency_ms: 842,
    ttft_ms: 120,
    error: "",
  },
];

const loadedWith = (summary: typeof SUMMARY): FetchStub =>
  routes([
    ["/api/v1/analytics/summary", () => ({ data: [summary] })],
    ["/api/v1/analytics/timeseries", () => ({ data: SERIES })],
    ["/api/v1/analytics/by-model", () => ({ data: BY_MODEL })],
    ["/api/v1/analytics/invocations", () => ({ data: RECENT })],
    ["/api/v1/currency", () => ({ base: "USD", codes: ["USD"], rates: {} })],
  ]);

const loaded = loadedWith(SUMMARY);

/** the error-rate tile's delta line, as `t("pages.dashboard.errors")` renders it */
const errorCount = (n: number) => en.pages.dashboard.errors_other.replace("{{count}}", String(n));

// the first-run checklist the screen now opens with links to four screens, so
// the dashboard's stories need a router around them (#1585)
const render = (stub: FetchStub, role?: StoryRole, pollMs?: number) => (
  <MemoryRouter>
    <Harness fetchStub={stub} role={role}>
      <Dashboard pollMs={pollMs} />
    </Harness>
  </MemoryRouter>
);

/** every card the screen draws, by the test id it carries */
const CARDS = [
  "dashboard-figures",
  "dashboard-spend",
  "dashboard-traffic",
  "dashboard-by-model",
  "dashboard-recent",
];

/** the tile a figure is drawn in: its label sits in the tile's own element */
const tile = (canvas: ReturnType<typeof within>, label: string): HTMLElement =>
  canvas.getByText(label).closest("div") as HTMLElement;

const pathOf = (input: RequestInfo | URL) => new URL(String(input), "http://localhost").pathname;

const meta = {
  title: "Screens/Dashboard",
  component: Dashboard,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Dashboard>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => render(loaded),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the count is both a stat card and the donut centre
    await expect(await canvas.findAllByText(fmt.number(132))).not.toHaveLength(0);

    // 7 errors in 132 requests is 5.30%, over the 1% line, so the count reads
    // in the danger text colour. it used to come out success green under an
    // up arrow, because the arrow picked the colour (#1974)
    const errors = await canvas.findByText(errorCount(7));
    await expect(getComputedStyle(errors).color).toBe(resolveColorToken("--status-danger-text"));
    await expect(getComputedStyle(errors).color).not.toBe(
      resolveColorToken("--status-success-text"),
    );
    // the summary is one window with nothing earlier to compare against, so
    // the tile draws no arrow: it would claim a movement nobody measured
    await expect(errors.querySelector("svg")).toBeNull();

    // with requests to average, the latency tile is a measurement: the mean
    // rounded, in milliseconds, and no reason line under it
    const latency = tile(canvas, en.pages.dashboard.statAvgLatency);
    await expect(latency).toHaveTextContent(`${fmt.number(215)}${en.pages.dashboard.colMs}`);
    await expect(latency).not.toHaveTextContent(en.pages.dashboard.noRequestsInWindow);
  },
};

/**
 * Errors under the 1% line are reported, not flagged: 3 in 1,000 requests
 * keeps its count in the same muted grey as the tile's label. Red is kept for
 * the case that needs a look.
 */
export const ErrorsBelowThreshold: Story = {
  render: () => render(loadedWith({ ...SUMMARY, requests: 1000, errors: 3 })),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const errors = await canvas.findByText(errorCount(3));
    await expect(getComputedStyle(errors).color).toBe(
      getComputedStyle(canvas.getByText(en.pages.dashboard.statErrorRate)).color,
    );
    await expect(getComputedStyle(errors).color).not.toBe(
      resolveColorToken("--status-danger-text"),
    );
    await expect(errors.querySelector("svg")).toBeNull();
  },
};

export const Loading: Story = {
  render: () => render(pending),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    // every card stands in its own placeholder, and none of them says it has
    // found nothing: the by-model bars used to read "No traffic yet." here
    const canvas = within(canvasElement);
    for (const id of CARDS) {
      await expect(
        within(canvas.getByTestId(id)).getAllByLabelText(LOADING_LABEL),
      ).not.toHaveLength(0);
    }
    await expect(canvas.queryByText(en.pages.dashboard.noTraffic)).toBeNull();
    await expect(canvas.queryByText(en.pages.dashboard.nothingLogged)).toBeNull();
    await expect(canvas.queryByText(en.analytics.noRowsYet)).toBeNull();
  },
};

// a deployment that has served nothing yet. the summary is an aggregate with no
// `group by`, so the control plane always answers one row — of zeroes. this stub
// used to answer `data: []`, which is not a quiet day but a query failure, and
// the story sat on the "cannot reach the control plane" screen with no `play`
// to notice
const QUIET = Object.fromEntries(Object.keys(SUMMARY).map((k) => [k, 0]));

const quiet: FetchStub = routes([
  ["/api/v1/analytics/summary", () => ({ data: [QUIET] })],
  ["/api/v1/analytics", () => ({ data: [] })],
  ["/api/v1/currency", () => ({ base: "USD", codes: ["USD"], rates: {} })],
]);

export const Empty: Story = {
  render: () => render(quiet),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(en.pages.dashboard.statRequests)).toBeVisible();
    // zero traffic is an answer, not a failure: no error panel, and no `NaN%`
    // from dividing the error count by no requests
    await expect(canvas.queryByRole("alert")).toBeNull();
    await expect(canvasElement.textContent ?? "").not.toMatch(/NaN|undefined/);

    // the counts and the sum are real zeroes: nothing was asked, nothing was spent
    await expect(tile(canvas, en.pages.dashboard.statRequests)).toHaveTextContent(fmt.number(0));
    await expect(tile(canvas, en.pages.dashboard.statSpend)).toHaveTextContent(
      fmt.currency(0, "USD"),
    );
    // an average and a rate over no requests are not zero, and "0 ms" and
    // "0.00 %" said they had been measured. they read "—", with the reason
    for (const label of [en.pages.dashboard.statAvgLatency, en.pages.dashboard.statErrorRate]) {
      const figure = tile(canvas, label);
      await expect(within(figure).getByText(en.pages.dashboard.notMeasured)).toBeVisible();
      await expect(within(figure).getByText(en.pages.dashboard.noRequestsInWindow)).toBeVisible();
      await expect(figure).not.toHaveTextContent(/0\s*ms|0[.,]00|%/);
    }
    // each chart card answers its own empty read in its own words
    await expect(
      within(canvas.getByTestId("dashboard-spend")).getByText(en.analytics.noRowsYet),
    ).toBeVisible();
    await expect(
      within(canvas.getByTestId("dashboard-traffic")).getByText(en.pages.dashboard.noTraffic),
    ).toBeVisible();
    await expect(
      within(canvas.getByTestId("dashboard-by-model")).getByText(en.pages.dashboard.noTraffic),
    ).toBeVisible();
    await expect(
      within(canvas.getByTestId("dashboard-recent")).getByText(en.pages.dashboard.nothingLogged),
    ).toBeVisible();
  },
};

/**
 * The same quiet day, answered as an *empty* envelope rather than a row of
 * zeroes. `fetchAnalyticsSummary` used to resolve `r.data[0]`, which is
 * `undefined` here, and react-query v5 rejects a query function that resolves
 * to `undefined` — so this rendered "cannot reach the control plane" (#1608).
 *
 * The sibling of `Empty` rather than a replacement for it: the control plane
 * answers one row today, and this story is what keeps the other shape from
 * becoming an outage screen if a backend change or a proxy ever answers it.
 */
export const EmptySummaryEnvelope: Story = {
  render: () =>
    render(
      routes([
        ["/api/v1/analytics/summary", () => ({ data: [] })],
        ["/api/v1/analytics", () => ({ data: [] })],
        ["/api/v1/currency", () => ({ base: "USD", codes: ["USD"], rates: {} })],
      ]),
    ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(en.pages.dashboard.statRequests)).toBeVisible();
    await expect(await canvas.findByText(fmt.currency(0, "USD"))).toBeVisible();
    await expect(canvas.queryByRole("alert")).toBeNull();
    await expect(canvasElement.textContent ?? "").not.toMatch(/NaN|undefined/);
  },
};

/**
 * #959 was measured here: at 375px the stat cards were cut mid-value — `132`
 * rendered as `13`, `5.30%` as `5.3` — because four columns were four columns
 * at every width. One card per row, and the value is whole again.
 */
export const Mobile: Story = {
  ...atMobile,
  render: () => render(loaded),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.number(132));
    await expectNoHorizontalOverflow();
  },
};

export const Tablet: Story = {
  ...atTablet,
  render: () => render(loaded),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.number(132));
    await expectNoHorizontalOverflow();
  },
};

/**
 * Analytics off is neither an empty state nor an outage. The `EmptyState` this
 * used to render said "nothing has happened yet" about a control plane that was
 * never asked to record anything (#1236), and the `LoadError` after it was the
 * red `role="alert"` a 500 gets, announced to a screen reader on every visit
 * (#1976). It is one calm `status` panel for the screen, and no card is drawn
 * around it.
 */
export const NoAnalyticsStore: Story = {
  render: () =>
    render(
      scoped(async (input, init) =>
        pathOf(input).startsWith("/api/v1/analytics")
          ? json({ error: { message: "no clickhouse_url" } }, 503)
          : quiet(input, init),
      ),
    ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const title = await canvas.findByText(en.pages.dashboard.noAnalytics.title);
    const panel = title.closest('[role="status"]') as HTMLElement | null;
    await expect(panel).not.toBeNull();
    await expect(panel).toBeVisible();
    // nothing on the screen is an alert: the setup checklist above reads rows
    // and is not part of this
    await expect(canvas.queryAllByRole("alert")).toHaveLength(0);
    // the setting to change is named in monospace, twice: what is missing and
    // what to set
    const names = within(panel!).getAllByText("CLICKHOUSE_URL");
    await expect(names).toHaveLength(2);
    for (const name of names) await expect(name.tagName).toBe("CODE");
    // the control plane's own words stay under it (#962), and the retry that
    // cannot help is withheld
    await expect(within(panel!).getByText("no clickhouse_url")).toBeVisible();
    await expect(canvas.queryByRole("button", { name: /try again/i })).toBeNull();
    // no card is drawn: each would be a skeleton or an error about a store
    // that was never there
    for (const id of CARDS) await expect(canvas.queryByTestId(id)).toBeNull();
  },
};

/**
 * The quiet deployment #1848 was found on, as a caller below admin sees it: the
 * setup checklist's three lists all answer 403. A fresh recorder per story,
 * since each asserts on its own calls.
 */
function refusedChecklist(): Recorder {
  return recording(async (input, init) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (/\/(providers|routes|virtual-keys)$/.test(path)) {
      return json({ error: { message: "forbidden" } }, 403);
    }
    return quiet(input, init);
  });
}

/** the screen, with no checklist and no error card where it used to be */
async function expectNoChecklist(canvasElement: HTMLElement, calls: Recorder) {
  const canvas = within(canvasElement);
  await expect(await canvas.findByText(en.pages.dashboard.statRequests)).toBeVisible();
  // hidden, as opposed to not rendered yet, only once the gate has answered
  await expectGateAnswered();
  await expect(canvas.queryByText(en.pages.gettingStarted.title)).toBeNull();
  await expect(canvas.queryByRole("alert")).toBeNull();
  for (const fragment of ["/providers", "/routes", "/virtual-keys"]) {
    calls.expectNotSent("GET", fragment);
  }
}

const asMember = refusedChecklist();

/**
 * A member lands on the Dashboard to see traffic, and the first-run checklist
 * is not theirs: every step on it is an admin task. It used to open the screen
 * with "You do not have access to the setup checklist" (#1848).
 */
export const AsMember: Story = {
  render: () => render(asMember.stub, "member"),
  play: async ({ canvasElement }) => expectNoChecklist(canvasElement, asMember),
};

const asViewer = refusedChecklist();

/** a viewer — a FinOps analyst, say — gets the same screen with no error card */
export const AsViewer: Story = {
  render: () => render(asViewer.stub, "viewer"),
  play: async ({ canvasElement }) => expectNoChecklist(canvasElement, asViewer),
};

/**
 * The same quiet deployment for the admin who sets it up: the checklist is
 * there. What keeps the two stories above from passing on a card that never
 * renders for anyone.
 */
export const AsAdmin: Story = {
  render: () => render(quiet, "admin"),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(en.pages.gettingStarted.subtitle)).toBeVisible();
    await expect(canvas.queryByRole("alert")).toBeNull();
  },
};

/**
 * A polling cadence a play can watch several intervals of (#1975). The recent
 * requests card asks every 15s and the figures every minute, and the
 * test-runner gives a whole story 15s, so the screen takes `pollMs` from a
 * story and both run at this pace. The cadence is the only thing that changes.
 */
const FAST_POLL_MS = 300;
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const DAY_MS = 86_400_000;

const ENDPOINTS = [
  "/api/v1/analytics/summary",
  "/api/v1/analytics/timeseries",
  "/api/v1/analytics/by-model",
  "/api/v1/analytics/invocations",
];

/** every `since` the screen has sent to `endpoint`, oldest first */
const sinceOf = (recorder: Recorder, endpoint: string): number[] =>
  recorder.calls
    .filter((c) => new URL(c.url, "http://localhost").pathname === endpoint)
    .map((c) => Date.parse(new URL(c.url, "http://localhost").searchParams.get("since") ?? ""));

/** how many times the screen has asked `endpoint` */
const readsOf = (recorder: Recorder, endpoint: string) => sinceOf(recorder, endpoint).length;

const rolling = recording(loaded);

/**
 * #1975: "Last 24h" is the 24 hours before each read, not before the page was
 * opened. `since` used to be worked out once when the module loaded, so every
 * poll and every refresh sent the same lower bound and a tab left open for an
 * afternoon reported the last 24 hours plus the afternoon. Four endpoints read
 * the window, and each one's second read starts later than its first.
 */
export const TheWindowRollsForwardWithEveryRead: Story = {
  render: () => render(rolling.stub, undefined, FAST_POLL_MS),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findAllByText(fmt.number(132));
    for (const endpoint of ENDPOINTS) {
      await waitFor(() => expect(readsOf(rolling, endpoint)).toBeGreaterThan(1));
      const [first, second] = sinceOf(rolling, endpoint);
      await expect(second).toBeGreaterThan(first);
    }
    // and a read's `since` is 24 hours behind the moment it was sent, not
    // behind some earlier instant
    const sent = sinceOf(rolling, ENDPOINTS[0]);
    const behind = Date.now() - sent[sent.length - 1];
    await expect(behind).toBeGreaterThanOrEqual(DAY_MS);
    await expect(behind).toBeLessThan(DAY_MS + 10_000);
  },
};

const refreshed = recording(loaded);

/**
 * The header's refresh button still re-reads everything, polling or not: the
 * intervals here are the real ones (15s and a minute), so the second read of
 * each endpoint can only have come from the click, and it starts later too.
 */
export const TheHeaderRefreshStillReadsTheWindowAgain: Story = {
  render: () => (
    <MemoryRouter>
      <Harness fetchStub={refreshed.stub}>
        <ScreenHeader title={en.screens.dashboard.title} subtitle={en.screens.dashboard.subtitle} />
        <Dashboard />
      </Harness>
    </MemoryRouter>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.number(132));
    await waitFor(() => ENDPOINTS.forEach((e) => expect(readsOf(refreshed, e)).toBe(1)));
    // its name reads "refreshing" while anything is in flight, so finding it by
    // the idle name waits for the page to settle
    await userEvent.click(await canvas.findByRole("button", { name: en.shell.refreshData }));
    await waitFor(() => ENDPOINTS.forEach((e) => expect(readsOf(refreshed, e)).toBe(2)));
    for (const endpoint of ENDPOINTS) {
      const [first, second] = sinceOf(refreshed, endpoint);
      await expect(second).toBeGreaterThan(first);
    }
  },
};

const NEWER = {
  ...RECENT[0],
  ts: "2026-10-05T12:35:30.000Z",
  request_id: "req-2",
  model: "gemini-2.5-flash",
  status: 429,
};

// the log answers with one request, then with that one and a newer one
let recentReads = 0;
const arriving = recording(
  scoped(async (input) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (path === "/api/v1/analytics/invocations") {
      recentReads += 1;
      return json({ data: recentReads > 1 ? [NEWER, ...RECENT] : RECENT });
    }
    return loaded(input);
  }),
);

/**
 * #1975: the card says "Live", so a request that lands after the page opened
 * shows up without the header's refresh button. Nothing on the Dashboard
 * polled before, so the word described a card that never moved.
 */
export const TheLiveCardShowsNewRequestsByItself: Story = {
  beforeEach: () => {
    recentReads = 0;
  },
  render: () => render(arriving.stub, undefined, FAST_POLL_MS),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(en.pages.dashboard.live)).toBeVisible();
    await expect(canvas.queryByText("gemini-2.5-flash")).toBeNull();
    // no click: the poll alone brings the newer request in
    await expect(await canvas.findByText("gemini-2.5-flash")).toBeVisible();
    await expect(canvas.getByText(en.pages.dashboard.live)).toBeVisible();
  },
};

// refuses every analytics read, and leaves the rest of the control plane
// answering: the setup checklist reads its own lists and has its own error
const analyticsRefused = (input: RequestInfo | URL) =>
  new URL(String(input), "http://localhost").pathname.startsWith("/api/v1/analytics");

// answers well until the play says otherwise, so a story can watch a screen
// that loaded lose its analytics store and get it back
let upstream: "ok" | "failing" = "ok";
const flaky = recording(
  scoped(async (input, init) =>
    upstream === "failing" && analyticsRefused(input)
      ? json({ error: { message: "clickhouse refused" } }, 500)
      : loaded(input, init),
  ),
);

const REFRESH_FAILED = /^Refresh failed at .*, retrying$/;

/**
 * A poll that fails after the page loaded must not take the page down with it:
 * an alert in place of a card that had loaded would appear on every blip. The
 * figures and the charts stay, and each card says when its refresh failed, so
 * nothing on the screen goes stale with no sign of it (#1976). The Recent card
 * stops saying "Live" and says it in its label instead, and the next poll that
 * lands takes every line away again.
 */
export const AFailedPollKeepsWhatLoaded: Story = {
  beforeEach: () => {
    upstream = "ok";
  },
  render: () => render(flaky.stub, undefined, FAST_POLL_MS),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.number(132));
    await expect(await canvas.findByText(en.pages.dashboard.live)).toBeVisible();
    await expect(canvas.queryAllByText(REFRESH_FAILED)).toHaveLength(0);

    upstream = "failing";
    // five cards read four endpoints, and each says so for itself
    await waitFor(() => expect(canvas.getAllByText(REFRESH_FAILED)).toHaveLength(CARDS.length));
    for (const id of CARDS) {
      await expect(within(canvas.getByTestId(id)).getAllByText(REFRESH_FAILED)).toHaveLength(1);
    }
    // it is a failure, so it reads as one, and "Live" is not said over it
    for (const note of canvas.getAllByText(REFRESH_FAILED)) {
      await expect(getComputedStyle(note).color).toBe(resolveColorToken("--status-danger-text"));
    }
    await expect(canvas.queryByText(en.pages.dashboard.live)).toBeNull();
    // the page that loaded is still there: no alert, no skeleton, and the
    // figures, the chart, the donut, the bars and the rows are not blanked
    await expect(canvas.queryByRole("alert")).toBeNull();
    await expect(canvas.queryAllByLabelText(LOADING_LABEL)).toHaveLength(0);
    await expect(canvas.getAllByText(fmt.number(132))).not.toHaveLength(0);
    await expect(
      within(canvas.getByTestId("dashboard-spend")).getByRole("img", {
        name: en.pages.dashboard.spendChartAria,
      }),
    ).toBeVisible();
    await expect(canvas.getByTestId("dashboard-traffic")).toHaveTextContent(
      en.pages.dashboard.requests,
    );
    await expect(
      within(canvas.getByTestId("dashboard-by-model")).getByText("claude-sonnet-4"),
    ).toBeVisible();
    await expect(canvas.getByText("gpt-4o", { selector: "td" })).toBeVisible();

    // the poll goes on, so a control plane that comes back is noticed
    upstream = "ok";
    await expect(await canvas.findByText(en.pages.dashboard.live)).toBeVisible();
    await waitFor(() => expect(canvas.queryAllByText(REFRESH_FAILED)).toHaveLength(0));
  },
};

const failing = recording(
  scoped(async (input, init) =>
    analyticsRefused(input)
      ? json({ error: { message: "clickhouse refused" } }, 500)
      : loaded(input, init),
  ),
);

/**
 * A first read that fails stays failed until someone retries it. Polling a
 * query that never held data sends it back to pending on every refetch, which
 * unmounts its error: the alert and a skeleton would take turns, and a screen
 * reader would hear the alert again each cycle (#1984 found it on LLM Logs).
 *
 * Every read here fails, so the screen says it once (#2342): one alert in place
 * of the five cards, which used to announce five times on mount, two of them
 * about the same endpoint. The alert is held across three intervals with
 * nothing asked, and its one retry asks for each of the four reads.
 */
export const AFirstLoadThatFailsStopsPolling: Story = {
  render: () => render(failing.stub, undefined, FAST_POLL_MS),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /failed to return analytics/i);
    await waitFor(() => expect(canvas.getAllByRole("alert")).toHaveLength(1));
    const alert = canvas.getByRole("alert");
    // the control plane's own words stay under it
    await expect(within(alert).getByText("clickhouse refused")).toBeVisible();
    // no card is drawn around an error about every read, and none says it
    // found nothing
    for (const id of CARDS) await expect(canvas.queryByTestId(id)).toBeNull();
    await expectNoFalseEmpty(
      canvasElement,
      new RegExp(`${en.pages.dashboard.noTraffic}|${en.pages.dashboard.nothingLogged}`),
    );
    const reads = ENDPOINTS.map((e) => readsOf(failing, e));

    // three intervals later it is the same alert node, and nothing was asked: a
    // poll would have sent each query to pending and unmounted it
    await sleep(FAST_POLL_MS * 3);
    await expect(alert.isConnected).toBe(true);
    await expect(canvas.queryAllByLabelText(LOADING_LABEL)).toHaveLength(0);
    await expect(ENDPOINTS.map((e) => readsOf(failing, e))).toEqual(reads);

    // the screen has one retry, and it asks for every read, once
    await expect(canvas.getAllByRole("button", { name: "Try again" })).toHaveLength(1);
    await userEvent.click(within(alert).getByRole("button", { name: "Try again" }));
    await waitFor(() =>
      ENDPOINTS.forEach((e, i) => expect(readsOf(failing, e)).toBe(reads[i] + 1)),
    );
    // they fail again, and the screen goes back to saying it once
    await expectLoadError(canvasElement, /failed to return analytics/i);
    await waitFor(() => expect(canvas.getAllByRole("alert")).toHaveLength(1));
    for (const id of CARDS) await expect(canvas.queryByTestId(id)).toBeNull();
  },
};

// every read answers as `loaded` does, except the ones at `endpoints`, which
// `answer` decides: a story that wants one card down and the rest up names it.
// `recovered` lets a play bring that read back to ask whether a retry worked
const except = (
  endpoints: string[],
  answer: () => Response | Promise<Response>,
  recovered: () => boolean = () => false,
): FetchStub =>
  scoped(async (input, init) =>
    endpoints.includes(pathOf(input)) && !recovered() ? answer() : loaded(input, init),
  );

const never = () => new Promise<Response>(() => {});
const refused = () => json({ error: { message: "clickhouse refused" } }, 500);

/** what the four cards that did not fail still show */
async function expectTheOthersLoaded(canvasElement: HTMLElement, skip: string[]) {
  const canvas = within(canvasElement);
  const shown: Record<string, () => Promise<void>> = {
    "dashboard-figures": async () => {
      await expect(
        await within(canvas.getByTestId("dashboard-figures")).findByText(fmt.number(132)),
      ).toBeVisible();
    },
    "dashboard-spend": async () => {
      await expect(
        await within(canvas.getByTestId("dashboard-spend")).findByRole("img", {
          name: en.pages.dashboard.spendChartAria,
        }),
      ).toBeVisible();
    },
    "dashboard-traffic": async () => {
      await expect(
        await within(canvas.getByTestId("dashboard-traffic")).findByText(
          en.pages.dashboard.requests,
        ),
      ).toBeVisible();
    },
    "dashboard-by-model": async () => {
      await expect(
        await within(canvas.getByTestId("dashboard-by-model")).findByText("gpt-4o"),
      ).toBeVisible();
    },
    "dashboard-recent": async () => {
      await expect(
        await within(canvas.getByTestId("dashboard-recent")).findByText("gpt-4o", {
          selector: "td",
        }),
      ).toBeVisible();
    },
  };
  for (const id of CARDS) if (!skip.includes(id)) await shown[id]();
}

/**
 * While one read is out, its card stands in a skeleton and the rest of the
 * screen is up. The by-model bars used to read "No traffic yet." here, about a
 * read that had not answered (#1976), so each story also asserts that neither
 * the card nor the page says it has found nothing.
 */
export const TheFiguresAreStillLoading: Story = {
  render: () => render(except([ENDPOINTS[0]], never)),
  play: async ({ canvasElement }) => {
    const figures = within(canvasElement).getByTestId("dashboard-figures");
    await expectSkeleton(figures);
    await expectNoFalseEmpty(figures, new RegExp(en.pages.dashboard.noRequestsInWindow));
    await expectTheOthersLoaded(canvasElement, ["dashboard-figures"]);
    await expect(within(canvasElement).queryByRole("alert")).toBeNull();
  },
};

export const TheSpendChartIsStillLoading: Story = {
  render: () => render(except([ENDPOINTS[1]], never)),
  play: async ({ canvasElement }) => {
    const spend = within(canvasElement).getByTestId("dashboard-spend");
    await expectSkeleton(spend);
    await expect(within(spend).queryByText(en.analytics.noRowsYet)).toBeNull();
    await expectTheOthersLoaded(canvasElement, ["dashboard-spend"]);
    await expect(within(canvasElement).queryByRole("alert")).toBeNull();
  },
};

/** the donut and the bars read the same endpoint, and each holds its own skeleton */
export const TheTrafficCardsAreStillLoading: Story = {
  render: () => render(except([ENDPOINTS[2]], never)),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    for (const id of ["dashboard-traffic", "dashboard-by-model"]) {
      const card = canvas.getByTestId(id);
      await expectSkeleton(card);
      await expectNoFalseEmpty(card, new RegExp(en.pages.dashboard.noTraffic));
    }
    await expect(canvas.queryByText(en.pages.dashboard.noTraffic)).toBeNull();
    await expectTheOthersLoaded(canvasElement, ["dashboard-traffic", "dashboard-by-model"]);
    await expect(canvas.queryByRole("alert")).toBeNull();
  },
};

export const TheRecentRequestsAreStillLoading: Story = {
  render: () => render(except([ENDPOINTS[3]], never)),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const recent = canvas.getByTestId("dashboard-recent");
    await expectSkeleton(recent);
    await expectNoFalseEmpty(recent, new RegExp(en.pages.dashboard.nothingLogged));
    // the card is not live until a read has answered, so its label says what it
    // is doing instead (#2341)
    await expect(within(recent).getByText(en.pages.dashboard.feed.loading)).toBeVisible();
    await expect(canvas.queryByText(en.pages.dashboard.live)).toBeNull();
    await expectTheOthersLoaded(canvasElement, ["dashboard-recent"]);
    await expect(canvas.queryByRole("alert")).toBeNull();
  },
};

// the recent read, held until the play lets it go. every call is kept: a story
// that remounts asks again, and the read in flight is the last one
let recentHeld: Array<() => void> = [];
const holdingRecent = except(
  [ENDPOINTS[3]],
  () =>
    new Promise<Response>((resolve) => {
      recentHeld.push(() => resolve(json({ data: RECENT })));
    }),
);

/**
 * "Live" is a claim about a read that succeeded. The label said it from the
 * first paint, over a skeleton, because it asked whether the read had failed
 * and a read nobody has answered has not (#2341). It says "Loading" until the
 * first read lands, and "Live" once it has.
 */
export const TheRecentLabelSaysLiveOnlyAfterTheFirstRead: Story = {
  beforeEach: () => {
    recentHeld = [];
  },
  render: () => render(holdingRecent),
  play: async ({ canvasElement }) => {
    const recent = within(canvasElement).getByTestId("dashboard-recent");
    await expectSkeleton(recent);
    await waitFor(() => expect(recentHeld).not.toHaveLength(0));
    await expect(within(recent).getByText(en.pages.dashboard.feed.loading)).toBeVisible();
    await expect(within(recent).queryByText(en.pages.dashboard.live)).toBeNull();

    recentHeld.forEach((release) => release());
    await expect(await within(recent).findByText(en.pages.dashboard.live)).toBeVisible();
    await expect(within(recent).queryByText(en.pages.dashboard.feed.loading)).toBeNull();
    await expect(within(recent).getByText("gpt-4o", { selector: "td" })).toBeVisible();
  },
};

let spendDown = true;
const partial = recording(except([ENDPOINTS[1]], refused, () => !spendDown));

/**
 * One failing read takes down one card. The spend chart's endpoint answers
 * 500; the figures, the donut, the bars and the recent rows stay up instead of
 * giving way to a whole-screen alert (#1976). The card's own alert names what
 * it could not read, carries its own retry, and says nothing about an empty
 * window. The retry asks for the spend series and nothing else.
 */
export const OneFailedCardLeavesTheRestUp: Story = {
  beforeEach: () => {
    spendDown = true;
  },
  render: () => render(partial.stub),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const spend = canvas.getByTestId("dashboard-spend");
    await expectLoadError(spend, /failed to return hourly spend/i);
    await expectNoFalseEmpty(spend, /No requests logged in this window/);
    await expectTheOthersLoaded(canvasElement, ["dashboard-spend"]);
    // the screen has one alert, the failed card's
    await expect(canvas.getAllByRole("alert")).toHaveLength(1);

    const reads = ENDPOINTS.map((e) => readsOf(partial, e));
    spendDown = false;
    await userEvent.click(within(spend).getByRole("button", { name: "Try again" }));
    await expect(
      await within(spend).findByRole("img", { name: en.pages.dashboard.spendChartAria }),
    ).toBeVisible();
    await expect(canvas.queryByRole("alert")).toBeNull();
    // the retry asked for the spend series once, and for nothing else
    await expect(readsOf(partial, ENDPOINTS[1])).toBe(reads[1] + 1);
    for (const i of [0, 2, 3]) await expect(readsOf(partial, ENDPOINTS[i])).toBe(reads[i]);
  },
};

const recentDown = recording(except([ENDPOINTS[3]], refused));

/**
 * A failed recent-invocations read holds no rows, exactly as an empty one does,
 * and the card said "Nothing logged yet." under a label that said the load had
 * failed (#1976). It is an alert with a retry now, and the label still says when.
 */
export const AFailedRecentReadIsNotAnEmptyOne: Story = {
  render: () => render(recentDown.stub),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const recent = canvas.getByTestId("dashboard-recent");
    await expectLoadError(recent, /failed to return recent requests/i);
    await expectNoFalseEmpty(recent, new RegExp(en.pages.dashboard.nothingLogged));
    await expect(within(recent).getByText(/^Load failed at /)).toBeVisible();
    await expect(within(recent).queryByText(en.pages.dashboard.live)).toBeNull();
    await expectTheOthersLoaded(canvasElement, ["dashboard-recent"]);
    await expect(canvas.getAllByRole("alert")).toHaveLength(1);
  },
};

let modelsDown = true;
const byModelDown = recording(except([ENDPOINTS[2]], refused, () => !modelsDown));

/**
 * The donut and the bars read one endpoint, so one failure of it is one alert.
 * The traffic share holds the error and its retry; requests by model says it
 * reads the same data and where to retry, in a plain sentence with no alert role
 * and no button of its own (#2342). Neither says "No traffic yet." about a read
 * that failed, and the retry asks for that read once and brings both back.
 */
export const TheTwoCardsOnOneReadShareOneAlert: Story = {
  beforeEach: () => {
    modelsDown = true;
  },
  render: () => render(byModelDown.stub),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const traffic = canvas.getByTestId("dashboard-traffic");
    const bars = canvas.getByTestId("dashboard-by-model");
    await expectLoadError(traffic, /failed to return the traffic share/i);
    await expect(canvas.getAllByRole("alert")).toHaveLength(1);
    await expect(within(bars).queryByRole("alert")).toBeNull();
    await expect(within(bars).queryByRole("button", { name: "Try again" })).toBeNull();
    // the sentence names the card that holds the alert and the button to use
    const shared = en.pages.dashboard.sharedRead
      .replace("{{card}}", en.pages.dashboard.trafficTitle)
      .replace("{{retry}}", en.errors.load.retry);
    await expect(within(bars).getByText(shared)).toBeVisible();
    await expect(canvas.queryByText(en.pages.dashboard.noTraffic)).toBeNull();
    await expectTheOthersLoaded(canvasElement, ["dashboard-traffic", "dashboard-by-model"]);

    const reads = readsOf(byModelDown, ENDPOINTS[2]);
    modelsDown = false;
    await userEvent.click(within(traffic).getByRole("button", { name: "Try again" }));
    await expect(await within(bars).findByText("claude-sonnet-4")).toBeVisible();
    await expect(await within(traffic).findByText(en.pages.dashboard.requests)).toBeVisible();
    await expect(canvas.queryByRole("alert")).toBeNull();
    await expect(canvas.queryByText(shared)).toBeNull();
    await expect(readsOf(byModelDown, ENDPOINTS[2])).toBe(reads + 1);
  },
};

// the figures, the spend chart and the recent rows fail; the by-model read
// answers. three of four is still a partial failure
const mostDown = recording(except([ENDPOINTS[0], ENDPOINTS[1], ENDPOINTS[3]], refused));

/**
 * The screen-level alert is for every read failing, not most of them. With one
 * read still answering, each failed card keeps its own alert and its own retry
 * (#2343), and the card that loaded stays up. A retry asks for its own read.
 */
export const SeveralFailedCardsEachHoldTheirOwnAlert: Story = {
  render: () => render(mostDown.stub),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const failed = ["dashboard-figures", "dashboard-spend", "dashboard-recent"];
    await expectLoadError(
      canvas.getByTestId("dashboard-figures"),
      /failed to return the overview figures/i,
    );
    await expectLoadError(canvas.getByTestId("dashboard-spend"), /failed to return hourly spend/i);
    await expectLoadError(
      canvas.getByTestId("dashboard-recent"),
      /failed to return recent requests/i,
    );
    await expect(canvas.getAllByRole("alert")).toHaveLength(failed.length);
    for (const id of failed) {
      await expect(
        within(canvas.getByTestId(id)).getAllByRole("button", { name: "Try again" }),
      ).toHaveLength(1);
    }
    await expect(canvas.queryByText(/failed to return analytics/i)).toBeNull();
    await expectTheOthersLoaded(canvasElement, failed);

    // each retry asks for its own read
    const reads = ENDPOINTS.map((e) => readsOf(mostDown, e));
    await userEvent.click(
      within(canvas.getByTestId("dashboard-spend")).getByRole("button", { name: "Try again" }),
    );
    await waitFor(() => expect(readsOf(mostDown, ENDPOINTS[1])).toBe(reads[1] + 1));
    for (const i of [0, 2, 3]) await expect(readsOf(mostDown, ENDPOINTS[i])).toBe(reads[i]);
  },
};

// the figures, the spend chart and the recent rows fail while the by-model read
// has not answered yet
const lastOneOut = recording(
  scoped(async (input, init) =>
    [ENDPOINTS[0], ENDPOINTS[1], ENDPOINTS[3]].includes(pathOf(input))
      ? refused()
      : pathOf(input) === ENDPOINTS[2]
        ? never()
        : loaded(input, init),
  ),
);

/**
 * A read that has not answered is not a failure, so the screen is not yet an
 * outage: the three that failed each hold their own alert, and the two cards on
 * the read still out stand in skeletons rather than saying anything about it.
 */
export const AReadStillOutIsNotYetAnOutage: Story = {
  render: () => render(lastOneOut.stub),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(
      canvas.getByTestId("dashboard-figures"),
      /failed to return the overview figures/i,
    );
    await waitFor(() => expect(canvas.getAllByRole("alert")).toHaveLength(3));
    for (const id of ["dashboard-traffic", "dashboard-by-model"]) {
      await expectSkeleton(canvas.getByTestId(id));
    }
    await expect(canvas.queryByText(/failed to return analytics/i)).toBeNull();
    await expect(canvas.queryByText(en.pages.dashboard.noTraffic)).toBeNull();
  },
};

// the spend chart's read never recovers, and the others answer every poll
const cardDown = recording(except([ENDPOINTS[1]], refused));

/**
 * The no-polling rule belongs to the read, not to the screen: the spend chart
 * failed holding nothing, so it stops asking and keeps its alert, while the
 * figures, the donut, the bars and the rows go on refreshing around it.
 */
export const AFailedCardHoldsItsAlertWhileTheOthersKeepPolling: Story = {
  render: () => render(cardDown.stub, undefined, FAST_POLL_MS),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const spend = canvas.getByTestId("dashboard-spend");
    await expectLoadError(spend, /failed to return hourly spend/i);
    const alert = within(spend).getByRole("alert");
    const reads = ENDPOINTS.map((e) => readsOf(cardDown, e));

    await sleep(FAST_POLL_MS * 3);
    await expect(alert.isConnected).toBe(true);
    await expect(readsOf(cardDown, ENDPOINTS[1])).toBe(reads[1]);
    for (const i of [0, 2, 3]) {
      await expect(readsOf(cardDown, ENDPOINTS[i])).toBeGreaterThan(reads[i]);
    }
    await expect(canvas.getAllByRole("alert")).toHaveLength(1);
    await expectTheOthersLoaded(canvasElement, ["dashboard-spend"]);
  },
};

const euro = scoped(async (input, init) =>
  pathOf(input) === "/api/v1/currency"
    ? json({ base: "EUR", codes: ["EUR"], rates: {} })
    : loaded(input, init),
);

/**
 * The amounts follow the deployment's currency, and so does the line under the
 * chart's title. It said "USD" while the figures were in euros, the mislabel
 * #1182 fixed on the other screens (#1976).
 */
export const TheSpendSubtitleNamesTheDeploymentCurrency: Story = {
  render: () => render(euro),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const spend = canvas.getByTestId("dashboard-spend");
    await expect(await within(spend).findByText("Hourly gateway spend, EUR")).toBeVisible();
    await expect(within(spend).queryByText(/USD/)).toBeNull();
    await expect(await canvas.findByText(fmt.currency(41.27, "EUR"))).toBeVisible();
  },
};

const unconfigured = recording(
  scoped(async (input, init) =>
    pathOf(input).startsWith("/api/v1/analytics")
      ? json({ error: { message: "no clickhouse_url" } }, 503)
      : quiet(input, init),
  ),
);

/**
 * The panel holds still: a read that answered "no analytics" never held data, so
 * it stops polling, and the panel is not swapped for a skeleton and back on each
 * interval (#1984 found the same on LLM Logs).
 */
export const TheNoAnalyticsPanelHoldsStill: Story = {
  render: () => render(unconfigured.stub, undefined, FAST_POLL_MS),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const title = await canvas.findByText(en.pages.dashboard.noAnalytics.title);
    const panel = title.closest('[role="status"]') as HTMLElement;
    const reads = ENDPOINTS.map((e) => readsOf(unconfigured, e));
    await sleep(FAST_POLL_MS * 3);
    await expect(panel.isConnected).toBe(true);
    await expect(ENDPOINTS.map((e) => readsOf(unconfigured, e))).toEqual(reads);
  },
};
