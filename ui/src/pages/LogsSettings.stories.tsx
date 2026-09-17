import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import LogsSettings from "./LogsSettings";
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
import type { LoggingSettingsDto } from "@/lib/api";

const BASE: LoggingSettingsDto = {
  sample_rate: 0.25,
  payload_capture_enabled: true,
  payload_capture_max_bytes: 65536,
  payload_capture_redact_fields: ["authorization", "api_key"],
  payload_capture_models: [],
  payload_capture_virtual_key_ids: [],
  retention_days: 90,
  payload_retention_hours: 168,
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
      <LogsSettings />
    </Toasted>
    </ScreenHarness>
  );
}

const meta = {
  title: "Screens/LogsSettings",
  component: LogsSettings,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof LogsSettings>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
};

/**
 * #954: what the switch actually does, said where it is thrown.
 *
 * The shipped default is off and stays off — rolter proxies other people's
 * traffic — so the operator turning it on is choosing to retain prompt text.
 * That choice deserves the retention window, the truncation limit and the
 * current redaction list stated at the point of decision, not three cards
 * further down the page. The summary reads from live form state, so an unsaved
 * edit is reflected in it.
 */
export const TheCaptureSwitchStatesWhatItStores: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const note = await waitFor(() => canvas.getByRole("note"));
    await expect(note).toHaveTextContent(/truncated at 65536 bytes/);
    await expect(note).toHaveTextContent(/kept for 168 hours/);
    await expect(note).toHaveTextContent(/authorization, api_key/);
    await expect(note).toHaveTextContent(/every model/);
  },
};

// with capture off the same note says the honest opposite: nothing is stored,
// and the metadata stream is untouched
export const TheCaptureSwitchStatesWhatItDoesNotStore: Story = {
  render: () => (
    <Harness
      fetchStub={async () => json({ ...BASE, payload_capture_enabled: false })}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const note = await waitFor(() => canvas.getByRole("note"));
    await expect(note).toHaveTextContent(/Nothing is stored/);
    await expect(note).toHaveTextContent(/Log metadata/);
  },
};

// payload capture off: the capture-scoped fields dim and disable, since they
// only narrow a capture that is not happening
export const CaptureDisabled: Story = {
  render: () => (
    <Harness
      fetchStub={async () => json({ ...BASE, payload_capture_enabled: false })}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByLabelText("Max bytes per payload")).toBeDisabled(),
    );
    await expect(canvas.getByLabelText("Redacted Fields")).toBeDisabled();
    // retention is not capture-scoped, so it stays editable
    await expect(canvas.getByLabelText("Retention days")).toBeEnabled();
  },
};

// the request never settles, so the skeleton stays up
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

// payloads outliving the metadata they belong to would leak prompt content the
// operator meant to expire, so save is blocked before the round trip
export const PayloadRetentionCannotOutliveLogs: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const days = await canvas.findByLabelText("Retention days");
    await userEvent.clear(days);
    await userEvent.type(days, "1");
    await waitFor(() =>
      expect(
        canvas.getByText("Payload retention cannot outlive log retention."),
      ).toBeVisible(),
    );
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled();
  },
};

// interaction: the percent field is converted to the 0..1 fraction the API takes
export const SavesSampleRateAsFraction: Story = {
  render: () => {
    let sent: unknown = null;
    const stub: FetchStub = async (_input, init) => {
      if (init?.method === "PUT") {
        sent = JSON.parse(String(init.body));
        return json({ ...BASE, ...(sent as object) });
      }
      return json(BASE);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const rate = await canvas.findByLabelText("Sample rate percent");
    await expect(rate).toHaveValue("25");
    await userEvent.clear(rate);
    await userEvent.type(rate, "10");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /logs settings updated/i);
    // the response echoes the stored fraction, which renders back as percent
    await expect(canvas.getByLabelText("Sample rate percent")).toHaveValue("10");
  },
};

// What a non-superadmin gets, which is the screen refused before it asks
// (#1606).
//
// The logging settings are is a deployment-scoped resource, so `superadminOnly` never mounts the
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
