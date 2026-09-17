import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import ClientSettings from "./ClientSettings";
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
import type { ClientSettingsDto } from "@/lib/api";

const RESERVED = ["authorization", "x-api-key", "host", "cookie"];

const BASE: ClientSettingsDto = {
  public_base_url: null,
  forwarded_headers: [],
  injected_headers: {},
  request_id_header: "x-request-id",
  updated_at: "2026-08-05T12:00:00Z",
  always_propagated: ["traceparent", "tracestate", "b3"],
  reserved: RESERVED,
};

const CONFIGURED: ClientSettingsDto = {
  ...BASE,
  public_base_url: "https://gateway.example.com",
  forwarded_headers: ["x-tenant-id"],
  injected_headers: { "x-partner-id": "acme" },
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
      <ClientSettings />
    </Toasted>
    </ScreenHarness>
  );
}

const meta = {
  title: "Screens/ClientSettings",
  component: ClientSettings,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof ClientSettings>;

export default meta;
type Story = StoryObj<typeof meta>;

// the shipped default: no base URL override, no header policy
export const Empty: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText(/No headers injected/)).toBeVisible());
  },
};

export const Configured: Story = {
  render: () => <Harness fetchStub={async () => json(CONFIGURED)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByLabelText("Public base URL")).toHaveValue(
      "https://gateway.example.com",
    );
    await expect(canvas.getByLabelText("Injected header name 1")).toHaveValue(
      "x-partner-id",
    );
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
      expect(canvas.getByText(/You do not have access to client settings/)).toBeVisible(),
    );
  },
};

// forwarding the caller's Authorization header upstream would hand a provider
// credential to whoever asked, so the server rejects it and so does the screen
export const RejectsAReservedHeader: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const forwarded = await canvas.findByLabelText("Forwarded request headers");
    await userEvent.type(forwarded, "authorization");
    await waitFor(() =>
      expect(canvas.getByText("'authorization' is managed by the gateway.")).toBeVisible(),
    );
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled();
  },
};

export const RejectsANonHttpBaseUrl: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const url = await canvas.findByLabelText("Public base URL");
    await userEvent.type(url, "gateway.example.com");
    await waitFor(() =>
      expect(
        canvas.getByText("Base URL must start with http:// or https://."),
      ).toBeVisible(),
    );
  },
};

// interaction: adding an injected header round-trips and confirms
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
    await userEvent.click(await canvas.findByRole("button", { name: "Add header" }));
    await userEvent.type(
      canvas.getByLabelText("Injected header name 1"),
      "x-partner-id",
    );
    await userEvent.type(canvas.getByLabelText("Injected header value 1"), "acme");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /client settings updated/i);
    await expect(canvas.getByLabelText("Injected header value 1")).toHaveValue("acme");
  },
};

// What a non-superadmin gets, which is the screen refused before it asks
// (#1606).
//
// `the client settings` is a deployment-scoped resource, so `superadminOnly` never mounts
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
