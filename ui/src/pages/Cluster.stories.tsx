import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Cluster from "./Cluster";
import {
  cancelConfirmation,
  confirmDestructive,
  expectForbidden,
  expectLoadError,
  expectNoFalseEmpty,
  expectSkeleton,
  expectTableStateInFrame,
  expectToast,
  Harness as ScreenHarness,
  json,
  recording,
  Toasted,
  type FetchStub,
  type StoryRole,
} from "./story-harness";
import type { ClusterNodeRow } from "@/lib/api";
import ru from "@/lib/i18n/locales/ru.json";
import { resolveColorToken } from "@/lib/story-tokens";
import { atMobile, phoneFits } from "@/lib/story-viewport";

const node = (over: Partial<ClusterNodeRow> = {}): ClusterNodeRow => ({
  id: "gw-1",
  role: "gateway",
  build_version: "0.0.10",
  config_version: 7,
  desired_state: "active",
  state_changed_at: new Date().toISOString(),
  first_seen_at: new Date().toISOString(),
  last_seen_at: new Date().toISOString(),
  live: true,
  converged: true,
  ...over,
});

const FLEET: ClusterNodeRow[] = [
  node(),
  node({ id: "gw-2", config_version: 6, converged: false }),
  node({ id: "gw-3", desired_state: "draining" }),
  node({
    id: "gw-old",
    live: false,
    last_seen_at: new Date(Date.now() - 3_600_000).toISOString(),
  }),
  node({ id: "cp-1", role: "control" }),
];

/**
 * The screen under the shared fetch-stub harness, with a role to render as.
 *
 * `role` is what a story needs to mount a `CapabilityProvider` at all: with no
 * provider above it `can()` answers "unknown", the `superadminOnly` wrapper
 * never blocks, and a story can only reach the 403 by stubbing one — which
 * tests the screen's own error path rather than the gate (#1606).
 */
function Harness({ fetchStub, role }: { fetchStub: FetchStub; role?: StoryRole }) {
  return (
    <ScreenHarness fetchStub={fetchStub} role={role}>
      <Toasted>
        <Cluster />
      </Toasted>
    </ScreenHarness>
  );
}

const meta = {
  title: "Screens/Cluster",
  component: Cluster,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Cluster>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => <Harness fetchStub={async () => json(FLEET)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // liveness and convergence are separate axes: gw-2 polls but lags
    await waitFor(() => expect(canvas.getByText("Lagging")).toBeVisible());
    await expect(canvas.getByText("Stale")).toBeVisible();
    await expect(canvas.getByText("Draining")).toBeVisible();
    // the summary names what "lagging" is behind
    await expect(canvas.getByText(/newest config v7/)).toBeVisible();
    // draining is the operator's choice: info, not the red of a stale node
    await expect(getComputedStyle(canvas.getByText("Draining")).color).toBe(
      resolveColorToken("--status-info-text"),
    );
    // draining is read by the gateway watcher, so a control node has no Drain
    await expect(canvas.getByRole("button", { name: "Drain node gw-1" })).toBeVisible();
    await expect(canvas.queryByRole("button", { name: "Drain node cp-1" })).toBeNull();
    await expect(canvas.getByRole("button", { name: "Forget node cp-1" })).toBeVisible();
  },
};

/**
 * Each node's snapshot format (#3351): gw-1 is served the current one, gw-2
 * an older one, which leaves it lagging on the current config version, and
 * gw-3 was refused one. The title of each cell names the formats it reads.
 */
export const SnapshotFormats: Story = {
  render: () => (
    <Harness
      fetchStub={async () =>
        json([
          node({ formats_min: 2, formats_max: 2, served_format: 2 }),
          node({ id: "gw-2", formats_min: 1, formats_max: 1, served_format: 1, converged: false }),
          node({ id: "gw-3", formats_min: 3, formats_max: 3, served_format: null }),
        ])
      }
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByRole("columnheader", { name: "Format" })).toBeVisible());
    await expect(canvas.getByText("Lagging")).toBeVisible();
    const refused = canvas.getByText("Refused");
    await expect(refused).toBeVisible();
    await expect(getComputedStyle(refused).color).toBe(resolveColorToken("--status-danger-text"));
    await expect(refused.parentElement).toHaveAttribute("title", "Reads snapshot formats 3 to 3");
    await expect(canvas.getByTitle("Reads snapshot formats 1 to 1")).toHaveTextContent("1");
  },
};

