import type { Meta, StoryObj } from "@storybook/react";
import { expect, waitFor, within } from "storybook/test";

import { FleetHealthTile } from "./FleetHealthTile";
import { ScreenHeader } from "./ScreenHeader";
import { formattersFor } from "@/lib/i18n/format";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { resolveColorToken } from "@/lib/story-tokens";
import {
  Harness,
  pending,
  withGateway,
  type FleetAnswer,
  type GatewayAnswer,
} from "@/pages/story-harness";

// the dashboard home's gateway tile (#3263). it reads the hook behind the
// header's pill, so every state here is also asserted against the pill

const fmt = formattersFor("en");
const D = en.pages.dashboard;

const counts = (live: number, stale: number, lagging: number) => ({
  inventory: true,
  gateways: { live, stale, lagging },
});

function TileStory({
  gateway = "ready",
  fleet,
  withPill = false,
}: {
  gateway?: GatewayAnswer;
  fleet?: FleetAnswer;
  /** the header's pill beside the tile, for the stories that compare them */
  withPill?: boolean;
}) {
  return (
    <Harness fetchStub={withGateway(gateway, pending, fleet)}>
      <div className="max-w-sm">
        {withPill && <ScreenHeader title="Dashboard" subtitle="Overview" />}
        <FleetHealthTile />
      </div>
    </Harness>
  );
}

const meta = {
  title: "Components/FleetHealthTile",
  component: FleetHealthTile,
  parameters: { layout: "padded" },
} satisfies Meta<typeof FleetHealthTile>;
export default meta;
type Story = StoryObj<typeof meta>;

const tile = (canvasElement: HTMLElement) => within(canvasElement).getByTestId("dashboard-fleet");

/** Three live gateways, none behind: the count, "live", and a line saying so. */
export const Healthy: Story = {
  render: () => <TileStory fleet={counts(3, 0, 0)} />,
  play: async ({ canvasElement }) => {
    await waitFor(() =>
      expect(tile(canvasElement)).toHaveTextContent(`${fmt.number(3)}${D.fleet.liveUnit}`),
    );
    await expect(tile(canvasElement)).toHaveTextContent(D.fleet.allCurrent);
  },
};

/** A live gateway on an old config version is counted live and named as behind. */
export const Lagging: Story = {
  render: () => <TileStory fleet={counts(3, 0, 1)} />,
  play: async ({ canvasElement }) => {
    await waitFor(() =>
      expect(tile(canvasElement)).toHaveTextContent(D.fleet.lagging_one.replace("{{count}}", "1")),
    );
    await expect(tile(canvasElement)).not.toHaveTextContent(D.fleet.allCurrent);
    // amber, as the header pill is: a lagging gateway is degraded, not down
    const note = within(tile(canvasElement)).getByText(
      D.fleet.lagging_one.replace("{{count}}", "1"),
    );
    await expect(getComputedStyle(note).color).toBe(resolveColorToken("--status-warning-text"));
    await expect(getComputedStyle(note).color).not.toBe(resolveColorToken("--status-danger-text"));
  },
};

/** A gateway that stopped polling is counted apart from the live ones. */
export const Stale: Story = {
  render: () => <TileStory fleet={counts(2, 1, 0)} />,
  play: async ({ canvasElement }) => {
    await waitFor(() =>
      expect(tile(canvasElement)).toHaveTextContent(D.fleet.stale_one.replace("{{count}}", "1")),
    );
    await expect(tile(canvasElement)).toHaveTextContent(`${fmt.number(2)}${D.fleet.liveUnit}`);
  },
};

/** Nothing live: the tile says so rather than showing a bare zero. */
export const NoneLive: Story = {
  render: () => <TileStory fleet={counts(0, 2, 0)} />,
  play: async ({ canvasElement }) => {
    await waitFor(() => expect(tile(canvasElement)).toHaveTextContent(D.fleet.none));
  },
};

/**
 * `inventory: false` leaves nothing to count, so the tile reads the gateway's
 * `/readyz` instead and says it saw one gateway and could not count the fleet.
 */
