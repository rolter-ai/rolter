import type { Meta, StoryObj } from "@storybook/react";
import { useQuery } from "@tanstack/react-query";
import type * as React from "react";
import { Link, useLocation } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import FeatureFlags from "./FeatureFlags";
import {
  expectForbidden,
  expectSkeleton,
  expectToast,
  Harness as ScreenHarness,
  expectRetryRecovers,
  failingThenRecovering,
  json,
  recording,
  Toasted,
  type FetchStub,
  type StoryRole,
} from "./story-harness";
import { fetchConfig, type FeatureFlagsDto } from "@/lib/api";
import ru from "@/lib/i18n/locales/ru.json";

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
function Harness({
  fetchStub,
  role,
  children,
}: {
  fetchStub: FetchStub;
  role?: StoryRole;
  children?: React.ReactNode;
}) {
  return (
    <ScreenHarness fetchStub={fetchStub} role={role}>
      <Toasted>
        {children}
        <FeatureFlags />
      </Toasted>
    </ScreenHarness>
  );
}

// the confirmation lives in a portal, outside the canvas
const dialog = () => within(document.body).findByRole("dialog");

async function confirmChanges(_canvasElement: HTMLElement) {
  const d = within(await dialog());
  await userEvent.click(d.getByRole("button", { name: "Apply changes" }));
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
    await waitFor(() => expect(canvas.getAllByText("Unavailable")).toHaveLength(2));
    await expect(canvas.getByRole("switch", { name: "Response Cache" })).toBeDisabled();
    await expect(canvas.getByRole("switch", { name: "Circuit Breaker" })).toBeEnabled();
  },
};

// the badge and the lead-in are the dashboard's words; the reason after the
// colon is the control plane's own, so the lead-in says whose it is (#2098)
export const UnavailableInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness
      fetchStub={async () =>
        json({
          ...BASE,
          response_cache: false,
          unavailable: [
            { flag: "response_cache", reason: "no redis url is configured" },
            { flag: "cache_aware_routing", reason: "no provider publishes kv-cache events" },
          ],
        })
      }
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const { featureFlags } = ru.pages;
    await waitFor(() => expect(canvas.getAllByText(featureFlags.unavailable)).toHaveLength(2));
    await expect(
      canvas.getByText(
        featureFlags.unavailableReason.replace("{{reason}}", "no redis url is configured"),
      ),
    ).toBeVisible();
    await expect(canvas.queryByText("Unavailable")).toBeNull();
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
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await confirmChanges(canvasElement);
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
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await confirmChanges(canvasElement);
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
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await confirmChanges(canvasElement);

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

