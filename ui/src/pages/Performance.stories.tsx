import type { Meta, StoryObj } from "@storybook/react";
import type * as React from "react";
import { Link, useLocation } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Performance from "./Performance";
import {
  expectForbidden,
  expectLoadError,
  expectNoFalseEmpty,
  openOptions,
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
import en from "@/lib/i18n/locales/en.json";

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
        <Performance />
        {children}
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
    await pickOption(mode, "Block");
    const block = canvas.getByLabelText("Block timeout (ms)");
    await waitFor(() => expect(block).toHaveAttribute("aria-invalid", "true"));
    await expect(block).toHaveAccessibleDescription(
      "Block backpressure needs a non-zero block timeout.",
    );
    await expect(canvas.getByRole("button", { name: "Save changes" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    // giving it a timeout clears the block
    await userEvent.clear(canvas.getByLabelText("Block timeout (ms)"));
    await userEvent.type(canvas.getByLabelText("Block timeout (ms)"), "500");
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: "Save changes" })).not.toHaveAttribute(
        "aria-disabled",
      ),
    );
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
    await expect(canvas.getByLabelText("Capacity")).not.toHaveAttribute("aria-invalid");
    await expect(canvas.queryByText(/attention/)).toBeNull();
    await userEvent.click(canvas.getByRole("switch", { name: "Admission queue" }));
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: "Save changes" })).toHaveAttribute(
        "aria-disabled",
        "true",
      ),
    );
    await expect(canvas.getByLabelText("Capacity")).toHaveAccessibleDescription(
      /Queue capacity must be/,
    );
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
    await expect(canvas.queryByText(/attention/)).toBeNull();
    await pickOption(mode, "Block");
    await waitFor(() =>
      expect(canvas.getByLabelText("Block timeout (ms)")).toHaveAccessibleDescription(
        /Block timeout must be/,
      ),
    );
    await expect(canvas.getByRole("button", { name: "Save changes" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
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
    const save = canvas.getByRole("button", { name: "Save changes" });
    await expect(save).not.toHaveAttribute("aria-disabled");
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
    await waitFor(() => expect(cap).toHaveAttribute("aria-invalid", "true"));
    await expect(cap).toHaveAccessibleDescription("Retry cap cannot be lower than the retry base.");
    await expect(canvas.getByRole("button", { name: "Save changes" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  },
};

// every failing field is marked at once, each with its own message, and a
// press on Save moves focus to the first one instead of doing nothing (#2651)
export const MarksEveryInvalidField: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const retries = await canvas.findByLabelText("Max retries");
    const workers = canvas.getByLabelText("Workers");
    await userEvent.clear(retries);
    await userEvent.type(retries, "11");
    await userEvent.clear(workers);
    await userEvent.type(workers, "0");
    await waitFor(() => {
      expect(retries).toHaveAttribute("aria-invalid", "true");
      expect(workers).toHaveAttribute("aria-invalid", "true");
    });
    await expect(retries).toHaveAccessibleDescription("Max retries must be between 0 and 10.");
    await expect(workers).toHaveAccessibleDescription("Queue workers must be between 1 and 2048.");
    await expect(canvas.getByText("2 fields need attention")).toBeVisible();
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expect(retries).toHaveFocus();
  },
};

// a 400 that names a field lands on that field, with focus, not in a toast
export const ServerRejectionLandsOnTheField: Story = {
  render: () => {
    const stub: FetchStub = async (_input, init) => {
      if (init?.method === "PUT") {
        return json({ error: { message: "timeout_request_s must be between 0 and 3600" } }, 400);
      }
      return json(BASE);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const request = await canvas.findByLabelText("Request (s)");
    await userEvent.clear(request);
    await userEvent.type(request, "60");
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(request).toHaveAttribute("aria-invalid", "true"));
    await expect(request).toHaveAccessibleDescription(
      "timeout_request_s must be between 0 and 3600",
    );
    await waitFor(() => expect(request).toHaveFocus());
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
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
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
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
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

// a read that failed for a reason a retry can mend (a 5xx, an unreachable
// control plane) says so and offers the retry; the form appears once it works
export const LoadFailedThenRetried: Story = {
  render: () => {
    let reads = 0;
    return (
      <Harness
        fetchStub={async () => {
          reads += 1;
          return reads === 1
            ? json({ error: { message: "the policy store is unavailable" } }, 503)
            : json(BASE);
        }}
      />
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /performance settings/i);
    await expectNoFalseEmpty(canvasElement, /max retries/i);
    await userEvent.click(canvas.getByRole("button", { name: en.errors.load.retry }));
    await expect(await canvas.findByLabelText("Max retries")).toHaveValue("2");
    await expect(canvas.queryByText("the policy store is unavailable")).toBeNull();
  },
};

/**
 * Save waits for an edit (#2123). A panel says it was edited, the footer counts
 * the settings, and Discard puts every field back to what was loaded. An edit
 * typed back to the stored value is no edit.
 */
export const SaveWaitsForAnEditAndDiscardRestores: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const retries = await canvas.findByLabelText("Max retries");
    const save = canvas.getByRole("button", { name: "Save changes" });
    const discard = canvas.getByRole("button", { name: "Discard changes" });
    await expect(save).toHaveAttribute("aria-disabled", "true");
    await expect(discard).toBeDisabled();
    await expect(canvas.queryAllByText("Edited")).toHaveLength(0);

    await userEvent.clear(retries);
    await userEvent.type(retries, "4");
    await expect(save).not.toHaveAttribute("aria-disabled");
    await expect(discard).toBeEnabled();
    await expect(canvas.getAllByText("Edited")).toHaveLength(1);
    await expect(canvas.getByText(/^1 change not saved yet/)).toBeVisible();

    await userEvent.clear(retries);
    await userEvent.type(retries, "2");
    await expect(save).toHaveAttribute("aria-disabled", "true");
    await expect(canvas.queryAllByText("Edited")).toHaveLength(0);

    await userEvent.type(retries, "{Backspace}5");
    await userEvent.clear(canvas.getByLabelText("Connect (s)"));
    await userEvent.type(canvas.getByLabelText("Connect (s)"), "30");
    await expect(canvas.getAllByText("Edited")).toHaveLength(2);
    await expect(canvas.getByText(/^2 changes not saved yet/)).toBeVisible();

    await userEvent.click(discard);
    await expect(retries).toHaveValue("2");
    await expect(canvas.getByLabelText("Connect (s)")).toHaveValue("10");
    await expect(canvas.queryAllByText("Edited")).toHaveLength(0);
    await expect(canvas.queryByText(/changes? not saved yet/)).toBeNull();
    await expect(save).toHaveAttribute("aria-disabled", "true");
  },
};

// the dropdown offers translated labels over the same wire values
export const BackpressureOptionsAreTranslated: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const mode = await canvas.findByLabelText("When the queue is full");
    await expect(mode).toHaveValue("Error");
    const listbox = await openOptions(mode);
    await expect(
      within(listbox)
        .getAllByRole("option")
        .map((o) => o.textContent),
    ).toEqual(["Drop", "Block", "Error"]);
  },
};

