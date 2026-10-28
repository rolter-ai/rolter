import type { Meta, StoryObj } from "@storybook/react-vite";
import { useLocation } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { AlertChannels, AlertHistory, AlertRules } from "./Alerting";
import {
  cancelConfirmation,
  clickWhenEnabled,
  confirmDestructive,
  expectAllowed,
  expectClosesWithoutPrompting,
  openOptions,
  expectSheetClosed,
  expectListTable,
  expectLoadError,
  expectNoFalseEmpty,
  expectSkeleton,
  expectToast,
  Harness,
  json,
  pending,
  pickOption,
  recording,
  routes,
  failingThenRecovering,
  scoped,
  sheet,
  Toasted,
  answerDiscardPrompt,
  type FetchStub,
} from "./story-harness";
import type { AlertChannelRow, AlertNotificationRow, AlertRuleRow } from "@/lib/api";
import { formattersFor } from "@/lib/i18n/format";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { expectFieldHints } from "@/lib/story-hints";
import { atMobile, atWide, expectInFrame, expectNoHorizontalOverflow } from "@/lib/story-viewport";

const CHANNELS: AlertChannelRow[] = [
  {
    id: "chan-1",
    name: "ops-slack",
    kind: "webhook",
    // a relay in front of the chat tool: the body is rolter's own shape, which
    // a chat or paging service does not accept directly
    endpoint: "https://alerts.example.com/rolter/slack",
    enabled: true,
    secret_configured: true,
    created_at: "2026-05-01T00:00:00Z",
    updated_at: "2026-05-01T00:00:00Z",
  },
  {
    id: "chan-2",
    name: "pager",
    kind: "webhook",
    endpoint: "https://alerts.example.com/rolter/pagerduty",
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
    comparison: "above",
    no_data: "ignore",
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
    comparison: "above",
    no_data: "ignore",
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
  {
    id: "rule-3",
    name: "spend spike",
    signal: "spend_velocity",
    threshold: 50,
    comparison: "above",
    no_data: "ignore",
    window_secs: 3600,
    channel_id: "chan-1",
    enabled: true,
    state: "error",
    last_value: 12.5,
    last_evaluated_at: "2026-08-11T11:00:00Z",
    last_error: "alert evaluation requires CLICKHOUSE_URL",
    created_at: "2026-05-03T00:00:00Z",
    updated_at: "2026-08-11T12:00:00Z",
  },
  // the two counts: requests and failed health events in the window, not rates
  {
    id: "rule-4",
    name: "traffic surge",
    signal: "request_volume",
    threshold: 1000,
    comparison: "above",
    no_data: "ignore",
    window_secs: 300,
    channel_id: "chan-1",
    enabled: true,
    state: "ok",
    last_value: 340,
    last_evaluated_at: "2026-08-11T12:00:00Z",
    last_error: null,
    created_at: "2026-05-04T00:00:00Z",
    updated_at: "2026-08-11T12:00:00Z",
  },
  {
    id: "rule-5",
    name: "provider trouble",
    signal: "provider_health_flaps",
    threshold: 10,
    comparison: "above",
    no_data: "ignore",
    window_secs: 300,
    channel_id: "chan-1",
    enabled: true,
    state: "ok",
    last_value: 1,
    last_evaluated_at: "2026-08-11T12:00:00Z",
    last_error: null,
    created_at: "2026-05-05T00:00:00Z",
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
    detail: "HTTP 200",
    sent_at: "2026-08-11T12:00:00Z",
  },
  {
    id: "note-2",
    rule_id: "rule-1",
    channel_id: "chan-1",
    state: "resolved",
    delivery_status: "failed",
    detail: "could not connect to the endpoint",
    sent_at: "2026-08-10T09:30:00Z",
  },
  {
    id: "note-3",
    rule_id: "rule-2",
    channel_id: null,
    state: "firing",
    delivery_status: "skipped",
    detail: "no channel configured",
    sent_at: "2026-08-09T08:00:00Z",
  },
];

/**
 * The history endpoint as the control plane answers it: newest first by
 * `(sent_at, id)`, narrowed by `rule_id`, `state` and `delivery_status`, and
 * paged by the keyset cursor `before=<sent_at>,<id>` (#2333).
 */
function historyApi(all: AlertNotificationRow[], fallback: FetchStub) {
  const newestFirst = [...all].sort(
    (a, b) => b.sent_at.localeCompare(a.sent_at) || b.id.localeCompare(a.id),
  );
  return scoped(async (input, init) => {
    const url = new URL(String(input), "http://localhost");
    if (!url.pathname.endsWith("/alert-notifications")) return fallback(input, init);
    const q = url.searchParams;
    const limit = Number(q.get("limit") ?? 100);
    const [sentAt, id] = (q.get("before") ?? "").split(",");
    const page = newestFirst
      .filter((n) => !q.get("rule_id") || n.rule_id === q.get("rule_id"))
      .filter((n) => !q.get("state") || n.state === q.get("state"))
      .filter((n) => !q.get("delivery_status") || n.delivery_status === q.get("delivery_status"))
      .filter((n) => !sentAt || n.sent_at < sentAt || (n.sent_at === sentAt && n.id < id));
    return json(page.slice(0, limit));
  });
}

const loaded = routes([
  ["/alert-channels", () => CHANNELS],
  ["/alert-rules", () => RULES],
  ["/alert-notifications", () => HISTORY],
  // a deployment that settles in euros, so a spend figure printed in dollars
  // is caught rather than matching the fallback
  ["/api/v1/currency", () => ({ base: "EUR", codes: ["EUR"], rates: {} })],
]);

const filtering = recording(historyApi(HISTORY, loaded));

/** the value under a rule card's figure label, read as the `dd` its `dt` names */
const stat = (card: HTMLElement, label: string) =>
  within(card).getByText(label, { selector: "dt" }).nextElementSibling;
const empty = routes([
  ["/alert-channels", () => []],
  ["/alert-rules", () => []],
  ["/alert-notifications", () => []],
]);
// every alerting endpoint is superadmin-only, so 403 is the state a normal
// operator actually sees — worth a story of its own rather than a generic error
const forbidden = scoped(async () => json({ error: { message: "forbidden" } }, 403));

// an empty history links to the rules screen, under the harness's router
function HistoryScreen() {
  return (
    <>
      <AlertHistory />
      <PathProbe />
    </>
  );
}

/** the router's path, on a data attribute with no text, so a play can read where a link went */
function PathProbe() {
  const { pathname } = useLocation();
  return <span data-testid="path" data-pathname={pathname} hidden />;
}

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
    // the kind is a catalog word, not the stored identifier (#2126)
    await expect(canvas.getAllByText("Webhook")).toHaveLength(2);
    await expect(canvas.queryByText("webhook")).toBeNull();
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
    await expectNoFalseEmpty(canvasElement, /No channels yet/);
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
    // the toolbar's button and the empty state's, both named by the words alone:
    // the `+` is an icon, so it is not part of the name (#2126)
    await expect(canvas.getAllByRole("button", { name: "Add channel" })).toHaveLength(2);
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
    await expectNoFalseEmpty(canvasElement, /No channels yet/);
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
    await userEvent.click(canvas.getByRole("button", { name: "Delete channel ops-slack" }));
    await confirmDestructive(/ops-slack/, /delete channel/i);
    await waitFor(() =>
      expect(within(document.body).getByRole("alert")).toHaveTextContent(
        /referenced by 1 alert rule/,
      ),
    );
  },
};

// a channel could only be deleted and added again, which cleared it off every
// rule delivering through it (#1873). an edit is one PUT that replaces the
// whole row, so it carries the switch through, and it leaves `managed_secret`
// out unless a new one was typed, which the API reads as "keep the stored one"
type ChannelBody = { name: string; endpoint: string; enabled: boolean; managed_secret?: string };

const editsChannel = () =>
  recording(
    scoped(async (input, init) => {
      if (init?.method === "PUT") return json(CHANNELS[0]);
      return loaded(input, init);
    }),
  );

const channelKeepsSecret = editsChannel();

export const EditsAChannelKeepingItsSecret: Story = {
  render: () => (
    <Harness fetchStub={channelKeepsSecret.stub}>
      <Toasted>
        <AlertChannels />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit channel ops-slack");
    const form = within(
      await within(document.body).findByRole("dialog", { name: "Edit channel ops-slack" }),
    );
    const name = form.getByLabelText("Name");
    await waitFor(() => expect(name).toHaveValue("ops-slack"));
    await expect(form.getByLabelText("Endpoint URL")).toHaveValue(
      "https://alerts.example.com/rolter/slack",
    );
    // the stored secret is never read back, and the field says what a blank does
    await expect(form.getByLabelText(/Bearer secret/)).toHaveValue("");
    await waitFor(() =>
      expect(
        form.getByText(
          "A secret is stored. Leave this blank to keep it, or type a new one to replace it.",
        ),
      ).toBeVisible(),
    );

    await userEvent.clear(name);
    await userEvent.type(name, "ops-chat");
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    const body = await channelKeepsSecret.expectSentBody<ChannelBody>(
      "PUT",
      "/alert-channels/chan-1",
    );
    // exactly these keys: no `managed_secret`, so the stored one stays
    await expect(body).toEqual({
      name: "ops-chat",
      endpoint: "https://alerts.example.com/rolter/slack",
      enabled: true,
    });
    channelKeepsSecret.expectNotSent("POST", "/alert-channels");
    await expectSheetClosed();
    await expectToast(canvasElement, /ops-chat updated/);
  },
};

