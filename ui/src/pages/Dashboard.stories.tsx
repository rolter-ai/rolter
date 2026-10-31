import type { Meta, StoryObj } from "@storybook/react-vite";
import { MemoryRouter, useLocation } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Dashboard from "./Dashboard";
import {
  BY_MODEL,
  CLUSTER_SUMMARY,
  RECENT,
  SERIES,
  SUMMARY,
  recentRow,
} from "./dashboard-fixtures";
import { ScreenHeader } from "@/components/ScreenHeader";
import {
  Harness,
  LOADING_LABEL,
  expectAnalyticsUnavailable,
  expectGateAnswered,
  expectLoadError,
  expectNoFalseEmpty,
  expectSkeleton,
  expectUxEvent,
  json,
  pending,
  recordUxEvents,
  recording,
  routes,
  scoped,
  StaleSession,
  uxEvents,
  withSampleRate,
  type FetchStub,
  type Recorder,
  type StoryRole,
  AddressProbe,
  addressOf,
  ORG,
} from "./story-harness";
import { formattersFor } from "@/lib/i18n/format";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import type { UptimeRow } from "@/lib/api";
import { atMobile, atTablet, expectNoHorizontalOverflow } from "@/lib/story-viewport";
import { resolveColorToken } from "@/lib/story-tokens";
import { UxScreenProvider } from "@/lib/ux-react";

const fmt = formattersFor("en");

/** the spend chart's accessible name, over the window the screen opens on */
const SPEND_CHART = en.pages.dashboard.spendChartAria.replace(
  "{{window}}",
  en.common.timeWindow.last24h,
);

/** a summary read of the window before the one on screen: it is the one with an `until` (#2010) */
/** a provider-grain row of the health rollup: `uptime` of 1 is a provider that never failed */
const uptimeRow = (provider: string, uptime: number, breached = false): UptimeRow => ({
  provider,
  target_id: provider,
  grain: "provider",
  sources: ["probe"],
  events: 1000,
  ok: Math.round(uptime * 1000),
  errors: Math.round((1 - uptime) * 1000),
  timeouts: 0,
  uptime,
  failure_rate: 1 - uptime,
  error_budget_burn: breached ? 4 : 0.1,
  sla_breached: breached ? 1 : 0,
  last_event: "2026-08-06T10:00:00Z",
});
const UPTIME: UptimeRow[] = [uptimeRow("openai-prod", 0.9995), uptimeRow("anthropic-prod", 0.9991)];

const isPreviousWindow = (input: RequestInfo | URL) => {
  const url = new URL(String(input), "http://localhost");
  return url.pathname === "/api/v1/analytics/summary" && url.searchParams.has("until");
};

/**
 * The screen's reads answered with `summary`. The window before answers
 * `previous`, and by default holds no traffic, so a tile has nothing to
 * compare with and draws no arrow: the case of a deployment's first day.
 */
const loadedWith = (summary: typeof SUMMARY, previous?: typeof SUMMARY): FetchStub => {
  const current = routes([
    ["/api/v1/analytics/summary", () => ({ data: [summary] })],
    ["/api/v1/analytics/timeseries", () => ({ data: SERIES })],
    ["/api/v1/analytics/by-model", () => ({ data: BY_MODEL })],
    ["/api/v1/analytics/invocations", () => ({ data: RECENT })],
    ["/api/v1/cluster/summary", () => CLUSTER_SUMMARY],
    ["/api/v1/health/uptime", () => ({ data: UPTIME })],
    ["/api/v1/currency", () => ({ base: "USD", codes: ["USD"], rates: {} })],
  ]);
  return scoped(async (input, init) =>
    isPreviousWindow(input) ? json({ data: previous ? [previous] : [] }) : current(input, init),
  );
};

const loaded = loadedWith(SUMMARY);

/** `loaded`, with the by-model read answering `rows` and the recent read `recent` */
const loadedAnswering = (answers: { byModel?: unknown[]; recent?: unknown[] }): FetchStub =>
  scoped(async (input, init) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (answers.byModel && path === "/api/v1/analytics/by-model") {
      return json({ data: answers.byModel });
    }
    if (answers.recent && path === "/api/v1/analytics/invocations") {
      return json({ data: answers.recent });
    }
    return loaded(input, init);
  });
const withRecent = (recent: unknown[]) => loadedAnswering({ recent });
const withByModel = (byModel: unknown[]) => loadedAnswering({ byModel });

/** a by-model row: the fixture's, for `model` and `requests` */
const modelRow = (model: string, requests: number, cost_usd = 1) => ({
  ...BY_MODEL[0],
  model,
  requests,
  cost_usd,
});

// a long model name, a 429 and a request from before midnight: what makes the
// recent requests widest, at the width a phone has
const MOBILE_ROWS = [
  recentRow("req-m1", "claude-sonnet-4-20250514", 0, { latency_ms: 1530 }),
  recentRow("req-m2", "gemini-2.5-flash-lite", 3, { status: 429, latency_ms: 88 }),
  recentRow("req-m3", "gpt-4o", 30 * 60, { latency_ms: 842 }),
];

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
const CARDS = ["dashboard-figures", "dashboard-spend", "dashboard-by-model", "dashboard-recent"];

/**
 * the tile a figure is drawn in: its label sits in the tile's own element. read
 * within the figures, since "Requests" is also a column of the by-model table
 */
const labelOf = (canvas: ReturnType<typeof within>, label: string): HTMLElement =>
  within(canvas.getByTestId("dashboard-figures")).getByText(label);
const tile = (canvas: ReturnType<typeof within>, label: string): HTMLElement =>
  labelOf(canvas, label).closest("div") as HTMLElement;

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
    await expect(await canvas.findAllByText(fmt.number(132))).not.toHaveLength(0);

    // 7 errors in 132 requests is 5.30%, over the 1% line, so the count reads
    // in the danger text colour. it used to come out success green under an
    // up arrow, because the arrow picked the colour (#1974)
    const errors = await canvas.findByText(errorCount(7));
    await expect(getComputedStyle(errors).color).toBe(resolveColorToken("--status-danger-text"));
    await expect(getComputedStyle(errors).color).not.toBe(
      resolveColorToken("--status-success-text"),
    );
    // the window before held no traffic, so there is nothing to compare with
    // and the tile draws no arrow: it would claim a movement nobody measured
    await expect(errors.querySelector("svg")).toBeNull();

    // with requests to measure, the latency tile is the 95th percentile and not
    // the mean (214.6 ms here): rounded, in milliseconds, and no reason line
    const latency = tile(canvas, en.pages.dashboard.statP95);
    await expect(latency).toHaveTextContent(`${fmt.number(980)}${en.pages.dashboard.colMs}`);
    await expect(latency).not.toHaveTextContent(fmt.number(215));
    await expect(latency).not.toHaveTextContent(en.pages.dashboard.noRequestsInWindow);

    // the cards carry the one frame: the recent requests table inside its card
    // draws none of its own, which was a second hairline 24px inside the first
    const recent = canvas.getByTestId("dashboard-recent");
    await expect(getComputedStyle(recent).borderTopWidth).toBe("1px");
    const table = within(recent).getByRole("table");
    await expect(getComputedStyle(table.closest("[tabindex]")!).borderTopWidth).toBe("0px");

    // a card title is the Headline tier, 18px at 600, the size the screen title
    // and "Getting started" use. they were 16px, a size the design does not have
    const headline = getComputedStyle(canvas.getByText(en.pages.gettingStarted.title));
    await expect(headline.fontSize).toBe("18px");
    for (const id of CARDS.filter((card) => card !== "dashboard-figures")) {
      const title = getComputedStyle(within(canvas.getByTestId(id)).getByRole("heading"));
      await expect(title.fontSize).toBe(headline.fontSize);
      await expect(title.fontWeight).toBe(headline.fontWeight);
    }
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
      getComputedStyle(labelOf(canvas, en.pages.dashboard.statErrorRate)).color,
    );
    await expect(getComputedStyle(errors).color).not.toBe(
      resolveColorToken("--status-danger-text"),
    );
    await expect(errors.querySelector("svg")).toBeNull();
  },
};

/**
 * #1978: the cache-hit rate is the share of the window's requests a cache
 * answered, from the summary's weighted `cache_hit_requests`. 33 of 132 is a
 * quarter, and the line under it counts the hits.
 */
export const TheCacheHitRateIsTheShareOfRequestsACacheAnswered: Story = {
  render: () => render(loaded),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const hits = await waitFor(() => {
      const found = tile(canvas, en.pages.dashboard.statCacheHit);
      expect(found).toHaveTextContent(`${fmt.number(25, { minimumFractionDigits: 1 })}%`);
      return found;
    });
    await expect(hits).toHaveTextContent(
      en.pages.dashboard.cacheHits_other.replace("{{count}}", "33"),
    );
  },
};

/**
 * No response cache on a deployment reads as 0.0% beside a line saying nothing
 * came from the response cache, not as a failure. The tile measures rolter's
 * response cache; provider prompt caching is not in it.
 */
