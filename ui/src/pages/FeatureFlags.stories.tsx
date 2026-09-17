import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import FeatureFlags from "./FeatureFlags";
import { Toasted, expectSkeleton, expectToast, expectForbidden } from "./story-harness";
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

type FetchStub = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

// each story owns its query client so a cached result never leaks into the next.
// the stub is installed during render, not in an effect: child effects run
// before the parent's, so an effect would let the first real fetch through
function Harness({ fetchStub }: { fetchStub: FetchStub }) {
  const original = React.useRef<typeof globalThis.fetch | null>(null);
  const client = React.useMemo(() => {
    original.current ??= globalThis.fetch;
    globalThis.fetch = fetchStub as typeof globalThis.fetch;
    return new QueryClient({ defaultOptions: { queries: { retry: false } } });
  }, [fetchStub]);
  React.useEffect(
    () => () => {
      if (original.current) globalThis.fetch = original.current;
    },
    [],
  );
  return (
    <QueryClientProvider client={client}>
      <Toasted>
        <FeatureFlags />
      </Toasted>
    </QueryClientProvider>
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
  render: () => (
    <Harness fetchStub={async () => json({ error: { message: "forbidden" } }, 403)} />
  ),
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
