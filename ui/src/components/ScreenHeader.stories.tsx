import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import type { Meta, StoryObj } from "@storybook/react";
import { useMemo, type ReactNode } from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { ScreenHeader } from "./ScreenHeader";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { atMobile, expectNoHorizontalOverflow } from "@/lib/story-viewport";
import {
  Harness,
  json,
  pending,
  recording,
  StaleSession,
  withGateway,
  type FetchStub,
  type FleetAnswer,
  type GatewayAnswer,
} from "@/pages/story-harness";

// the pill's copy, read out of the catalog so rewording it cannot leave these
// stories asserting a sentence the header no longer renders
const GATEWAY = en.shell.gateway;
// the part of each timestamp sentence before its `{{time}}`
const CHECKED_AT = GATEWAY.checkedAt.split("{{")[0];
const LAST_ANSWER_AT = GATEWAY.lastAnswerAt.split("{{")[0];

// a query client whose refresh never finishes, for the busy state. nested
// inside `Harness`, which still owns the fetch stub
function StalledRefresh({ children }: { children: ReactNode }) {
  const client = useMemo(() => {
    const stalled = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    stalled.invalidateQueries = () => new Promise<void>(() => {});
    return stalled;
  }, []);
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

// a query that is always in flight: the polling screen the refresh button must not
// mistake for the reader's own request (#1997)
function BackgroundFetch() {
  const poll = useQuery({
    queryKey: ["story-poll"],
    queryFn: () => new Promise<never>(() => {}),
  });
  return <p data-testid="poll">{poll.isFetching ? "polling" : "idle"}</p>;
}

function HeaderStory({
  title,
  subtitle,
  gateway = "ready",
  fleet,
  fetchStub,
  pendingRefresh = false,
  background = false,
  onOpenNav,
}: {
  title: string;
  subtitle: string;
  /** what `GET /gw/readyz` answers; must be a stable reference */
  gateway?: GatewayAnswer | (() => GatewayAnswer);
  /** what `GET /api/v1/cluster/summary` answers; a 404 when omitted. Must be a stable reference */
  fleet?: FleetAnswer;
  /** replaces the whole stub, for a story that records what was sent */
  fetchStub?: FetchStub;
  pendingRefresh?: boolean;
  /** a screen query that is fetching on its own, as a polling screen does */
  background?: boolean;
  onOpenNav?: () => void;
}) {
  // the header asks for nothing but the gateway, so every other request hangs
  const stub = useMemo(
    () => fetchStub ?? withGateway(gateway, pending, fleet),
    [fetchStub, gateway, fleet],
  );
  const header = <ScreenHeader title={title} subtitle={subtitle} onOpenNav={onOpenNav} />;
  return (
    <Harness fetchStub={stub}>
      {background && <BackgroundFetch />}
      {pendingRefresh ? <StalledRefresh>{header}</StalledRefresh> : header}
    </Harness>
  );
}

const meta = {
  title: "Components/ScreenHeader",
  component: ScreenHeader,
  parameters: { layout: "fullscreen" },
  args: { title: "Models", subtitle: "Manage routed models" },
} satisfies Meta<typeof ScreenHeader>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * Assert the gateway pill says `label`, in `tone`, and breathes only when
 * `pulse`.
 *
 * The dot is a shape, so it takes the status fill; the label is a glyph, so it
 * takes the `-text` half or stays muted. Both are asserted, since a pill that
 * turned its dot red and left "gateway healthy" beside it would be the bug this
 * story exists for.
 */
async function expectPill(
  canvasElement: HTMLElement,
  label: string,
  tone: { dot: string; label: string },
  pulse: boolean,
): Promise<HTMLElement> {
  const pill = await waitFor(() => {
    const found = within(canvasElement).getByRole("status");
    expect(found).toHaveTextContent(label);
    return found;
  });
  const dot = pill.querySelector("[aria-hidden]");
  await expect(dot).toHaveClass(tone.dot);
  await expect(within(pill).getByText(label)).toHaveClass(tone.label);
  if (pulse) await expect(dot).toHaveClass("rl-pulse");
  else await expect(dot).not.toHaveClass("rl-pulse");
  // a status is words, not something to paste: Geist, never Geist Mono (#2015)
  await expect(getComputedStyle(pill).fontFamily).not.toMatch(/Mono/);
  return pill;
}

const MUTED = "text-muted-foreground";
const SUBTLE_DOT = "bg-[color:var(--text-subtle)]";

/** The gateway answered `200 ok`: green, breathing, and the title says when. */
export const Default: Story = {
  render: (args) => <HeaderStory {...args} />,
  play: async ({ canvasElement }) => {
    const pill = await expectPill(
      canvasElement,
      GATEWAY.healthy,
      { dot: "bg-[color:var(--status-success)]", label: MUTED },
      true,
    );
    await expect(pill.getAttribute("title")).toContain(GATEWAY.detail.healthy);
    await expect(pill.getAttribute("title")).toContain(CHECKED_AT);
  },
};

/** A gateway drained from the Cluster screen answers `503 draining`. */
export const Draining: Story = {
  render: (args) => <HeaderStory {...args} gateway="draining" />,
  play: async ({ canvasElement }) => {
    const pill = await expectPill(
      canvasElement,
      GATEWAY.degraded,
      {
        dot: "bg-[color:var(--status-warning)]",
        label: "text-[color:var(--status-warning-text)]",
      },
      false,
    );
    await expect(pill.getAttribute("title")).toContain(GATEWAY.detail.degraded);
  },
};

/**
 * The control plane could not reach the gateway, and its `/gw` proxy said so
 * in its own `502` JSON — the incident the hard-coded pill stayed green
 * through (#1973).
 */
export const Unreachable: Story = {
  render: (args) => <HeaderStory {...args} gateway="unreachable" />,
  play: async ({ canvasElement }) => {
    const pill = await expectPill(
      canvasElement,
      GATEWAY.down,
      {
        dot: "bg-[color:var(--status-danger)]",
        label: "text-[color:var(--status-danger-text)]",
      },
      false,
    );
    await expect(pill.getAttribute("title")).toContain(GATEWAY.detail.down);
  },
};

/**
 * A gateway that cannot read the control plane's snapshot format answers
 * `503 snapshot_format_unsupported` (ADR-0038). It has no config and refuses
 * every key, so the pill is red, and its words send the operator to the upgrade
 * order rather than to the network as "unreachable" would (#3350).
 */
export const SnapshotFormatUnsupported: Story = {
  render: (args) => <HeaderStory {...args} gateway="snapshotFormatUnsupported" />,
  play: async ({ canvasElement }) => {
    const pill = await expectPill(
      canvasElement,
      GATEWAY.snapshotFormatUnsupported,
      {
        dot: "bg-[color:var(--status-danger)]",
        label: "text-[color:var(--status-danger-text)]",
      },
      false,
    );
    await expect(pill.getAttribute("title")).toContain(GATEWAY.detail.snapshotFormatUnsupported);
    await expect(pill.getAttribute("title")).toContain("/deployment/upgrading#upgrade-order");
    await expect(pill).not.toHaveTextContent(GATEWAY.down);
  },
};

/**
 * A gateway managed by a control plane that has not applied its first config
 * snapshot answers `503 awaiting_snapshot` (#3348): red too, and worded as a
 * gateway waiting for its config.
 */
export const AwaitingSnapshot: Story = {
  render: (args) => <HeaderStory {...args} gateway="awaitingSnapshot" />,
  play: async ({ canvasElement }) => {
    const pill = await expectPill(
      canvasElement,
      GATEWAY.awaitingSnapshot,
      {
        dot: "bg-[color:var(--status-danger)]",
        label: "text-[color:var(--status-danger-text)]",
      },
      false,
    );
    await expect(pill.getAttribute("title")).toContain(GATEWAY.detail.awaitingSnapshot);
  },
};

/**
 * A `502` page from something in front of the control plane says nothing
 * about the gateway, so the pill says it does not know rather than calling the
 * gateway unreachable and sending the operator to the wrong process.
 */
export const Unknown: Story = {
  render: (args) => <HeaderStory {...args} gateway="unrecognised" />,
  play: async ({ canvasElement }) => {
    const pill = await expectPill(
      canvasElement,
      GATEWAY.unknown,
      { dot: SUBTLE_DOT, label: MUTED },
      false,
    );
    await expect(pill.getAttribute("title")).toBe(GATEWAY.detail.unknown);
  },
};

const WARNING = {
  dot: "bg-[color:var(--status-warning)]",
  label: "text-[color:var(--status-warning-text)]",
};
const DANGER = {
  dot: "bg-[color:var(--status-danger)]",
  label: "text-[color:var(--status-danger-text)]",
};
const HEALTHY = { dot: "bg-[color:var(--status-success)]", label: MUTED };

/** the cluster summary's `gateways` object for a story */
const counts = (live: number, stale: number, lagging: number) => ({
  inventory: true,
  gateways: { live, stale, lagging },
});
const HEALTHY_FLEET = counts(3, 0, 0);
const LAGGING_FLEET = counts(3, 0, 1);
const STALE_FLEET = counts(2, 1, 0);
const DOWN_FLEET = counts(0, 2, 0);
const NO_INVENTORY = { inventory: false, gateways: { live: 0, stale: 0, lagging: 0 } };

const fill = (text: string, values: Record<string, string | number>) =>
  Object.entries(values).reduce((out, [k, v]) => out.split(`{{${k}}}`).join(String(v)), text);

/**
 * #3263: the pill reads the cluster summary first and says how many gateways
 * are live. Three live and none behind or stale is the resting state: green,
 * breathing, and the title says what was counted.
 */
export const FleetHealthy: Story = {
  render: (args) => <HeaderStory {...args} fleet={HEALTHY_FLEET} />,
  play: async ({ canvasElement }) => {
    const pill = await expectPill(
      canvasElement,
      fill(GATEWAY.fleet.live_other, { count: 3 }),
      HEALTHY,
      true,
    );
    await expect(pill.getAttribute("title")).toContain(
      fill(GATEWAY.detail.fleet.healthy_other, { count: 3 }),
    );
    await expect(pill.getAttribute("title")).toContain(CHECKED_AT);
  },
};

/** One gateway answers but runs an old `config_version`: the warning tone, no pulse. */
export const FleetLagging: Story = {
  render: (args) => <HeaderStory {...args} fleet={LAGGING_FLEET} />,
  play: async ({ canvasElement }) => {
    const pill = await expectPill(
      canvasElement,
      fill(GATEWAY.fleet.behind_other, { lagging: 1, count: 3 }),
      WARNING,
      false,
    );
    await expect(pill.getAttribute("title")).toContain(
      fill(GATEWAY.detail.fleet.behind_one, { count: 1, live: 3 }),
    );
  },
};

/** A gateway that went quiet but is not yet forgotten: crashed or partitioned. */
export const FleetStale: Story = {
  render: (args) => <HeaderStory {...args} fleet={STALE_FLEET} />,
  play: async ({ canvasElement }) => {
    const pill = await expectPill(
      canvasElement,
      fill(GATEWAY.fleet.stale_one, { live: 2, count: 1 }),
      WARNING,
      false,
    );
    await expect(pill.getAttribute("title")).toContain(
      fill(GATEWAY.detail.fleet.stale_one, { count: 1 }),
    );
  },
};

/** Nothing live, though gateways are in the inventory: the danger tone. */
export const FleetNoneLive: Story = {
  render: (args) => <HeaderStory {...args} fleet={DOWN_FLEET} />,
  play: async ({ canvasElement }) => {
    const pill = await expectPill(canvasElement, GATEWAY.fleet.none, DANGER, false);
    await expect(pill.getAttribute("title")).toContain(GATEWAY.detail.fleet.none);
  },
};

/**
 * A control plane with no database answers `inventory: false` and zero counts.
 * That is "nothing was counted", not "no gateway is up", so the pill probes the
 * gateway's `/readyz` and says what that returned.
 */
const noInventory = recording(withGateway("draining", pending, NO_INVENTORY));
export const InventoryFalseFallsBackToTheProbe: Story = {
  render: (args) => <HeaderStory {...args} fetchStub={noInventory.stub} />,
  play: async ({ canvasElement }) => {
    await expectPill(canvasElement, GATEWAY.degraded, WARNING, false);
    await expect(
      noInventory.calls.some((c) => new URL(c.url, "http://localhost").pathname === "/gw/readyz"),
    ).toBe(true);
  },
};

/** An older control plane answers the summary with a 404: the probe is the answer. */
export const MissingSummaryFallsBackToTheProbe: Story = {
  render: (args) => <HeaderStory {...args} gateway="ready" />,
  play: async ({ canvasElement }) => {
    await expectPill(canvasElement, GATEWAY.healthy, HEALTHY, true);
  },
};

/** A summary that errors is not an outage of the pill: it probes instead. */
export const FailedSummaryFallsBackToTheProbe: Story = {
  render: (args) => <HeaderStory {...args} gateway="unreachable" fleet="failing" />,
  play: async ({ canvasElement }) => {
    await expectPill(canvasElement, GATEWAY.down, DANGER, false);
  },
};

/** The count agrees with its noun in Russian, which has four forms. */
export const FleetLaggingInRussian: Story = {
  globals: { locale: "ru" },
  render: (args) => <HeaderStory {...args} fleet={counts(5, 0, 2)} />,
  play: async ({ canvasElement }) => {
    await expectPill(
      canvasElement,
      fill(ru.shell.gateway.fleet.behind_many, { lagging: 2, count: 5 }),
      WARNING,
      false,
    );
  },
};

/**
 * The `/gw` proxy requires a dashboard session (#2486), so the probe carries
 * it as `Authorization: Bearer`, and nothing else: the proxy never forwards it
 * and the gateway's `/readyz` takes no key.
 */
const probeAuth: (string | null)[] = [];
export const ProbesWithTheSession: Story = {
  render: (args) => (
    <StaleSession token="session-abc">
      <HeaderStory
        {...args}
        fetchStub={async (input, init) => {
          // a control plane older than the cluster summary (#3263): the check
          // falls back to the probe this story is about
          if (String(input).includes("/api/v1/cluster/summary")) {
            return json({ error: { message: "not found" } }, 404);
          }
          if (String(input).includes("/gw/readyz")) {
            probeAuth.push(new Headers(init?.headers).get("Authorization"));
            return new Response("ok", { status: 200 });
          }
          return pending(input, init);
        }}
      />
    </StaleSession>
  ),
  play: async ({ canvasElement }) => {
    await expectPill(
      canvasElement,
      GATEWAY.healthy,
      { dot: "bg-[color:var(--status-success)]", label: MUTED },
      true,
    );
    await expect(probeAuth.length).toBeGreaterThan(0);
    await expect(probeAuth.every((a) => a === "Bearer session-abc")).toBe(true);
  },
};

/**
 * A `401` from the proxy is a refused session, not a statement about the
 * gateway, so the pill says it does not know rather than calling it down.
 */
export const RefusedSessionIsUnknown: Story = {
  render: (args) => (
    <HeaderStory
      {...args}
      fetchStub={async (input) =>
        String(input).includes("/api/v1/cluster/summary")
          ? json({ error: { message: "unauthorized" } }, 401)
          : String(input).includes("/gw/readyz")
            ? json({ error: { message: "unauthorized" } }, 401)
            : pending(input)
      }
    />
  ),
  play: async ({ canvasElement }) => {
    await expectPill(canvasElement, GATEWAY.unknown, { dot: SUBTLE_DOT, label: MUTED }, false);
  },
};

const checks = recording(withGateway("pending", pending));

/**
 * The first check is still out. The pill says it is checking, and the refresh
 * button does not spin for it: the probe polls on a clock of its own, and
 * counting it spun and disabled that button every thirty seconds. A manual
 * refresh does not wait for the probe either, or a gateway host that drops
 * packets would hold the button busy for the probe's whole timeout.
 */
export const Checking: Story = {
  render: (args) => <HeaderStory {...args} fetchStub={checks.stub} />,
  play: async ({ canvasElement }) => {
    await checks.expectSent("GET", "/gw/readyz");
    const pill = await expectPill(
      canvasElement,
      GATEWAY.checking,
      { dot: SUBTLE_DOT, label: MUTED },
      false,
    );
    await expect(pill.getAttribute("title")).toBe(GATEWAY.detail.checking);
    // the probe is in flight now; give the fetch counter a moment to re-render
    // the header before reading the button, or "not busy" is only the state it
    // started in
    await new Promise((resolve) => setTimeout(resolve, 100));
    const refresh = within(canvasElement).getByRole("button", { name: "Refresh data" });
    await expect(refresh).not.toHaveAttribute("aria-busy", "true");
    await expect(refresh).toBeEnabled();

    // a refresh while the first probe is still out joins it rather than
    // sending a second one, and the button does not wait for it
    await userEvent.click(refresh);
    await waitFor(() =>
      expect(
        within(canvasElement).getByRole("button", { name: "Refresh data" }),
      ).not.toHaveAttribute("aria-busy", "true"),
    );
    await expectPill(canvasElement, GATEWAY.checking, { dot: SUBTLE_DOT, label: MUTED }, false);
  },
};

function FlakyGatewayHeader(args: { title: string; subtitle: string }) {
  // a fresh sequence per mount: ready once, then an answer that is not the
  // gateway's
  const gateway = useMemo(() => {
    let calls = 0;
    return (): GatewayAnswer => (calls++ === 0 ? "ready" : "unrecognised");
  }, []);
  return <HeaderStory {...args} gateway={gateway} />;
}

/**
 * One failed check does not blank a known answer. The pill keeps saying
 * healthy but stops breathing, and the title says the answer is from an
 * earlier check. Past one and a half polls it gives up and says unknown —
 * `gatewayHealthFrom` in `lib/gateway-health.test.ts` pins that half, since a
 * story cannot wait out 45 seconds.
 */
export const HeldThroughOneFailedCheck: Story = {
  render: (args) => <FlakyGatewayHeader {...args} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const healthy = { dot: "bg-[color:var(--status-success)]", label: MUTED };
    await expectPill(canvasElement, GATEWAY.healthy, healthy, true);

    // the manual refresh re-asks every query, the gateway's included
    await userEvent.click(canvas.getByRole("button", { name: "Refresh data" }));

    await waitFor(() =>
      expect(canvas.getByRole("status").querySelector("[aria-hidden]")).not.toHaveClass("rl-pulse"),
    );
    const pill = await expectPill(canvasElement, GATEWAY.healthy, healthy, false);
    await expect(pill.getAttribute("title")).toContain(LAST_ANSWER_AT);
  },
};