export const ADeploymentWithoutAResponseCacheSaysNoneCameFromIt: Story = {
  render: () => render(loadedWith({ ...SUMMARY, cache_hit_requests: 0 })),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const hits = await waitFor(() => {
      const found = tile(canvas, en.pages.dashboard.statCacheHit);
      expect(found).toHaveTextContent(`${fmt.number(0, { minimumFractionDigits: 1 })}%`);
      return found;
    });
    await expect(hits).toHaveTextContent(en.pages.dashboard.cacheHitsNone);
    await expect(hits).not.toHaveTextContent(/0 cache hits/);
  },
};

/** More hits than the window before is good news, in the success colour, with an arrow. */
export const TheCacheHitRateIsComparedWithThePreviousWindow: Story = {
  render: () => render(loadedWith(SUMMARY, { ...SUMMARY, requests: 100, cache_hit_requests: 20 })),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const delta = await waitFor(() => deltaOf(canvas, en.pages.dashboard.statCacheHit));
    // 25% against 20% is a quarter more
    await expect(delta).toHaveTextContent(vsPrevious("+25%"));
    await expect(getComputedStyle(delta).color).toBe(resolveColorToken("--status-success-text"));
  },
};

/**
 * #3356: the prompt-cache read share is the provider's prompt cache, apart from
 * the response-cache tile beside it: 360k of the 900k prompt tokens is 40%, the
 * line under it counts them, and it is compared with the window before.
 */
export const ThePromptCacheReadShareIsTheShareOfPromptTokensServedFromTheProvidersCache: Story = {
  render: () => render(loadedWith(SUMMARY, { ...SUMMARY, cache_read_tokens: 180_000 })),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const share = await waitFor(() => {
      const found = canvas.getByTestId("dashboard-prompt-cache");
      expect(found).toHaveTextContent(`${fmt.number(40, { minimumFractionDigits: 1 })}%`);
      return found;
    });
    await expect(share).toHaveTextContent("360K of 900K prompt tokens");
    // it doubled against the window before, and more of the prompt cached is good news
    await expect(share).toHaveTextContent(vsPrevious("+100%"));
    // and it is not the response-cache tile: that one still counts requests
    await expect(tile(canvas, en.pages.dashboard.statCacheHit)).toHaveTextContent("25.0%");
  },
};

/**
 * A window in which no provider reported a cached token is not a 0.0% hit rate:
 * a provider that does not report it looks the same as one that never hits, so
 * the tile reads "—" and says why.
 */
export const NoProviderReportingCachedTokensReadsUnmeasured: Story = {
  render: () => render(loadedWith({ ...SUMMARY, cache_read_tokens: 0 })),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const share = await waitFor(() => {
      const found = canvas.getByTestId("dashboard-prompt-cache");
      expect(found).toHaveTextContent(en.pages.dashboard.promptCacheNone);
      return found;
    });
    await expect(share).toHaveTextContent(en.pages.dashboard.notMeasured);
    await expect(share).not.toHaveTextContent("%");
  },
};

/**
 * #3410: a fleet that mixes providers which state their cache reads with ones
 * that do not. The share is over the prompt tokens of the requests that
 * reported it, 360k of 600k is 60%, and the line says how many requests it
 * could not measure.
 */
export const ThePromptCacheReadShareLeavesOutProvidersThatDoNotReportIt: Story = {
  render: () =>
    render(
      loadedWith({
        ...SUMMARY,
        cache_reported_prompt_tokens: 600_000,
        cache_unmeasured_requests: 12,
      } as typeof SUMMARY),
    ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const share = await waitFor(() => {
      const found = canvas.getByTestId("dashboard-prompt-cache");
      expect(found).toHaveTextContent(`${fmt.number(60, { minimumFractionDigits: 1 })}%`);
      return found;
    });
    await expect(share).toHaveTextContent("360K of 600K prompt tokens");
    await expect(share).toHaveTextContent("12 requests could not be measured");
  },
};

/**
 * #3410: providers that state their cache reads and never hit are a measured
 * 0.0%, not the "—" a window with no report at all reads.
 */
export const ReportingProvidersThatNeverHitReadZero: Story = {
  render: () =>
    render(
      loadedWith({
        ...SUMMARY,
        cache_read_tokens: 0,
        cache_reported_prompt_tokens: 900_000,
        cache_unmeasured_requests: 0,
      } as typeof SUMMARY),
    ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const share = await waitFor(() => {
      const found = canvas.getByTestId("dashboard-prompt-cache");
      expect(found).toHaveTextContent(`${fmt.number(0, { minimumFractionDigits: 1 })}%`);
      return found;
    });
    await expect(share).toHaveTextContent("0 of 900K prompt tokens");
    await expect(share).not.toHaveTextContent(en.pages.dashboard.promptCacheNone);
    await expect(share).not.toHaveTextContent(en.pages.dashboard.notMeasured);
  },
};

/** An older control plane sends no `cache_read_tokens`, and the tile says it is not measured. */
export const AnOlderControlPlaneLeavesThePromptCacheShareUnmeasured: Story = {
  render: () => {
    const { cache_read_tokens: _read, ...older } = SUMMARY;
    return render(loadedWith(older as typeof SUMMARY));
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.number(132));
    const share = canvas.getByTestId("dashboard-prompt-cache");
    await expect(share).toHaveTextContent(en.pages.dashboard.notMeasured);
    await expect(share).not.toHaveTextContent(en.pages.dashboard.promptCacheNone);
  },
};

/** A window with no requests gives the tile the reason every other tile gives. */
export const ANoTrafficWindowGivesThePromptCacheTileItsReason: Story = {
  render: () => render(loadedWith(QUIET_SUMMARY)),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const share = await waitFor(() => {
      const found = canvas.getByTestId("dashboard-prompt-cache");
      expect(found).toHaveTextContent(en.pages.dashboard.noRequestsInWindow);
      return found;
    });
    await expect(share).toHaveTextContent(en.pages.dashboard.notMeasured);
  },
};

export const ThePromptCacheTileInRussian: Story = {
  globals: { locale: "ru" },
  render: () => render(loaded),
  play: async ({ canvasElement }) => {
    const share = await waitFor(() => {
      const found = within(canvasElement).getByTestId("dashboard-prompt-cache");
      expect(found).toHaveTextContent(ru.pages.dashboard.statPromptCache);
      return found;
    });
    await expect(share).toHaveTextContent("40,0");
    await expect(share).toHaveTextContent("токенов промпта");
  },
};

/**
 * A control plane older than #1978 sends neither a p95 nor a cache-hit count.
 * The tiles say they are not measured rather than showing the mean as a p95 or a
 * rate of zero.
 */
export const AnOlderControlPlaneLeavesP95AndCacheHitsUnmeasured: Story = {
  render: () => {
    const { cache_hit_requests: _hits, p95_latency_ms: _p95, ...older } = SUMMARY;
    return render(loadedWith(older as typeof SUMMARY));
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.number(132));
    for (const label of [en.pages.dashboard.statP95, en.pages.dashboard.statCacheHit]) {
      const figure = tile(canvas, label);
      await expect(within(figure).getByText(en.pages.dashboard.notMeasured)).toBeVisible();
      await expect(figure).not.toHaveTextContent(/%|\bms\b/);
    }
    await expect(canvas.queryByText(fmt.number(215))).toBeNull();
  },
};

/** the same stub with the health read answered by `answer` */
const withHealth = (answer: () => Response | Promise<Response>): FetchStub =>
  scoped(async (input, init) =>
    pathOf(input) === "/api/v1/health/uptime" ? answer() : loaded(input, init),
  );

/**
 * #3344: one tile says how many providers meet the SLA of those that reported,
 * names the worst, and links to Health; the read is the rollup the Health screen
 * shares, not one per provider.
 */
export const TheProviderTileCountsTheProvidersMeetingTheirSla: Story = {
  render: () =>
    render(
      withHealth(() =>
        json({
          data: [
            uptimeRow("openai-prod", 0.9995),
            uptimeRow("azure-west", 0.96, true),
            uptimeRow("bedrock-east", 0.9, true),
          ],
        }),
      ),
    ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const providers = await canvas.findByTestId("dashboard-providers");
    await waitFor(() => expect(providers).toHaveTextContent(`${fmt.number(1)}of 3 up`));
    // the worst provider is named with its uptime, and the others counted
    await expect(providers).toHaveTextContent("Below the SLA: bedrock-east 90.00%");
    await expect(providers).toHaveTextContent("and 1 more");
    await expect(within(providers).getByRole("link", { name: "Providers" })).toHaveAttribute(
      "href",
      "/circuit-breaker",
    );
    // part of the fleet is down: amber, and not the red of "nothing is up"
    await expect(getComputedStyle(within(providers).getByText(/Below the SLA/)).color).toBe(
      resolveColorToken("--status-warning-text"),
    );
  },
};

