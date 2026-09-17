import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import ModelSettings from "./ModelSettings";
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
import type { ModelDefaultsDto } from "@/lib/api";

const BASE: ModelDefaultsDto = {
  enabled: false,
  default_model: null,
  default_temperature: null,
  default_top_p: null,
  default_max_tokens: null,
  updated_at: "2026-08-05T12:00:00Z",
};

const CONFIGURED: ModelDefaultsDto = {
  ...BASE,
  enabled: true,
  default_model: "gpt-4o-mini",
  default_temperature: 0.7,
  default_max_tokens: 2048,
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
      <ModelSettings />
    </Toasted>
    </ScreenHarness>
  );
}

const meta = {
  title: "Screens/ModelSettings",
  component: ModelSettings,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof ModelSettings>;

export default meta;
type Story = StoryObj<typeof meta>;

// the shipped default: nothing set, nothing applied
export const Empty: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("inactive")).toBeVisible());
  },
};

export const Configured: Story = {
  render: () => <Harness fetchStub={async () => json(CONFIGURED)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("active")).toBeVisible());
    await expect(canvas.getByLabelText("Default model")).toHaveValue("gpt-4o-mini");
  },
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
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText(/You do not have access to model settings/)).toBeVisible(),
    );
  },
};

// enabled with nothing filled in is a no-op, and the screen says so rather
// than implying traffic is being changed
export const EnabledWithNoDefaults: Story = {
  render: () => <Harness fetchStub={async () => json({ ...BASE, enabled: true })} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("inactive")).toBeVisible());
    await expect(canvas.getByText(/No defaults are set yet/)).toBeVisible();
  },
};

// the value is forwarded to the provider, so an out-of-range one is blocked
// before it can fail every request
export const RejectsAnOutOfRangeTemperature: Story = {
  render: () => <Harness fetchStub={async () => json(CONFIGURED)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const temperature = await canvas.findByLabelText("Temperature");
    await userEvent.clear(temperature);
    await userEvent.type(temperature, "3");
    await waitFor(() =>
      expect(canvas.getByText("Temperature must be between 0 and 2.")).toBeVisible(),
    );
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled();
  },
};

// interaction: a valid edit round-trips and confirms
export const SavesChanges: Story = {
  render: () => {
    const stub: FetchStub = async (_input, init) => {
      if (init?.method === "PUT") {
        return json({ ...CONFIGURED, ...JSON.parse(String(init.body)) });
      }
      return json(CONFIGURED);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const tokens = await canvas.findByLabelText("Max tokens");
    await userEvent.clear(tokens);
    await userEvent.type(tokens, "4096");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /model settings updated/i);
    await expect(canvas.getByLabelText("Max tokens")).toHaveValue("4096");
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
        return json({ error: { message: "4096 is above the ceiling this gateway enforces" } }, 422);
      }
      return json(CONFIGURED);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const field = await canvas.findByLabelText("Max tokens");
    await userEvent.clear(field);
    await userEvent.type(field, "4096");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /above the ceiling/, "error");
    await waitFor(() => expect(canvas.getByLabelText("Max tokens")).toHaveValue("4096"));
  },
};

// What a non-superadmin gets, which is the screen refused before it asks
// (#1606).
//
// The model defaults are is a deployment-scoped resource, so `superadminOnly` never mounts the
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