// nothing flipped yet: the stored state is the draft, so there is nothing to save
export const SaveWaitsForAChange: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const complexity = await canvas.findByRole("switch", { name: "Complexity Routing" });
    await expect(canvas.getByRole("button", { name: "Save changes" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await expect(canvas.getByText(/last saved/i)).toBeVisible();
    await userEvent.click(complexity);
    await expect(canvas.getByText("Not saved")).toBeVisible();
    await expect(canvas.getByRole("button", { name: "Save changes" })).not.toHaveAttribute(
      "aria-disabled",
    );
    // flipped back to the stored value, it is no longer a change
    await userEvent.click(complexity);
    await expect(canvas.queryByText("Not saved")).toBeNull();
    await expect(canvas.getByRole("button", { name: "Save changes" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  },
};

// the confirmation names each switch, its direction and, for guardrails, the
// consequence for every org
export const ConfirmShowsTheDiff: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("switch", { name: "Guardrails" }));
    await userEvent.click(await canvas.findByRole("switch", { name: "Complexity Routing" }));
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    const d = within(await dialog());
    await expect(d.getByText("Guardrails")).toBeVisible();
    await expect(d.getByText(/every pre- and post-call guardrail rule stops/i)).toBeVisible();
    await expect(d.getByText("Complexity Routing")).toBeVisible();
    await expect(d.getByText(/estimate prompt complexity/i)).toBeVisible();
    await expect(d.getByText(/for the whole deployment, every org/i)).toBeVisible();
    await expect(d.queryByText("Circuit Breaker")).toBeNull();
  },
};

// cancel closes the dialog, sends nothing and leaves the draft where it was
export const CancelLeavesStateUnchanged: Story = {
  render: () => {
    const rec = recording(async () => json(BASE));
    return <Harness fetchStub={rec.stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const guardrails = await canvas.findByRole("switch", { name: "Guardrails" });
    await userEvent.click(guardrails);
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    const d = within(await dialog());
    await userEvent.click(d.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(within(document.body).queryByRole("dialog")).toBeNull());
    await expect(guardrails).toHaveAttribute("aria-checked", "false");
    await expect(canvas.getByRole("button", { name: "Save changes" })).not.toHaveAttribute(
      "aria-disabled",
    );
  },
};

// confirming is what sends the PUT, with every flag
const putRecorder = recording(async (_input, init) => {
  if (init?.method === "PUT") return json({ ...BASE, ...JSON.parse(String(init.body)) });
  return json(BASE);
});

export const ConfirmSendsThePut: Story = {
  render: () => <Harness fetchStub={putRecorder.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("switch", { name: "Circuit Breaker" }));
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expect(await dialog()).toBeVisible();
    putRecorder.expectNotSent("PUT", "/feature-flags");
    await confirmChanges(canvasElement);
    const body = await putRecorder.expectSentBody<Record<string, boolean>>("PUT", "/feature-flags");
    await expect(body.circuit_breaker).toBe(false);
    await expect(body.guardrails).toBe(true);
    await expectToast(canvasElement, /feature flags updated/i);
  },
};

// stands in for the Effective config and Models screens, which share this key
function ConfigReader() {
  useQuery({ queryKey: ["config"], queryFn: fetchConfig });
  return null;
}

const configRecorder = recording(async (input, init) => {
  if (init?.method === "PUT") return json({ ...BASE, ...JSON.parse(String(init.body)) });
  if (String(input).includes("/api/v1/config")) return json({});
  return json(BASE);
});

// a saved flag rewrites the effective config, so the shared config query is
// fetched again rather than served from its cache (#2284)
export const SaveRefetchesTheEffectiveConfig: Story = {
  render: () => (
    <ScreenHarness fetchStub={configRecorder.stub}>
      <Toasted>
        <ConfigReader />
        <FeatureFlags />
      </Toasted>
    </ScreenHarness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const configReads = () =>
      configRecorder.calls.filter((c) => c.method === "GET" && c.url.includes("/api/v1/config"))
        .length;
    await userEvent.click(await canvas.findByRole("switch", { name: "Circuit Breaker" }));
    await waitFor(() => expect(configReads()).toBe(1));
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await confirmChanges(canvasElement);
    await expectToast(canvasElement, /feature flags updated/i);
    await waitFor(() => expect(configReads()).toBe(2));
  },
};

// stands in for the nav: a link away from the screen and a readout of where
// the router ended up
function Elsewhere() {
  const { pathname } = useLocation();
  return (
    <>
      <Link to="/elsewhere">Go elsewhere</Link>
      <output data-testid="pathname">{pathname}</output>
    </>
  );
}

function LeaveHarness() {
  return (
    <Harness fetchStub={async () => json(BASE)}>
      <Elsewhere />
    </Harness>
  );
}

/**
 * Leaving with a switch flipped asks first (#2584).
 *
 * Cancelling keeps the screen and the draft; confirming lets the navigation
 * through.
 */
export const LeavingDirtyAsksFirst: Story = {
  render: () => <LeaveHarness />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const complexity = await canvas.findByRole("switch", { name: "Complexity Routing" });
    await userEvent.click(complexity);
    await userEvent.click(canvas.getByRole("link", { name: "Go elsewhere" }));
    const d = within(await dialog());
    await expect(d.getByText("Discard unsaved changes?")).toBeVisible();
    await expect(canvas.getByTestId("pathname")).toHaveTextContent("/");
    await userEvent.click(d.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(within(document.body).queryByRole("dialog")).toBeNull());
    await expect(complexity).toHaveAttribute("aria-checked", "true");
    await expect(canvas.getByTestId("pathname")).toHaveTextContent("/");
    await userEvent.click(canvas.getByRole("link", { name: "Go elsewhere" }));
    await userEvent.click(within(await dialog()).getByRole("button", { name: "Discard" }));
    await waitFor(() => expect(canvas.getByTestId("pathname")).toHaveTextContent("/elsewhere"));
  },
};

// nothing was touched, so there is nothing to lose and no prompt
export const LeavingCleanDoesNotAsk: Story = {
  render: () => <LeaveHarness />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole("switch", { name: "Complexity Routing" });
    await userEvent.click(canvas.getByRole("link", { name: "Go elsewhere" }));
    await waitFor(() => expect(canvas.getByTestId("pathname")).toHaveTextContent("/elsewhere"));
    await expect(within(document.body).queryByRole("dialog")).toBeNull();
  },
};

// the control plane answers 5xx: the retry is on offer, reaches the network,
// and the screen comes back once the control plane does (#2220)
const featureFlagsDown = failingThenRecovering("/api/v1/feature-flags", async () => json(BASE));
export const LoadFailsAndRetries: Story = {
  render: () => {
    featureFlagsDown.reset();
    return <Harness fetchStub={featureFlagsDown.stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectRetryRecovers(
      canvasElement,
      featureFlagsDown,
      /failed to return feature flags/,
      async () =>
        await expect(await canvas.findByRole("switch", { name: "Circuit Breaker" })).toBeChecked(),
    );
  },
};