export const EveryProviderWithinItsSlaReadsGood: Story = {
  render: () => render(loaded),
  play: async ({ canvasElement }) => {
    const providers = await within(canvasElement).findByTestId("dashboard-providers");
    await waitFor(() => expect(providers).toHaveTextContent("2of 2 up"));
    await expect(providers).toHaveTextContent("Lowest uptime: anthropic-prod 99.91%");
    await expect(getComputedStyle(within(providers).getByText(/Lowest uptime/)).color).toBe(
      resolveColorToken("--status-success-text"),
    );
  },
};

export const NoProviderHasReportedHealthYet: Story = {
  render: () => render(withHealth(() => json({ data: [] }))),
  play: async ({ canvasElement }) => {
    const providers = await within(canvasElement).findByTestId("dashboard-providers");
    await expect(providers).toHaveTextContent(en.pages.dashboard.providers.none);
    await expect(providers).toHaveTextContent(en.pages.dashboard.notMeasured);
    // nothing reported is not an outage: no figure, and no red
    await expect(providers).not.toHaveTextContent("of 0");
  },
};

export const TheProviderTileIsASkeletonWhileTheRollupLoads: Story = {
  render: () => render(withHealth(() => new Promise<Response>(() => {}))),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the rest of the strip is up: the slow read holds back one tile only
    await expect(await canvas.findAllByText(fmt.number(132))).not.toHaveLength(0);
    await expect(canvas.queryByTestId("dashboard-providers")).toBeNull();
    await expect(
      within(canvas.getByTestId("dashboard-figures")).getAllByLabelText(LOADING_LABEL),
    ).not.toHaveLength(0);
  },
};

export const AFailedProviderReadIsTheTilesOwnError: Story = {
  render: () => render(withHealth(() => json({ error: { message: "boom" } }, 500))),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /provider health/i);
    // the other figures are not taken down with it
    await expect(await canvas.findAllByText(fmt.number(132))).not.toHaveLength(0);
    await expectNoFalseEmpty(canvasElement, /No provider has reported health yet/);
  },
};

export const ProviderHealthNeedsClickHouse: Story = {
  render: () => render(withHealth(() => json({ error: { message: "no clickhouse_url" } }, 503))),
  play: async ({ canvasElement }) => {
    await expectAnalyticsUnavailable(
      canvasElement,
      en.pages.dashboard.providers.noAnalytics.title,
      "no clickhouse_url",
    );
  },
};

export const TheProviderTileInRussian: Story = {
  globals: { locale: "ru" },
  render: () => render(loaded),
  play: async ({ canvasElement }) => {
    const providers = await within(canvasElement).findByTestId("dashboard-providers");
    await waitFor(() => expect(providers).toHaveTextContent("из 2 работают"));
    await expect(providers).toHaveTextContent("Наименьшая доступность");
  },
};

/**
 * #1978, #3263: the fleet leads the strip. Three live gateways, all on the
 * control plane's config, read as the count with "live" after it and a line
 * saying none is behind.
 */
export const TheFleetTileCountsTheLiveGateways: Story = {
  render: () => render(loaded),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const fleet = await canvas.findByTestId("dashboard-fleet");
    await waitFor(() =>
      expect(fleet).toHaveTextContent(`${fmt.number(3)}${en.pages.dashboard.fleet.liveUnit}`),
    );
    await expect(fleet).toHaveTextContent(en.pages.dashboard.fleet.allCurrent);
    // the first tile of the strip: it is read before any figure of the window
    await expect(canvas.getByTestId("dashboard-figures").firstElementChild?.firstElementChild).toBe(
      fleet,
    );
  },
};

/** the delta line under a figure, which the arrow sits in */
const deltaOf = (canvas: ReturnType<typeof within>, label: string): HTMLElement =>
  tile(canvas, label).querySelector("svg")?.parentElement as HTMLElement;

const vsPrevious = (change: string) => en.pages.dashboard.vsPrevious.replace("{{change}}", change);

/**
 * #2010: each figure is compared with the window before. 132 requests against
 * 100 is a 32% rise, good news in the success colour; latency down from 250ms
 * is good news too; an error rate up from 2% is bad news; and spend rising
 * with traffic is neither, so it stays neutral.
 */
export const ComparesWithThePreviousWindow: Story = {
  render: () =>
    render(
      loadedWith(SUMMARY, {
        ...SUMMARY,
        requests: 100,
        errors: 2,
        p95_latency_ms: 1200,
        cost_usd: 30,
      }),
    ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const d = en.pages.dashboard;
    const requests = await waitFor(() => deltaOf(canvas, d.statRequests));
    await expect(requests).toHaveTextContent(vsPrevious("+32%"));
    await expect(getComputedStyle(requests).color).toBe(resolveColorToken("--status-success-text"));
    const latency = await waitFor(() => deltaOf(canvas, d.statP95));
    await expect(latency.textContent).toMatch(/^-|^−/);
    await expect(getComputedStyle(latency).color).toBe(resolveColorToken("--status-success-text"));
    const errors = await waitFor(() => deltaOf(canvas, d.statErrorRate));
    await expect(errors).toHaveTextContent(errorCount(7));
    await expect(getComputedStyle(errors).color).toBe(resolveColorToken("--status-danger-text"));
    const spend = await waitFor(() => deltaOf(canvas, d.statSpend));
    await expect(spend.textContent).toMatch(/^\+/);
    await expect(getComputedStyle(spend).color).toBe(
      getComputedStyle(labelOf(canvas, d.statSpend)).color,
    );
  },
};

/** A change inside the 1% dead band is flat: a level arrow, neither good nor bad. */
export const AnUnchangedWindowIsFlat: Story = {
  render: () => render(loadedWith(SUMMARY, SUMMARY)),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const d = en.pages.dashboard;
    for (const label of [d.statRequests, d.statP95, d.statSpend]) {
      const delta = await waitFor(() => deltaOf(canvas, label));
      await expect(delta.querySelector("path")).toHaveAttribute("d", "M5 12h14");
      await expect(getComputedStyle(delta).color).toBe(
        getComputedStyle(labelOf(canvas, label)).color,
      );
    }
  },
};

/** The comparison is said in the dashboard's language, Russian included. */
export const ComparesWithThePreviousWindowInRussian: Story = {
  globals: { locale: "ru" },
  render: () => render(loadedWith(SUMMARY, { ...SUMMARY, requests: 100 })),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const delta = await waitFor(() => deltaOf(canvas, ru.pages.dashboard.statRequests));
    await expect(delta.textContent).toContain(
      ru.pages.dashboard.vsPrevious.replace("{{change}}", "").trim(),
    );
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
const QUIET_SUMMARY = QUIET as typeof SUMMARY;

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
    // a percentile and a rate over no requests are not zero, and "0 ms" and
    // "0.00 %" said they had been measured. they read "—", with the reason
    for (const label of [
      en.pages.dashboard.statP95,
      en.pages.dashboard.statErrorRate,
      en.pages.dashboard.statCacheHit,
    ]) {
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
  render: () => render(withRecent(MOBILE_ROWS)),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.number(132));
    await expectNoHorizontalOverflow();

    // the recent requests fit the card at a phone's width: the `ms` column was
    // scrolled out of reach, four columns at 16px padding being wider than the
    // card. every column is inside the card, and the table has nothing to scroll
    const recent = canvas.getByTestId("dashboard-recent");
    const table = await within(recent).findByRole("table");
    const scroller = table.closest("[tabindex]") as HTMLElement;
    await expect(scroller.scrollWidth).toBeLessThanOrEqual(scroller.clientWidth);
    const card = recent.getBoundingClientRect();
    for (const head of within(recent).getAllByRole("columnheader")) {
      const box = head.getBoundingClientRect();
      await expect(box.left).toBeGreaterThanOrEqual(card.left);
      await expect(box.right).toBeLessThanOrEqual(card.right);
    }
    await expect(
      within(recent).getByRole("columnheader", { name: en.pages.dashboard.colMs }),
    ).toBeVisible();

    // the spend chart is drawn at the width it has, so its axis text is read at
    // the size it was set: the viewBox was a fixed 640 wide, squeezed to about a
    // third of that, and 9px text came out near 3px
    const spend = canvas.getByTestId("dashboard-spend");
    const svg = (await within(spend).findByRole("img", {
      name: SPEND_CHART,
    })) as unknown as SVGSVGElement;
    const scale = svg.getBoundingClientRect().width / svg.viewBox.baseVal.width;
    await expect(scale).toBeGreaterThan(0.95);
    await expect(scale).toBeLessThan(1.05);
    const axis = [...svg.querySelectorAll("text")];
    for (const text of axis) {
      await expect(parseFloat(getComputedStyle(text).fontSize) * scale).toBeGreaterThanOrEqual(11);
    }
    // and the labels are thinned to what fits, so none runs into the next
    const boxes = axis
      .filter((text) => /^\d{2}:\d{2}$/.test(text.textContent ?? ""))
      .map((text) => text.getBoundingClientRect());
    await expect(boxes.length).toBeGreaterThan(1);
    for (let i = 1; i < boxes.length; i += 1) {
      await expect(boxes[i].left).toBeGreaterThanOrEqual(boxes[i - 1].right);
    }
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
    // the gateways do not need ClickHouse, and an operator without analytics
    // still wants to know whether they are up
    await expect(await canvas.findByTestId("dashboard-fleet")).toBeVisible();
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
 * With traffic the telemetry leads: an unfinished checklist sits under the
 * figures instead of pushing them to the fold (#1977). On a deployment with no
 * traffic it stays on top, where there is nothing else to look at.
 */
export const ChecklistSitsBelowTheFiguresWhenServing: Story = {
  render: () => render(withRecent(MOBILE_ROWS), "admin"),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.number(132));
    // the card remounts below the figures once traffic arrives, so read both
    // nodes afresh on every attempt rather than holding one found earlier
    await waitFor(() => {
      const checklist = canvas.getByText(en.pages.gettingStarted.subtitle);
      const figures = canvas.getByTestId("dashboard-figures");
      expect(
        figures.compareDocumentPosition(checklist) & Node.DOCUMENT_POSITION_FOLLOWING,
      ).toBeTruthy();
    });
  },
};