export const InventoryFalseFallsBackToTheProbe: Story = {
  render: () => (
    <TileStory
      gateway="ready"
      fleet={{ inventory: false, gateways: { live: 0, stale: 0, lagging: 0 } }}
    />
  ),
  play: async ({ canvasElement }) => {
    await waitFor(() => expect(tile(canvasElement)).toHaveTextContent(D.fleet.probe.healthy));
    await expect(tile(canvasElement)).toHaveTextContent(D.fleet.probeNote);
  },
};

/** An older control plane has no summary (a 404): the probe is the answer. */
export const MissingSummaryFallsBackToTheProbe: Story = {
  render: () => <TileStory gateway="draining" />,
  play: async ({ canvasElement }) => {
    await waitFor(() => expect(tile(canvasElement)).toHaveTextContent(D.fleet.probe.degraded));
    await expect(tile(canvasElement)).toHaveTextContent(D.fleet.probeNote);
  },
};

/** A summary that errors falls back too; an unreachable gateway then reads as down. */
export const FailedSummaryFallsBackToTheProbe: Story = {
  render: () => <TileStory gateway="unreachable" fleet="failing" />,
  play: async ({ canvasElement }) => {
    await waitFor(() => expect(tile(canvasElement)).toHaveTextContent(D.fleet.probe.down));
  },
};

/**
 * A probed gateway with no config refuses every key: the tile is red like an
 * unreachable one, and says why in its own words (#3350).
 */
export const SnapshotFormatUnsupported: Story = {
  render: () => <TileStory gateway="snapshotFormatUnsupported" />,
  play: async ({ canvasElement }) => {
    await waitFor(() =>
      expect(tile(canvasElement)).toHaveTextContent(D.fleet.probe.snapshotFormatUnsupported),
    );
    await expect(tile(canvasElement)).not.toHaveTextContent(D.fleet.probe.down);
  },
};

/** Neither the summary nor the probe could be read: the tile says it does not know. */
export const Unknown: Story = {
  render: () => <TileStory gateway="unrecognised" />,
  play: async ({ canvasElement }) => {
    await waitFor(() => expect(tile(canvasElement)).toHaveTextContent(D.fleet.probe.unknown));
    await expect(tile(canvasElement)).toHaveTextContent(D.fleet.unknownNote);
  },
};

/** The first check has not come back: neither a count nor a verdict yet. */
export const Checking: Story = {
  render: () => <TileStory gateway="pending" fleet="failing" />,
  play: async ({ canvasElement }) => {
    await expect(tile(canvasElement)).toHaveTextContent(D.fleet.probe.checking);
    await expect(tile(canvasElement)).not.toHaveTextContent(D.fleet.probeNote);
  },
};

/**
 * The tile and the header's pill read one query, so they cannot disagree: one
 * lagging gateway is a warning in the pill and a line under the count in the tile.
 */
export const AgreesWithTheHeaderPill: Story = {
  render: () => <TileStory fleet={counts(3, 0, 1)} withPill />,
  play: async ({ canvasElement }) => {
    const pill = await waitFor(() => {
      const found = within(canvasElement).getByRole("status");
      expect(found).toHaveTextContent(
        en.shell.gateway.fleet.behind_other.replace("{{lagging}}", "1").replace("{{count}}", "3"),
      );
      return found;
    });
    await expect(pill).toBeVisible();
    await expect(tile(canvasElement)).toHaveTextContent(
      D.fleet.lagging_one.replace("{{count}}", "1"),
    );
    await expect(tile(canvasElement)).toHaveTextContent(`${fmt.number(3)}${D.fleet.liveUnit}`);
  },
};

/** The tile is read in Russian, plural forms included. */
export const StaleInRussian: Story = {
  globals: { locale: "ru" },
  render: () => <TileStory fleet={counts(2, 3, 0)} />,
  play: async ({ canvasElement }) => {
    await waitFor(() =>
      expect(tile(canvasElement)).toHaveTextContent(
        ru.pages.dashboard.fleet.stale_few.replace("{{count}}", "3"),
      ),
    );
  },
};
