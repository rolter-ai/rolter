import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import LogsSettings from "./LogsSettings";
import {
  expectAnalyticsUnavailable,
  expectForbidden,
  expectSkeleton,
  expectToast,
  Harness as ScreenHarness,
  expectRetryRecovers,
  failingThenRecovering,
  json,
  pickOption,
  scopeResponse,
  Toasted,
  type FetchStub,
  type StoryRole,
  openOptions,
} from "./story-harness";
import type { AnalyticsByModelRow, LoggingSettingsDto, VirtualKeyRow } from "@/lib/api";

const BASE: LoggingSettingsDto = {
  sample_rate: 0.25,
  payload_capture_enabled: true,
  payload_capture_max_bytes: 65536,
  payload_capture_redact_fields: ["authorization", "api_key"],
  payload_capture_redact_detectors: ["aws_key", "jwt"],
  payload_capture_redact_patterns: [],
  payload_capture_models: [],
  payload_capture_virtual_key_ids: [],
  retention_days: 90,
  payload_retention_hours: 168,
  ui_events: true,
  updated_at: "2026-07-30T12:00:00Z",
};

const model = (name: string): AnalyticsByModelRow => ({
  model: name,
  requests: 10,
  tokens: 1000,
  cost_usd: 1,
  unpriced_requests: 0,
  errors: 0,
  p50_latency_ms: 100,
  p95_latency_ms: 200,
});

const key = (id: string, name: string, prefix: string): VirtualKeyRow => ({
  id,
  project_id: "project-1",
  key_prefix: prefix,
  name,
  models: [],
  providers: [],
  disabled: false,
  created_by: null,
  business_unit_id: null,
  customer_id: null,
  created_at: "2026-01-01T00:00:00Z",
});

const CI_KEY = "key-ci-bot";
const SW_KEY = "key-support";
const KEYS = [key(CI_KEY, "ci-bot", "rk_live_ci01"), key(SW_KEY, "support-widget", "rk_live_sw02")];
const MODELS = [model("gpt-4o"), model("claude-sonnet-5")];

/** what the two pickers read beside the settings: the scope's keys and the recent models */
interface Reads {
  keys?: VirtualKeyRow[] | number;
  models?: AnalyticsByModelRow[] | number;
}

/**
 * Answers the reads the pickers make, and leaves the settings (and a save) to
 * `inner`. A number is the status of a refused read.
 */