/**
 * A polling cadence a play can watch several intervals of (#1975). The recent
 * requests card asks every 15s and the figures every minute, and the
 * story tests give a whole story 15s, so the screen takes `pollMs` from a
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

/**
 * every `since` the screen has sent to `endpoint` for the window on screen,
 * oldest first. the summary of the window before, read for the arrows, is
 * left out: it is the read with an `until` (#2010)
 */
const sinceOf = (recorder: Recorder, endpoint: string): number[] =>
  recorder.calls
    .filter((c) => new URL(c.url, "http://localhost").pathname === endpoint)
    .filter((c) => !isPreviousWindow(c.url))
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

const NEWER = recentRow("req-2", "gemini-2.5-flash", 0, { status: 429 });

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
    // four cards read four endpoints, and each says so for itself
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
    // figures, the chart, the by-model table and the rows are not blanked
    await expect(canvas.queryByRole("alert")).toBeNull();
    await expect(canvas.queryAllByLabelText(LOADING_LABEL)).toHaveLength(0);
    await expect(canvas.getAllByText(fmt.number(132))).not.toHaveLength(0);
    await expect(
      within(canvas.getByTestId("dashboard-spend")).getByRole("img", {
        name: SPEND_CHART,
      }),
    ).toBeVisible();
    await expect(
      within(canvas.getByTestId("dashboard-by-model")).getByText("claude-sonnet-4"),
    ).toBeVisible();
    await expect(
      within(canvas.getByTestId("dashboard-recent")).getByText("gpt-4o", { selector: "td span" }),
    ).toBeVisible();

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

const timedOut = recording(
  scoped(async (input, init) =>
    analyticsRefused(input)
      ? json(
          { error: { message: "analytics query timed out", code: "analytics_query_timeout" } },
          504,
        )
      : loaded(input, init),
  ),
);

/**
 * Every analytics read ran past the request timeout (#2713): the store is up, so
 * the one alert says the query took too long and to narrow the range, not that
 * the store failed.
 */
export const QueryTimeout: Story = {
  render: () => render(timedOut.stub),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /took too long/i);
    await waitFor(() => expect(canvas.getAllByRole("alert")).toHaveLength(1));
    await expect(canvas.getByRole("alert")).not.toHaveTextContent(/failed to return/i);
    await expect(canvas.getByRole("button", { name: "Try again" })).toBeVisible();
  },
};

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
          name: SPEND_CHART,
        }),
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
          selector: "td span",
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
    // the fleet does not wait for the summary
    await expect(await within(figures).findByTestId("dashboard-fleet")).toBeVisible();
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

export const TheByModelCardIsStillLoading: Story = {
  render: () => render(except([ENDPOINTS[2]], never)),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const card = canvas.getByTestId("dashboard-by-model");
    await expectSkeleton(card);
    await expectNoFalseEmpty(card, new RegExp(en.pages.dashboard.noTraffic));
    await expect(canvas.queryByText(en.pages.dashboard.noTraffic)).toBeNull();
    await expectTheOthersLoaded(canvasElement, ["dashboard-by-model"]);
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
    await expect(within(recent).getByText("gpt-4o", { selector: "td span" })).toBeVisible();
  },
};

let spendDown = true;
const partial = recording(except([ENDPOINTS[1]], refused, () => !spendDown));

/**
 * One failing read takes down one card. The spend chart's endpoint answers
 * 500; the figures, the by-model table and the recent rows stay up instead of
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
    await expect(await within(spend).findByRole("img", { name: SPEND_CHART })).toBeVisible();
    await expect(canvas.queryByRole("alert")).toBeNull();
    // the retry asked for the spend series once, and for nothing else
    await expect(readsOf(partial, ENDPOINTS[1])).toBe(reads[1] + 1);
    for (const i of [0, 2, 3]) await expect(readsOf(partial, ENDPOINTS[i])).toBe(reads[i]);
  },
};

const summaryDown = recording(except([ENDPOINTS[0]], refused));

/**
 * #1978: the fleet is not analytics. The summary is the heaviest read on the
 * screen, and when it fails the Gateways tile stays, as the first tile of the
 * strip, with the summary's own alert beside it in place of the other five.
 */
export const AFailedSummaryLeavesTheFleetTileUp: Story = {
  render: () => render(summaryDown.stub),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const figures = canvas.getByTestId("dashboard-figures");
    await expectLoadError(figures, /failed to return the overview figures/i);
    const fleet = await within(figures).findByTestId("dashboard-fleet");
    await waitFor(() =>
      expect(fleet).toHaveTextContent(`${fmt.number(3)}${en.pages.dashboard.fleet.liveUnit}`),
    );
    await expect(fleet).toBeVisible();
    await expect(figures.querySelector("[data-testid]")).toBe(fleet);
    // the other reads answered, so this is a partial failure with one alert
    await expect(canvas.getAllByRole("alert")).toHaveLength(1);
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
 * The by-model card is the one reader of its endpoint, so one failure of it is
 * one alert with its own retry, and the card does not say "No traffic yet." about
 * a read that failed. The retry asks for that read once and brings the table back.
 */
export const AFailedByModelReadIsOneAlertWithItsOwnRetry: Story = {
  beforeEach: () => {
    modelsDown = true;
  },
  render: () => render(byModelDown.stub),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const card = canvas.getByTestId("dashboard-by-model");
    await expectLoadError(card, /failed to return requests by model/i);
    await expectNoFalseEmpty(card, new RegExp(en.pages.dashboard.noTraffic));
    await expect(canvas.getAllByRole("alert")).toHaveLength(1);
    await expectTheOthersLoaded(canvasElement, ["dashboard-by-model"]);

    const reads = readsOf(byModelDown, ENDPOINTS[2]);
    modelsDown = false;
    await userEvent.click(within(card).getByRole("button", { name: "Try again" }));
    await expect(await within(card).findByText("claude-sonnet-4")).toBeVisible();
    await expect(canvas.queryByRole("alert")).toBeNull();
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
 * outage: the three that failed each hold their own alert, and the card on
 * the read still out stands in a skeleton rather than saying anything about it.
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
    await expectSkeleton(canvas.getByTestId("dashboard-by-model"));
    await expect(canvas.queryByText(/failed to return analytics/i)).toBeNull();
    await expect(canvas.queryByText(en.pages.dashboard.noTraffic)).toBeNull();
  },
};

// the spend chart's read never recovers, and the others answer every poll
const cardDown = recording(except([ENDPOINTS[1]], refused));

/**
 * The no-polling rule belongs to the read, not to the screen: the spend chart
 * failed holding nothing, so it stops asking and keeps its alert, while the
 * figures, the by-model table and the rows go on refreshing around it.
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

// models in the order the control plane answers, by cost. the cheap ones are
// the busy ones, so the request order is nothing like it: ranked by requests,
// claude leads and gpt-4o-mini follows. nine of them, so the card lists the
// first eight, one for each colour of the sequence, and says one is not shown
const BY_COST = [
  modelRow("gpt-4o", 48, 30.1),
  modelRow("claude-sonnet-4", 84, 11.17),
  modelRow("o3", 12, 9.5),
  modelRow("gemini-2.5-pro", 30, 7.1),
  modelRow("llama-3.3-70b", 66, 5.2),
  modelRow("mistral-large", 22, 3.3),
  modelRow("haiku-4", 57, 2.2),
  modelRow("gpt-4o-mini", 75, 0.9),
  modelRow("gemma-2", 5, 0.1),
];
const BUSIEST_FIRST = [
  "claude-sonnet-4",
  "gpt-4o-mini",
  "llama-3.3-70b",
  "haiku-4",
  "gpt-4o",
  "gemini-2.5-pro",
  "mistral-large",
  "o3",
];

/** what `var(<token>)` computes to as a background, which is how a swatch and a bar are painted */
function backgroundOf(token: string): string {
  const probe = document.createElement("span");
  probe.style.backgroundColor = `var(${token})`;
  document.body.appendChild(probe);
  const color = getComputedStyle(probe).backgroundColor;
  probe.remove();
  return color;
}

const swatchIn = (card: HTMLElement, model: string) =>
  getComputedStyle(within(card).getByText(model).previousElementSibling as HTMLElement)
    .backgroundColor;