const channelSwitchedOff = editsChannel();

// the add form creates a channel switched on, and an edit must not do the same
// to one somebody switched off
export const EditsASwitchedOffChannel: Story = {
  render: () => (
    <Harness fetchStub={channelSwitchedOff.stub}>
      <AlertChannels />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit channel pager");
    const form = within(
      await within(document.body).findByRole("dialog", { name: "Edit channel pager" }),
    );
    await waitFor(() => expect(form.getByLabelText("Name")).toHaveValue("pager"));
    await waitFor(() =>
      expect(form.getByText("No secret is stored. Type one to add it.")).toBeVisible(),
    );

    await userEvent.type(form.getByLabelText(/Bearer secret/), "pager-bearer");
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    const body = await channelSwitchedOff.expectSentBody<ChannelBody>(
      "PUT",
      "/alert-channels/chan-2",
    );
    await expect(body).toEqual({
      name: "pager",
      endpoint: "https://alerts.example.com/rolter/pagerduty",
      enabled: false,
      managed_secret: "pager-bearer",
    });
    await expectSheetClosed();
  },
};

const channelMoves = editsChannel();

// the API drops a stored secret when an edit moves the endpoint to another
// scheme, host or port and brings no new one, so the field says so before the
// save rather than the badge vanishing after it
export const AChannelEditSaysWhenTheSecretIsDropped: Story = {
  render: () => (
    <Harness fetchStub={channelMoves.stub}>
      <AlertChannels />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit channel ops-slack");
    const form = within(
      await within(document.body).findByRole("dialog", { name: "Edit channel ops-slack" }),
    );
    const endpoint = form.getByLabelText("Endpoint URL");
    await waitFor(() => expect(endpoint).toHaveValue("https://alerts.example.com/rolter/slack"));
    const keeps = /^A secret is stored\. Leave this blank to keep it/;
    const drops = /^The endpoint now points at another scheme, host or port/;

    // another path on the same origin keeps it
    await userEvent.clear(endpoint);
    await userEvent.type(endpoint, "https://alerts.example.com/rolter/chat");
    await waitFor(() => expect(form.getByText(keeps)).toBeVisible());
    await expect(form.queryByText(drops)).toBeNull();

    // another host does not
    await userEvent.clear(endpoint);
    await userEvent.type(endpoint, "https://hooks.example.net/rolter");
    await expect(form.getByText(drops)).toBeVisible();
    await expect(form.queryByText(keeps)).toBeNull();

    // unless the new receiver's own secret comes with it
    await userEvent.type(form.getByLabelText(/Bearer secret/), "receiver-bearer");
    await expect(form.getByText(keeps)).toBeVisible();
    await expect(form.queryByText(drops)).toBeNull();

    await userEvent.click(form.getByRole("button", { name: "Save" }));
    const body = await channelMoves.expectSentBody<ChannelBody>("PUT", "/alert-channels/chan-1");
    await expect(body.endpoint).toBe("https://hooks.example.net/rolter");
    await expect(body.managed_secret).toBe("receiver-bearer");
    await expectSheetClosed();
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
    // a rule whose evaluation failed says why on its card (#1871)
    await expect(canvas.getByText("Error")).toBeInTheDocument();
    await expect(canvas.getByText("alert evaluation requires CLICKHOUSE_URL")).toBeInTheDocument();
    // the states are catalog words, not the stored identifiers (#2126)
    await expect(canvas.getByText("Firing")).toBeInTheDocument();
    await expect(canvas.getAllByText("OK")).toHaveLength(3);
    await expect(canvas.queryByText(/^(firing|ok|error|unknown)$/)).toBeNull();
  },
};

// every figure on a card used to be a bare number: `THRESHOLD 0.05`, `WINDOW
// 300s`, and no way to tell a spend per hour from a count per window (#2125)
export const RuleCardsReadInTheSignalsUnit: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const cards: [string, string, RegExp, RegExp, RegExp][] = [
      // name, signal, threshold, last value, window
      ["high error rate", "Error rate", /^above 5%$/, /^11%$/, /^5m$/],
      ["slow p95", "p95 latency", /^above 2,000 ms$/, /^840 ms$/, /^10m$/],
      ["spend spike", "Spend rate", /^above €50\.00\/h$/, /^€12\.50\/h$/, /^1h$/],
      [
        "traffic surge",
        "Request volume",
        /^above 1,000 requests in 5m$/,
        /^340 requests in 5m$/,
        /^5m$/,
      ],
      [
        "provider trouble",
        "Provider health failures",
        /^above 10 failed health events in 5m$/,
        /^1 failed health event in 5m$/,
        /^5m$/,
      ],
    ];
    for (const [name, signal, threshold, last, window] of cards) {
      const card = await canvas.findByRole("article", { name });
      await expect(stat(card, "Signal")).toHaveTextContent(signal);
      // the spend waits on the settlement currency, which is its own request
      await waitFor(() => expect(stat(card, "Threshold")).toHaveTextContent(threshold));
      await expect(stat(card, "Last value")).toHaveTextContent(last);
      await expect(stat(card, "Window")).toHaveTextContent(window);
    }
  },
};

// the counts are declined, and the numbers take the locale's grouping
export const RuleCardsReadInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={loaded}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const { statThreshold, statLastValue } = ru.pages.alerting.rules;
    const traffic = await canvas.findByRole("article", { name: "traffic surge" });
    await waitFor(() =>
      expect(stat(traffic, statThreshold)).toHaveTextContent(/^выше 1\s000 запросов за 5\sмин$/),
    );
    await expect(stat(traffic, statLastValue)).toHaveTextContent(/^340 запросов за 5\sмин$/);
    const health = canvas.getByRole("article", { name: "provider trouble" });
    await expect(stat(health, statLastValue)).toHaveTextContent(/^1 отказ за 5\sмин$/);
    const errors = canvas.getByRole("article", { name: "high error rate" });
    await expect(stat(errors, statThreshold)).toHaveTextContent(/^выше 5\s%$/);
  },
};

// the form asked for a bare threshold whatever the signal, and started every
// one from 0.05 (#2125)
export const TheThresholdFollowsTheSignal: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add rule/i);
    const form = within(sheet());
    await expect(await form.findByLabelText("Threshold (%)")).toHaveValue(5);
    await waitFor(() => expect(form.getByText("From 0 to 100%.")).toBeVisible());
    await expect(
      form.getByText("Share of requests in the window that ended in a 5xx status."),
    ).toBeVisible();

    const signals: [RegExp, string, number, RegExp][] = [
      // option, threshold label, default, description
      [/^p95 latency/, "Threshold (ms)", 8000, /^95th-percentile request latency/],
      [/^Spend rate/, "Threshold (EUR per hour)", 50, /scaled to EUR per hour\.$/],
      [/^Request volume/, "Threshold (requests per window)", 1000, /a count, not a rate\.$/],
      [
        /^Provider health failures/,
        "Threshold (failed health events per window)",
        10,
        /^Provider health events in the window that were not ok/,
      ],
    ];
    for (const [option, label, value, description] of signals) {
      await pickOption(form.getByLabelText("Signal"), option);
      await expect(await form.findByLabelText(label)).toHaveValue(value);
      await expect(form.getByText(description)).toBeVisible();
    }
    // back on the percentage, the threshold is the percentage default again
    // rather than the 10 health events it held a moment ago
    await pickOption(form.getByLabelText("Signal"), /^Error rate/);
    await expect(await form.findByLabelText("Threshold (%)")).toHaveValue(5);
  },
};

// the operator types 5 meaning 5 %, the API stores the fraction the query
// returns, and the card reads the stored 0.05 back as 5 % (#2125)
const percentCreates = recording(
  scoped(async (input, init) => {
    if (init?.method === "POST") return json(RULES[0], 201);
    return loaded(input, init);
  }),
);

