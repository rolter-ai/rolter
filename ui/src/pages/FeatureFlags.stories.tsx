import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import FeatureFlags from "./FeatureFlags";
import {
  expectForbidden,
  expectSkeleton,
  expectToast,
  Harness as ScreenHarness,
  json,
  Toasted,
  type FetchStub,
  type StoryRole,
} from "./story-harness";
import type { FeatureFlagsDto } from "@/lib/api";

const BASE: FeatureFlagsDto = {
  response_cache: true,
  cache_aware_routing: false,
  circuit_breaker: true,
  active_health_checks: true,
  complexity_routing: false,
  guardrails: true,
  updated_at: "2026-07-30T12:00:00Z",
  unavailable: [],
};

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
        <FeatureFlags />
      </Toasted>
    </ScreenHarness>
  );
}

const meta = {
  title: "Screens/FeatureFlags",
  component: FeatureFlags,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof FeatureFlags>;

export default meta;
type Story = StoryObj<typeof meta>;

// every flag available, the ordinary case
export const Loaded: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
};

// the request never settles, so the skeleton stays up
export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

// a non-superadmin principal gets 403; the screen says why rather than
// rendering switches it cannot save
export const Forbidden: Story = {
  render: () => <Harness fetchStub={async () => json({ error: { message: "forbidden" } }, 403)} />,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

// deployment facts make two subsystems unrunnable: those render as unavailable
// with the server's reason and their switch is disabled (#535)
export const Unavailable: Story = {
  render: () => (
    <Harness
      fetchStub={async () =>
        json({
          ...BASE,
          response_cache: false,
          unavailable: [
            {
              flag: "response_cache",
              reason: "no redis url is configured; cache entries are shared through redis",
            },
            {
              flag: "cache_aware_routing",
              reason: "no provider publishes kv-cache events or lmcache metrics",
            },
          ],
        })
      }
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("UNAVAILABLE")).toHaveLength(2));
    await expect(canvas.getByRole("switch", { name: "Response Cache" })).toBeDisabled();
    await expect(canvas.getByRole("switch", { name: "Circuit Breaker" })).toBeEnabled();
  },
};

/**
 * A flag switched on before its subsystem became unavailable (#1856).
 *
 * Every save carries every flag, and the server used to refuse any save with
 * an unavailable flag on in it — so with the switch disabled as well, nothing
 * on the screen could be saved again. Its switch now stays live while it is
 * on, says why, and turning it off saves.
 */
export const TurnsOffAFlagThatBecameUnavailable: Story = {
  render: () => {
    const stored: FeatureFlagsDto = {
      ...BASE,
      cache_aware_routing: true,
      unavailable: [
        {
          flag: "cache_aware_routing",
          reason: "no provider publishes kv-cache events or lmcache metrics",
        },
      ],
    };
    // the server keeps what it saved: the screen refetches after a save
    let current = stored;
    const stub: FetchStub = async (_input, init) => {
      if (init?.method === "PUT") {
        current = { ...current, ...JSON.parse(String(init.body)) };
      }
      return json(current);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const cacheAware = await canvas.findByRole("switch", { name: "Cache-Aware Routing" });
    await expect(cacheAware).toHaveAttribute("aria-checked", "true");
    await expect(cacheAware).toBeEnabled();
    await expect(canvas.getByText(/cannot be turned back on/i)).toBeVisible();
    await userEvent.click(cacheAware);
    await expect(cacheAware).toHaveAttribute("aria-checked", "false");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /feature flags updated/i);
    // saved off, it is an ordinary unavailable flag again: no way back on
    await waitFor(() => expect(cacheAware).toBeDisabled());
  },
};

// interaction: flipping a switch and saving PUTs the full flag set and shows
// the confirmation
export const SavesChanges: Story = {
  render: () => {
    const stub: FetchStub = async (_input, init) => {
      if (init?.method === "PUT") {
        return json({ ...BASE, ...JSON.parse(String(init.body)) });
      }
      return json(BASE);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const complexity = await canvas.findByRole("switch", { name: "Complexity Routing" });
    await expect(complexity).toHaveAttribute("aria-checked", "false");
    await userEvent.click(complexity);
    await expect(complexity).toHaveAttribute("aria-checked", "true");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /feature flags updated/i);
  },
};

/**
 * The save is refused (#1607).
 *
 * There is no text to lose here, so "the draft survives" is the switch: it has
 * to stay where the operator put it rather than snapping back to the flag the
 * server last confirmed, which would read as if the click had never landed.
 */
export const SaveRejectedByTheServer: Story = {
  render: () => {
    const stub: FetchStub = async (_input, init) => {
      if (init?.method === "PUT") {
        return json(
          { error: { message: "complexity routing needs an adaptive policy first" } },
          409,
        );
      }
      return json(BASE);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const complexity = await canvas.findByRole("switch", { name: "Complexity Routing" });
    await userEvent.click(complexity);
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));

    await expectToast(canvasElement, /needs an adaptive policy first/, "error");
    await waitFor(() => expect(complexity).toHaveAttribute("aria-checked", "true"));
  },
};

// What a non-superadmin gets, which is the screen refused before it asks
// (#1606).
//
// `the feature flags` is a deployment-scoped resource, so `superadminOnly` never mounts
// the screen for an org role however high. The stub answers the screen's own
// request with a perfectly good payload on purpose: if the wrapper is dropped
// the screen renders that payload and this story fails, which the `Forbidden`
// story below cannot do — it stubs the 403 itself, so it passes either way.
export const RefusedToAnAdmin: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} role="admin" />,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

export const RefusedToAViewer: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} role="viewer" />,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};