const barIn = (card: HTMLElement, model: string) =>
  getComputedStyle(
    within(card).getByText(model).closest("tr")!.querySelector("[style*='width']") as HTMLElement,
  ).backgroundColor;

/**
 * #1994, #1978: one table ranks the models by requests, busiest first, and
 * colours the nth row the nth colour of the sequence, in the swatch and in the
 * bar. The read arrives by cost, which is not the order, and the ninth model is
 * not listed but is counted in the line under the table.
 */
export const ModelsAreRankedByRequestsAndColouredByRank: Story = {
  render: () => render(withByModel(BY_COST)),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const card = canvas.getByTestId("dashboard-by-model");
    await within(card).findByText("claude-sonnet-4");
    const seen = new Set<string>();
    for (const [rank, model] of BUSIEST_FIRST.entries()) {
      const colour = backgroundOf(`--chart-${rank + 1}`);
      await expect(swatchIn(card, model)).toBe(colour);
      await expect(barIn(card, model)).toBe(colour);
      seen.add(colour);
    }
    await expect(seen.size).toBe(BUSIEST_FIRST.length);
    const names = within(card)
      .getAllByRole("row")
      .slice(1)
      .map((row) => within(row).getAllByRole("cell")[0].textContent);
    await expect(names).toEqual(BUSIEST_FIRST);
    await expect(within(card).queryByText("gemma-2")).toBeNull();
    await expect(
      within(card).getByText(en.pages.dashboard.byModelMore_one.replace("{{count}}", "1")),
    ).toBeVisible();
  },
};

/**
 * #1978: each model's row carries what an operator compares models by: its
 * requests, its p95 latency (not a mean) and its spend, in the dashboard's
 * currency, under the headers that name them.
 */
export const EachModelRowCarriesRequestsP95AndSpend: Story = {
  render: () => render(loaded),
  play: async ({ canvasElement }) => {
    const card = within(canvasElement).getByTestId("dashboard-by-model");
    const d = en.pages.dashboard;
    for (const name of [d.colModel, d.colRequests, d.colP95, d.colSpend]) {
      await expect(await within(card).findByRole("columnheader", { name })).toBeVisible();
    }
    const row = within(card).getByText("gpt-4o").closest("tr") as HTMLElement;
    await expect(row).toHaveTextContent(fmt.number(84));
    await expect(row).toHaveTextContent(`${fmt.number(820)} ${d.colMs}`);
    await expect(row).toHaveTextContent(fmt.currency(30.1, "USD"));
    // the mean is not on the card
    await expect(card).not.toHaveTextContent(/avg|mean/i);
  },
};

/** a second organization a superadmin reads beside the harness's own */
const OTHER_ORG = { ...ORG, id: "org-2", name: "Umbrella", slug: "umbrella" };

/**
 * #3486: two organizations may each have a route named `gpt-4o` (#1857), and
 * the by-model read answers them as two rows. The card keeps them apart, each
 * with its own requests, latency and spend, and names the owner of a name that
 * is shared; a model only one organization serves carries no pill.
 */
export const TwoOrganizationsSameNamedModelsAreTwoRows: Story = {
  render: () =>
    render(async (input, init) => {
      if (pathOf(input) === "/api/v1/orgs") return json([ORG, OTHER_ORG]);
      return withByModel([
        { ...BY_MODEL[0], org_id: ORG.id, requests: 84, p95_latency_ms: 820, cost_usd: 30.1 },
        {
          ...BY_MODEL[0],
          org_id: OTHER_ORG.id,
          requests: 61,
          p95_latency_ms: 1240,
          cost_usd: 7.25,
        },
        { ...BY_MODEL[1], org_id: ORG.id },
      ])(input, init);
    }, "superadmin"),
  play: async ({ canvasElement }) => {
    const card = within(canvasElement).getByTestId("dashboard-by-model");
    const owner = (name: string) => en.common.ownerOrg.replace("{{name}}", name);
    const d = en.pages.dashboard;
    // two rows of one name, never one row of the two summed
    await waitFor(() => expect(within(card).getAllByText("gpt-4o")).toHaveLength(2));
    await expect(card).not.toHaveTextContent(fmt.number(84 + 61));
    const rowOf = async (org: string) =>
      (await within(card).findByText(owner(org))).closest("tr") as HTMLElement;
    const ours = await rowOf(ORG.name);
    await expect(ours).toHaveTextContent("gpt-4o");
    await expect(ours).toHaveTextContent(fmt.number(84));
    await expect(ours).toHaveTextContent(`${fmt.number(820)} ${d.colMs}`);
    await expect(ours).toHaveTextContent(fmt.currency(30.1, "USD"));
    const theirs = await rowOf(OTHER_ORG.name);
    await expect(theirs).toHaveTextContent("gpt-4o");
    await expect(theirs).toHaveTextContent(fmt.number(61));
    await expect(theirs).toHaveTextContent(`${fmt.number(1240)} ${d.colMs}`);
    await expect(theirs).toHaveTextContent(fmt.currency(7.25, "USD"));
    // a name only one organization has needs nobody to tell it apart
    const sonnet = within(card).getByText("claude-sonnet-4").closest("tr") as HTMLElement;
    await expect(sonnet).not.toHaveTextContent(owner(ORG.name));
  },
};

const TODAY_ROW = recentRow("req-today", "gpt-4o", 0);
// 30 hours back is never today, whatever the hour the story runs at
const EARLIER_ROW = recentRow("req-earlier", "claude-sonnet-4", 30 * 60, { latency_ms: 1530 });
const NO_ID_ROW = recentRow("", "gemini-2.5-flash", 0, { status: 429 });

// where the router is, for a story to read after a click
function Where() {
  const location = useLocation();
  return (
    <span data-testid="where" className="sr-only">
      {location.pathname + location.search}
    </span>
  );
}

/** the day a row fell on, as the column writes it under the clock */
const dayOf = (ts: string) => fmt.date(ts, { month: "short", day: "numeric" });

/** the name of the link that opens a request, as the catalog words it */
const openName = (model: string, ts: string) =>
  en.pages.dashboard.openRequest.replace("{{model}}", model).replace("{{time}}", fmt.dateTime(ts));

/**
 * The recent requests window is 24 hours and crosses midnight, where a bare
 * clock reads as today's. A row from another day carries its date under the
 * clock; a row from today stays a clock. Each row opens its request in LLM Logs
 * by the id it carries, over its whole width, and a row with no id has nothing
 * to open and is not a link.
 */
export const RecentRequestsShowTheirDayAndOpenInLlmLogs: Story = {
  render: () => (
    <MemoryRouter>
      <Harness fetchStub={withRecent([TODAY_ROW, EARLIER_ROW, NO_ID_ROW])}>
        <Dashboard />
        <Where />
      </Harness>
    </MemoryRouter>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const recent = canvas.getByTestId("dashboard-recent");
    const rowOf = async (model: string) =>
      (await within(recent).findByText(model, { selector: "td span" })).closest(
        "tr",
      ) as HTMLTableRowElement;
    const today = await rowOf("gpt-4o");
    const earlier = await rowOf("claude-sonnet-4");
    const noId = await rowOf("gemini-2.5-flash");

    // a clock alone for today, and the day under it for the earlier request
    await expect(today.cells[0].textContent).toBe(fmt.time(TODAY_ROW.ts));
    await expect(earlier.cells[0]).toHaveTextContent(fmt.time(EARLIER_ROW.ts));
    await expect(earlier.cells[0]).toHaveTextContent(dayOf(EARLIER_ROW.ts));

    // each row with an id is a link to that request, named for it
    const link = within(earlier).getByRole("link", {
      name: openName("claude-sonnet-4", EARLIER_ROW.ts),
    });
    await expect(link).toHaveAttribute("href", "/logs?request_id=req-earlier");
    await expect(
      within(today).getByRole("link", { name: openName("gpt-4o", TODAY_ROW.ts) }),
    ).toHaveAttribute("href", "/logs?request_id=req-today");
    await expect(within(noId).queryByRole("link")).toBeNull();
    // the two rows, and the way into the whole log beneath them
    await expect(within(recent).getAllByRole("link")).toHaveLength(3);

    // the link covers the row, so a press on the model's cell lands on it
    earlier.scrollIntoView({ block: "center" });
    const cell = within(earlier).getByText("claude-sonnet-4", { selector: "td span" });
    const box = cell.getBoundingClientRect();
    await expect(
      document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2),
    ).toBe(link);

    await userEvent.click(link);
    await expect(canvas.getByTestId("where")).toHaveTextContent("/logs?request_id=req-earlier");
  },
};

/**
 * #1978: the way into the whole log. The link under the recent rows carries the
 * window and the filters the figures were read with, under the names LLM Logs
 * reads them by, so what the operator was looking at is what opens.
 */