export const ThePercentThresholdRoundTrips: Story = {
  render: () => (
    <Harness fetchStub={percentCreates.stub}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const errors = await canvas.findByRole("article", { name: "high error rate" });
    await expect(stat(errors, "Threshold")).toHaveTextContent(/^above 5%$/);

    await clickWhenEnabled(canvasElement, /add rule/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Name"), "error spike");
    const threshold = within(form).getByLabelText("Threshold (%)");
    const create = within(form).getByRole("button", { name: "Create" });
    await expect(threshold).toHaveAttribute("max", "100");

    // the fraction never passes 1, so over 100 % is a rule that never fires
    await userEvent.clear(threshold);
    await userEvent.type(threshold, "101");
    await waitFor(() => expect(threshold).toHaveAttribute("aria-invalid", "true"));
    await expect(create).toHaveAttribute("aria-disabled", "true");
    percentCreates.expectNotSent("POST", "/alert-rules");

    await userEvent.clear(threshold);
    await userEvent.type(threshold, "5");
    await waitFor(() => expect(threshold).not.toHaveAttribute("aria-invalid"));
    await userEvent.click(create);
    const body = await percentCreates.expectSentBody<{ signal: string; threshold: number }>(
      "POST",
      "/alert-rules",
    );
    await expect(body.signal).toBe("error_rate");
    await expect(body.threshold).toBe(0.05);
    await expectSheetClosed();
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
    await expectNoFalseEmpty(canvasElement, /No alert rules/);
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
    await expect(canvas.getAllByRole("button", { name: "Add rule" })).toHaveLength(2);
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
    await expectNoFalseEmpty(canvasElement, /No alert rules/);
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
    await pickOption(within(form).getByLabelText("Signal"), /^Spend rate/);
    await userEvent.click(within(form).getByRole("button", { name: "Create" }));
    await expectSheetClosed();
  },
};

// the window input allowed 30 while the API refuses anything under 60, so a
// value the form accepted came back as a 400 (#1872)
const ruleCreates = recording(
  scoped(async (input, init) => {
    if (init?.method === "POST") return json(RULES[0], 201);
    return String(input).includes("/alert-channels") ? json(CHANNELS) : json(RULES);
  }),
);

export const TheRuleWindowHoldsToTheApiBounds: Story = {
  render: () => (
    <Harness fetchStub={ruleCreates.stub}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add rule/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Name"), "fast window");
    const windowInput = within(form).getByLabelText("Window (seconds)");
    const create = within(form).getByRole("button", { name: "Create" });
    await expect(windowInput).toHaveAttribute("min", "60");
    await expect(windowInput).toHaveAttribute("max", "86400");

    // under the floor, over the ceiling, and not a whole number of seconds
    for (const outside of ["30", "86401", "90.5"]) {
      await userEvent.clear(windowInput);
      await userEvent.type(windowInput, outside);
      await waitFor(() => expect(windowInput).toHaveAttribute("aria-invalid", "true"));
      await expect(create).toHaveAttribute("aria-disabled", "true");
      await waitFor(() =>
        expect(within(form).getByText("From 60 to 86400 seconds.")).toBeVisible(),
      );
    }
    ruleCreates.expectNotSent("POST", "/alert-rules");

    await userEvent.clear(windowInput);
    await userEvent.type(windowInput, "60");
    await waitFor(() => expect(windowInput).not.toHaveAttribute("aria-invalid"));
    await userEvent.click(create);
    const body = await ruleCreates.expectSentBody<{ window_secs: number }>("POST", "/alert-rules");
    await expect(body.window_secs).toBe(60);
    await expectSheetClosed();
  },
};

// a rule could only be deleted and added again, which took its history with
// it (#1873). the edit opens on the row in the form's units and sends one PUT
// with every field, the switch carried through as it was
type RuleBody = {
  name: string;
  signal: string;
  threshold: number;
  comparison?: string;
  no_data?: string;
  window_secs: number;
  channel_id: string | null;
  enabled: boolean;
};

const editsRules = (rules: AlertRuleRow[]) =>
  recording(
    scoped(async (input, init) => {
      if (init?.method === "PUT") return json(rules[0]);
      if (String(input).includes("/alert-rules")) return json(rules);
      return loaded(input, init);
    }),
  );

// switched off, so an edit that quietly switched it back on is caught
const ruleEdits = editsRules([{ ...RULES[0], enabled: false }, ...RULES.slice(1)]);

export const EditsARule: Story = {
  render: () => (
    <Harness fetchStub={ruleEdits.stub}>
      <Toasted>
        <AlertRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit rule high error rate");
    const form = within(
      await within(document.body).findByRole("dialog", { name: "Edit rule high error rate" }),
    );
    await waitFor(() => expect(form.getByLabelText("Name")).toHaveValue("high error rate"));
    // the stored fraction opens as the percentage it is typed in, not as the
    // signal's default
    const threshold = form.getByLabelText("Threshold (%)");
    await expect(threshold).toHaveValue(5);
    await expect(form.getByLabelText("Signal")).toHaveValue("Error rate");
    await expect(form.getByLabelText("Window (seconds)")).toHaveValue(300);
    await expect(form.getByLabelText("Channel")).toHaveValue("ops-slack");
    await waitFor(() =>
      expect(form.getByText("The rule keeps its state and history.")).toBeVisible(),
    );

    await userEvent.clear(threshold);
    await userEvent.type(threshold, "2");
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    const body = await ruleEdits.expectSentBody<RuleBody>("PUT", "/alert-rules/rule-1");
    await expect(body).toEqual({
      name: "high error rate",
      signal: "error_rate",
      threshold: 0.02,
      comparison: "above",
      no_data: "ignore",
      window_secs: 300,
      channel_id: "chan-1",
      enabled: false,
    });
    ruleEdits.expectNotSent("POST", "/alert-rules");
    await expectSheetClosed();
    await expectToast(canvasElement, /high error rate updated/);
  },
};

// the add form resets the threshold whenever a signal is picked; an edit that
// did the same on opening, or on picking the signal it already had, would
// overwrite the rule's own threshold with a default
export const AnEditKeepsTheThresholdUntilTheSignalChanges: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit rule slow p95");
    let form = within(
      await within(document.body).findByRole("dialog", { name: "Edit rule slow p95" }),
    );
    await waitFor(() => expect(form.getByLabelText("Threshold (ms)")).toHaveValue(2000));
    await expect(form.getByLabelText("Channel")).toHaveValue("none (record only)");
    await pickOption(form.getByLabelText("Signal"), /^p95 latency/);
    await expect(form.getByLabelText("Threshold (ms)")).toHaveValue(2000);
    // nothing changed, so nothing asks to be discarded
    await expectClosesWithoutPrompting();

    await clickWhenEnabled(canvasElement, "Edit rule slow p95");
    form = within(await within(document.body).findByRole("dialog", { name: "Edit rule slow p95" }));
    await waitFor(() => expect(form.getByLabelText("Threshold (ms)")).toHaveValue(2000));
    await pickOption(form.getByLabelText("Signal"), /^Request volume/);
    await expect(await form.findByLabelText("Threshold (requests per window)")).toHaveValue(1000);
  },
};

// the form holds twelve significant digits, and a rename must not round a
// threshold stored with more
const PRECISE: AlertRuleRow = { ...RULES[0], threshold: 0.123456789012345 };
const preciseEdits = editsRules([PRECISE, ...RULES.slice(1)]);

export const AnUntouchedThresholdGoesBackAsStored: Story = {
  render: () => (
    <Harness fetchStub={preciseEdits.stub}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit rule high error rate");
    const form = within(
      await within(document.body).findByRole("dialog", { name: "Edit rule high error rate" }),
    );
    const name = form.getByLabelText("Name");
    await waitFor(() => expect(name).toHaveValue("high error rate"));
    await expect(form.getByLabelText("Threshold (%)")).toHaveValue(12.3456789012);
    await userEvent.type(name, " (5xx)");
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    const body = await preciseEdits.expectSentBody<RuleBody>("PUT", "/alert-rules/rule-1");
    await expect(body.name).toBe("high error rate (5xx)");
    await expect(body.threshold).toBe(0.123456789012345);
    await expectSheetClosed();
  },
};

// the edit takes the authority the switch beside it does. a superadmin reaches
// every control
export const EditIsOfferedToASuperadmin: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="superadmin">
      <AlertChannels />
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, "Edit channel ops-slack");
    await expectAllowed(canvasElement, "Edit rule high error rate");
  },
};

// a deployment operator holds no membership anywhere, yet runs every rule and
// channel; the capability table publishes that (#3362), so the controls are
// enabled for them instead of read as a scoped admin's alone
export const EditIsOfferedToADeploymentOperator: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="operator">
      <AlertChannels />
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, "Edit channel ops-slack");
    await expectAllowed(canvasElement, "Edit rule high error rate");
    await expectAllowed(canvasElement, "Delete rule high error rate");
  },
};

// evaluating the rule reports a firing transition the channel refused
const deliveryFails = scoped(async (input, init) => {
  if (init?.method === "POST") {
    return json({
      rule: RULES[0],
      notified: false,
      notification: {
        id: "note-9",
        rule_id: "rule-1",
        channel_id: "chan-1",
        state: "firing",
        delivery_status: "failed",
        detail: "HTTP 500",
        sent_at: "2026-08-11T12:01:00Z",
      },
    });
  }
  return loaded(input, init);
});

// a transition the channel refused is an alert nobody received, so the toast
// says so instead of reading as a plain success (#1871)
export const EvaluateReportsAFailedDelivery: Story = {
  render: () => (
    <Harness fetchStub={deliveryFails}>
      <Toasted>
        <AlertRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Evaluate rule high error rate now");
    await expectToast(
      canvasElement,
      /Reported firing, but delivery failed: HTTP 500\. It is retried/,
      "error",
    );
  },
};

// the state in the toast was the stored identifier in every locale (#2126)
export const EvaluateNamesTheStateInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={deliveryFails}>
      <Toasted>
        <AlertRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(
      canvasElement,
      ru.pages.alerting.rules.evaluateAria.replace("{{name}}", "high error rate"),
    );
    await expectToast(
      canvasElement,
      /Отправлено состояние «сработало», но доставка не удалась/,
      "error",
    );
    await expect(within(canvasElement).queryByText(/состояние firing/)).toBeNull();
  },
};