/** The manual refresh shows it is busy until the screen's answers are in. */
export const Refreshing: Story = {
  render: (args) => <HeaderStory {...args} pendingRefresh />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const refresh = canvas.getByRole("button", { name: "Refresh data" });

    await userEvent.click(refresh);

    const busyRefresh = canvas.getByRole("button", { name: "Refreshing data" });
    await expect(busyRefresh).toBeDisabled();
    await expect(busyRefresh).toHaveAttribute("aria-busy", "true");
    await expect(busyRefresh.querySelector("svg")).toHaveClass("motion-safe:animate-spin");
  },
};

/** A screen's own background refetch is not a refresh the reader asked for. */
export const BackgroundFetchLeavesRefreshIdle: Story = {
  render: (args) => <HeaderStory {...args} background />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByTestId("poll")).toHaveTextContent("polling"));
    const refresh = canvas.getByRole("button", { name: "Refresh data" });
    await expect(refresh).toBeEnabled();
    await expect(refresh).not.toHaveAttribute("aria-busy", "true");
    await expect(refresh.querySelector("svg")).not.toHaveClass("motion-safe:animate-spin");
  },
};

/**
 * Below `md` the header carries the only way into the navigation, and the
 * status pill takes a row of its own rather than landing on the subtitle
 * (#959). The Dashboard's subtitle is the one that used to wrap to one word per
 * line.
 */