export const RecentRequestsLinkTheWholeLogWithTheWindowAndFilters: Story = {
  render: () => (
    <MemoryRouter initialEntries={["/?window=7d&model=gpt-4o"]}>
      <Harness fetchStub={loaded}>
        <Dashboard />
        <Where />
      </Harness>
    </MemoryRouter>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const link = await canvas.findByRole("link", { name: en.pages.dashboard.recentAll });
    await expect(link).toHaveAttribute("href", "/logs?window=7d&model=gpt-4o");
    await userEvent.click(link);
    await expect(canvas.getByTestId("where")).toHaveTextContent("/logs?window=7d&model=gpt-4o");
  },
};

/** nothing logged, nothing to open: the link is not offered under an empty card */
export const NoLinkToTheLogsWhenNothingIsLogged: Story = {
  render: () => render(withRecent([])),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await within(canvas.getByTestId("dashboard-recent")).findByText(
      en.pages.dashboard.nothingLogged,
    );
    await expect(canvas.queryByTestId("dashboard-recent-all")).toBeNull();
  },
};

/**
 * #1978: the figures say what they cover. Without a session (open mode) or for a
 * superadmin that is every request on the deployment; the window is named beside
 * it, and a filter is counted.
 */
export const TheScopeLineSaysWhatTheFiguresCover: Story = {
  render: () => (
    <MemoryRouter initialEntries={["/?window=7d&model=gpt-4o"]}>
      <Harness fetchStub={loaded}>
        <Dashboard />
      </Harness>
    </MemoryRouter>
  ),
  play: async ({ canvasElement }) => {
    const scope = await within(canvasElement).findByTestId("dashboard-scope");
    await expect(scope).toHaveTextContent(
      [
        en.pages.dashboard.scope.all,
        en.common.timeWindow.last7d,
        en.pages.dashboard.scope.filters_one.replace("{{count}}", "1"),
      ].join(" · "),
    );
  },
};

/**
 * #3302: spend a policy withheld, and successes with no reported usage, are in
 * the figures above and would read as clean. The home says how much and how many
 * when either is non-zero, and each notice links to LLM Logs over the same window
 * and filters, narrowed to those rows.
 */
export const WithheldSpendAndUnknownUsageAreSaidOnTheHome: Story = {
  render: () => (
    <MemoryRouter initialEntries={["/?window=7d&model=gpt-4o"]}>
      <Harness
        fetchStub={loadedWith({
          ...SUMMARY,
          withheld_requests: 3,
          withheld_cost: 1.25,
          usage_unknown_requests: 2,
        })}
      >
        <Dashboard />
        <Where />
      </Harness>
    </MemoryRouter>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const d = en.pages.dashboard;
    const withheld = await canvas.findByText(d.withheld.title_other.replace("{{count}}", "3"));
    await expect(withheld.closest('[role="status"]')).toHaveTextContent(
      d.withheld.body.replace("{{cost}}", fmt.currency(1.25, "USD")),
    );
    await expect(
      await canvas.findByText(d.usageUnknown.title_other.replace("{{count}}", "2")),
    ).toBeVisible();
    await userEvent.click(canvas.getByRole("link", { name: d.withheld.link }));
    await expect(canvas.getByTestId("where")).toHaveTextContent(
      "/logs?window=7d&model=gpt-4o&withheld=true",
    );
  },
};

/** A window with nothing withheld and no unknown usage shows neither notice. */
export const NoWithheldNoticeWhenNothingWasWithheld: Story = {
  render: () => render(loaded),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.number(132));
    await expect(canvas.queryByText(/billed but/)).toBeNull();
    await expect(canvas.queryByText(/reported no usage/)).toBeNull();
  },
};

const MEMBER_ME = {
  user: { id: "user-1", email: "anya@acme.co", is_superadmin: false },
  memberships: [{ role: "member", org_id: "org-1", team_id: null, project_id: null }],
};

/** A member's figures are the rows their roles reach, and the line says so rather than "every request". */
export const TheScopeLineNamesTheRowsAMemberMayRead: Story = {
  render: () => (
    <MemoryRouter>
      <Harness
        fetchStub={scoped(async (input, init) =>
          pathOf(input) === "/api/v1/auth/me" ? json(MEMBER_ME) : loaded(input, init),
        )}
      >
        <StaleSession>
          <Dashboard />
        </StaleSession>
      </Harness>
    </MemoryRouter>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const scope = await canvas.findByTestId("dashboard-scope");
    await waitFor(() => expect(scope).toHaveTextContent(en.pages.dashboard.scope.member));
    await expect(scope).not.toHaveTextContent(en.pages.dashboard.scope.all);
  },
};

const VIEW_7D = {
  id: "11111111-1111-4111-8111-111111111111",
  surface: "dashboard",
  name: "This week",
  filters: { window: "7d" },
  effective_filters: { window: "7d" },
  unavailable: [],
  created_at: "2026-09-01T09:00:00Z",
  updated_at: "2026-09-01T09:00:00Z",
};

const withSavedViews = recording(
  scoped(async (input, init) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (path === "/api/v1/me/saved-views") {
      return init?.method === "POST" ? json(VIEW_7D, 201) : json([VIEW_7D]);
    }
    return loaded(input, init);
  }),
);

/**
 * #2452: the dashboard keeps its window in the address, so a saved view of it
 * is one name; applying it re-reads the figures over that window, and saving
 * sends the window being read.
 */
export const ASavedViewChangesTheWindow: Story = {
  render: () => (
    <MemoryRouter initialEntries={["/"]}>
      <Harness fetchStub={withSavedViews.stub}>
        <Dashboard />
        <Where />
      </Harness>
    </MemoryRouter>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Saved views" }));
    const sheet = within(await within(document.body).findByRole("dialog", { name: "Saved views" }));
    await userEvent.click(await sheet.findByRole("button", { name: "Apply This week" }));
    await waitFor(() => expect(canvas.getByTestId("where")).toHaveTextContent("?window=7d"));
    await waitFor(() => {
      const reads = withSavedViews.calls.filter((c) => c.url.includes("/analytics/summary"));
      const since = new URL(reads[reads.length - 1].url, "http://localhost").searchParams.get(
        "since",
      );
      expect(Date.now() - Date.parse(since ?? "")).toBeGreaterThan(6.9 * 24 * 3600_000);
    });
    await expect(canvas.getByRole("combobox", { name: "Time window" })).toHaveValue("Last 7 days");

    await userEvent.click(canvas.getByRole("button", { name: "Saved views" }));
    const again = within(await within(document.body).findByRole("dialog", { name: "Saved views" }));
    await userEvent.type(await again.findByLabelText("Save the current filters as"), "Mine");
    await userEvent.click(again.getByRole("button", { name: "Save view" }));
    const body = await withSavedViews.expectSentBody<{ surface: string; filters: unknown }>(
      "POST",
      "/api/v1/me/saved-views",
    );
    await expect(body).toEqual({ surface: "dashboard", name: "Mine", filters: { window: "7d" } });
  },
};

/** `render`, under the screen key the app shell supplies, for a story reading the UX stream */
const renderOnScreen = (stub: FetchStub) => (
  <UxScreenProvider screen="dashboard">{render(stub)}</UxScreenProvider>
);

/** the regions of every `error_state` recorded so far, in order */
const errorRegions = () =>
  uxEvents()
    .filter((e) => e.action === "error_state")
    .map((e) => e.target);

/**
 * The error states come from the alerts on screen, which record their own
 * (#2444): three failed cards are three rows, one per card under its own
 * region, and the card that loaded adds none. No screen-level row is recorded
 * for what is not an outage.
 */
export const EachFailedCardRecordsOneErrorState: Story = {
  beforeEach: recordUxEvents,
  render: () => renderOnScreen(mostDown.stub),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(
      canvas.getByTestId("dashboard-recent"),
      /failed to return recent requests/i,
    );
    await waitFor(() => expect(canvas.getAllByRole("alert")).toHaveLength(3));
    const figures = await expectUxEvent("error_state", "dashboard");
    await expect(figures.screen).toBe("dashboard");
    await waitFor(() =>
      expect([...errorRegions()].sort()).toEqual([
        "dashboard",
        "dashboard-recent",
        "dashboard-spend",
      ]),
    );
  },
};

/** The by-model card's failure is one row, under the card that holds the alert. */
export const AFailedByModelReadRecordsOneErrorState: Story = {
  beforeEach: () => {
    modelsDown = true;
    return recordUxEvents();
  },
  render: () => renderOnScreen(byModelDown.stub),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(
      canvas.getByTestId("dashboard-by-model"),
      /failed to return requests by model/i,
    );
    await expectUxEvent("error_state", "dashboard-by-model");
    await expect(errorRegions()).toEqual(["dashboard-by-model"]);
  },
};

/**
 * Every read failing is one alert for the screen, and one screen-level row for
 * it, `dashboard-analytics`, however long the alert stays up.
 */
export const AnOutageRecordsOneScreenLevelErrorState: Story = {
  beforeEach: recordUxEvents,
  render: () => renderOnScreen(failing.stub),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return analytics/i);
    const outage = await expectUxEvent("error_state", "dashboard-analytics");
    await expect(outage.screen).toBe("dashboard");
    await expect(errorRegions().filter((r) => r === "dashboard-analytics")).toHaveLength(1);
  },
};