// the backend records state=error and last_error before it answers with the
// failure, so the card has to be read again to show them (#1953)
export const EvaluateFailureRefreshesTheRuleCard: Story = {
  render: () => {
    let evaluated = false;
    return (
      <Harness
        fetchStub={scoped(async (input, init) => {
          const url = typeof input === "string" ? input : input.toString();
          if (init?.method === "POST" && url.includes("/evaluate")) {
            evaluated = true;
            return json({ error: { message: "could not connect to ClickHouse" } }, 502);
          }
          if (evaluated && url.includes("/alert-rules")) {
            return json([
              { ...RULES[0], state: "error", last_error: "could not connect to ClickHouse" },
              ...RULES.slice(1),
            ]);
          }
          return loaded(input, init);
        })}
      >
        <Toasted>
          <AlertRules />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await clickWhenEnabled(canvasElement, "Evaluate rule high error rate now");
    await expectToast(canvasElement, /The control plane hit an error/, "error");
    // on the card as well as in the toast, without a reload
    await waitFor(() =>
      expect(
        canvas
          .getAllByText("could not connect to ClickHouse")
          .some((node) => node.closest('[role="alert"]') === null),
      ).toBe(true),
    );
  },
};

export const EvaluateReportsADelivery: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        if (init?.method === "POST") {
          return json({
            rule: RULES[0],
            notified: true,
            notification: {
              id: "note-9",
              rule_id: "rule-1",
              channel_id: "chan-1",
              state: "firing",
              delivery_status: "delivered",
              detail: "HTTP 204",
              sent_at: "2026-08-11T12:01:00Z",
            },
          });
        }
        return loaded(input, init);
      })}
    >
      <Toasted>
        <AlertRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Evaluate rule high error rate now");
    await expectToast(canvasElement, /the channel accepted it \(HTTP 204\)/);
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

    await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
    await answerDiscardPrompt(false);
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();

    await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
    await answerDiscardPrompt(true);
    await expectSheetClosed();
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
    // the rule's history goes with it, which the dialog says before the click
    await expect(
      await within(document.body).findByText(/alert history is deleted with it/),
    ).toBeVisible();
    await confirmDestructive(/high error rate/, /delete rule/i);
    await ruleDeletes.expectSent("DELETE", "/alert-rules/rule-1");
  },
};

const ruleEvaluations = recording(
  scoped(async (input, init) => {
    if (init?.method === "POST" && String(input).includes("/evaluate")) {
      return json({ evaluated: true, firing: true }, 200);
    }
    return loaded(input, init);
  }),
);

export const EvaluatesARule: Story = {
  render: () => (
    <Harness fetchStub={ruleEvaluations.stub}>
      <Toasted>
        <AlertRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("high error rate")).toBeInTheDocument();
    const button = canvas.getByRole("button", {
      name: "Evaluate rule high error rate now",
    });

    await userEvent.click(button);
    await ruleEvaluations.expectSent("POST", "/alert-rules/rule-1/evaluate");
    await expectToast(canvasElement, /high error rate evaluated/);
  },
};

export const HistoryLoaded: Story = {
  render: () => (
    <Harness fetchStub={loaded} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // a failed delivery is the row that matters most: the alert fired and
    // nobody was told
    await expect(await canvas.findByText("could not connect to the endpoint")).toBeInTheDocument();
    // a transition with no channel is still recorded, as skipped
    await expect(canvas.getByText("no channel configured")).toBeInTheDocument();
    await expectListTable(canvasElement, "Alert History");
    // state and delivery are catalog words, not the stored identifiers (#2126)
    await expect(canvas.getAllByText("Firing")).toHaveLength(2);
    await expect(canvas.getByText("Resolved")).toBeInTheDocument();
    for (const delivery of ["Delivered", "Failed", "Skipped"]) {
      await expect(canvas.getByText(delivery)).toBeInTheDocument();
    }
    await expect(canvas.queryByText(/^(firing|resolved|delivered|failed|skipped)$/)).toBeNull();
    // three rows is everything there is, so nothing says older ones are missing
    await expect(canvas.queryByText(/older ones exist/)).toBeNull();
  },
};

// the delivery table is a list, so its placeholder is a header bar over row
// bars rather than a word on one line
export const HistoryLoading: Story = {
  render: () => (
    <Harness fetchStub={() => new Promise<Response>(() => {})}>
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No alert transitions yet/);
  },
};

export const HistoryEmpty: Story = {
  render: () => (
    <Harness fetchStub={empty} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/no alert transitions yet/i)).toBeInTheDocument();
    // the table keeps its header while it has no rows
    await expectListTable(canvasElement, "Alert History");
  },
};

// the link was a bare `<a href>`, which reloaded the whole app; it is a router
// link, so the path changes and the page stays (#2126)
export const TheEmptyHistoryLinksToTheRulesInApp: Story = {
  render: () => (
    <Harness fetchStub={empty} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const link = await canvas.findByRole("link", { name: "Open alert rules" });
    await expect(link).toHaveAttribute("href", "/alerting-rules");
    await expect(canvas.getByTestId("path").dataset.pathname).toBe("/alerting-history");
    await userEvent.click(link);
    await waitFor(() =>
      expect(canvas.getByTestId("path").dataset.pathname).toBe("/alerting-rules"),
    );
  },
};

export const HistoryForbidden: Story = {
  render: () => (
    <Harness fetchStub={forbidden} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByText(/You do not have access to alert history/i),
    ).toBeInTheDocument();
    await expectNoFalseEmpty(canvasElement, /No alert transitions yet/);
  },
};

// --- #2126: translated states, one tone per fault, dates, history filters ------

/** a colour token as the browser resolves it, to compare with what the screen painted */
function tokenColor(name: string): string {
  const probe = document.createElement("span");
  probe.style.color = `var(${name})`;
  document.body.appendChild(probe);
  const resolved = getComputedStyle(probe).color;
  probe.remove();
  return resolved;
}

/** the status dot on a rule card: the first element with an inline background */
const dotOf = (card: HTMLElement) => card.querySelector<HTMLElement>('span[style*="background"]')!;

// the dot took the -text half of the hue, and a rule whose evaluation failed
// drew an amber pill over a red error line (#2126). a shape takes the fill, a
// label takes the -text half, and one fault has one tone
export const RuleStatesUseOneHuePerMeaning: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const colour = (node: Element) => getComputedStyle(node).color;
    const fill = (node: Element) => getComputedStyle(node).backgroundColor;

    const firing = await canvas.findByRole("article", { name: "high error rate" });
    await expect(fill(dotOf(firing))).toBe(tokenColor("--status-danger"));
    await expect(fill(dotOf(firing))).not.toBe(tokenColor("--status-danger-text"));
    await expect(colour(within(firing).getByText("Firing"))).toBe(
      tokenColor("--status-danger-text"),
    );

    const ok = canvas.getByRole("article", { name: "slow p95" });
    await expect(fill(dotOf(ok))).toBe(tokenColor("--status-success"));
    await expect(colour(within(ok).getByText("OK"))).toBe(tokenColor("--status-success-text"));

    // the failed evaluation: the dot, the pill and the line under the card agree,
    // and none of them is the red a breach is painted in
    const failed = canvas.getByRole("article", { name: "spend spike" });
    const pill = colour(within(failed).getByText("Error"));
    const line = colour(within(failed).getByText("alert evaluation requires CLICKHOUSE_URL"));
    await expect(fill(dotOf(failed))).toBe(tokenColor("--status-warning"));
    await expect(pill).toBe(tokenColor("--status-warning-text"));
    await expect(line).toBe(pill);
    await expect(line).not.toBe(tokenColor("--status-danger-text"));
  },
};

// half a unit over, so the figure a card reads is the same side of the rounding
// however long the story waited between the fetch and the screen's own clock
const ago = (seconds: number) => new Date(Date.now() - seconds * 1000).toISOString();

// "Evaluated" was a clock time with no date, so it read `14:00:00` whether that
// was a minute or three days ago (#2126)
export const EvaluatedSaysHowLongAgo: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        String(input).includes("/alert-rules")
          ? json([
              { ...RULES[0], last_evaluated_at: ago(3.5 * 60) },
              { ...RULES[1], last_evaluated_at: ago(3.5 * 86_400) },
              { ...RULES[2], last_evaluated_at: null },
            ])
          : loaded(input, init),
      )}
    >
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const fmt = formattersFor("en");
    const recent = await canvas.findByRole("article", { name: "high error rate" });
    await expect(stat(recent, "Evaluated")).toHaveTextContent(/^3 min\. ago$/);
    const old = canvas.getByRole("article", { name: "slow p95" });
    await expect(stat(old, "Evaluated")).toHaveTextContent(/^3 days ago$/);
    // the exact stamp, with its date, is on hover
    const when = within(old).getByText("3 days ago");
    await expect(when).toHaveAttribute("title", fmt.dateTime(when.getAttribute("datetime")!));
    await expect(when.getAttribute("title")).not.toBe(fmt.time(when.getAttribute("datetime")!));
    // a rule never evaluated has nothing to be relative to
    const never = canvas.getByRole("article", { name: "spend spike" });
    await expect(stat(never, "Evaluated")).toHaveTextContent(/^never$/);
    await expect(never.querySelector("time")).toBeNull();
  },
};

// every stored identifier, in the locale the operator reads (#2126)
export const ChannelsReadInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={loaded}>
      <AlertChannels />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findAllByText(ru.pages.alerting.channels.kinds.webhook)).toHaveLength(
      2,
    );
    await expect(canvas.queryByText(/^webhook$/i)).toBeNull();
  },
};

