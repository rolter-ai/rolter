import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Performance from "./Performance";
import {
  expectForbidden,
  expectSkeleton,
  expectToast,
  Harness as ScreenHarness,
  json,
  pickOption,
  recording,
  Toasted,
  type FetchStub,
  type StoryRole,
} from "./story-harness";
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
        <Performance />
      </Toasted>
    </ScreenHarness>
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
  render: () => <Harness fetchStub={async () => json({ error: { message: "forbidden" } }, 403)} />,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

// queue off: the queue-scoped fields disable, since they only shape a queue
// that is not admitting anything
export const QueueDisabled: Story = {
  render: () => <Harness fetchStub={async () => json({ ...BASE, queue_enabled: false })} />,
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
      expect(canvas.getByText("Block backpressure needs a non-zero block timeout.")).toBeVisible(),
    );
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled();
    // giving it a timeout clears the block
    await userEvent.clear(canvas.getByLabelText("Block timeout (ms)"));
    await userEvent.type(canvas.getByLabelText("Block timeout (ms)"), "500");
    await waitFor(() => expect(canvas.getByRole("button", { name: "Save Changes" })).toBeEnabled());
  },
};

// the queue fields are disabled while the queue is off, so an out-of-range
// stored value must neither show an error nor block saving (#2645); turning
// the queue back on validates it again
export const QueueOffIgnoresAnOutOfRangeCapacity: Story = {
  render: () => (
    <Harness fetchStub={async () => json({ ...BASE, queue_enabled: false, queue_capacity: 0 })} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByLabelText("Capacity")).toBeDisabled());
    await expect(canvas.queryByText(/Queue capacity must be/)).toBeNull();
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeEnabled();
    await userEvent.click(canvas.getByRole("switch", { name: "Admission queue" }));
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled(),
    );
    await expect(canvas.getByText(/Queue capacity must be/)).toBeVisible();
  },
};

// the block timeout is disabled unless the policy is `block`, so the same
// holds for it: ignored under `error`, checked again once `block` is picked
export const BlockTimeoutIgnoredOutsideBlockMode: Story = {
  render: () => <Harness fetchStub={async () => json({ ...BASE, queue_block_ms: 999_999 })} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const mode = await canvas.findByLabelText("When the queue is full");
    await expect(canvas.getByLabelText("Block timeout (ms)")).toBeDisabled();
    await expect(canvas.queryByText(/Block timeout must be/)).toBeNull();
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeEnabled();
    await pickOption(mode, "block");
    await waitFor(() => expect(canvas.getByText(/Block timeout must be/)).toBeVisible());
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled();
  },
};

const switchingOff = recording(async (_input, init) =>
  init?.method === "PUT" ? json({ ...BASE, ...JSON.parse(String(init.body)) }) : json(BASE),
);

// a bad draft left behind by switching the queue off is not what gets saved:
// the field cannot be reached any more, so the stored value is kept
export const SwitchingTheQueueOffKeepsTheStoredValue: Story = {
  render: () => <Harness fetchStub={switchingOff.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const capacity = await canvas.findByLabelText("Capacity");
    await userEvent.clear(capacity);
    await userEvent.type(capacity, "0");
    await userEvent.click(canvas.getByRole("switch", { name: "Admission queue" }));
    const save = canvas.getByRole("button", { name: "Save Changes" });
    await expect(save).toBeEnabled();
    await userEvent.click(save);
    const body = await switchingOff.expectSentBody<RuntimePolicyDto>("PUT", "runtime-policy");
    await expect(body.queue_enabled).toBe(false);
    await expect(body.queue_capacity).toBe(BASE.queue_capacity);
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
      expect(canvas.getByText("Retry cap cannot be lower than the retry base.")).toBeVisible(),
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

/**
 * The save is refused (#1607).
 *
 * `SavesChanges` covers the answer; this covers the other one. The refusal
 * reaches the toast queue, and the form keeps the value that was typed rather
 * than snapping back to the setting the server last confirmed — a settings
 * screen that reverts on a rejected save loses the edit without saying so.
 */
export const SaveRejectedByTheServer: Story = {
  render: () => {
    const stub: FetchStub = async (_input, init) => {
      if (init?.method === "PUT") {
        return json({ error: { message: "4 retries exceeds the configured budget" } }, 422);
      }
      return json(BASE);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const field = await canvas.findByLabelText("Max retries");
    await userEvent.clear(field);
    await userEvent.type(field, "4");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /exceeds the configured budget/, "error");
    await waitFor(() => expect(canvas.getByLabelText("Max retries")).toHaveValue("4"));
  },
};

// What a non-superadmin gets, which is the screen refused before it asks
// (#1606).
//
// The runtime policy is is a deployment-scoped resource, so `superadminOnly` never mounts the
// screen for an org role however high. The stub answers the screen's own
// request with a perfectly good payload on purpose: if the wrapper is dropped
// the screen renders that payload and this story fails, which the `Forbidden`
// story cannot do — it stubs the 403 itself, so it passes either way.
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