/**
 * A poll that fails over figures already on screen shows no alert, so it is not
 * an `error_state` of a `LoadError`, but it is data going stale and it records
 * one (#2640): under the card's region with `-stale` after it, one per card per
 * appearance. Polls that fail again while the line is up add none, and a line
 * that went away and came back is a second appearance.
 */
export const AStaleRefreshRecordsOneErrorStatePerAppearance: Story = {
  beforeEach: () => {
    upstream = "ok";
    return recordUxEvents();
  },
  render: () => (
    <UxScreenProvider screen="dashboard">
      {render(flaky.stub, undefined, FAST_POLL_MS)}
    </UxScreenProvider>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.number(132));
    await expect(await canvas.findByText(en.pages.dashboard.live)).toBeVisible();
    await expect(errorRegions()).toEqual([]);

    upstream = "failing";
    const stale = [
      "dashboard-by-model-stale",
      "dashboard-recent-stale",
      "dashboard-spend-stale",
      "dashboard-stale",
    ];
    // one line per stale region: the figures, the spend, the by-model table and the recent feed
    await waitFor(() => expect(canvas.getAllByText(REFRESH_FAILED)).toHaveLength(stale.length));
    await waitFor(() => expect([...errorRegions()].sort()).toEqual(stale));
    const first = await expectUxEvent("error_state", "dashboard-stale");
    await expect(first.screen).toBe("dashboard");
    await expect(first.outcome).toBe("error");
    // the polls go on failing under the lines, and none of them is a new row
    await sleep(FAST_POLL_MS * 3);
    await expect([...errorRegions()].sort()).toEqual(stale);

    // the lines go when a poll lands, and a failure after that is a new one
    upstream = "ok";
    await waitFor(() => expect(canvas.queryAllByText(REFRESH_FAILED)).toHaveLength(0));
    await expect(await canvas.findByText(en.pages.dashboard.live)).toBeVisible();
    upstream = "failing";
    await waitFor(() => expect(errorRegions()).toHaveLength(stale.length * 2));
    await expect([...errorRegions()].sort()).toEqual([...stale, ...stale].sort());
  },
};

/**
 * #2231: below 100 % the figures are scaled up from a sample (#2239), and the
 * screen says so with the share the log keeps.
 */
export const ASampledLogSaysTheFiguresAreEstimates: Story = {
  render: () => render(withSampleRate(loaded, 0.25), "superadmin"),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const notice = await canvas.findByTestId("sampled-log-notice");
    await expect(notice).toHaveTextContent("The request log keeps about 1 in 4 requests.");
    await expect(notice).toHaveTextContent(/estimates scaled up from that sample/);
    await expect(notice).toHaveAttribute("role", "status");
    // the figures themselves are still drawn
    await expect(await canvas.findAllByText(fmt.number(132))).not.toHaveLength(0);
  },
};

const fullLog = recording(withSampleRate(loaded, 1));

// a log that keeps every request has nothing to qualify
export const AFullLogShowsNoSamplingNote: Story = {
  render: () => render(fullLog.stub, "superadmin"),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect((await canvas.findAllByText(fmt.number(132))).length).toBeGreaterThan(0);
    await fullLog.expectSent("GET", "/api/v1/analytics/sampling");
    // the answer is in the cache a beat after the request leaves
    await sleep(100);
    await expect(canvas.queryByTestId("sampled-log-notice")).toBeNull();
  },
};

const viewerOfSampledLog = recording(withSampleRate(loaded, 0.25));

/**
 * #3057: a viewer reads the same estimates, so they get the same note. The rate
 * comes from the analytics read every role makes, never from the logging
 * settings, which stay a superadmin's.
 */
export const AViewerIsToldTheRateToo: Story = {
  render: () => render(viewerOfSampledLog.stub, "viewer"),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const notice = await canvas.findByTestId("sampled-log-notice");
    await expect(notice).toHaveTextContent("The request log keeps about 1 in 4 requests.");
    await expect(notice).toHaveTextContent(/estimates scaled up from that sample/);
    await viewerOfSampledLog.expectSent("GET", "/api/v1/analytics/sampling");
    viewerOfSampledLog.expectNotSent("GET", "/api/v1/logging-settings");
  },
};

// the row filters (#2725): the same four LLM Logs has, kept in the address and
// sent to every read on the screen
const UNIT = { id: "unit-1", org_id: "org-1", name: "Platform Engineering", slug: "platform" };
const CUSTOMER = { id: "cust-1", org_id: "org-1", name: "Acme Corp", slug: "acme" };
const KEY = {
  id: "vk-ci",
  project_id: "project-1",
  name: "ci-runner",
  key_prefix: "rk_live_ab12",
  models: [],
  providers: [],
  disabled: false,
};
const SAVED_VIEW_ID = "11111111-1111-4111-8111-111111111111";
const GONE_CUSTOMER = "44444444-4444-4444-8444-444444444444";
const SAVED_VIEW = {
  id: SAVED_VIEW_ID,
  surface: "dashboard",
  name: "Platform spend",
  filters: {
    window: "7d",
    model: "gpt-4o",
    business_unit: ["unit-1"],
    customer: ["cust-1", GONE_CUSTOMER],
  },
  effective_filters: {
    window: "7d",
    model: "gpt-4o",
    business_unit: ["unit-1"],
    customer: ["cust-1"],
  },
  unavailable: [{ filter: "customer", id: GONE_CUSTOMER }],
  created_at: "2026-09-01T09:00:00Z",
  updated_at: "2026-09-01T09:00:00Z",
};

/** the four analytics reads and the lists the pickers offer, answering what `answer` says of the model filter */
const filterable = (
  extra: (path: string, init?: RequestInit) => Response | undefined = () => undefined,
) =>
  recording(
    scoped(async (input, init) => {
      const url = new URL(String(input), "http://localhost");
      const own = extra(url.pathname, init);
      if (own) return own;
      const model = url.searchParams.get("model");
      if (url.pathname === "/api/v1/analytics/summary") {
        return json({ data: [model ? { ...SUMMARY, requests: 84 } : SUMMARY] });
      }
      if (url.pathname === "/api/v1/analytics/by-model") {
        return json({ data: BY_MODEL.filter((m) => !model || m.model === model) });
      }
      if (url.pathname === "/api/v1/models")
        return json([{ model: "gpt-4o" }, { model: "claude-sonnet-4" }]);
      if (url.pathname.includes("/business-units")) return json([UNIT]);
      if (url.pathname.includes("/customers")) return json([CUSTOMER]);
      if (url.pathname.endsWith("/virtual-keys")) return json([KEY]);
      return loaded(input, init);
    }),
  );

const renderAt = (recorder: Recorder, address = "/", role?: StoryRole) => (
  <MemoryRouter initialEntries={[address]}>
    <Harness fetchStub={recorder.stub} role={role}>
      <Dashboard />
      <AddressProbe />
    </Harness>
  </MemoryRouter>
);

/** the query string of the latest read of `path` */
const lastRead = (recorder: Recorder, path: string) => {
  const reads = recorder.calls.filter((c) => new URL(c.url, "http://localhost").pathname === path);
  return new URL(reads[reads.length - 1]?.url ?? "", "http://localhost").searchParams;
};
const READS = [
  "/api/v1/analytics/summary",
  "/api/v1/analytics/timeseries",
  "/api/v1/analytics/by-model",
  "/api/v1/analytics/invocations",
];

const picking = filterable();

/**
 * Picking a model and a business unit writes both to the address, counts them
 * on the Filters button and sends them to all four reads; Clear filters takes
 * them out again and the reads go back to unfiltered.
 */
export const FiltersNarrowEveryReadAndLiveInTheAddress: Story = {
  render: () => renderAt(picking),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.number(132));
    await userEvent.click(canvas.getByRole("button", { name: "Filters" }));
    const model = await canvas.findByRole("combobox", { name: "Model" });
    await userEvent.click(model);
    await userEvent.click(await canvas.findByRole("option", { name: /gpt-4o/ }));
    await userEvent.click(await canvas.findByRole("checkbox", { name: /Platform Engineering/ }));
    await waitFor(() => expect(addressOf(canvasElement).get("business_unit")).toBe("unit-1"));
    await expect(addressOf(canvasElement).get("model")).toBe("gpt-4o");
    await expect(canvas.getByRole("button", { name: "Filters · 2" })).toBeVisible();
    await waitFor(() => {
      for (const path of READS) {
        const sent = lastRead(picking, path);
        expect(sent.get("model")).toBe("gpt-4o");
        expect(sent.get("business_unit")).toBe("unit-1");
      }
    });
    await canvas.findAllByText(fmt.number(84));

    await userEvent.click(canvas.getByRole("button", { name: "Clear filters" }));
    await waitFor(() => expect(addressOf(canvasElement).has("model")).toBe(false));
    await expect(addressOf(canvasElement).has("business_unit")).toBe(false);
    await waitFor(() => {
      for (const path of READS) expect(lastRead(picking, path).has("model")).toBe(false);
    });
    await expect(canvas.getByRole("button", { name: "Filters" })).toBeVisible();
  },
};