function withReads(inner: FetchStub, { keys = KEYS, models = MODELS }: Reads): FetchStub {
  return async (input, init) => {
    const scope = scopeResponse(String(input));
    if (scope) return scope;
    const path = new URL(String(input), "http://localhost").pathname;
    const answer = (rows: unknown[] | number, wrap: boolean) =>
      typeof rows === "number"
        ? json({ error: { message: "refused" } }, rows)
        : json(wrap ? { data: rows } : rows);
    if (path === "/api/v1/analytics/by-model") return answer(models, true);
    if (/^\/api\/v1\/projects\/[^/]+\/virtual-keys$/.test(path)) return answer(keys, false);
    return inner(input, init);
  };
}

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
  reads = {},
}: {
  fetchStub: FetchStub;
  role?: StoryRole;
  reads?: Reads;
}) {
  return (
    <ScreenHarness fetchStub={withReads(fetchStub, reads)} role={role}>
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
 * The gateway drops an unsampled row before it reaches ClickHouse and the
 * analytics reads scale the rest back up (#2239), so at 25 % the Dashboard shows
 * an estimate built from a quarter of the traffic. The story starts at 100 %, where there is
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
      canvas.getByText(/scaled up from this sample, so they are estimates/),
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
      canvas.getByText(
        /Dashboard, LLM Logs, Business Units, Customers, Account usage and alert rules read this log/,
      ),
    ).toBeVisible();
    await expect(
      canvas.getByText(/Budgets, rate limits and \/metrics still count every request/),
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
    await waitFor(() => expect(rate).toHaveAttribute("aria-invalid", "true"));
    await expect(rate).toHaveAccessibleDescription(
      /Sample rate must be between 0 and 100 percent\./,
    );
    await expect(canvas.getByRole("button", { name: "Save changes" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
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
    await expect(canvas.getByLabelText("Redacted fields")).toBeDisabled();
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
    const maxBytes = () => canvas.getByLabelText("Max bytes per payload");
    await waitFor(() => expect(maxBytes()).toBeDisabled());
    await expect(maxBytes()).not.toHaveAttribute("aria-invalid", "true");
    await expect(canvas.queryByText(/Max payload bytes must be/)).toBeNull();
    await expect(canvas.queryByText(/attention/)).toBeNull();
    // re-enabling capture re-validates the value
    await userEvent.click(canvas.getByRole("switch", { name: /capture/i }));
    await waitFor(() => expect(maxBytes()).toHaveAttribute("aria-invalid", "true"));
    await expect(maxBytes()).toHaveAccessibleDescription(/Max payload bytes must be/);
    await expect(canvas.getByRole("button", { name: "Save changes" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
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
    const hours = canvas.getByLabelText("Payload retention hours");
    await waitFor(() => expect(hours).toHaveAttribute("aria-invalid", "true"));
    await expect(hours).toHaveAccessibleDescription(
      "Payload retention cannot outlive log retention.",
    );
    await expect(canvas.getByRole("button", { name: "Save changes" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  },
};

// every failing field is marked at once, each with its own message, and a
// press on Save moves focus to the first one (#2096)
export const MarksEveryInvalidField: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const days = await canvas.findByLabelText("Retention days");
    const hours = canvas.getByLabelText("Payload retention hours");
    await userEvent.clear(days);
    await userEvent.type(days, "0");
    await userEvent.clear(hours);
    await userEvent.type(hours, "9999");
    await waitFor(() => {
      expect(days).toHaveAttribute("aria-invalid", "true");
      expect(hours).toHaveAttribute("aria-invalid", "true");
    });
    await expect(days).toHaveAccessibleDescription(
      "Retention must be a whole number of days between 1 and 3650.",
    );
    await expect(hours).toHaveAccessibleDescription(
      "Payload retention must be a whole number of hours between 1 and 8760.",
    );
    await expect(canvas.getByText("2 fields need attention")).toBeVisible();
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expect(days).toHaveFocus();
  },
};

// a 400 that names a field lands on that field, with focus, not in a toast
export const ServerRejectionLandsOnTheField: Story = {
  render: () => {
    const stub: FetchStub = async (_input, init) => {
      if (init?.method === "PUT") {
        return json({ error: { message: "retention_days must be between 1 and 3650" } }, 400);
      }
      return json(BASE);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const days = await canvas.findByLabelText("Retention days");
    await userEvent.type(days, "0");
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(days).toHaveAttribute("aria-invalid", "true"));
    await expect(days).toHaveAccessibleDescription("retention_days must be between 1 and 3650");
    await waitFor(() => expect(days).toHaveFocus());
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
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
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
    const toggle = await canvas.findByRole("switch", { name: "Dashboard usage events" });
    await expect(toggle).toBeChecked();
    await userEvent.click(toggle);
    await expect(toggle).not.toBeChecked();
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expectToast(canvasElement, /logs settings updated/i);
    await expect(canvas.getByRole("switch", { name: "Dashboard usage events" })).not.toBeChecked();
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
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));

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

// the analytics reads the screen probes with: a control plane with no
// `clickhouse_url` answers 503 on every one of them
const NO_CLICKHOUSE = "analytics unavailable: no clickhouse_url configured";

const withoutClickHouse: FetchStub = async (input) =>
  String(input).includes("/api/v1/analytics/")
    ? json({ error: { message: NO_CLICKHOUSE } }, 503)
    : json(BASE);

const withClickHouse: FetchStub = async (input) =>
  String(input).includes("/api/v1/analytics/") ? json({ data: [] }) : json(BASE);

/**
 * #2092: with no ClickHouse nothing receives the log, and the screen says so.
 *
 * Every control only acts on rows written to ClickHouse, so a deployment
 * without one used to show them as live. The notice names the setting, says
 * the values are still stored, and is a calm `status` rather than an alert;
 * the controls stay editable and a save still goes through.
 */
export const NoClickHouseSaysNothingReceivesTheLog: Story = {
  // no toaster here: its empty live region is an alert, and the story asserts
  // that nothing on the screen is one
  render: () => (
    <ScreenHarness fetchStub={withoutClickHouse}>
      <LogsSettings />
    </ScreenHarness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const panel = await expectAnalyticsUnavailable(
      canvasElement,
      "No ClickHouse receives the log",
      NO_CLICKHOUSE,
    );
    await expect(panel).toHaveTextContent(/still saved and apply once ClickHouse is added/);
    await expect(panel).toHaveTextContent(/clickhouse_url/);
    const rate = await canvas.findByLabelText("Sample rate percent");
    await expect(rate).toBeEnabled();
    await expect(canvas.getByLabelText("Retention days")).toBeEnabled();
    await expect(canvas.getByRole("switch", { name: /capture/i })).toBeEnabled();
  },
};

// the settings are stored whether or not anything is listening
export const NoClickHouseStillSaves: Story = {
  render: () => {
    const stub: FetchStub = async (input, init) => {
      if (init?.method === "PUT") return json({ ...BASE, ...JSON.parse(String(init.body)) });
      return withoutClickHouse(input, init);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const rate = await canvas.findByLabelText("Sample rate percent");
    await userEvent.clear(rate);
    await userEvent.type(rate, "10");
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expectToast(canvasElement, /logs settings updated/i);
  },
};

// a control plane that has ClickHouse, or one whose probe fails for another
// reason, says nothing: only the 503 is a statement about the deployment
export const WithClickHouseThereIsNoNotice: Story = {
  render: () => <Harness fetchStub={withClickHouse} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByLabelText("Sample rate percent");
    await waitFor(() => expect(canvas.queryByText("No ClickHouse receives the log")).toBeNull());
  },
};

/**
 * #2094: the three capture filters sit inside the capture section, so they go
 * dead with its switch rather than reading as separate top-level settings.
 */
export const CaptureFiltersLiveInsideTheCaptureSection: Story = {
  render: () => <Harness fetchStub={withClickHouse} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const heading = await canvas.findByRole("heading", { name: "Capture request payloads" });
    const section = heading.closest("section")!;
    for (const name of [
      "Redacted fields",
      "Capture only these models",
      "Capture only these virtual keys",
    ]) {
      await expect(within(section).getByLabelText(name)).toBeEnabled();
    }
    await userEvent.click(canvas.getByRole("switch", { name: "Capture request payloads" }));
    await expect(within(section).getByLabelText("Redacted fields")).toBeDisabled();
    // the other switch is a panel of its own with the same treatment
    await expect(canvas.getByRole("heading", { name: "Dashboard usage events" })).toBeVisible();
  },
};

/**
 * #2094: the screen says when the policy last changed, and Save stays disabled
 * until something is edited.
 */
export const SaveWaitsForAnEdit: Story = {
  render: () => <Harness fetchStub={withClickHouse} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const save = await canvas.findByRole("button", { name: "Save changes" });
    await expect(canvas.getByText(/Last saved/)).toBeVisible();
    await expect(save).toHaveAttribute("aria-disabled", "true");
    // an edit that leaves the form valid, so Save reads as available
    const rate = canvas.getByLabelText("Sample rate percent");
    await userEvent.clear(rate);
    await userEvent.type(rate, "5");
    await waitFor(() => expect(save).not.toHaveAttribute("aria-disabled"));
    await expect(canvas.queryByText(/attention/)).toBeNull();
  },
};

/** the body of the PUT the story under way made, read back by its play function */
let sent: { body?: LoggingSettingsDto } = {};

/** a saving stub: the settings come back as given, a PUT is recorded and echoed */
function saving(stored: LoggingSettingsDto, record: typeof sent): FetchStub {
  return async (_input, init) => {
    if (init?.method === "PUT") {
      record.body = JSON.parse(String(init.body)) as LoggingSettingsDto;
      return json({ ...stored, ...record.body });
    }
    return json(stored);
  };
}

/**
 * A virtual key is picked by its name and prefix, and what is saved is its id
 * (#2089). The payload is the list of ids it always was.
 */
export const PicksAVirtualKeyByName: Story = {
  render: () => {
    sent = {};
    return (
      <Harness fetchStub={saving({ ...BASE, payload_capture_virtual_key_ids: [SW_KEY] }, sent)} />
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const list = await canvas.findByRole("list", { name: "Capture only these virtual keys" });
    // the saved key reads as its name once the key list has loaded
    await waitFor(() => expect(list).toHaveTextContent("support-widget (rk_live_sw02)"));
    await pickOption(
      canvas.getByRole("combobox", { name: "Add a virtual key to capture" }),
      /ci-bot/,
    );
    await expect(within(list).getByText("ci-bot (rk_live_ci01)")).toBeVisible();
    // an uuid is never shown for a key the list knows
    await expect(within(list).queryByText(CI_KEY)).not.toBeInTheDocument();
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expectToast(canvasElement, /logs settings updated/i);
    await expect(sent.body?.payload_capture_virtual_key_ids).toEqual([SW_KEY, CI_KEY]);
  },
};

/**
 * A saved key the list no longer has is kept and marked, not dropped (#2089).
 * It can be removed, and until then a save sends it unchanged.
 */
export const MarksASavedKeyThatNoLongerExists: Story = {
  render: () => {
    sent = {};
    const gone = "key-deleted";
    return (
      <Harness
        fetchStub={saving({ ...BASE, payload_capture_virtual_key_ids: [CI_KEY, gone] }, sent)}
      />
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const list = await canvas.findByRole("list", { name: "Capture only these virtual keys" });
    await waitFor(() => expect(list).toHaveTextContent("ci-bot (rk_live_ci01)"));
    const items = within(list).getAllByRole("listitem");
    await expect(items).toHaveLength(2);
    await expect(items[0]).toHaveTextContent("ci-bot (rk_live_ci01)");
    await expect(items[0]).not.toHaveTextContent("not in this project's keys");
    await expect(items[1]).toHaveTextContent("key-deleted");
    await expect(items[1]).toHaveTextContent("not in this project's keys");
    // the save bar stays off until something changed, so change something else
    const rate = canvas.getByLabelText("Sample rate percent");
    await userEvent.clear(rate);
    await userEvent.type(rate, "10");
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expectToast(canvasElement, /logs settings updated/i);
    await expect(sent.body?.payload_capture_virtual_key_ids).toEqual([CI_KEY, "key-deleted"]);
  },
};

/** removing the stale entry is what drops it */
export const RemovesAStaleKey: Story = {
  render: () => (
    <Harness
      fetchStub={async () => json({ ...BASE, payload_capture_virtual_key_ids: ["key-deleted"] })}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Remove key-deleted" }));
    await waitFor(() => expect(canvas.queryByText("key-deleted")).not.toBeInTheDocument());
  },
};

/**
 * A model is picked from the ones that carried traffic, and a name that has
 * not (a typo, or a model not yet used) is accepted but marked, so `gpt4o` is
 * no longer saved in silence.
 */
export const PicksAModelAndMarksAnUnseenOne: Story = {
  render: () => {
    sent = {};
    return <Harness fetchStub={saving({ ...BASE, payload_capture_models: ["gpt4o"] }, sent)} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const list = await canvas.findByRole("list", { name: "Capture only these models" });
    await waitFor(() => expect(within(list).getByText(/no traffic in 30 days/)).toBeVisible());
    await pickOption(
      canvas.getByRole("combobox", { name: "Add a model to capture" }),
      "claude-sonnet-5",
    );
    await expect(within(list).getByText("claude-sonnet-5")).toBeVisible();
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expectToast(canvasElement, /logs settings updated/i);
    await expect(sent.body?.payload_capture_models).toEqual(["gpt4o", "claude-sonnet-5"]);
  },
};

/**
 * #3486: the traffic read answers one row per organization's model, and two
 * organizations may each have a `gpt-4o`. A capture filter matches names, so
 * the picker offers the name once.
 */
export const AModelTwoOrganizationsServeIsOfferedOnce: Story = {
  render: () => (
    <Harness
      reads={{
        models: [
          { ...model("gpt-4o"), org_id: "org-1" },
          { ...model("gpt-4o"), org_id: "org-2" },
          { ...model("claude-sonnet-5"), org_id: "org-1" },
        ],
      }}
      fetchStub={async () => json(BASE)}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const combobox = await canvas.findByRole("combobox", { name: "Add a model to capture" });
    await waitFor(() => expect(combobox).toBeEnabled());
    const listbox = within(await openOptions(combobox));
    await waitFor(() => expect(listbox.getAllByRole("option", { name: "gpt-4o" })).toHaveLength(1));
    await expect(listbox.getAllByRole("option", { name: "claude-sonnet-5" })).toHaveLength(1);
  },
};

/**
 * With no list to check against (analytics unavailable, keys refused), nothing
 * is marked stale and a model can still be typed.
 */
export const MarksNothingWhenTheListsAreUnavailable: Story = {
  render: () => (
    <Harness
      reads={{ keys: 403, models: 503 }}
      fetchStub={async () =>
        json({
          ...BASE,
          payload_capture_models: ["gpt-4o"],
          payload_capture_virtual_key_ids: [CI_KEY],
        })
      }
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const keys = await canvas.findByRole("list", { name: "Capture only these virtual keys" });
    await expect(within(keys).getByText(CI_KEY)).toBeVisible();
    await waitFor(() =>
      expect(canvas.getByRole("combobox", { name: "Add a model to capture" })).toBeEnabled(),
    );
    await expect(canvas.queryByText(/not in this project's keys/)).not.toBeInTheDocument();
    await expect(canvas.queryByText(/no traffic in 30 days/)).not.toBeInTheDocument();
    const adder = canvas.getByRole("combobox", { name: "Add a model to capture" });
    await userEvent.type(adder, "gpt-4o-mini");
    await userEvent.click(
      await within(document.body).findByRole("option", { name: /gpt-4o-mini/ }),
    );
    await expect(
      within(canvas.getByRole("list", { name: "Capture only these models" })).getByText(
        "gpt-4o-mini",
      ),
    ).toBeVisible();
  },
};

// the control plane answers 5xx: the retry is on offer, reaches the network,
// and the screen comes back once the control plane does (#2220)
const logsSettingsDown = failingThenRecovering("/api/v1/logging-settings", async () => json(BASE));
export const LoadFailsAndRetries: Story = {
  render: () => {
    logsSettingsDown.reset();
    return <Harness fetchStub={logsSettingsDown.stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectRetryRecovers(
      canvasElement,
      logsSettingsDown,
      /failed to return logs settings/,
      async () => await expect(await canvas.findByRole("note")).toBeVisible(),
    );
  },
};

// both switches wait for Save, and each says so beside its own title (#2214)
export const SwitchesWaitForSave: Story = {
  render: () => <Harness fetchStub={withClickHouse} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const events = await canvas.findByRole("switch", { name: "Dashboard usage events" });
    await expect(canvas.queryByText("Not saved")).toBeNull();
    await userEvent.click(events);
    await expect(canvas.getAllByText("Not saved")).toHaveLength(1);
    await userEvent.click(canvas.getByRole("button", { name: "Discard changes" }));
    await expect(canvas.queryByText("Not saved")).toBeNull();
  },
};

/**
 * #1835: the preview runs the policy as edited against a pasted sample and
 * shows what capture would store, without saving anything. The request carries
 * the form's own detectors and patterns, so an unsaved edit is what is tried.
 */
export const PreviewShowsTheRedactedSample: Story = {
  render: () => {
    const stub: FetchStub = async (input, init) => {
      const path = new URL(String(input), "http://localhost").pathname;
      if (path === "/api/v1/logging-settings/redaction-preview") {
        const sent = JSON.parse(String(init?.body)) as {
          sample: string;
          redact_detectors: string[];
        };
        const hit = sent.redact_detectors.includes("aws_key");
        return json({
          output: hit ? "key [redacted:aws_key]" : sent.sample,
          report: { fields: 0, detections: hit ? { aws_key: 1 } : {} },
        });
      }
      return json(BASE);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const sample = await canvas.findByLabelText("Sample body");
    await userEvent.type(sample, "key AKIAIOSFODNN7EXAMPLE");
    await userEvent.click(canvas.getByRole("button", { name: "Run preview" }));
    await expect(await canvas.findByText("Found: aws_key ×1")).toBeVisible();
    await expect(canvas.getByText("key [redacted:aws_key]")).toBeVisible();
  },
};
