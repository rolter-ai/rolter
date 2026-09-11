import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { AlertChannels, AlertHistory, AlertRules } from "./Alerting";
import {
  Harness,
  Toasted,
  cancelConfirmation,
  clickWhenEnabled,
  confirmDestructive,
  expectClosesWithoutPrompting,
  expectSheetClosed,
  expectSkeleton,
  expectToast,
  json,
  pending,
  recording,
  routes,
  scoped,
  sheet,
  withConfirm,
} from "./story-harness";
import type { AlertChannelRow, AlertNotificationRow, AlertRuleRow } from "@/lib/api";
import { atMobile, expectNoHorizontalOverflow } from "@/lib/story-viewport";

const CHANNELS: AlertChannelRow[] = [
  {
    id: "chan-1",
    name: "ops-slack",
    kind: "webhook",
    endpoint: "https://hooks.slack.com/services/T000/B000/xxx",
    enabled: true,
    secret_configured: true,
    created_at: "2026-05-01T00:00:00Z",
    updated_at: "2026-05-01T00:00:00Z",
  },
  {
    id: "chan-2",
    name: "pager",
    kind: "webhook",
    endpoint: "https://events.pagerduty.com/v2/enqueue",
    enabled: false,
    secret_configured: false,
    created_at: "2026-05-02T00:00:00Z",
    updated_at: "2026-05-02T00:00:00Z",
  },
];

const RULES: AlertRuleRow[] = [
  {
    id: "rule-1",
    name: "high error rate",
    signal: "error_rate",
    threshold: 0.05,
    window_secs: 300,
    channel_id: "chan-1",
    enabled: true,
    state: "firing",
    last_value: 0.11,
    last_evaluated_at: "2026-08-11T12:00:00Z",
    last_error: null,
    created_at: "2026-05-01T00:00:00Z",
    updated_at: "2026-08-11T12:00:00Z",
  },
  {
    id: "rule-2",
    name: "slow p95",
    signal: "p95_latency_ms",
    threshold: 2000,
    window_secs: 600,
    channel_id: null,
    enabled: true,
    state: "ok",
    last_value: 840,
    last_evaluated_at: "2026-08-11T12:00:00Z",
    last_error: null,
    created_at: "2026-05-02T00:00:00Z",
    updated_at: "2026-08-11T12:00:00Z",
  },
];

const HISTORY: AlertNotificationRow[] = [
  {
    id: "note-1",
    rule_id: "rule-1",
    channel_id: "chan-1",
    state: "firing",
    delivery_status: "delivered",
    detail: "error_rate 0.11 over 300s",
    sent_at: "2026-08-11T12:00:00Z",
  },
  {
    id: "note-2",
    rule_id: "rule-1",
    channel_id: "chan-1",
    state: "resolved",
    delivery_status: "failed",
    detail: "connection refused",
    sent_at: "2026-08-10T09:30:00Z",
  },
];

const loaded = routes([
  ["/alert-channels", () => CHANNELS],
  ["/alert-rules", () => RULES],
  ["/alert-notifications", () => HISTORY],
]);
const empty = routes([
  ["/alert-channels", () => []],
  ["/alert-rules", () => []],
  ["/alert-notifications", () => []],
]);
// every alerting endpoint is superadmin-only, so 403 is the state a normal
// operator actually sees — worth a story of its own rather than a generic error
const forbidden = scoped(async () => json({ error: { message: "forbidden" } }, 403));

const meta = {
  title: "Screens/Alerting",
  component: AlertChannels,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof AlertChannels>;
export default meta;
type Story = StoryObj<typeof meta>;

export const ChannelsLoaded: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <AlertChannels />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("ops-slack")).toBeInTheDocument();
    // a channel with a stored secret says so; one without must not claim it
    await expect(canvas.getByText("secret set")).toBeInTheDocument();
  },
};

export const ChannelsLoading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <AlertChannels />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

export const ChannelsEmpty: Story = {
  render: () => (
    <Harness fetchStub={empty}>
      <AlertChannels />
    </Harness>
  ),
  // a fresh deployment used to render nothing under the header here
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("No channels yet")).toBeVisible();
    await expect(canvas.getByRole("button", { name: "Add channel" })).toBeVisible();
  },
};

export const ChannelsForbidden: Story = {
  render: () => (
    <Harness fetchStub={forbidden}>
      <AlertChannels />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByText(/You do not have access to alert channels/i),
    ).toBeInTheDocument();
  },
};

export const CreatesAChannel: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (_input, init) =>
        init?.method === "POST" ? json(CHANNELS[0], 201) : json(CHANNELS),
      )}
    >
      <Toasted>
        <AlertChannels />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add channel/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Name"), "oncall");
    await userEvent.type(
      within(form).getByLabelText("Endpoint URL"),
      "https://hooks.example.com/alert",
    );
    await userEvent.click(within(form).getByRole("button", { name: "Create" }));
    await expectSheetClosed();
    // the sheet takes any inline confirmation with it, so the outcome is
    // asserted where it actually lives now (#1197)
    await expectToast(canvasElement, /oncall created/);
  },
};

export const AnUntouchedChannelFormClosesWithoutPrompting: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <AlertChannels />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add channel/i);
    await expectClosesWithoutPrompting();
  },
};

