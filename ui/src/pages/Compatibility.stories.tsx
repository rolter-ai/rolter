import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Compatibility from "./Compatibility";
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
import type { CompatibilityPolicyDto } from "@/lib/api";

const BASE: CompatibilityPolicyDto = {
  anthropic_version: "2023-06-01",
  default_max_tokens: 4096,
  updated_at: "2026-07-30T12:00:00Z",
  restart_required: [],
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
      <Compatibility />
    </Toasted>
    </ScreenHarness>
  );
}

const meta = {
  title: "Screens/Compatibility",
  component: Compatibility,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Compatibility>;

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

// the server owns the restart list, so the notice appears whenever it is
// non-empty without the screen knowing which fields are on it
export const RestartRequired: Story = {
  render: () => (
    <Harness
      fetchStub={async () =>
        json({ ...BASE, restart_required: ["anthropic_version"] })
      }
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("RESTART")).toBeVisible());
    await expect(canvas.getByText("anthropic_version")).toBeVisible();
  },
};

// the version is forwarded verbatim as the anthropic-version header, so a
// non-dated value is blocked before it can fail every Anthropic call upstream
export const RejectsAnUndatedVersion: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const version = await canvas.findByLabelText("Anthropic API version");
    await userEvent.clear(version);
    await userEvent.type(version, "latest");
    await waitFor(() =>
      expect(
        canvas.getByText("Anthropic version must be a dated release like 2023-06-01."),
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
    const tokens = await canvas.findByLabelText("Default max tokens");
    await userEvent.clear(tokens);
    await userEvent.type(tokens, "8192");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /compatibility settings updated/i);
    await expect(canvas.getByLabelText("Default max tokens")).toHaveValue("8192");
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
        return json({ error: { message: "8192 is above this deployment's ceiling of 4096" } }, 422);
      }
      return json(BASE);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const field = await canvas.findByLabelText("Default max tokens");
    await userEvent.clear(field);
    await userEvent.type(field, "8192");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /above this deployment's ceiling/, "error");
    await waitFor(() => expect(canvas.getByLabelText("Default max tokens")).toHaveValue("8192"));
  },
};

// What a non-superadmin gets, which is the screen refused before it asks
// (#1606).
//
// `the compatibility policy` is a deployment-scoped resource, so `superadminOnly` never mounts
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
