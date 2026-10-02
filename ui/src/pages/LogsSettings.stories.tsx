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
  ui_events: true,
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
    <Harness fetchStub={async () => json({ ...BASE, payload_capture_enabled: false })} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const note = await waitFor(() => canvas.getByRole("note"));
    await expect(note).toHaveTextContent(/Nothing is stored/);
    await expect(note).toHaveTextContent(/Log metadata/);
  },
};

/**
 * #2088: lowering the sample rate shrinks every figure read from the log.
 *
 * The gateway drops an unsampled row before it reaches ClickHouse, and nothing
 * that reads the log scales the rest back up, so at 25 % the Dashboard shows
 * about a quarter of the real spend. The story starts at 100 %, where there is
 * nothing to warn about, sets 25 % and asserts the warning states the share
 * before anything is saved, and that the field carries it as its description.
 */
export const SettingARateBelowFullWarnsWithTheShare: Story = {
  render: () => <Harness fetchStub={async () => json({ ...BASE, sample_rate: 1 })} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const rate = await canvas.findByLabelText("Sample rate percent");
    await expect(rate).toHaveValue("100");
    await expect(canvas.queryByText(/The log keeps/)).toBeNull();

    await userEvent.clear(rate);
    await userEvent.type(rate, "25");
    await waitFor(() =>
      expect(canvas.getByText("The log keeps about 1 in 4 requests.")).toBeVisible(),
    );
    await expect(
      canvas.getByText(/read from it come to about 25% of the real figures/),
    ).toBeVisible();
    await expect(rate).toHaveAccessibleDescription(/about 1 in 4 requests/);
  },
};

// a saved rate below 100 % warns on load, not only while it is being edited
export const ASavedRateBelowFullWarnsOnLoad: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByLabelText("Sample rate percent")).toHaveValue("25");
    await expect(await canvas.findByText("The log keeps about 1 in 4 requests.")).toBeVisible();
  },
};

// at 100 % every request is logged, so there is no warning, and the hint still
// says what a lower rate would change and what it would not
export const FullSampleRateShowsNoWarning: Story = {
  render: () => <Harness fetchStub={async () => json({ ...BASE, sample_rate: 1 })} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByLabelText("Sample rate percent")).toHaveValue("100");
    await expect(canvas.queryByText(/The log keeps/)).toBeNull();
    await expect(
      canvas.getByText(/Dashboard, LLM Logs, Business Units and Customers/),
    ).toBeVisible();
    await expect(
      canvas.getByText(/budgets, rate limits and \/metrics still count every request/),
    ).toBeVisible();
  },
};

// 0 % logs nothing at all, which is its own sentence rather than "1 in infinity"
export const ZeroSampleRateSaysNothingIsLogged: Story = {
  render: () => <Harness fetchStub={async () => json({ ...BASE, sample_rate: 1 })} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const rate = await canvas.findByLabelText("Sample rate percent");
    await userEvent.clear(rate);
    await userEvent.type(rate, "0");
    await waitFor(() => expect(canvas.getByText("The log keeps no requests.")).toBeVisible());
    await expect(canvas.getByText(/get no new requests, spend or tokens/)).toBeVisible();
  },
};

// a cleared field is not 0 %: `Number("")` would have saved a policy that logs
// nothing, so save is refused and no share is claimed for an empty value
export const AClearedSampleRateCannotBeSaved: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const rate = await canvas.findByLabelText("Sample rate percent");
    await userEvent.clear(rate);
    await waitFor(() =>
      expect(canvas.getByText("Sample rate must be between 0 and 100 percent.")).toBeVisible(),
    );
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled();
    await expect(canvas.queryByText(/The log keeps/)).toBeNull();
  },
};

// payload capture off: the capture-scoped fields dim and disable, since they
// only narrow a capture that is not happening
export const CaptureDisabled: Story = {
  render: () => (
    <Harness fetchStub={async () => json({ ...BASE, payload_capture_enabled: false })} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByLabelText("Max bytes per payload")).toBeDisabled());
    await expect(canvas.getByLabelText("Redacted Fields")).toBeDisabled();
    // retention is not capture-scoped, so it stays editable
    await expect(canvas.getByLabelText("Retention days")).toBeEnabled();
  },
};

// the max-bytes field is disabled while capture is off, so an out-of-range
// stored value must neither show an error nor block saving (#2575); turning
// capture back on validates it again
export const CaptureOffIgnoresAnOutOfRangeMaxBytes: Story = {
  render: () => {
    const stored = {
      ...BASE,
      payload_capture_enabled: false,
      payload_capture_max_bytes: 2_000_000,
    };
    return <Harness fetchStub={async () => json(stored)} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByLabelText("Max bytes per payload")).toBeDisabled());
    await expect(canvas.queryByText(/Max payload bytes must be/)).toBeNull();
    const save = canvas.getByRole("button", { name: "Save Changes" });
    await expect(save).toBeEnabled();
    // re-enabling capture re-validates the value
    await userEvent.click(canvas.getByRole("switch", { name: /capture/i }));
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled(),
    );
    await expect(canvas.getByText(/Max payload bytes must be/)).toBeVisible();
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
  render: () => <Harness fetchStub={async () => json({ error: { message: "forbidden" } }, 403)} />,
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
      expect(canvas.getByText("Payload retention cannot outlive log retention.")).toBeVisible(),
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

/**
 * The UX event opt-out is a real, saved setting (#1748).
 *
 * Before the column existed the switch had nowhere to live, so a postgres
 * deployment could not turn the stream off at all. The story loads it on,
 * switches it off and asserts the PUT carried `ui_events: false` — the field
 * the ingest endpoint reads — and that the saved value renders back.
 */
export const SwitchesDashboardUsageEventsOff: Story = {
  render: () => {
    const stub: FetchStub = async (_input, init) => {
      if (init?.method === "PUT") {
        const sent = JSON.parse(String(init.body)) as Partial<LoggingSettingsDto>;
        if (sent.ui_events !== false) {
          return json({ error: { message: "ui_events was not sent as false" } }, 422);
        }
        return json({ ...BASE, ...sent });
      }
      return json(BASE);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const toggle = await canvas.findByRole("switch", { name: "Dashboard Usage Events" });
    await expect(toggle).toBeChecked();
    await userEvent.click(toggle);
    await expect(toggle).not.toBeChecked();
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /logs settings updated/i);
    await expect(canvas.getByRole("switch", { name: "Dashboard Usage Events" })).not.toBeChecked();
  },
};

/**
 * The save is refused (#1607).
 *
 * `SavesSampleRateAsFraction` covers the answer; this covers the other one. The
 * percent field keeps what was typed rather than reverting to the fraction the
 * server last confirmed.
 */
export const SaveRejectedByTheServer: Story = {
  render: () => {
    const stub: FetchStub = async (_input, init) => {
      if (init?.method === "PUT") {
        return json({ error: { message: "the collector rejected the sample rate" } }, 422);
      }
      return json(BASE);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const rate = await canvas.findByLabelText("Sample rate percent");
    await userEvent.clear(rate);
    await userEvent.type(rate, "10");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));

    await expectToast(canvasElement, /collector rejected the sample rate/, "error");
    await waitFor(() => expect(canvas.getByLabelText("Sample rate percent")).toHaveValue("10"));
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