// a channel is the destination every rule delivers through, so deleting one
// strands rules that are still firing — it asks by name first (#1179)
const channelDeletes = recording(
  scoped(async (input, init) => {
    if (init?.method === "DELETE") return json({}, 204);
    return loaded(input, init);
  }),
);

export const ConfirmsBeforeDeletingAChannel: Story = {
  render: () => (
    <Harness fetchStub={channelDeletes.stub}>
      <AlertChannels />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("ops-slack")).toBeInTheDocument();
    // by name, not by index: each row control names its own channel (#1214)
    const button = canvas.getByRole("button", { name: "Delete channel ops-slack" });

    await userEvent.click(button);
    await cancelConfirmation();
    channelDeletes.expectNotSent("DELETE", "/alert-channels/chan-1");

    await userEvent.click(button);
    await confirmDestructive(/ops-slack/, /delete channel/i);
    await channelDeletes.expectSent("DELETE", "/alert-channels/chan-1");
  },
};

// the failure stays in the dialog rather than closing on a delete that never
// happened
export const ChannelDeleteFails: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        init?.method === "DELETE"
          ? json({ error: { message: "channel is referenced by 1 alert rule" } }, 409)
          : loaded(input, init),
      )}
    >
      <AlertChannels />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("ops-slack")).toBeInTheDocument();
    await userEvent.click(
      canvas.getByRole("button", { name: "Delete channel ops-slack" }),
    );
    await confirmDestructive(/ops-slack/, /delete channel/i);
    await waitFor(() =>
      expect(within(document.body).getByRole("alert")).toHaveTextContent(
        /referenced by 1 alert rule/,
      ),
    );
  },
};

export const RulesLoaded: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("high error rate")).toBeInTheDocument();
    // a rule with no channel still renders rather than blanking the card
    await expect(canvas.getByText("slow p95")).toBeInTheDocument();
  },
};

export const RulesLoading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

export const RulesEmpty: Story = {
  render: () => (
    <Harness fetchStub={empty}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("No alert rules")).toBeVisible();
    await expect(canvas.getByRole("button", { name: "Add rule" })).toBeVisible();
  },
};

export const RulesForbidden: Story = {
  render: () => (
    <Harness fetchStub={forbidden}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByText(/You do not have access to alert rules/i),
    ).toBeInTheDocument();
  },
};

export const CreatesARule: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        if (init?.method === "POST") return json(RULES[0], 201);
        return String(input).includes("/alert-channels") ? json(CHANNELS) : json(RULES);
      })}
    >
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add rule/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Name"), "spend spike");
    await userEvent.selectOptions(within(form).getByLabelText("Signal"), "spend_velocity");
    await userEvent.click(within(form).getByRole("button", { name: "Create" }));
    await expectSheetClosed();
  },
};

export const AnEditedRuleFormPromptsBeforeDiscarding: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add rule/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Name"), "half typed");

    await withConfirm(false, async () => {
      await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
      await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
    });
    await withConfirm(true, async () => {
      await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
      await expectSheetClosed();
    });
  },
};

const ruleDeletes = recording(
  scoped(async (input, init) => {
    if (init?.method === "DELETE") return json({}, 204);
    return loaded(input, init);
  }),
);

export const ConfirmsBeforeDeletingARule: Story = {
  render: () => (
    <Harness fetchStub={ruleDeletes.stub}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("high error rate")).toBeInTheDocument();
    const button = canvas.getByRole("button", {
      name: "Delete rule high error rate",
    });

    await userEvent.click(button);
    await cancelConfirmation();
    ruleDeletes.expectNotSent("DELETE", "/alert-rules/rule-1");

    await userEvent.click(button);
    await confirmDestructive(/high error rate/, /delete rule/i);
    await ruleDeletes.expectSent("DELETE", "/alert-rules/rule-1");
  },
};

export const HistoryLoaded: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <AlertHistory />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // a failed delivery is the row that matters most: the alert fired and
    // nobody was told
    await expect(await canvas.findByText("connection refused")).toBeInTheDocument();
  },
};

// the delivery table is a list, so its placeholder is a header bar over row
// bars rather than a word on one line
export const HistoryLoading: Story = {
  render: () => (
    <Harness fetchStub={() => new Promise<Response>(() => {})}>
      <AlertHistory />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

export const HistoryEmpty: Story = {
  render: () => (
    <Harness fetchStub={empty}>
      <AlertHistory />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/no alerts delivered yet/i)).toBeInTheDocument();
  },
};

export const HistoryForbidden: Story = {
  render: () => (
    <Harness fetchStub={forbidden}>
      <AlertHistory />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByText(/You do not have access to alert history/i),
    ).toBeInTheDocument();
  },
};

// the channel and rule toolbars wrap on a phone (#1242)
export const ChannelsMobile: Story = {
  ...atMobile,
  render: () => (
    <Harness fetchStub={loaded}>
      <AlertChannels />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("ops-slack");
    await expectNoHorizontalOverflow();
  },
};

export const RulesMobile: Story = {
  ...atMobile,
  render: () => (
    <Harness fetchStub={loaded}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("high error rate");
    await expectNoHorizontalOverflow();
  },
};