export const LoadedInRussian: Story = {
  globals: { locale: "ru" },
  render: () => <Harness fetchStub={async () => json(FLEET)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const { states } = ru.pages.cluster;
    await waitFor(() => expect(canvas.getByText(states.lagging)).toBeVisible());
    await expect(canvas.getByText(states.stale)).toBeVisible();
    await expect(canvas.getByText(states.draining)).toBeVisible();
    await expect(canvas.queryByText("LAGGING")).toBeNull();
  },
};

// the control plane answers 5xx: a retry is on offer, and it reaches the network
let controlPlaneDown = true;
const reads: string[] = [];
export const LoadFailsAndRetries: Story = {
  render: () => {
    controlPlaneDown = true;
    reads.length = 0;
    return (
      <Harness
        fetchStub={async (input) => {
          reads.push(String(input));
          return controlPlaneDown
            ? json({ error: { message: "control plane unavailable" } }, 503)
            : json(FLEET);
        }}
      />
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /failed to return cluster nodes/i);
    await expect(canvas.getByText("control plane unavailable")).toBeInTheDocument();
    await expectNoFalseEmpty(canvasElement, /No nodes have reported in/);
    const before = reads.length;
    controlPlaneDown = false;
    await userEvent.click(canvas.getByRole("button", { name: "Try again" }));
    await expect(await canvas.findByText("gw-old")).toBeInTheDocument();
    await expect(reads.length).toBeGreaterThan(before);
  },
};

export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No nodes have reported in/);
  },
};

// a single-node deployment that sends no identity headers reports nothing
export const Empty: Story = {
  render: () => <Harness fetchStub={async () => json([])} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("No nodes have reported in")).toBeVisible());
  },
};

// the table is wider than a phone's card, so it scrolls sideways inside it, and
// the placeholder was centred on the whole table: the title ran 18px past the
// card's edge at 375px (#2420)
export const EmptyFitsThePhone: Story = {
  ...atMobile,
  render: () => <Harness fetchStub={async () => json([])} />,
  play: async ({ canvasElement }) => {
    await expectTableStateInFrame(canvasElement, {
      says: /No nodes have reported in/,
      body: /Nodes appear here once a gateway identifies itself/,
    });
  },
};

export const EmptyFitsThePhoneInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => <Harness fetchStub={async () => json([])} />,
  play: async ({ canvasElement }) => {
    const { emptyTitle, emptyBody } = ru.pages.cluster;
    await expectTableStateInFrame(canvasElement, {
      says: new RegExp(emptyTitle),
      body: new RegExp(emptyBody.slice(0, 24)),
    });
  },
};

// a non-superadmin principal gets 403
export const Forbidden: Story = {
  render: () => <Harness fetchStub={async () => json({ error: { message: "forbidden" } }, 403)} />,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No nodes have reported in/);
  },
};

