import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import AdaptiveSettings from "./AdaptiveSettings";
import { Toasted, expectSkeleton, expectToast, expectForbidden } from "./story-harness";
import type { AdaptiveRoutingPolicyDto } from "@/lib/api";

const BASE: AdaptiveRoutingPolicyDto = {
  enabled: true,
  latency_weight: 1,
  cost_weight: 0.5,
  load_weight: 0.25,
  exploration_ratio: 0.05,
  min_samples: 50,
  updated_at: "2026-07-31T12:00:00Z",
  affected_routes: ["gpt-4o", "claude-sonnet"],
};

type FetchStub = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

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
        <AdaptiveSettings />
      </Toasted>
    </QueryClientProvider>
  );
}

const meta = {
  title: "Screens/AdaptiveSettings",
  component: AdaptiveSettings,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof AdaptiveSettings>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the blast radius of the kill switch is visible before it is flipped
    await waitFor(() => expect(canvas.getByText("gpt-4o")).toBeVisible());
    await expect(canvas.getByText("claude-sonnet")).toBeVisible();
  },
};

export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

// a deployment where nothing is on the adaptive strategy yet
export const NoAffectedRoutes: Story = {
  render: () => <Harness fetchStub={async () => json({ ...BASE, affected_routes: [] })} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(
        canvas.getByText("No route currently uses the adaptive strategy."),
      ).toBeVisible(),
    );
  },
};

// a non-superadmin principal gets 403
export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={async () => json({ error: { message: "forbidden" } }, 403)} />
  ),
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

// an all-zero blend does not stop adaptive routing, it turns the strategy into
// a random balancer — the kill switch is the honest way to stop it
export const RejectsAnAllZeroBlend: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    for (const label of ["Latency weight", "Cost weight", "Load weight"]) {
      const field = await canvas.findByLabelText(label);
      await userEvent.clear(field);
      await userEvent.type(field, "0");
    }
    await waitFor(() =>
      expect(canvas.getByText(/At least one weight must be positive/)).toBeVisible(),
    );
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled();
  },
};

// the gateway clamps above 0.5, so the value is refused rather than quietly
// reinterpreted
export const RejectsAnOutOfRangeExplorationRatio: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const ratio = await canvas.findByLabelText("Exploration ratio");
    await userEvent.clear(ratio);
    await userEvent.type(ratio, "0.9");
    await waitFor(() =>
      expect(
        canvas.getByText("Exploration ratio must be between 0 and 0.5."),
      ).toBeVisible(),
    );
  },
};

// interaction: a valid edit round-trips and confirms
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
    const cost = await canvas.findByLabelText("Cost weight");
    await userEvent.clear(cost);
    await userEvent.type(cost, "2");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /adaptive routing settings updated/i);
    await expect(canvas.getByLabelText("Cost weight")).toHaveValue("2");
  },
};