export const RuleStatesReadInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={loaded}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const { states } = ru.pages.alerting;
    await expect(await canvas.findByText(states.firing)).toBeInTheDocument();
    await expect(canvas.getAllByText(states.ok)).toHaveLength(3);
    await expect(canvas.getByText(states.error)).toBeInTheDocument();
    await expect(canvas.queryByText(/^(firing|ok|error|unknown)$/i)).toBeNull();
    // the evaluation time is relative, in Russian too
    const card = canvas.getByRole("article", { name: "high error rate" });
    await expect(stat(card, ru.pages.alerting.rules.statEvaluated)).toHaveTextContent(/назад/);
  },
};

export const HistoryReadInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={filtering.stub} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const { states, deliveries, history } = ru.pages.alerting;
    await expect(await canvas.findAllByText(states.firing)).toHaveLength(2);
    await expect(canvas.getByText(states.resolved)).toBeInTheDocument();
    for (const word of [deliveries.delivered, deliveries.failed, deliveries.skipped]) {
      await expect(canvas.getByText(word)).toBeInTheDocument();
    }
    await expect(canvas.queryByText(/^(firing|resolved|delivered|failed|skipped)$/i)).toBeNull();
    // the filters are in Russian as well, closed and open
    await expect(canvas.getByLabelText(history.stateFilterAria)).toHaveValue(history.allStates);
    await expect(canvas.getByLabelText(history.deliveryFilterAria)).toHaveValue(
      history.allDeliveries,
    );
    await pickOption(canvas.getByLabelText(history.deliveryFilterAria), deliveries.failed);
    await waitFor(() => expect(canvas.queryByText(deliveries.delivered)).toBeNull());
    await expect(canvas.getByText(deliveries.failed)).toBeInTheDocument();
  },
};

// --- the history: paged by keyset, filtered by the API (#3049) --------------

// more rows than one page holds: the first read comes back full, so older rows
// may exist, and the next page starts strictly below the last row read
const MANY: AlertNotificationRow[] = Array.from({ length: 150 }, (_, i) => ({
  ...HISTORY[i % HISTORY.length],
  id: `note-${String(i).padStart(3, "0")}`,
  sent_at: new Date(Date.UTC(2026, 7, 11, 12) - i * 60_000).toISOString(),
}));

const manyApi = historyApi(MANY, loaded);
const pagedHistory = recording(manyApi);

export const HistoryPagesOlderRows: Story = {
  render: () => (
    <Harness fetchStub={pagedHistory.stub} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByText("100 newest transitions loaded · older ones follow"),
    ).toBeInTheDocument();
    await pagedHistory.expectSent("GET", "limit=100");
    const table = canvas.getByRole("table", { name: "Alert History" });
    await expect(within(table).getAllByRole("row")).toHaveLength(101);

    // the cursor is the last row's `<sent_at>,<id>`, so a row written in the
    // same instant is neither skipped nor shown twice
    await userEvent.click(canvas.getByRole("button", { name: "Load older transitions" }));
    await pagedHistory.expectSent(
      "GET",
      `before=${encodeURIComponent(`${MANY[99].sent_at},${MANY[99].id}`)}`,
    );
    await expect(await canvas.findByText("150 transitions · newest first")).toBeInTheDocument();
    await expect(within(table).getAllByRole("row")).toHaveLength(151);
    // the page was short, so that was the end
    await expect(canvas.queryByRole("button", { name: "Load older transitions" })).toBeNull();
    await expect(canvas.queryByText(/older ones exist/)).toBeNull();
  },
};

// a filter starts the list over at the newest row of what it matches
export const HistoryFilterRestartsThePaging: Story = {
  render: () => (
    <Harness fetchStub={pagedHistory.stub} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Load older transitions" }));
    await expect(await canvas.findByText("150 transitions · newest first")).toBeInTheDocument();
    await pickOption(canvas.getByLabelText("Filter by delivery"), "Skipped");
    // 50 of the 150 are skipped: one page, no older button
    await expect(await canvas.findByText("50 transitions · newest first")).toBeInTheDocument();
    await expect(canvas.queryByRole("button", { name: "Load older transitions" })).toBeNull();
  },
};

// an older page that fails keeps the rows already read, and retries from there
const olderFails = (() => {
  let failing = true;
  return {
    stub: scoped(async (input, init) => {
      const url = new URL(String(input), "http://localhost");
      if (failing && url.searchParams.has("before")) {
        return json({ error: { message: "control plane unavailable" } }, 500);
      }
      return manyApi(input, init);
    }),
    recover: () => {
      failing = false;
    },
    reset: () => {
      failing = true;
    },
  };
})();

export const HistoryOlderPageFailsAndRetries: Story = {
  render: () => {
    olderFails.reset();
    return (
      <Harness fetchStub={olderFails.stub} route="/alerting-history">
        <HistoryScreen />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Load older transitions" }));
    await expectLoadError(canvasElement, /failed to return alert history/);
    // the first page is still on screen, not replaced by the failure
    await expect(canvas.getAllByRole("row").length).toBeGreaterThan(100);
    olderFails.recover();
    await userEvent.click(canvas.getByRole("button", { name: "Try again" }));
    await expect(await canvas.findByText("150 transitions · newest first")).toBeInTheDocument();
    await waitFor(() => expect(canvas.queryByRole("alert")).toBeNull());
  },
};

// all three filters go to the API, so each reaches its own older rows
const historyByRule = recording(
  scoped(async (input, init) => {
    const url = new URL(String(input), "http://localhost");
    if (url.pathname.endsWith("/alert-notifications")) {
      const rule = url.searchParams.get("rule_id");
      return json(rule ? HISTORY.filter((n) => n.rule_id === rule) : HISTORY);
    }
    return loaded(input, init);
  }),
);

export const HistoryFiltersByRule: Story = {
  render: () => (
    <Harness fetchStub={historyByRule.stub} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("could not connect to the endpoint")).toBeInTheDocument();
    await expect(canvas.getByText("3 transitions · newest first")).toBeInTheDocument();

    await pickOption(await canvas.findByLabelText("Filter by rule"), "slow p95");
    await historyByRule.expectSent("GET", "rule_id=rule-2");
    await waitFor(() => expect(canvas.queryByText("could not connect to the endpoint")).toBeNull());
    await expect(canvas.getByText("no channel configured")).toBeInTheDocument();
    await expect(canvas.getByText("1 transition · newest first")).toBeInTheDocument();
  },
};

export const HistoryFiltersByStateAndDelivery: Story = {
  render: () => (
    <Harness fetchStub={filtering.stub} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("HTTP 200")).toBeInTheDocument();

    // delivery: only the alert nobody received
    await pickOption(canvas.getByLabelText("Filter by delivery"), "Failed");
    await filtering.expectSent("GET", "delivery_status=failed");
    await waitFor(() => expect(canvas.queryByText("HTTP 200")).toBeNull());
    await expect(canvas.getByText("could not connect to the endpoint")).toBeInTheDocument();
    await expect(canvas.getByText("1 transition · newest first")).toBeInTheDocument();

    // state: nothing both firing and failed, so the filters have an empty state
    // of their own, which names them and offers to clear them
    await pickOption(canvas.getByLabelText("Filter by state"), "Firing");
    await filtering.expectSent("GET", "state=firing");
    await expect(await canvas.findByText("No transitions match these filters")).toBeVisible();
    await expect(canvas.queryByText("No alert transitions yet")).toBeNull();
    await userEvent.click(canvas.getByRole("button", { name: "Clear filters" }));
    await expect(await canvas.findByText("HTTP 200")).toBeInTheDocument();
    await expect(canvas.getByText("3 transitions · newest first")).toBeInTheDocument();
    await expect(canvas.getByLabelText("Filter by state")).toHaveValue("All states");
    await expect(canvas.getByLabelText("Filter by delivery")).toHaveValue("All deliveries");
  },
};

// the state and delivery columns, which are the point of the table, sat off the
// right edge of the scroll area at 375px (#2126)
export const HistoryStateAndDeliveryAreInViewOnAPhone: Story = {
  ...atMobile,
  render: () => (
    <Harness fetchStub={loaded} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const table = await canvas.findByRole("table", { name: "Alert History" });
    await within(table).findByText("Delivered");
    const frame = table.getBoundingClientRect();
    const inView = async (node: Element) => {
      const box = node.getBoundingClientRect();
      await expect(box.left).toBeGreaterThanOrEqual(frame.left);
      await expect(box.right).toBeLessThanOrEqual(frame.right);
    };
    for (const name of ["State", "Delivery"]) {
      await inView(within(table).getByRole("columnheader", { name }));
    }
    for (const word of ["Firing", "Resolved", "Delivered", "Failed", "Skipped"]) {
      for (const pill of within(table).getAllByText(word)) await inView(pill);
    }
    await expectNoHorizontalOverflow();
  },
};

// the longest Russian words fit their columns as well
export const HistoryReadInRussianOnAPhone: Story = {
  ...atMobile,
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={loaded} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const { deliveries, states } = ru.pages.alerting;
    const table = await canvas.findByRole("table", { name: ru.screens["alerting-history"].title });
    await within(table).findByText(deliveries.failed);
    const frame = table.getBoundingClientRect();
    for (const word of [states.firing, states.resolved, deliveries.delivered, deliveries.failed]) {
      for (const pill of within(table).getAllByText(word)) {
        const cell = pill.closest('[role="cell"]')!.getBoundingClientRect();
        const box = pill.getBoundingClientRect();
        // the pill stays inside its own column and inside the visible frame
        await expect(box.right).toBeLessThanOrEqual(cell.right + 0.5);
        await expect(box.right).toBeLessThanOrEqual(frame.right);
      }
    }
    await expectNoHorizontalOverflow();
  },
};