const restoring = filterable();

/** An address that names filters opens narrowed: the reads carry them and the pickers show them. */
export const FiltersComeBackFromTheAddress: Story = {
  render: () =>
    renderAt(restoring, "/?window=7d&model=gpt-4o&key=vk-ci&business_unit=unit-1&customer=cust-1"),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.number(84));
    await waitFor(() => {
      for (const path of READS) {
        const sent = lastRead(restoring, path);
        expect(sent.get("model")).toBe("gpt-4o");
        expect(sent.get("key")).toBe("vk-ci");
        expect(sent.get("business_unit")).toBe("unit-1");
        expect(sent.get("customer")).toBe("cust-1");
      }
    });
    await userEvent.click(canvas.getByRole("button", { name: "Filters · 4" }));
    await expect(await canvas.findByRole("combobox", { name: "Model" })).toHaveValue("gpt-4o");
    await expect(await canvas.findByRole("combobox", { name: "Virtual key" })).toHaveValue(
      "ci-runner (rk_live_ab12…)",
    );
    await expect(
      await canvas.findByRole("checkbox", { name: /Platform Engineering/ }),
    ).toBeChecked();
  },
};

const ranged = filterable();

/**
 * A custom range comes back from the address (#3407): every read is sent the
 * start of its first day and the start of the day after its last, in the
 * viewer's calendar, and the arrows compare it with the same number of days
 * straight before it. The picker shows the range, not a stale name.
 */
export const ACustomRangeComesBackFromTheAddress: Story = {
  render: () => renderAt(ranged, "/?window=custom&since=2026-10-01&until=2026-10-07"),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.number(132));
    const first = new Date(2026, 9, 1).toISOString();
    const after = new Date(2026, 9, 8).toISOString();
    const before = new Date(2026, 8, 24).toISOString();
    await waitFor(() => {
      for (const path of READS.slice(0, 3)) {
        const reads = ranged.calls
          .map((c) => new URL(c.url, "http://localhost"))
          .filter((u) => u.pathname === path);
        expect(reads.some((u) => u.searchParams.get("since") === first)).toBe(true);
        expect(
          reads.some(
            (u) => u.searchParams.get("since") === first && u.searchParams.get("until") === after,
          ),
        ).toBe(true);
      }
      // the previous period: seven days ending where the range begins
      const summaries = ranged.calls
        .map((c) => new URL(c.url, "http://localhost"))
        .filter((u) => u.pathname === "/api/v1/analytics/summary");
      expect(
        summaries.some(
          (u) => u.searchParams.get("since") === before && u.searchParams.get("until") === first,
        ),
      ).toBe(true);
    });
    await expect(canvas.getByRole("combobox", { name: "Time window" })).toHaveValue(
      "Custom range…",
    );
    await expect(canvas.getByLabelText("From")).toHaveValue("2026-10-01");
    await expect(canvas.getByLabelText("To")).toHaveValue("2026-10-07");
    // seven days drawn an hour at a time would be 168 points: the chart is cut by the day
    await waitFor(() =>
      expect(lastRead(ranged, "/api/v1/analytics/timeseries").get("bucket")).toBe("day"),
    );
  },
};

const rangedPick = filterable();

/**
 * Choosing Custom range… and typing the days writes all three parameters to
 * the address; going back to a named window takes the days out again.
 */
export const PickingACustomRangeWritesItToTheAddress: Story = {
  render: () => renderAt(rangedPick),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.number(132));
    await userEvent.click(canvas.getByRole("combobox", { name: "Time window" }));
    await userEvent.click(await canvas.findByRole("option", { name: "Custom range…" }));
    const from = await canvas.findByLabelText("From");
    await userEvent.clear(from);
    await userEvent.type(from, "2026-09-01");
    const to = canvas.getByLabelText("To");
    await userEvent.clear(to);
    await userEvent.type(to, "2026-09-03");
    await waitFor(() => {
      const address = addressOf(canvasElement);
      expect(address.get("window")).toBe("custom");
      expect(address.get("since")).toBe("2026-09-01");
      expect(address.get("until")).toBe("2026-09-03");
    });
    await userEvent.click(canvas.getByRole("combobox", { name: "Time window" }));
    await userEvent.click(await canvas.findByRole("option", { name: "Last 7 days" }));
    await waitFor(() => expect(addressOf(canvasElement).get("window")).toBe("7d"));
    await expect(addressOf(canvasElement).has("since")).toBe(false);
    await expect(addressOf(canvasElement).has("until")).toBe(false);
  },
};

const noMatch = filterable((path, init) => {
  void init;
  if (path === "/api/v1/analytics/summary")
    return json({ data: [{ ...SUMMARY, requests: 0, errors: 0, cost_usd: 0 }] });
  if (
    path === "/api/v1/analytics/timeseries" ||
    path === "/api/v1/analytics/by-model" ||
    path === "/api/v1/analytics/invocations"
  )
    return json({ data: [] });
  return undefined;
});

/** A filter that matches nothing says so, rather than reading as a deployment that has served no traffic. */
export const AFilterThatMatchesNothingSaysSo: Story = {
  render: () => renderAt(noMatch, "/?model=gpt-4o"),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getAllByText(en.pages.dashboard.noMatch).length).toBeGreaterThanOrEqual(3),
    );
    await expect(canvas.queryByText(en.pages.dashboard.noTraffic)).toBeNull();
    await expect(canvas.queryByText(en.pages.dashboard.nothingLogged)).toBeNull();
  },
};

const withView = filterable((path) =>
  path === "/api/v1/me/saved-views" ? json([SAVED_VIEW]) : undefined,
);

/**
 * Applying a saved view writes its `effective_filters` into the address and
 * counts what the account could no longer read, as LLM Logs does.
 */
export const ApplyingASavedViewSetsTheFilters: Story = {
  render: () => renderAt(withView, "/?key=vk-ci"),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Saved views" }));
    await userEvent.click(
      await within(document.body).findByRole("button", { name: "Apply Platform spend" }),
    );
    await waitFor(() => expect(addressOf(canvasElement).get("model")).toBe("gpt-4o"));
    const address = addressOf(canvasElement);
    await expect(address.get("window")).toBe("7d");
    await expect(address.get("business_unit")).toBe("unit-1");
    await expect(address.get("customer")).toBe("cust-1");
    // the view does not name a key, so the one already picked is cleared
    await expect(address.has("key")).toBe(false);
    await expect(await canvas.findByRole("status")).toHaveTextContent(
      "Applied without: 1 customer.",
    );
    await waitFor(() => expect(lastRead(withView, READS[0]).get("customer")).toBe("cust-1"));
  },
};

const saving = filterable((path, init) => {
  if (path !== "/api/v1/me/saved-views") return undefined;
  return init?.method === "POST" ? json(SAVED_VIEW, 201) : json([]);
});

/** Saving keeps the window and the filters applied now, and leaves out the empty ones. */
export const SavingAViewKeepsTheFilters: Story = {
  render: () => renderAt(saving, "/?window=30d&model=gpt-4o&customer=cust-1"),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Saved views" }));
    const sheet = within(await within(document.body).findByRole("dialog", { name: "Saved views" }));
    await userEvent.type(await sheet.findByLabelText("Save the current filters as"), "Mine");
    await userEvent.click(sheet.getByRole("button", { name: "Save view" }));
    const body = await saving.expectSentBody<{ filters: Record<string, unknown> }>(
      "POST",
      "/api/v1/me/saved-views",
    );
    await expect(body.filters).toEqual({ window: "30d", model: "gpt-4o", customer: ["cust-1"] });
  },
};

const keyless = filterable((path) =>
  path.endsWith("/virtual-keys") ? json({ error: { message: "forbidden" } }, 403) : undefined,
);

/** A caller who may not list keys gets the other pickers and no key picker, not an empty one. */
export const WithoutKeyAccessThereIsNoKeyPicker: Story = {
  render: () => renderAt(keyless, "/", "viewer"),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.number(132));
    await userEvent.click(canvas.getByRole("button", { name: "Filters" }));
    await expect(await canvas.findByRole("combobox", { name: "Model" })).toBeVisible();
    await expectGateAnswered();
    await expect(canvas.queryByRole("combobox", { name: "Virtual key" })).toBeNull();
  },
};

/** The panel is a disclosure: the button names its state and Escape-free keyboard use opens and closes it. */
export const TheFilterPanelOpensFromTheKeyboard: Story = {
  render: () => renderAt(filterable()),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.number(132));
    const button = canvas.getByRole("button", { name: "Filters" });
    await expect(button).toHaveAttribute("aria-expanded", "false");
    button.focus();
    await userEvent.keyboard("{Enter}");
    await expect(button).toHaveAttribute("aria-expanded", "true");
    await expect(await canvas.findByRole("region", { name: "Filters" })).toBeVisible();
    await userEvent.keyboard("{Enter}");
    await expect(button).toHaveAttribute("aria-expanded", "false");
    await expect(button).toHaveFocus();
  },
};
