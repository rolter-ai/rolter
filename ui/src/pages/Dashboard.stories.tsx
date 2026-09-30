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
  play: async ({ canvasElement }) => expectSkeleton(canvasElement),
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
 * Analytics off is a load state, not an empty one: the `EmptyState` this used
 * to render said "nothing has happened yet" about a control plane that was
 * never asked to record anything (#1236).
 */
export const NoAnalyticsStore: Story = {
  render: () =>
    render(
      scoped(async (input) =>
        String(input).includes("/api/v1/analytics")
          ? json({ error: { message: "no clickhouse_url" } }, 503)
          : json({ base: "USD", codes: ["USD"], rates: {} }),
      ),
    ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/Analytics are not configured/i)).toBeVisible();
    await expect(canvas.getByText(/CLICKHOUSE_URL/)).toBeVisible();
    await expect(canvas.queryByRole("button", { name: /try again/i })).toBeNull();
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

/**
 * A poll that fails after the page loaded must not take the page down with it:
 * the alert that replaces the screen when the first read fails would otherwise
 * appear on every blip. The figures stay, the card stops saying "Live" and
 * says when the refresh failed instead, and the next poll that lands brings the
 * word back.
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

    upstream = "failing";
    const failed = await canvas.findByText(/^Refresh failed at .*, retrying$/);
    await expect(failed).toBeVisible();
    // it is a failure, so it reads as one, and "Live" is not said over it
    await expect(getComputedStyle(failed).color).toBe(resolveColorToken("--status-danger-text"));
    await expect(canvas.queryByText(en.pages.dashboard.live)).toBeNull();
    // the page that loaded is still there: no alert, and the figures are not blanked
    await expect(canvas.queryByRole("alert")).toBeNull();
    await expect(canvas.getAllByText(fmt.number(132))).not.toHaveLength(0);
    await expect(canvas.getByText("gpt-4o", { selector: "td" })).toBeVisible();

    // the poll goes on, so a control plane that comes back is noticed
    upstream = "ok";
    await expect(await canvas.findByText(en.pages.dashboard.live)).toBeVisible();
    await expect(canvas.queryByText(/^Refresh failed at /)).toBeNull();
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
 */
export const AFirstLoadThatFailsStopsPolling: Story = {
  render: () => render(failing.stub, undefined, FAST_POLL_MS),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /failed to return analytics/i);
    const alert = canvas
      .getAllByRole("alert")
      .find((a) => /failed to return analytics/i.test(a.textContent ?? ""));
    const reads = ENDPOINTS.map((e) => readsOf(failing, e));

    // three intervals later it is the same alert node, and nothing was asked:
    // a poll would have sent the query to pending and unmounted it
    await sleep(FAST_POLL_MS * 3);
    await expect(alert?.isConnected).toBe(true);
    await expect(canvas.queryAllByLabelText(LOADING_LABEL)).toHaveLength(0);
    await expect(ENDPOINTS.map((e) => readsOf(failing, e))).toEqual(reads);

    // the retry the alert offers asks all four again, the recent log included
    await userEvent.click(within(alert!).getByRole("button", { name: "Try again" }));
    await waitFor(() =>
      ENDPOINTS.forEach((e, i) => expect(readsOf(failing, e)).toBeGreaterThan(reads[i])),
    );
  },
};