// --- #3049: coded details, and the reading a row was written for -------------

// every detail below carries a distinct English fallback, so a row that reads
// as the catalog's words was translated from its code and not echoed
const CODED: AlertNotificationRow[] = [
  {
    id: "note-c1",
    rule_id: "rule-1",
    channel_id: "chan-1",
    state: "firing",
    delivery_status: "delivered",
    detail: "fallback for http_ok",
    detail_code: "http_ok",
    detail_params: { status: 204 },
    value: 0.11,
    threshold: 0.05,
    sent_at: "2026-08-11T12:05:00Z",
  },
  {
    id: "note-c2",
    rule_id: "rule-1",
    channel_id: "chan-1",
    state: "firing",
    delivery_status: "failed",
    detail: "fallback for timeout",
    detail_code: "timeout",
    detail_params: { seconds: 10 },
    value: 0.09,
    threshold: 0.05,
    sent_at: "2026-08-11T12:04:00Z",
  },
  {
    id: "note-c3",
    rule_id: "rule-2",
    channel_id: null,
    state: "resolved",
    delivery_status: "skipped",
    detail: "fallback for no_channel",
    detail_code: "no_channel",
    detail_params: null,
    value: 840,
    threshold: 2000,
    sent_at: "2026-08-11T12:03:00Z",
  },
  // a code a later build added: the control plane's English is all there is
  {
    id: "note-c4",
    rule_id: "rule-2",
    channel_id: "chan-1",
    state: "firing",
    delivery_status: "failed",
    detail: "the proxy refused the tunnel",
    detail_code: "proxy_refused",
    detail_params: { host: "alerts.example.com" },
    value: 3100,
    threshold: 2000,
    sent_at: "2026-08-11T12:02:00Z",
  },
  // a known code whose number did not arrive would print `{{seconds}}`
  {
    id: "note-c5",
    rule_id: "rule-1",
    channel_id: "chan-1",
    state: "firing",
    delivery_status: "failed",
    detail: "timed out after ten seconds",
    detail_code: "timeout",
    detail_params: {},
    value: 0.2,
    threshold: 0.05,
    sent_at: "2026-08-11T12:01:00Z",
  },
  // written for a window with no data: no reading to show
  {
    id: "note-c6",
    rule_id: "rule-2",
    channel_id: "chan-1",
    state: "firing",
    delivery_status: "delivered",
    detail: "fallback for no data",
    detail_code: "http_ok",
    detail_params: { status: 200, no_data: true },
    value: null,
    threshold: 2000,
    sent_at: "2026-08-11T12:00:00Z",
  },
  // written before codes and readings existed
  {
    id: "note-c7",
    rule_id: "rule-1",
    channel_id: "chan-1",
    state: "resolved",
    delivery_status: "delivered",
    detail: "HTTP 200 from before codes",
    detail_code: null,
    detail_params: null,
    value: null,
    threshold: null,
    sent_at: "2026-08-10T12:00:00Z",
  },
];

const coded = routes([
  ["/alert-notifications", () => CODED],
  ["/alert-channels", () => CHANNELS],
  ["/alert-rules", () => RULES],
  ["/api/v1/currency", () => ({ base: "EUR", codes: ["EUR"], rates: {} })],
]);

export const HistoryTranslatesDeliveryCodes: Story = {
  render: () => (
    <Harness fetchStub={coded} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("HTTP 204")).toBeInTheDocument();
    await expect(canvas.getByText("timed out after 10s")).toBeInTheDocument();
    await expect(canvas.getByText("no channel configured")).toBeInTheDocument();
    // the English fallback stays for what the build cannot say
    await expect(canvas.getByText("the proxy refused the tunnel")).toBeInTheDocument();
    await expect(canvas.getByText("timed out after ten seconds")).toBeInTheDocument();
    await expect(canvas.queryByText(/\{\{/)).toBeNull();
    await expect(canvas.getByText("HTTP 200 from before codes")).toBeInTheDocument();
    for (const fallback of [
      "fallback for http_ok",
      "fallback for timeout",
      "fallback for no_channel",
    ]) {
      await expect(canvas.queryByText(fallback)).toBeNull();
    }
  },
};

export const HistoryTranslatesDeliveryCodesInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={coded} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const { delivery } = ru.pages.alerting.codes;
    await expect(
      await canvas.findByText(delivery.timeout.replace("{{seconds}}", "10")),
    ).toBeInTheDocument();
    await expect(canvas.getByText(delivery.no_channel)).toBeInTheDocument();
    await expect(canvas.getByText("the proxy refused the tunnel")).toBeInTheDocument();
    await expect(canvas.queryByText("timed out after 10s")).toBeNull();
    await expect(canvas.queryByText(/\{\{/)).toBeNull();
  },
};

// --- #3147: a delivery still in flight ---------------------------------------

const PENDING: AlertNotificationRow[] = [
  {
    id: "note-p1",
    rule_id: "rule-1",
    channel_id: "chan-1",
    state: "firing",
    delivery_status: "pending",
    detail: "fallback for delivery_pending",
    detail_code: "delivery_pending",
    detail_params: null,
    value: 0.11,
    threshold: 0.05,
    sent_at: "2026-08-11T12:05:00Z",
  },
];

const pendingDelivery = routes([
  ["/alert-notifications", () => PENDING],
  ["/alert-channels", () => CHANNELS],
  ["/alert-rules", () => RULES],
  ["/api/v1/currency", () => ({ base: "EUR", codes: ["EUR"], rates: {} })],
]);

export const HistoryShowsAPendingDelivery: Story = {
  render: () => (
    <Harness fetchStub={pendingDelivery} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("delivery in progress")).toBeInTheDocument();
    await expect(canvas.queryByText("fallback for delivery_pending")).toBeNull();
    await expect(canvas.queryByText("pending")).toBeNull();
    await expect(canvas.getAllByText("Pending").length).toBeGreaterThan(0);
  },
};

export const HistoryShowsAPendingDeliveryInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={pendingDelivery} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const { alerting } = ru.pages;
    await expect(
      await canvas.findByText(alerting.codes.delivery.delivery_pending),
    ).toBeInTheDocument();
    await expect(canvas.getAllByText(alerting.deliveries.pending).length).toBeGreaterThan(0);
  },
};

export const HistoryShowsEachReadingInItsUnit: Story = {
  render: () => (
    <Harness fetchStub={coded} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const table = await canvas.findByRole("table", { name: "Alert History" });
    await expectListTable(canvasElement, "Alert History");
    const row = (detail: string) =>
      within(table).getByText(detail).closest('[role="row"]') as HTMLElement;
    // an error-rate fraction reads as a percentage, a latency in milliseconds,
    // each against the threshold the rule held at the time
    const rate = within(await waitFor(() => row("HTTP 204")));
    await expect(rate.getByText("11%")).toBeInTheDocument();
    await expect(rate.getByText("above 5%")).toBeInTheDocument();
    const latency = within(row("no channel configured"));
    await expect(latency.getByText("840 ms")).toBeInTheDocument();
    await expect(latency.getByText(/^above 2,000.ms$/)).toBeInTheDocument();
    // a no-data transition has no reading, and says so
    await expect(within(row("HTTP 200 from before codes")).getAllByText("—")).not.toHaveLength(0);
    await expect(within(table).getAllByText("No data")).toHaveLength(1);
  },
};

// --- the Evaluate toast and the rule card read the same codes ------------------

const codedEvaluation = scoped(async (input, init) => {
  if (init?.method === "POST") {
    return json({
      rule: RULES[0],
      notified: false,
      notification: {
        id: "note-9",
        rule_id: "rule-1",
        channel_id: "chan-1",
        state: "firing",
        delivery_status: "failed",
        detail: "fallback for http_error",
        detail_code: "http_error",
        detail_params: { status: 503 },
        value: 0.11,
        threshold: 0.05,
        sent_at: "2026-08-11T12:01:00Z",
      },
    });
  }
  return loaded(input, init);
});

export const EvaluateToastShowsTheReadingAndTheCodedDetail: Story = {
  render: () => (
    <Harness fetchStub={codedEvaluation}>
      <Toasted>
        <AlertRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Evaluate rule high error rate now");
    await expectToast(
      canvasElement,
      /delivery failed: HTTP 503\..*It read 11% against a threshold of 5%\./,
      "error",
    );
  },
};

export const EvaluateToastReadsInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={codedEvaluation}>
      <Toasted>
        <AlertRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(
      canvasElement,
      ru.pages.alerting.rules.evaluateAria.replace("{{name}}", "high error rate"),
    );
    await expectToast(canvasElement, /HTTP 503\..*Значение: 11\s%, порог: 5\s%\./, "error");
    await expect(within(canvasElement).queryByText(/fallback for http_error/)).toBeNull();
  },
};

// a failed Evaluate answers with the rule error's code in `error.code`
export const EvaluateFailureIsTranslatedFromItsCode: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        if (init?.method === "POST") {
          return json(
            { error: { code: "clickhouse_not_configured", message: "english fallback" } },
            500,
          );
        }
        return loaded(input, init);
      })}
    >
      <Toasted>
        <AlertRules />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Evaluate rule high error rate now");
    await expectToast(canvasElement, /alert evaluation requires CLICKHOUSE_URL/, "error");
    await expect(within(canvasElement).queryByText(/english fallback/)).toBeNull();
  },
};