// draining the only live gateway would take the data plane offline; the server
// refuses it and the screen surfaces the refusal rather than swallowing it
export const RefusesDrainingTheLastGateway: Story = {
  render: () => {
    const stub: FetchStub = async (_input, init) => {
      if (init?.method === "PUT") {
        return json(
          {
            error: {
              message:
                "node gw-1 is the only live gateway still serving; draining it would take the data plane offline",
            },
          },
          400,
        );
      }
      return json([node()]);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // by name, not by index: each row control names its own node (#1214)
    await userEvent.click(await canvas.findByRole("button", { name: "Drain node gw-1" }));
    // the refusal used to sit in a line beside the node count; it is an
    // assertive toast now, carrying the control plane's own words (#1197)
    await expectToast(canvasElement, /only live gateway still serving/, "error");
  },
};

// the other half of the drain: the refusal has a story, the answer did not.
// draining is deliberately not confirmed — it is reversible from the same
// control, so a dialog would be ceremony — which makes the request itself the
// only thing a story can hold onto (#1607)
const drains = recording(async (_input, init) => {
  if (init?.method === "PUT") return json(node({ desired_state: "draining" }));
  return json([node()]);
});

export const DrainsANode: Story = {
  render: () => <Harness fetchStub={drains.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Drain node gw-1" }));
    const body = await drains.expectSentBody<{ draining: boolean }>(
      "PUT",
      "/cluster/nodes/gw-1/drain",
    );
    // `draining: true`, not a bare toggle: the same endpoint returns the node
    // to service, and sending the wrong flag would look identical on screen
    await expect(body.draining).toBe(true);
    await expectToast(canvasElement, /gw-1/);
  },
};

// forgetting a node that is still polling is pointless — it reappears on its
// next snapshot poll — so the action is only offered once it has gone stale
export const ForgetOnlyOfferedForStaleNodes: Story = {
  render: () => (
    <Harness fetchStub={async () => json([node(), node({ id: "gw-old", live: false })])} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the live node's Forget is refused, the stale one's is offered
    await expect(await canvas.findByRole("button", { name: "Forget node gw-1" })).toBeDisabled();
    await expect(canvas.getByRole("button", { name: "Forget node gw-old" })).toBeEnabled();
  },
};

// forgetting drops a node's history from the inventory, so it is confirmed by
// id — and the dialog repeats the thing that surprises people, that a node
// still running comes straight back (#1179)
const forgets = recording(async (_input, init) => {
  if (init?.method === "DELETE") return json({}, 204);
  return json([node(), node({ id: "gw-old", live: false })]);
});

const NODE_PATH = "/cluster/nodes/gw-old";

export const ConfirmsBeforeForgettingANode: Story = {
  render: () => <Harness fetchStub={forgets.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const stale = async () => canvas.findByRole("button", { name: "Forget node gw-old" });

    await userEvent.click(await stale());
    await cancelConfirmation();
    forgets.expectNotSent("DELETE", NODE_PATH);

    await userEvent.click(await stale());
    await confirmDestructive(/gw-old/, /forget node/i);
    await forgets.expectSent("DELETE", NODE_PATH);
  },
};

// What a non-superadmin gets, which is the screen refused before it asks
// (#1606).
//
// The node fleet is is a deployment-scoped resource, so `superadminOnly` never mounts the
// screen for an org role however high. The stub answers the screen's own
// request with a perfectly good payload on purpose: if the wrapper is dropped
// the screen renders that payload and this story fails, which the `Forbidden`
// story cannot do — it stubs the 403 itself, so it passes either way.
export const RefusedToAnAdmin: Story = {
  render: () => <Harness fetchStub={async () => json(FLEET)} role="admin" />,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

export const RefusedToAViewer: Story = {
  render: () => <Harness fetchStub={async () => json(FLEET)} role="viewer" />,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

/**
 * The forget is refused (#1607).
 *
 * `ConfirmDialog` owns the pending state, so the dialog has to stay open on a
 * refusal rather than closing over a node that is still in the inventory — the
 * operator would walk away believing it had gone.
 */
export const ForgetRejectedByTheServer: Story = {
  render: () => (
    <Harness
      fetchStub={async (_input, init) => {
        if (init?.method === "DELETE") {
          return json({ error: { message: "gw-old reported in while you were deciding" } }, 409);
        }
        return json([node(), node({ id: "gw-old", live: false })]);
      }}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Forget node gw-old" }));
    await confirmDestructive(/gw-old/, /forget node/i);
    await expectToast(canvasElement, /reported in while you were deciding/, "error");
    await waitFor(() => expect(within(document.body).getByRole("dialog")).toBeInTheDocument());
  },
};

// the same screen at a phone's width in both languages: Russian runs a third
// longer than English and overflowed twice as many screens (#2004)
const clusterFits = phoneFits({
  render: () => <Harness fetchStub={async () => json(FLEET)} />,
  ready: (canvas) => canvas.findByText("gw-old"),
});
export const MobileFits: Story = clusterFits("mobile", "en");
export const MobileFitsInRussian: Story = clusterFits("mobile", "ru");