// the store keeps what was written, as the real one does, so the refetch after a
// save does not hand the old policy back
let stored: RuntimePolicyDto = BASE;
const saves = recording(async (_input, init) => {
  if (init?.method === "PUT") stored = { ...BASE, ...JSON.parse(String(init.body)) };
  return json(stored);
});

// a saved draft is no longer dirty: Save and Discard wait again
export const SavingClearsTheDirtyState: Story = {
  render: () => <Harness fetchStub={saves.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const retries = await canvas.findByLabelText("Max retries");
    await userEvent.clear(retries);
    await userEvent.type(retries, "4");
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await saves.expectSentBody<RuntimePolicyDto>("PUT", "runtime-policy");
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: "Save changes" })).toHaveAttribute(
        "aria-disabled",
        "true",
      ),
    );
    await expect(canvas.queryAllByText("Edited")).toHaveLength(0);
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

const dialog = () => within(document.body).findByRole("dialog");

/**
 * Leaving with an unsaved edit asks first (#2123, #2584): cancelling keeps the
 * screen and the draft, confirming lets the navigation through.
 */
export const LeavingDirtyAsksFirst: Story = {
  render: () => (
    <Harness fetchStub={async () => json(BASE)}>
      <Elsewhere />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const retries = await canvas.findByLabelText("Max retries");
    await userEvent.clear(retries);
    await userEvent.type(retries, "4");
    await userEvent.click(canvas.getByRole("link", { name: "Go elsewhere" }));
    const d = within(await dialog());
    await expect(d.getByText("Discard unsaved changes?")).toBeVisible();
    await expect(canvas.getByTestId("pathname")).toHaveTextContent("/");
    await userEvent.click(d.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(within(document.body).queryByRole("dialog")).toBeNull());
    await expect(retries).toHaveValue("4");
    await userEvent.click(canvas.getByRole("link", { name: "Go elsewhere" }));
    await userEvent.click(within(await dialog()).getByRole("button", { name: "Discard" }));
    await waitFor(() => expect(canvas.getByTestId("pathname")).toHaveTextContent("/elsewhere"));
  },
};

// nothing was touched, so there is nothing to lose and no prompt
export const LeavingCleanDoesNotAsk: Story = {
  render: () => (
    <Harness fetchStub={async () => json(BASE)}>
      <Elsewhere />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByLabelText("Max retries");
    await userEvent.click(canvas.getByRole("link", { name: "Go elsewhere" }));
    await waitFor(() => expect(canvas.getByTestId("pathname")).toHaveTextContent("/elsewhere"));
    await expect(within(document.body).queryByRole("dialog")).toBeNull();
  },
};

// the queue switch waits for Save like every other field of the screen (#2214)
export const QueueSwitchWaitsForSave: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const queue = await canvas.findByRole("switch", { name: "Admission queue" });
    await expect(canvas.queryByText("Not saved")).toBeNull();
    await userEvent.click(queue);
    await expect(canvas.getByText("Not saved")).toBeVisible();
    await userEvent.click(canvas.getByRole("button", { name: "Discard changes" }));
    await expect(canvas.queryByText("Not saved")).toBeNull();
  },
};