const ERRORED: AlertRuleRow[] = [
  {
    ...RULES[2],
    last_error: "analytics query timed out after 30s (fallback)",
    last_error_code: "query_timeout",
    last_error_params: { seconds: 30 },
  },
  {
    ...RULES[3],
    state: "error",
    last_error: "a failure only a later build names",
    last_error_code: "disk_on_fire",
    last_error_params: null,
  },
];

export const RuleCardTranslatesItsLastError: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/alert-rules", () => ERRORED],
        ["/alert-channels", () => CHANNELS],
      ])}
    >
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByText("analytics query timed out after 30s"),
    ).toBeInTheDocument();
    await expect(canvas.queryByText(/\(fallback\)/)).toBeNull();
    await expect(canvas.getByText("a failure only a later build names")).toBeInTheDocument();
  },
};

export const RuleCardTranslatesItsLastErrorInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness
      fetchStub={routes([
        ["/alert-rules", () => ERRORED],
        ["/alert-channels", () => CHANNELS],
      ])}
    >
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByText(
        ru.pages.alerting.codes.ruleError.query_timeout.replace("{{seconds}}", "30"),
      ),
    ).toBeInTheDocument();
    await expect(canvas.queryByText(/\(fallback\)/)).toBeNull();
  },
};

// --- #2335: the detail is the diagnosis, so it is read in full ----------------

const UNSEALED = "channel secret could not be unsealed; check ROLTER_KEK";
const EGRESS = "endpoint denied by the egress policy";
// one token with no space in it: it has to break inside its column, since a
// column that grew to hold it would push every row wider than the header
const ONE_TOKEN = "https://alerts.example.com/rolter/pagerduty/v2/enqueue/9f8e7d6c5b4a39281716";

const DIAGNOSES: AlertNotificationRow[] = [UNSEALED, EGRESS, ONE_TOKEN].map((detail, i) => ({
  id: `note-diagnosis-${i}`,
  rule_id: "rule-1",
  channel_id: "chan-1",
  state: "firing",
  delivery_status: "failed",
  detail,
  sent_at: "2026-08-11T12:00:00Z",
}));

const diagnosed = routes([
  ["/alert-notifications", () => DIAGNOSES],
  ["/alert-channels", () => CHANNELS],
  ["/alert-rules", () => RULES],
]);

/**
 * Every failed delivery's detail is on screen in full, and the columns did not
 * move to make room for it.
 *
 * `toBeVisible` and the text being in the document say nothing here: a cell cut
 * with an ellipsis holds all of its text and is visible. A cut shows as the
 * cell's content being wider than the cell (`scrollWidth`), and as a line that
 * runs past the edge of the frame the table scrolls in. The table is scrolled to
 * its end first, since the detail is the last column and sits off the edge of a
 * phone until the reader gets to it.
 */
async function expectDetailsInFull(canvasElement: HTMLElement, name: string) {
  const table = await within(canvasElement).findByRole("table", { name });
  const [header] = within(table).getAllByRole("row");
  const floor = header.getBoundingClientRect().width;
  table.scrollLeft = table.scrollWidth;
  for (const detail of DIAGNOSES.map((row) => row.detail!)) {
    const cell = await within(table).findByText((_, el) => el?.textContent === detail);
    await expect(cell).toHaveAttribute("role", "cell");
    await expect(cell.scrollWidth).toBeLessThanOrEqual(cell.clientWidth);
    await expect(getComputedStyle(cell).textOverflow).not.toBe("ellipsis");
    const text = document.createRange();
    text.selectNodeContents(cell);
    await expectInFrame(text, table);
    // the row is the header's width: a long token broke inside its column
    await expect(cell.parentElement!.getBoundingClientRect().width).toBe(floor);
  }
}

export const HistoryDetailIsReadInFullOnADesktop: Story = {
  ...atWide,
  render: () => (
    <Harness fetchStub={diagnosed} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectDetailsInFull(canvasElement, "Alert History");
    // a 1440px window has room for the columns: nothing scrolls
    const table = within(canvasElement).getByRole("table", { name: "Alert History" });
    await expect(table.scrollWidth).toBe(table.clientWidth);
  },
};

export const HistoryDetailIsReadInFullOnAPhone: Story = {
  ...atMobile,
  render: () => (
    <Harness fetchStub={diagnosed} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectDetailsInFull(canvasElement, "Alert History");
  },
};

export const HistoryDetailIsReadInFullInRussianOnADesktop: Story = {
  ...atWide,
  globals: { ...atWide.globals, locale: "ru" },
  render: () => (
    <Harness fetchStub={diagnosed} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectDetailsInFull(canvasElement, ru.screens["alerting-history"].title);
  },
};

export const HistoryDetailIsReadInFullInRussianOnAPhone: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => (
    <Harness fetchStub={diagnosed} route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectDetailsInFull(canvasElement, ru.screens["alerting-history"].title);
  },
};

// --- #2428: the rule name is read in full too ---------------------------------

const LONG_RULE = "production openai error rate";
const LONG_RULE_RU = "производственная доля ошибок openai за пять минут";
const LONG_RULE_ONE_TOKEN = "production-openai-chat-completions-error-rate-over-five-minutes";

const longRuleRoutes = (name: string) =>
  routes([
    [
      "/alert-notifications",
      () => [
        {
          id: "note-long-rule",
          rule_id: "rule-long",
          channel_id: "chan-1",
          state: "firing",
          delivery_status: "delivered",
          detail: "HTTP 200",
          sent_at: "2026-08-11T12:00:00Z",
        } satisfies AlertNotificationRow,
      ],
    ],
    ["/alert-channels", () => CHANNELS],
    ["/alert-rules", () => [{ ...RULES[0], id: "rule-long", name }]],
  ]);

async function expectRuleInFull(canvasElement: HTMLElement, tableName: string, name: string) {
  const table = await within(canvasElement).findByRole("table", { name: tableName });
  const [header] = within(table).getAllByRole("row");
  const floor = header.getBoundingClientRect().width;
  const cell = await within(table).findByText((_, el) => el?.textContent === name);
  await expect(cell).toHaveAttribute("role", "cell");
  await expect(cell.scrollWidth).toBeLessThanOrEqual(cell.clientWidth);
  await expect(getComputedStyle(cell).textOverflow).not.toBe("ellipsis");
  // the rule is a middle column: bring it into the table's frame like a reader scrolling to it
  cell.scrollIntoView({ inline: "center", block: "nearest" });
  const text = document.createRange();
  text.selectNodeContents(cell);
  await expectInFrame(text, table);
  await expect(cell.parentElement!.getBoundingClientRect().width).toBe(floor);
}

const ruleStory = (
  view: typeof atWide | typeof atMobile,
  locale: "en" | "ru",
  name: string,
): Story => ({
  ...view,
  globals: { ...view.globals, locale },
  render: () => (
    <Harness fetchStub={longRuleRoutes(name)}>
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const title = locale === "ru" ? ru.screens["alerting-history"].title : "Alert History";
    await expectRuleInFull(canvasElement, title, name);
  },
});

export const HistoryRuleNameIsReadInFullOnADesktop = ruleStory(atWide, "en", LONG_RULE);
export const HistoryRuleNameIsReadInFullOnAPhone = ruleStory(atMobile, "en", LONG_RULE);
export const HistoryOneTokenRuleNameBreaksOnAPhone = ruleStory(atMobile, "en", LONG_RULE_ONE_TOKEN);
export const HistoryRuleNameIsReadInFullInRussianOnADesktop = ruleStory(atWide, "ru", LONG_RULE_RU);
export const HistoryRuleNameIsReadInFullInRussianOnAPhone = ruleStory(atMobile, "ru", LONG_RULE_RU);

// --- a control plane that answers with a 5xx: LoadError offers a retry --------

const channelsDown = failingThenRecovering("/alert-channels", loaded);
const rulesDown = failingThenRecovering("/alert-rules", loaded);
const historyDown = failingThenRecovering("/alert-notifications", loaded);

export const ChannelsLoadFailsAndRetries: Story = {
  render: () => {
    channelsDown.reset();
    return (
      <Harness fetchStub={channelsDown.stub}>
        <AlertChannels />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /failed to return alert channels/);
    await expect(canvas.getByText("control plane unavailable")).toBeInTheDocument();
    await expectNoFalseEmpty(canvasElement, /No channels yet/);
    const before = channelsDown.reads.length;
    channelsDown.recover();
    await userEvent.click(canvas.getByRole("button", { name: "Try again" }));
    await expect(await canvas.findByText("ops-slack")).toBeInTheDocument();
    await waitFor(() => expect(canvas.queryByRole("alert")).toBeNull());
    await expect(channelsDown.reads.length).toBeGreaterThan(before);
  },
};

export const RulesLoadFailsAndRetries: Story = {
  render: () => {
    rulesDown.reset();
    return (
      <Harness fetchStub={rulesDown.stub}>
        <AlertRules />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /failed to return alert rules/);
    await expect(canvas.getByText("control plane unavailable")).toBeInTheDocument();
    await expectNoFalseEmpty(canvasElement, /No alert rules/);
    const before = rulesDown.reads.length;
    rulesDown.recover();
    await userEvent.click(canvas.getByRole("button", { name: "Try again" }));
    await expect(await canvas.findByText("high error rate")).toBeInTheDocument();
    await waitFor(() => expect(canvas.queryByRole("alert")).toBeNull());
    await expect(rulesDown.reads.length).toBeGreaterThan(before);
  },
};

