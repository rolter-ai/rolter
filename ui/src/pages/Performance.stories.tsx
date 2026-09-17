import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Performance from "./Performance";
import { Toasted, expectSkeleton, expectToast, pickOption, expectForbidden } from "./story-harness";
import type { RuntimePolicyDto } from "@/lib/api";

const BASE: RuntimePolicyDto = {
  retry_max_retries: 2,
  retry_base_ms: 200,
  retry_max_ms: 5000,
  timeout_connect_s: 10,
  timeout_request_s: 120,
  queue_enabled: true,
  queue_capacity: 1024,
  queue_workers: 32,
  queue_backpressure: "error",
  queue_block_ms: 0,
  updated_at: "2026-07-30T12:00:00Z",
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
        <Performance />
      </Toasted>
    </QueryClientProvider>
  );
}

const meta = {
  title: "Screens/Performance",
  component: Performance,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Performance>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
};

export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
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

// queue off: the queue-scoped fields disable, since they only shape a queue
// that is not admitting anything
export const QueueDisabled: Story = {
  render: () => (
    <Harness fetchStub={async () => json({ ...BASE, queue_enabled: false })} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByLabelText("Capacity")).toBeDisabled());
    await expect(canvas.getByLabelText("Workers")).toBeDisabled();
    // retries are not queue-scoped, so they stay editable
    await expect(canvas.getByLabelText("Max retries")).toBeEnabled();
  },
};

// block backpressure with a zero timeout would park callers forever, so the
// save is blocked before the round trip
export const BlockNeedsATimeout: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const mode = await canvas.findByLabelText("When the queue is full");
    // the block timeout only applies to block mode, so it starts disabled
    await expect(canvas.getByLabelText("Block timeout (ms)")).toBeDisabled();
    await pickOption(mode, "block");
    await waitFor(() =>
      expect(
        canvas.getByText("Block backpressure needs a non-zero block timeout."),
      ).toBeVisible(),
    );
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled();
    // giving it a timeout clears the block
    await userEvent.clear(canvas.getByLabelText("Block timeout (ms)"));
    await userEvent.type(canvas.getByLabelText("Block timeout (ms)"), "500");
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: "Save Changes" })).toBeEnabled(),
    );
  },
};

// a backoff cap below the base is incoherent and the server rejects it
export const RetryCapCannotBeBelowBase: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const cap = await canvas.findByLabelText("Backoff cap (ms)");
    await userEvent.clear(cap);
    await userEvent.type(cap, "10");
    await waitFor(() =>
      expect(
        canvas.getByText("Retry cap cannot be lower than the retry base."),
      ).toBeVisible(),
    );
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled();
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
    const retries = await canvas.findByLabelText("Max retries");
    await userEvent.clear(retries);
    await userEvent.type(retries, "4");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /performance settings updated/i);
    await expect(canvas.getByLabelText("Max retries")).toHaveValue("4");
  },
};