export const Mobile: Story = {
  ...atMobile,
  args: {
    title: en.screens.dashboard.title,
    subtitle: en.screens.dashboard.subtitle,
  },
  render: (args) => <HeaderStory {...args} onOpenNav={() => {}} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const menu = canvas.getByRole("button", { name: "Open navigation" });
    // a touch target, not a 24px pointer one
    await expect(menu.getBoundingClientRect().height).toBeGreaterThanOrEqual(40);
    // the pill and the title do not share a line, so neither sits on the other
    const pill = await canvas.findByText(GATEWAY.healthy);
    const heading = canvas.getByRole("heading", { name: en.screens.dashboard.title });
    await expect(pill.getBoundingClientRect().top).toBeGreaterThanOrEqual(
      heading.getBoundingClientRect().bottom,
    );
    await expectNoHorizontalOverflow();
  },
};

/**
 * The longest label the pill has, in the language that makes it longest, at
 * the narrowest width: it stays on one line and the page does not scroll
 * sideways.
 */
export const MobileLongestLabel: Story = {
  ...atMobile,
  globals: { locale: "ru" },
  args: {
    title: ru.screens.dashboard.title,
    subtitle: ru.screens.dashboard.subtitle,
  },
  render: (args) => <HeaderStory {...args} gateway="draining" onOpenNav={() => {}} />,
  play: async ({ canvasElement }) => {
    const label = await within(canvasElement).findByText(ru.shell.gateway.degraded);
    const pill = label.closest('[role="status"]') as HTMLElement;
    // one line: no taller than the 12px label's line box plus its padding
    await expect(pill.getBoundingClientRect().height).toBeLessThan(32);
    await expectNoHorizontalOverflow();
  },
};