export const HistoryLoadFailsAndRetries: Story = {
  render: () => {
    historyDown.reset();
    return (
      <Harness fetchStub={historyDown.stub} route="/alerting-history">
        <HistoryScreen />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /failed to return alert history/);
    await expect(canvas.getByText("control plane unavailable")).toBeInTheDocument();
    await expectNoFalseEmpty(canvasElement, /No alert transitions yet/);
    const before = historyDown.reads.length;
    historyDown.recover();
    await userEvent.click(canvas.getByRole("button", { name: "Try again" }));
    await expect(await canvas.findByText("could not connect to the endpoint")).toBeInTheDocument();
    await waitFor(() => expect(canvas.queryByRole("alert")).toBeNull());
    await expect(historyDown.reads.length).toBeGreaterThan(before);
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

// The rules and the history refused before they ask (#1606).
//
// Alerting is delegated (#1829): an org, team or project admin owns the rules
// and channels of their own scope, so the screens are no longer behind a
// superadmin-only wrapper. A caller who owns nothing is refused by the control
// plane, which `Forbidden` above covers with the 403 it really answers.
export const RulesAreAnAdminsToo: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="admin">
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("high error rate")).toBeVisible();
    // a rule with no scope reads the whole deployment, and says so
    await expect((await canvas.findAllByText("Whole deployment")).length).toBeGreaterThan(0);
  },
};

export const HistoryIsAnAdminsToo: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="admin" route="/alerting-history">
      <HistoryScreen />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expect(await within(canvasElement).findByRole("table")).toBeVisible();
  },
};

// a scoped admin cannot create a deployment-wide rule, so the sheet does not
// offer one and sends the scope it opened on
const adminRuleCreates = recording(
  scoped(async (input, init) => {
    if (init?.method === "POST") return json(RULES[0], 201);
    return String(input).includes("/alert-channels") ? json(CHANNELS) : json(RULES);
  }),
);

export const AnAdminsRuleIsCreatedOnAScopeNotTheDeployment: Story = {
  render: () => (
    <Harness fetchStub={adminRuleCreates.stub} role="admin">
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add rule/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Name"), "team errors");
    const offered = await openOptions(within(form).getByLabelText("Watches"));
    await expect(within(offered).queryByRole("option", { name: "Whole deployment" })).toBeNull();
    await expect(within(offered).getByRole("option", { name: "Team" })).toBeVisible();
    await userEvent.click(within(offered).getByRole("option", { name: "Team" }));
    await pickOption(within(form).getByLabelText("Scope"), /./);
    await userEvent.click(within(form).getByRole("button", { name: "Create" }));
    const body = await adminRuleCreates.expectSentBody<{ scope_type?: string; scope_id?: string }>(
      "POST",
      "/alert-rules",
    );
    await expect(body.scope_type).toBe("team");
    await expect(typeof body.scope_id).toBe("string");
  },
};

// the comparison and the no-data policy (#2423)
const BELOW: AlertRuleRow = {
  ...RULES[3],
  id: "rule-6",
  name: "traffic stopped",
  threshold: 0,
  comparison: "below",
};
const SILENT: AlertRuleRow = {
  ...RULES[0],
  id: "rule-7",
  name: "silent errors",
  no_data: "fire",
  state: "ok",
  last_value: null,
};
const withComparisons = routes([
  ["/alert-channels", () => CHANNELS],
  ["/alert-rules", () => [...RULES, BELOW, SILENT]],
  ["/alert-notifications", () => HISTORY],
  ["/api/v1/currency", () => ({ base: "EUR", codes: ["EUR"], rates: {} })],
]);

export const RuleCardShowsTheComparisonAndNoData: Story = {
  render: () => (
    <Harness fetchStub={withComparisons}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const below = await canvas.findByRole("article", { name: "traffic stopped" });
    await expect(stat(below, "Threshold")).toHaveTextContent(/^below 0 requests in 5m$/);
    // evaluated, but the window held nothing to measure
    const silent = await canvas.findByRole("article", { name: "silent errors" });
    await expect(stat(silent, "Last value")).toHaveTextContent(/^No data$/);
    // a rule never evaluated has no reading yet, which is not "no data"
    const fresh = within(canvasElement).getByRole("article", { name: "high error rate" });
    await expect(stat(fresh, "Last value")).toHaveTextContent(/^11%$/);
  },
};

const comparisonWrites = recording(
  scoped(async (input, init) => {
    if (init?.method === "POST") return json(RULES[0], 201);
    return loaded(input, init);
  }),
);

export const ComparisonAndNoDataAreChosenInTheForm: Story = {
  render: () => (
    <Harness fetchStub={comparisonWrites.stub}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add rule/i);
    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Name"), "errors gone quiet");
    const comparison = form.getByRole("radiogroup", { name: "Comparison" });
    await expect(within(comparison).getByRole("radio", { name: "Above or equal" })).toBeChecked();
    await waitFor(() => expect(form.getByText(/threshold is inclusive/)).toBeVisible());
    const noData = form.getByRole("radiogroup", { name: "When there is no data" });
    await expect(within(noData).getByRole("radio", { name: "Keep current state" })).toBeChecked();

    await userEvent.click(within(comparison).getByRole("radio", { name: "Below or equal" }));
    await userEvent.click(within(noData).getByRole("radio", { name: "Fire" }));
    await expect(within(noData).getByRole("radio", { name: "Fire" })).toBeChecked();
    await userEvent.click(form.getByRole("button", { name: "Create" }));
    const body = await comparisonWrites.expectSentBody<RuleBody>("POST", "/alert-rules");
    await expect(body).toMatchObject({
      signal: "error_rate",
      comparison: "below",
      no_data: "fire",
    });
    await expectSheetClosed();
  },
};

const volumeWrites = recording(
  scoped(async (input, init) => {
    if (init?.method === "POST") return json(RULES[3], 201);
    return loaded(input, init);
  }),
);

export const NoDataIsOfferedOnlyToSignalsThatHaveIt: Story = {
  render: () => (
    <Harness fetchStub={volumeWrites.stub}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add rule/i);
    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Name"), "traffic stopped");
    await waitFor(() =>
      expect(form.getByRole("radiogroup", { name: "When there is no data" })).toBeVisible(),
    );
    await userEvent.click(
      within(form.getByRole("radiogroup", { name: "When there is no data" })).getByRole("radio", {
        name: "Resolve",
      }),
    );

    await pickOption(form.getByLabelText("Signal"), /^Request volume/);
    await waitFor(() =>
      expect(form.queryByRole("radiogroup", { name: "When there is no data" })).toBeNull(),
    );
    // the traffic-stopped help appears for Below with a threshold of 0
    await expect(form.queryByText(/alerts when traffic stops/)).toBeNull();
    await userEvent.click(
      within(form.getByRole("radiogroup", { name: "Comparison" })).getByRole("radio", {
        name: "Below or equal",
      }),
    );
    const threshold = form.getByLabelText("Threshold (requests per window)");
    await userEvent.clear(threshold);
    await userEvent.type(threshold, "0");
    await expect(await form.findByText(/alerts when traffic stops/)).toBeVisible();

    await userEvent.click(form.getByRole("button", { name: "Create" }));
    const body = await volumeWrites.expectSentBody<RuleBody>("POST", "/alert-rules");
    await expect(body.comparison).toBe("below");
    await expect(body).not.toHaveProperty("no_data");
    await expectSheetClosed();
  },
};

const editsNoData = editsRules([SILENT, ...RULES.slice(1)]);

export const AnEditOpensOnTheStoredComparisonAndNoData: Story = {
  render: () => (
    <Harness fetchStub={editsNoData.stub}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit rule silent errors");
    const form = within(
      await within(document.body).findByRole("dialog", { name: "Edit rule silent errors" }),
    );
    await waitFor(() => expect(form.getByLabelText("Name")).toHaveValue("silent errors"));
    const noData = form.getByRole("radiogroup", { name: "When there is no data" });
    await expect(within(noData).getByRole("radio", { name: "Fire" })).toBeChecked();
    await userEvent.click(within(noData).getByRole("radio", { name: "Resolve" }));
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    const body = await editsNoData.expectSentBody<RuleBody>("PUT", "/alert-rules/rule-7");
    await expect(body).toMatchObject({ comparison: "above", no_data: "ok" });
    await expectSheetClosed();
  },
};

/** The channel's endpoint explains itself in an (i) (#3285). */
export const ChannelEndpointIsExplainedInAHint: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <AlertChannels />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add channel/i);
    const c = en.pages.alerting.channels;
    await expectFieldHints(sheet(), en.common.aboutField, [[c.fieldEndpoint, c.fieldEndpointInfo]]);
  },
};

/**
 * What a rule does with an empty window, and where its changes are delivered,
 * explain themselves in an (i) (#3285). The comparison is not one: its options
 * read "Above or equal" and "Below or equal" and the line under it says the
 * threshold is inclusive.
 */
export const RuleNoDataAndChannelAreExplainedInAHint: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <AlertRules />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add rule/i);
    const r = en.pages.alerting.rules;
    await expectFieldHints(sheet(), en.common.aboutField, [
      [r.fieldNoData, r.fieldNoDataInfo],
      [r.fieldChannel, r.fieldChannelInfo],
    ]);
  },
};
