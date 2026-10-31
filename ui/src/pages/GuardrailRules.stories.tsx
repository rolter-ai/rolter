import type { Meta, StoryObj } from "@storybook/react-vite";
import { MemoryRouter } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import GuardrailRules from "./GuardrailRules";
import {
  cancelConfirmation,
  confirmDestructive,
  expectAnalyticsUnavailable,
  expectEmptyState,
  expectForbidden,
  expectLoadError,
  expectSheetClosed,
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
import type { GuardrailRuleInput, GuardrailRuleRow } from "@/lib/api";
import type { EffectiveRule } from "@/lib/guardrail-policy";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { expectFieldHint } from "@/lib/story-hints";
import { atShort, expectInViewport, phoneFits } from "@/lib/story-viewport";

const RULES: GuardrailRuleRow[] = [
  {
    id: "rule-email",
    name: "Redact customer email",
    enabled: true,
    source_type: "builtin",
    builtin: "email",
    pattern: null,
    stage: "pre_call",
    action: "redact",
    replacement: "[REDACTED:EMAIL]",
    include_system: false,
    position: 10,
    created_at: "2026-08-02T00:00:00Z",
    updated_at: "2026-08-02T00:00:00Z",
  },
  {
    id: "rule-injection",
    name: "Block override attempts",
    enabled: true,
    source_type: "pattern",
    builtin: null,
    pattern: "(?i)ignore previous instructions",
    stage: "pre_call",
    action: "block",
    replacement: null,
    include_system: true,
    position: 20,
    created_at: "2026-08-02T00:00:00Z",
    updated_at: "2026-08-02T00:00:00Z",
  },
];

/** the effective rule the postgres store builds from a dashboard row */
const asEffective = (row: GuardrailRuleRow): EffectiveRule => ({
  name: row.name,
  ...(row.builtin ? { builtin: row.builtin } : {}),
  ...(row.pattern ? { pattern: row.pattern } : {}),
  stage: row.stage,
  action: row.action,
  ...(row.replacement ? { replacement: row.replacement } : {}),
  include_system: row.include_system,
});

/** a route's `advanced.guardrails`, as `RouteGuardrails` serializes it */
type RouteGuardrails = { disable?: string[]; enable?: string[] };

/** a route as the config document lists it, with a guardrail override */
const route = (model: string, guardrails: RouteGuardrails) => ({
  model,
  strategy: "round_robin",
  targets: [{ provider: "openai", model, weight: 1 }],
  advanced: { capabilities: [], guardrails },
});

/**
 * `GET /api/v1/config` as the store builds it: the config file's rules first,
 * then the enabled rows whose names the file does not take, with
 * `guardrails.enabled` set from the flag (#2157). `routes` carry the per-route
 * overrides (#2283).
 */
function effectiveConfig({
  rows = RULES,
  file = [],
  on = true,
  streaming = "reject",
  routes = [],
}: {
  rows?: GuardrailRuleRow[];
  file?: EffectiveRule[];
  on?: boolean;
  streaming?: "reject" | "passthrough";
  routes?: ReturnType<typeof route>[];
} = {}) {
  const taken = new Set(file.map((rule) => rule.name));
  return {
    providers: [],
    routes,
    virtual_keys: [],
    feature_flags: { guardrails: on },
    guardrails: {
      enabled: on,
      streaming_post_call: streaming,
      rules: [
        ...file,
        ...rows.filter((row) => row.enabled && !taken.has(row.name)).map(asEffective),
      ],
    },
  };
}

const isConfig = (input: RequestInfo | URL) =>
  new URL(String(input), "http://localhost").pathname === "/api/v1/config";

/**
 * Answer the effective config on its own path and `list` everywhere else. The
 * config defaults to the one `RULES` alone produce, which is what every story
 * not about the effective policy wants: both rules enforced, no banner.
 */
const withPolicy =
  (list: FetchStub, config: () => Response = () => json(effectiveConfig())): FetchStub =>
  async (input, init) =>
    isConfig(input) ? config() : list(input, init);

const isOutcomes = (input: RequestInfo | URL) =>
  new URL(String(input), "http://localhost").pathname === "/api/v1/analytics/guardrails";

/**
 * Answer the outcome rollup (#2158) on its own path and `list` everywhere
 * else. `outcomes` is the response, so a story can make it an error or a 503.
 */
const withOutcomes =
  (list: FetchStub, outcomes: () => Response): FetchStub =>
  async (input, init) =>
    isOutcomes(input) ? outcomes() : withPolicy(list)(input, init);

/**
 * The screen under the shared fetch-stub harness, with a role to render as.
 *
 * `role` is what a story needs to mount a `CapabilityProvider` at all: with no
 * provider above it `can()` answers "unknown", the `superadminOnly` wrapper
 * never blocks, and a story can only reach the 403 by stubbing one — which
 * tests the screen's own error path rather than the gate (#1606).
 *
 * `toasted` is opt-in: the Toaster contributes its own role="status" and
 * role="alert" regions, and the stories that query those by role would stop
 * being able to.
 */
function Harness({
  fetchStub,
  role,
  toasted,
}: {
  fetchStub: FetchStub;
  role?: StoryRole;
  toasted?: boolean;
}) {
  // the flag-off banner links to Feature Flags, and a link needs a router
  return (
    <MemoryRouter>
      <ScreenHarness fetchStub={fetchStub} role={role}>
        {toasted ? (
          <Toasted>
            <GuardrailRules />
          </Toasted>
        ) : (
          <GuardrailRules />
        )}
      </ScreenHarness>
    </MemoryRouter>
  );
}

const meta = {
  title: "Screens/GuardrailRules",
  component: GuardrailRules,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof GuardrailRules>;
export default meta;
type Story = StoryObj<typeof meta>;

/**
 * Every rule the dashboard lists is in the effective policy and the flag is on,
 * so each card says enforced and nothing above the cards says otherwise.
 */
export const Loaded: Story = {
  render: () => <Harness fetchStub={withPolicy(async () => json(RULES))} />,
  play: async ({ canvasElement }) => {
    for (const name of [/Redact customer email/, /Block override attempts/]) {
      const card = await ruleCard(canvasElement, name);
      await expect(within(card).getByText("enforced", { exact: true })).toBeVisible();
    }
    const canvas = within(canvasElement);
    await expect(
      canvas.queryByRole("region", { name: /switched off|Could not confirm/ }),
    ).toBeNull();
    await expect(canvas.queryByRole("heading", { name: "Config-file rules" })).toBeNull();
    // one ordered column states the run order (#2161)
    const order = within(canvas.getByRole("list", { name: "Rules in the order they run" }));
    await expect(order.getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      expect.stringContaining("10 · Redact customer email"),
      expect.stringContaining("20 · Block override attempts"),
    ]);
    // a pattern is copyable config: mono, and not clamped to two lines
    const pattern = canvas.getByText("(?i)ignore previous instructions");
    await expect(pattern).toHaveClass("font-mono");
    await expect(pattern).not.toHaveClass("line-clamp-2");
    // the shell's ScreenHeader owns the page's only h1 (#2241)
    await expect(within(canvasElement).queryAllByRole("heading", { level: 1 })).toHaveLength(0);
    // no route overrides a rule, so no card names one
    await expect(canvasElement).not.toHaveTextContent(/Off for traffic/);
  },
};
export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => expectSkeleton(canvasElement),
};
export const Empty: Story = {
  render: () => (
    <Harness
      fetchStub={withPolicy(
        async () => json([]),
        () => json(effectiveConfig({ rows: [] })),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No inspection rules/, /Add first rule/);
    // it used to tell the operator to turn the flag on later, while the
    // screen never read whether the flag was on (#2157)
    await expect(within(canvasElement).getByText(/published to the gateway/)).toBeVisible();
    await expect(canvasElement).not.toHaveTextContent(/flag/i);
  },
};
export const Error: Story = {
  render: () => (
    <Harness fetchStub={async () => json({ error: { message: "registry offline" } }, 503)} />
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return guardrail rules/);
  },
};

// the two failures the bespoke panel got wrong (#1259). a deployment-scoped
// screen is refused to every non-superadmin, and a "try again" on a permission
// suggests the refusal was transient
export const Forbidden: Story = {
  render: () => <Harness fetchStub={async () => json({ error: { message: "forbidden" } }, 403)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/do not have access to guardrail rules/i)).toBeVisible();
    await expect(canvas.queryByRole("button", { name: /try again/i })).toBeNull();
  },
};

// fetch never connected, so there is no status to read: that one *is* worth
// retrying, and the control plane's own message is still printed underneath
export const Unreachable: Story = {
  render: () => <Harness fetchStub={() => Promise.reject(new TypeError("Failed to fetch"))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/cannot reach the control plane/i)).toBeVisible();
    await expect(await canvas.findByRole("button", { name: /try again/i })).toBeVisible();
  },
};

export const CreatesCustomRule: Story = {
  render: () => (
    <Harness
      fetchStub={withPolicy(async (_input, init) =>
        init?.method === "POST" ? json(RULES[1], 201) : json(RULES),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /add rule/i }));
    const dialog = within(document.body).getByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Rule name"), "Prompt injection policy");
    await pickOption(within(dialog).getByLabelText("Source"), "Custom regex");
    // a custom regex has no detector, so the gateway's generic token applies
    await expect(within(dialog).getByLabelText("Replacement token")).toHaveAttribute(
      "placeholder",
      "[REDACTED]",
    );
    await userEvent.type(
      within(dialog).getByLabelText("Regular expression"),
      "ignore previous instructions",
    );
    await expect(within(dialog).getByRole("button", { name: "Publish rule" })).toBeEnabled();
  },
};

// every create answers with a row; the story reads what was sent
const creates = recording(
  withPolicy(async (_input, init) =>
    init?.method === "POST" ? json({ ...RULES[0], id: "rule-new" }, 201) : json(RULES),
  ),
);
const postedBody = async (n: number): Promise<GuardrailRuleInput> => {
  const posts = () => creates.calls.filter((call) => call.method === "POST");
  await waitFor(() => expect(posts()).toHaveLength(n));
  return JSON.parse(posts()[n - 1].body as string) as GuardrailRuleInput;
};

/**
 * A phone redaction and a block rule, created one after the other (#2160).
 *
 * Every new rule used to start with `[REDACTED:EMAIL]` and nothing moved it,
 * so a phone rule masked numbers with the email token and a block rule stored
 * a replacement its card then reported. A new rule starts with no token now:
 * the empty field shows the one the gateway writes, a detector change moves
 * it, and only a redact rule sends one.
 */
export const CreatesAPhoneRedactionAndABlockRule: Story = {
  render: () => {
    creates.calls.length = 0;
    return <Harness fetchStub={creates.stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const openDialog = async () => {
      await userEvent.click(await canvas.findByRole("button", { name: /add rule/i }));
      return within(await within(document.body).findByRole("dialog"));
    };

    let dialog = await openDialog();
    await userEvent.type(dialog.getByLabelText("Rule name"), "Mask phone numbers");
    const token = dialog.getByLabelText("Replacement token");
    await expect(token).toHaveValue("");
    await expect(token).toHaveAttribute("placeholder", "[REDACTED:EMAIL]");
    await pickOption(dialog.getByLabelText("Detector"), "Phone number");
    await expect(token).toHaveValue("");
    await expect(token).toHaveAttribute("placeholder", "[REDACTED:PHONE]");
    await expect(token).toHaveAccessibleDescription(
      "Leave empty to use the default token, [REDACTED:PHONE].",
    );
    await userEvent.click(dialog.getByRole("button", { name: "Publish rule" }));
    await expect(await postedBody(1)).toMatchObject({
      name: "Mask phone numbers",
      // after the last rule (20), not tied at 0 (#2161)
      position: 30,
      source_type: "builtin",
      builtin: "phone",
      pattern: null,
      action: "redact",
      replacement: null,
    });
    await expectSheetClosed();

    dialog = await openDialog();
    await userEvent.type(dialog.getByLabelText("Rule name"), "Block card numbers");
    // a token the user typed survives a detector change
    await userEvent.type(dialog.getByLabelText("Replacement token"), "<card>");
    await pickOption(dialog.getByLabelText("Detector"), "Payment card");
    await expect(dialog.getByLabelText("Replacement token")).toHaveValue("<card>");
    // and is dropped once the rule no longer rewrites anything
    await pickOption(dialog.getByLabelText("Action"), "Block traffic");
    await expect(dialog.queryByLabelText("Replacement token")).toBeNull();
    await userEvent.click(dialog.getByRole("button", { name: "Publish rule" }));
    await expect(await postedBody(2)).toMatchObject({
      name: "Block card numbers",
      builtin: "payment_card",
      action: "block",
      replacement: null,
    });
  },
};

/**
 * The rule is refused (#1607).
 *
 * The regex was written by hand, so a dialog that closed on a rejected publish
 * would cost the pattern as well as the name.
 */
export const CreateRejectedByTheServer: Story = {
  render: () => (
    <Harness
      toasted
      fetchStub={withPolicy(async (_input, init) =>
        init?.method === "POST"
          ? json({ error: { message: "the pattern does not compile: unbalanced group" } }, 422)
          : json(RULES),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /add rule/i }));
    const dialog = within(document.body).getByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Rule name"), "Prompt injection policy");
    await pickOption(within(dialog).getByLabelText("Source"), "Custom regex");
    await userEvent.type(
      within(dialog).getByLabelText("Regular expression"),
      "ignore previous instructions",
    );
    await userEvent.click(within(dialog).getByRole("button", { name: "Publish rule" }));

    await expectToast(canvasElement, /does not compile/, "error");
    await waitFor(() => expect(within(document.body).getByRole("dialog")).toBeInTheDocument());
    await expect(within(dialog).getByLabelText("Regular expression")).toHaveValue(
      "ignore previous instructions",
    );
  },
};

// the delete used to be a bare window.confirm — unstyled, untranslated, and a
// modal the story runner cannot answer. It is a real dialog now (#1179)
//
// the list shrinks once the DELETE lands, so the story can assert the outcome
// — the toast, the row gone — rather than that the request left. A stub that
// answers the full list forever passes either way, which is how a 204 fixture
// that threw went unnoticed (#1260). the effective config shrinks with it: a
// rule still in the policy after its row is gone is the config file's (#2157)
let ruleDeleted = false;
const remaining = () => (ruleDeleted ? RULES.filter((rule) => rule.id !== "rule-email") : RULES);
const deletes = recording(
  withPolicy(
    async (_input, init) => {
      if (init?.method === "DELETE") {
        ruleDeleted = true;
        return json({}, 204);
      }
      return json(remaining());
    },
    () => json(effectiveConfig({ rows: remaining() })),
  ),
);

export const ConfirmsBeforeDeletingARule: Story = {
  render: () => {
    ruleDeleted = false;
    return <Harness fetchStub={deletes.stub} toasted />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // by name, not by index: each row control names its own rule (#1214)
    const del = async () =>
      canvas.findByRole("button", { name: "Delete rule Redact customer email" });

    await userEvent.click(await del());
    await cancelConfirmation();
    deletes.expectNotSent("DELETE", "/guardrails/rules/rule-email");

    await userEvent.click(await del());
    await confirmDestructive(/Redact customer email/, "Delete");
    await deletes.expectSent("DELETE", "/guardrails/rules/rule-email");

    // the outcome, not just the request: the confirmation closes, the queue
    // announces it, and the row is gone from the list
    await expectSheetClosed();
    await expectToast(canvasElement, /Redact customer email deleted/);
    await waitFor(() =>
      expect(canvas.queryByText("Redact customer email")).not.toBeInTheDocument(),
    );
  },
};

export const EditsRule: Story = {
  render: () => <Harness fetchStub={withPolicy(async () => json(RULES))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Edit rule Redact customer email" }),
    );
    await waitFor(() => expect(within(document.body).getByRole("dialog")).toBeVisible());
    const dialog = within(within(document.body).getByRole("dialog"));
    await expect(dialog.getByLabelText("Rule name")).toHaveValue("Redact customer email");
    // the stored token is the email detector's own, so it was never edited and
    // follows the detector rather than masking phone numbers as emails (#2160)
    const token = dialog.getByLabelText("Replacement token");
    await expect(token).toHaveValue("[REDACTED:EMAIL]");
    await pickOption(dialog.getByLabelText("Detector"), "Phone number");
    await expect(token).toHaveValue("");
    await expect(token).toHaveAttribute("placeholder", "[REDACTED:PHONE]");
  },
};

// a phone redaction saved with no token, and a block rule still carrying the
// token the dialog used to seed every rule with (#2160)
const PHONE_RULE: GuardrailRuleRow = {
  ...RULES[0],
  id: "rule-phone",
  name: "Mask phone numbers",
  builtin: "phone",
  replacement: null,
  position: 30,
};
const STALE_BLOCK: GuardrailRuleRow = {
  ...RULES[0],
  id: "rule-cards",
  name: "Block card numbers",
  builtin: "payment_card",
  action: "block",
  replacement: "[REDACTED:EMAIL]",
  position: 40,
};
const edits = recording(
  withPolicy(
    async (_input, init) =>
      init?.method === "PUT" ? json(STALE_BLOCK) : json([PHONE_RULE, STALE_BLOCK]),
    () => json(effectiveConfig({ rows: [PHONE_RULE, STALE_BLOCK] })),
  ),
);

/**
 * A card states the token its rule writes, and nothing for a rule that never
 * rewrites (#2160).
 *
 * The phone rule stores no token, so the gateway writes its detector's; the
 * block rule's stored token never runs, and the card used to report it all the
 * same. Publishing that rule again drops the token.
 */
export const CardsReportTheTokenARuleWrites: Story = {
  render: () => {
    edits.calls.length = 0;
    return <Harness fetchStub={edits.stub} />;
  },
  play: async ({ canvasElement }) => {
    const phone = await ruleCard(canvasElement, /Mask phone numbers/);
    await expect(phone).toHaveTextContent("Replacement · [REDACTED:PHONE]");
    const block = await ruleCard(canvasElement, /Block card numbers/);
    await expect(block).toHaveTextContent("Content is not rewritten");
    await expect(block).not.toHaveTextContent(/Replacement|REDACTED/);

    await userEvent.click(
      within(block).getByRole("button", { name: "Edit rule Block card numbers" }),
    );
    const dialog = within(await within(document.body).findByRole("dialog"));
    // made a redaction, it starts from the detector's token, not the stale one
    await pickOption(dialog.getByLabelText("Action"), "Redact matches");
    const token = dialog.getByLabelText("Replacement token");
    await expect(token).toHaveValue("");
    await expect(token).toHaveAttribute("placeholder", "[REDACTED:CARD]");
    await pickOption(dialog.getByLabelText("Action"), "Block traffic");
    await userEvent.click(dialog.getByRole("button", { name: "Publish rule" }));
    const body = await edits.expectSentBody<GuardrailRuleInput>(
      "PUT",
      "/guardrails/rules/rule-cards",
    );
    await expect(body).toMatchObject({ action: "block", replacement: null });
  },
};

/**
 * The same dialog in a 640×360 window — 1280×720 at 200 % zoom (#2003).
 *
 * It used to open 422px tall and centered, its title and close button above
 * the top edge and its footer below the bottom, with the page's scroll locked
 * behind it. Now the panel is capped at the window and the fields scroll
 * between a header and a footer that stay on screen.
 */
export const EditsRuleOnAShortScreen: Story = {
  ...atShort,
  render: () => <Harness fetchStub={withPolicy(async () => json(RULES))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Edit rule Redact customer email" }),
    );
    const dialog = await within(document.body).findByRole("dialog", {
      name: "Edit inspection rule",
    });
    await expectInViewport(dialog);
    await expectInViewport(within(dialog).getByRole("heading", { name: "Edit inspection rule" }));
    await expectInViewport(within(dialog).getByRole("button", { name: "Close" }));
    const publish = within(dialog).getByRole("button", { name: "Publish rule" });
    await expectInViewport(publish);
    // the fields are what gave way, and the last of them is a scroll away
    const last = within(dialog).getByRole("switch", { name: "Inspect system messages" });
    const body = last.closest<HTMLElement>("[data-slot=dialog-body]");
    await expect(body).not.toBeNull();
    await expect(body!.scrollHeight).toBeGreaterThan(body!.clientHeight);
    last.scrollIntoView({ block: "nearest" });
    await waitFor(() => expectInViewport(last));
    await expectInViewport(publish);
  },
};

// a response rule beside the two request rules, for the streaming note (#2156)
const POST_CALL_RULE: GuardrailRuleRow = {
  id: "rule-answers",
  name: "Mask emails in answers",
  enabled: true,
  source_type: "builtin",
  builtin: "email",
  pattern: null,
  stage: "post_call",
  action: "redact",
  replacement: "[REDACTED:EMAIL]",
  include_system: false,
  position: 30,
  created_at: "2026-08-02T00:00:00Z",
  updated_at: "2026-08-02T00:00:00Z",
};

/**
 * The rules plus `GET /api/v1/config` answering with `streaming_post_call` set
 * to `mode`, or failing when the effective config cannot be read.
 */
const streamingStub = (mode: "reject" | "passthrough" | "unreadable"): FetchStub =>
  withPolicy(
    async () => json([...RULES, POST_CALL_RULE]),
    () =>
      mode === "unreadable"
        ? json({ error: { message: "store offline" } }, 503)
        : json(effectiveConfig({ rows: [...RULES, POST_CALL_RULE], streaming: mode })),
  );

/**
 * The card of the rule named `name`, found by its heading: level 2 for a rule
 * added on this screen, level 3 for one listed under "Config-file rules".
 */
async function ruleCard(
  canvasElement: HTMLElement,
  name: RegExp,
  level: 2 | 3 = 2,
): Promise<HTMLElement> {
  const heading = await within(canvasElement).findByRole("heading", { name, level });
  const card = heading.closest<HTMLElement>("article");
  await expect(card).not.toBeNull();
  return card!;
}

/**
 * Walk the stage picker through both values and assert the streaming note is
 * shown for "Before response" only, described on the stage it depends on.
 */
async function expectStreamingNoteOnlyForResponseStage(
  canvasElement: HTMLElement,
  says: RegExp,
): Promise<void> {
  await userEvent.click(await within(canvasElement).findByRole("button", { name: /add rule/i }));
  const dialog = await within(document.body).findByRole("dialog");
  const stage = within(dialog).getByLabelText("Stage");
  // a new rule starts on "Before upstream", which streaming does not touch
  await expect(within(dialog).queryByRole("note")).toBeNull();

  await pickOption(stage, "Before response");
  const note = await within(dialog).findByRole("note");
  await expect(note).toHaveTextContent(says);
  await expect(stage).toHaveAccessibleDescription(says);

  await pickOption(stage, "Before upstream");
  await waitFor(() => expect(within(dialog).queryByRole("note")).toBeNull());
  await expect(stage).not.toHaveAccessibleDescription(says);
}

/**
 * The default: an enforced response rule refuses every streamed request on the
 * routes it covers (#2156).
 *
 * The dialog offered "Before response" with no word about streaming, so one
 * rule could turn away most interactive traffic. Both the dialog, before
 * saving, and the card, after, say so now, and name the setting that decides.
 */
export const WarnsThatAResponseRuleRefusesStreams: Story = {
  render: () => <Harness fetchStub={streamingStub("reject")} />,
  play: async ({ canvasElement }) => {
    const response = await ruleCard(canvasElement, /Mask emails in answers/);
    await expect(
      await within(response).findByText(/Refuses streamed requests on its routes/),
    ).toBeVisible();
    await expect(response).toHaveTextContent('streaming_post_call = "reject"');
    // a request rule runs before the upstream call, so streaming is no concern
    const request = await ruleCard(canvasElement, /Redact customer email/);
    await expect(request).not.toHaveTextContent(/streamed/i);

    await expectStreamingNoteOnlyForResponseStage(
      canvasElement,
      /streamed requests on the routes it covers are refused with a 400.*streaming_post_call = "reject"/,
    );
  },
};

/**
 * `passthrough`: the stream is served and the rule never sees it, so a redact
 * rule whose card says "enforced" leaves streamed answers untouched (#2156).
 */
export const WarnsThatStreamsSkipAResponseRule: Story = {
  render: () => <Harness fetchStub={streamingStub("passthrough")} />,
  play: async ({ canvasElement }) => {
    const response = await ruleCard(canvasElement, /Mask emails in answers/);
    await expect(
      await within(response).findByText(/Streamed responses skip this rule/),
    ).toBeVisible();
    await expect(response).toHaveTextContent('streaming_post_call = "passthrough"');
    const request = await ruleCard(canvasElement, /Redact customer email/);
    await expect(request).not.toHaveTextContent(/streamed/i);

    await expectStreamingNoteOnlyForResponseStage(
      canvasElement,
      /Streamed responses skip this rule.*streaming_post_call = "passthrough"/,
    );
  },
};

/**
 * The effective config did not answer: the screen says the setting could not
 * be read and names both outcomes, rather than assuming the default (#2156).
 */
export const SaysWhenTheStreamingSettingIsUnreadable: Story = {
  render: () => <Harness fetchStub={streamingStub("unreadable")} />,
  play: async ({ canvasElement }) => {
    const response = await ruleCard(canvasElement, /Mask emails in answers/);
    await expect(
      await within(response).findByText(/Effect on streamed requests unknown/),
    ).toBeVisible();
    await expect(response).toHaveTextContent("streaming_post_call could not be read");
    await expect(response).not.toHaveTextContent('"reject"');
    const request = await ruleCard(canvasElement, /Redact customer email/);
    await expect(request).not.toHaveTextContent(/streamed/i);

    await expectStreamingNoteOnlyForResponseStage(
      canvasElement,
      /streaming_post_call could not be read.*Under reject.*refused.*under passthrough.*skip the rule/,
    );
  },
};

/** the status badge on a card, by its exact label */
const expectStatus = async (card: HTMLElement, label: string) =>
  expect(within(card).getByText(label, { exact: true })).toBeVisible();

/**
 * The `guardrails` feature flag is off, so no rule inspects traffic (#2157).
 *
 * The cards used to say enforced regardless, and the empty state told the
 * operator to turn the flag on later, on a screen that never read it. A banner
 * says so now and links to the one screen that changes it, and no card claims
 * enforcement.
 */
export const SaysWhenTheGuardrailsFlagIsOff: Story = {
  render: () => (
    <Harness
      fetchStub={withPolicy(
        async () => json(RULES),
        () => json(effectiveConfig({ on: false })),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const banner = await canvas.findByRole("region", { name: "Guardrails are switched off" });
    await expect(banner).toHaveTextContent(/no rule inspects traffic/);
    await expect(within(banner).getByRole("link", { name: "Open Feature Flags" })).toHaveAttribute(
      "href",
      "/feature-flags",
    );
    for (const name of [/Redact customer email/, /Block override attempts/]) {
      const card = await ruleCard(canvasElement, name);
      await expectStatus(card, "not enforced");
      await expect(within(card).queryByText("enforced", { exact: true })).toBeNull();
    }
  },
};

// the config file's version of the email rule: it blocks where the row redacts
const FILE_EMAIL: EffectiveRule = {
  name: "Redact customer email",
  builtin: "email",
  stage: "pre_call",
  action: "block",
  include_system: false,
};

// the injection rule again, in the file, beside a paused row of the same name
const FILE_INJECTION: EffectiveRule = {
  name: "Block override attempts",
  pattern: "(?i)disregard (all|previous) instructions",
  stage: "pre_call",
  action: "block",
  include_system: true,
};

const PAUSED_INJECTION: GuardrailRuleRow = { ...RULES[1], enabled: false };

/**
 * A row whose name the config file also uses (#2157).
 *
 * The store keeps the file's rule and drops the row, while its card kept
 * saying enforced. The card says overridden now and names the file rule, which
 * is listed below it. A paused row under a file rule's name says it would not
 * run if resumed, and the dialog warns before a name clash is saved.
 */
export const ShowsARowTheConfigFileOverrides: Story = {
  render: () => (
    <Harness
      fetchStub={withPolicy(
        async () => json([RULES[0], PAUSED_INJECTION]),
        () =>
          json(
            effectiveConfig({
              rows: [RULES[0], PAUSED_INJECTION],
              file: [FILE_EMAIL, FILE_INJECTION],
              routes: [route("support-bot", { disable: ["Redact customer email"] })],
            }),
          ),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const overridden = await ruleCard(canvasElement, /10 · Redact customer email/);
    await expectStatus(overridden, "overridden by config file");
    await expect(overridden).toHaveTextContent(
      "Not running. The config file also defines Redact customer email",
    );
    await expect(within(overridden).queryByText("enforced", { exact: true })).toBeNull();
    // the override names a rule, and the rule under that name is the file's
    await expect(overridden).not.toHaveTextContent(/Off for traffic/);

    const paused = await ruleCard(canvasElement, /20 · Block override attempts/);
    await expectStatus(paused, "paused");
    await expect(paused).toHaveTextContent(
      "The config file also defines Block override attempts, so this rule would not run if resumed",
    );

    // the rule that runs instead is on the screen, read-only
    const file = await ruleCard(canvasElement, /^Redact customer email$/, 3);
    await expectStatus(file, "enforced");
    await expect(within(file).getByText("block")).toBeVisible();
    await expect(file).toHaveTextContent("Off for traffic on 1 route");

    await userEvent.click(
      within(canvasElement).getByRole("button", { name: "Edit rule Redact customer email" }),
    );
    const dialog = await within(document.body).findByRole("dialog");
    const name = within(dialog).getByLabelText("Rule name");
    await expect(name).toHaveAccessibleDescription(/config file already defines a rule/);
    await userEvent.type(name, " (dashboard)");
    await waitFor(() => expect(name).not.toHaveAccessibleDescription(/config file/));
  },
};

/**
 * A config-file rule that is a copy of an enabled row, field for field, cannot
 * be told from the row by comparing them, and the card used to say enforced
 * (#2249). The control plane names the file as the source, so the card says the
 * row is overridden and the rule is listed as the file's.
 */
export const ShowsARowWhoseCopyTheConfigFileRuns: Story = {
  render: () => (
    <Harness
      fetchStub={async (input, init) =>
        String(input).includes("/api/v1/guardrails/effective")
          ? json({
              rules: [
                { ...asEffective(RULES[0]), source: "file", rule_id: null, overrides: RULES[0].id },
              ],
            })
          : withPolicy(
              async () => json([RULES[0]]),
              () => json(effectiveConfig({ rows: [RULES[0]], file: [asEffective(RULES[0])] })),
            )(input, init)
      }
    />
  ),
  play: async ({ canvasElement }) => {
    const overridden = await ruleCard(canvasElement, /10 · Redact customer email/);
    await expectStatus(overridden, "overridden by config file");
    await expect(within(overridden).queryByText("enforced", { exact: true })).toBeNull();
    const file = await ruleCard(canvasElement, /^Redact customer email$/, 3);
    await expectStatus(file, "enforced");
  },
};

// a rule only the config file defines
const FILE_AWS: EffectiveRule = {
  name: "Block AWS access keys",
  pattern: "AKIA[A-Z0-9]{16}",
  stage: "pre_call",
  action: "block",
  include_system: true,
};

// and a response rule there, so its card carries the streaming line too
const FILE_CARDS: EffectiveRule = {
  name: "Mask card numbers in answers",
  builtin: "payment_card",
  stage: "post_call",
  action: "redact",
  replacement: "[CARD]",
  include_system: false,
};

/**
 * Config-file rules run but were never listed, so the screen was not the whole
 * policy a reviewer signs off (#2157). They are listed now under a heading of
 * their own, read-only, with the source on every card and no edit or delete.
 */
export const ListsConfigFileRulesReadOnly: Story = {
  render: () => (
    <Harness
      fetchStub={withPolicy(
        async () => json(RULES),
        () => json(effectiveConfig({ file: [FILE_AWS, FILE_CARDS] })),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const section = await canvas.findByRole("region", { name: "Config-file rules" });
    await expect(section).toHaveTextContent("[[guardrails.rules]]");
    await expect(section).toHaveTextContent(/run before the rules above/);

    const aws = await ruleCard(canvasElement, /Block AWS access keys/, 3);
    await expectStatus(aws, "enforced");
    await expect(aws).toHaveTextContent("AKIA[A-Z0-9]{16}");
    await expect(within(aws).getByText("Read-only · config file")).toBeVisible();
    await expect(within(aws).queryByRole("button")).toBeNull();

    const cards = await ruleCard(canvasElement, /Mask card numbers in answers/, 3);
    await expect(cards).toHaveTextContent("Replacement · [CARD]");
    await expect(cards).toHaveTextContent(/Refuses streamed requests on its routes/);

    // the rows added here are unchanged
    await expectStatus(await ruleCard(canvasElement, /Redact customer email/), "enforced");
  },
};

/**
 * No rule added here, but the config file has some: "No inspection rules"
 * would misstate the policy, so the empty state says whose rules run (#2157).
 */
export const EmptyBesideConfigFileRules: Story = {
  render: () => (
    <Harness
      fetchStub={withPolicy(
        async () => json([]),
        () => json(effectiveConfig({ rows: [], file: [FILE_AWS] })),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No rules added here/, /Add first rule/);
    await expect(within(canvasElement).queryByText(/No inspection rules/)).toBeNull();
    await ruleCard(canvasElement, /Block AWS access keys/, 3);
  },
};

// flipped by the play function, so the retry is the request that succeeds
let configAnswers = false;

/**
 * The effective config did not answer, so the screen cannot say whether the
 * flag is on or which config-file rules apply (#2157). It says so rather than
 * calling every row enforced, and a retry that reaches the config clears it.
 */
export const SaysWhenTheEffectivePolicyIsUnreadable: Story = {
  render: () => {
    configAnswers = false;
    return (
      <Harness
        fetchStub={withPolicy(
          async () => json(RULES),
          () =>
            configAnswers
              ? json(effectiveConfig())
              : json({ error: { message: "store offline" } }, 503),
        )}
      />
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const banner = await canvas.findByRole("region", {
      name: "Could not confirm which rules the gateway runs",
    });
    await expect(banner).toHaveTextContent(/could not be read/);
    for (const name of [/Redact customer email/, /Block override attempts/]) {
      const card = await ruleCard(canvasElement, name);
      await expectStatus(card, "status unknown");
      await expect(within(card).queryByText("enforced", { exact: true })).toBeNull();
    }

    configAnswers = true;
    await userEvent.click(within(banner).getByRole("button", { name: "Try again" }));
    await waitFor(() =>
      expect(
        canvas.queryByRole("region", { name: "Could not confirm which rules the gateway runs" }),
      ).toBeNull(),
    );
    await expectStatus(await ruleCard(canvasElement, /Redact customer email/), "enforced");
  },
};

/**
 * The routes that switch rules off, beside a rule each route runs (#2283).
 *
 * A route's `advanced.guardrails.disable` turns a rule off for that route's
 * traffic, and no screen said so: a card read enforced while the one route
 * serving customers skipped it. `support-bot` switches off the email rule and
 * `internal-search` the config-file AWS rule; `claude-sonnet` disables the
 * injection rule but enables it too, and `enable` wins; `gpt-4o` names a rule
 * that no longer exists, which the config validator reports and this screen
 * leaves alone.
 */
const OFF_ON_ONE_ROUTE = withPolicy(
  async () => json(RULES),
  () =>
    json(
      effectiveConfig({
        file: [FILE_AWS],
        routes: [
          route("support-bot", { disable: ["Redact customer email"] }),
          route("internal-search", { disable: ["Block AWS access keys"] }),
          route("claude-sonnet", {
            disable: ["Block override attempts"],
            enable: ["Block override attempts"],
          }),
          route("gpt-4o", { disable: ["Deleted rule"] }),
        ],
      }),
    ),
);

/** the list of route names under a card's exception line, by that line's text */
const offRouteNames = (card: HTMLElement, summary: string) =>
  Array.from(
    within(card).getByRole("list", { name: summary }).querySelectorAll("li"),
    (item) => item.textContent,
  );

export const ShowsTheRoutesARuleIsOffOn: Story = {
  render: () => <Harness fetchStub={OFF_ON_ONE_ROUTE} />,
  play: async ({ canvasElement }) => {
    const email = await ruleCard(canvasElement, /Redact customer email/);
    // an exception is extra: the rule still runs on every other route
    await expectStatus(email, "enforced");
    await expect(await within(email).findByText("Off for traffic on 1 route")).toBeVisible();
    await expect(offRouteNames(email, "Off for traffic on 1 route")).toEqual(["support-bot"]);
    await expect(email).toHaveTextContent("Set in the route's advanced.guardrails.disable.");

    // the config-file rule's card names its route the same way
    const aws = await ruleCard(canvasElement, /Block AWS access keys/, 3);
    await expectStatus(aws, "enforced");
    await expect(offRouteNames(aws, "Off for traffic on 1 route")).toEqual(["internal-search"]);

    // enable wins a conflict, so the injection rule runs on claude-sonnet
    const injection = await ruleCard(canvasElement, /Block override attempts/);
    await expectStatus(injection, "enforced");
    await expect(injection).not.toHaveTextContent(/Off for traffic/);

    // an override naming a missing rule shows nowhere
    await expect(canvasElement).not.toHaveTextContent(/gpt-4o|Deleted rule/);
  },
};

/** The same card in Russian, whose count takes a different form per number. */
export const ShowsTheRoutesARuleIsOffOnInRussian: Story = {
  globals: { locale: "ru" },
  render: () => <Harness fetchStub={OFF_ON_ONE_ROUTE} />,
  play: async ({ canvasElement }) => {
    const copy = ru.pages.guardrailRules.offRoutes;
    const summary = copy.summary_one.replace("{{value}}", "1");
    const email = await ruleCard(canvasElement, /Redact customer email/);
    // the locale decorator switches language from an effect, after first paint
    await waitFor(() => expect(within(email).getByText(summary)).toBeVisible());
    await expectStatus(email, ru.guardrailPanel.enforced);
    await expect(offRouteNames(email, summary)).toEqual(["support-bot"]);
    await expect(email).toHaveTextContent(copy.source_one.replace(/<\/?0>/g, ""));
    await expect(email).not.toHaveTextContent(en.pages.guardrailRules.offRoutes.summary_one);
  },
};

// eight routes that switch the email rule off, listed out of order
const MANY_ROUTES = [
  "support-bot",
  "billing-assistant",
  "internal-search",
  "code-review",
  "sales-copilot",
  "hr-helpdesk",
  "legal-drafts",
  "analytics-sql",
];

/**
 * A rule switched off on more routes than a card lists in the open: the count
 * stays on the card and the names fold behind a disclosure, sorted (#2283).
 */
export const FoldsManyRoutesBehindADisclosure: Story = {
  render: () => (
    <Harness
      fetchStub={withPolicy(
        async () => json(RULES),
        () =>
          json(
            effectiveConfig({
              routes: MANY_ROUTES.map((model) =>
                route(model, { disable: ["Redact customer email"] }),
              ),
            }),
          ),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const email = await ruleCard(canvasElement, /Redact customer email/);
    await expectStatus(email, "enforced");
    const label = "Off for traffic on 8 routes";
    const summary = (await within(email).findByText(label)).closest("summary");
    await expect(summary).not.toBeNull();
    const list = within(email).getByRole("list", { hidden: true, name: label });
    await expect(list).not.toBeVisible();

    await userEvent.click(summary!);
    await waitFor(() => expect(list).toBeVisible());
    await expect(offRouteNames(email, label)).toEqual([...MANY_ROUTES].sort());
    await expect(email).toHaveTextContent("Set in each route's advanced.guardrails.disable.");

    // and folds again, the count still on the card
    await userEvent.click(summary!);
    await waitFor(() => expect(list).not.toBeVisible());
    await expect(summary).toBeVisible();
  },
};

// What a non-superadmin gets: the screen refused before it asks (#1606).
//
// `guardrail_rule` is superadmin at every action, so `superadminOnly` never
// mounts the screen for an org role however high. The stub answers with a good
// payload on purpose: if the wrapper is dropped the screen renders that payload
// and this story fails, which the `Forbidden` story cannot do.
export const RefusedToAnAdmin: Story = {
  render: () => <Harness fetchStub={async () => json(RULES)} role="admin" />,
  play: async ({ canvasElement }) => expectForbidden(canvasElement),
};

export const RefusedToAViewer: Story = {
  render: () => <Harness fetchStub={async () => json(RULES)} role="viewer" />,
  play: async ({ canvasElement }) => expectForbidden(canvasElement),
};

// the cards at a phone's width: the Russian action labels are the longest, and
// a card that cannot shrink pushed the page sideways at 320 px (#2252)
const rulesFit = phoneFits({
  render: () => <Harness fetchStub={withPolicy(async () => json(RULES))} />,
  ready: async (canvas, locale) => {
    await canvas.findByRole("heading", { name: /Redact customer email/, level: 2 });
    await waitFor(() =>
      expect(document.body).toHaveTextContent((locale === "ru" ? ru : en).guardrailPanel.enforced),
    );
  },
});
export const SmallPhone: Story = rulesFit("small", "en");
export const SmallPhoneInRussian: Story = rulesFit("small", "ru");

/**
 * Each card states what its rule did over the last seven days, and every
 * non-zero count opens the LLM Logs rows behind it (#2158). A rule that never
 * matched says so rather than printing three zeros.
 */
export const ShowsWhatEachRuleDid: Story = {
  render: () => (
    <Harness
      fetchStub={withOutcomes(
        async () => json(RULES),
        () =>
          json({
            recorded: true,
            data: [
              { action: "redacted", rule: "Redact customer email", requests: "12" },
              { action: "blocked", rule: "Block override attempts", requests: "3" },
            ],
          }),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const redact = within(await ruleCard(canvasElement, /Redact customer email/));
    await expect(await redact.findByText("Matches in the last 7 days")).toBeVisible();
    const redacted = redact.getByRole("link", { name: /12 requests Redacted/ });
    await expect(redacted).toHaveTextContent("12");
    await expect(redacted).toHaveAttribute(
      "href",
      "/logs?window=7d&guardrail=redacted%3ARedact%20customer%20email",
    );
    // a count of zero is plain text: there is no row to open
    await expect(redact.queryByRole("link", { name: /Blocked/ })).toBeNull();

    const block = within(await ruleCard(canvasElement, /Block override attempts/));
    await expect(await block.findByRole("link", { name: /3 requests Blocked/ })).toHaveAttribute(
      "href",
      "/logs?window=7d&guardrail=blocked%3ABlock%20override%20attempts",
    );
  },
};

/** A rule that has not matched in the window says "no matches", not a row of zeros. */
export const NoMatchesYet: Story = {
  render: () => (
    <Harness
      fetchStub={withOutcomes(
        async () => json(RULES),
        () => json({ recorded: true, data: [] }),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const card = within(await ruleCard(canvasElement, /Redact customer email/));
    await expect(await card.findByText("No matches in the last 7 days.")).toBeVisible();
    await expect(card.queryByRole("link", { name: /requests/ })).toBeNull();
  },
};

/**
 * A deployment with no ClickHouse cannot count anything. The screen says so
 * once, calmly, and the cards claim neither matches nor their absence.
 */
export const NoAnalytics: Story = {
  render: () => (
    <Harness
      fetchStub={withOutcomes(
        async () => json(RULES),
        () =>
          json({ error: { message: "analytics unavailable: no clickhouse_url configured" } }, 503),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    await expectAnalyticsUnavailable(
      canvasElement,
      "Match counts need an analytics store",
      "analytics unavailable: no clickhouse_url configured",
    );
    await expect(canvasElement).not.toHaveTextContent(/No matches in the last 7 days/);
    await expect(canvasElement).not.toHaveTextContent(/Counting matches/);
  },
};

/** ClickHouse is there but the file that keeps outcomes was never applied. */
export const OutcomesNotRecorded: Story = {
  render: () => (
    <Harness
      fetchStub={withOutcomes(
        async () => json(RULES),
        () => json({ recorded: false, data: [] }),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/Match counts are not being kept/)).toBeVisible();
    await expect(canvasElement).not.toHaveTextContent(/No matches in the last 7 days/);
  },
};

/** The rollup failing leaves the rules readable and says the counts are missing. */
export const OutcomesFailed: Story = {
  render: () => (
    <Harness
      fetchStub={withOutcomes(
        async () => json(RULES),
        () => json({ error: { message: "clickhouse offline" } }, 502),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const card = within(await ruleCard(canvasElement, /Redact customer email/));
    await expect(
      await card.findByText("Could not load match counts.", undefined, { timeout: 6000 }),
    ).toBeVisible();
    await expect(card.getByText("enforced", { exact: true })).toBeVisible();
  },
};

const isRuleTrial = (input: RequestInfo | URL) =>
  new URL(String(input), "http://localhost").pathname === "/api/v1/guardrails/rules/test";

/** the dialog, open on a new rule, with its trial panel expanded */
async function openTrial(canvasElement: HTMLElement) {
  await userEvent.click(await within(canvasElement).findByRole("button", { name: /add rule/i }));
  const dialog = within(await within(document.body).findByRole("dialog"));
  await userEvent.click(dialog.getByRole("button", { name: "Try it on a sample" }));
  return dialog;
}

const trials = recording(
  withPolicy(async (input) =>
    isRuleTrial(input)
      ? json({
          outcome: "redacted",
          match_count: 1,
          spans: [{ start: 9, end: 15 }],
          redacted: "write to [REDACTED:EMAIL] now",
        })
      : json(RULES),
  ),
);

/**
 * The draft is run on a sample before it is published (#2159): the match is
 * marked in the sample and the redacted text is shown, and the request carries
 * the draft as it is filled in, not a stored row.
 */
export const TriesADraftOnASample: Story = {
  render: () => <Harness fetchStub={trials.stub} />,
  play: async ({ canvasElement }) => {
    const dialog = await openTrial(canvasElement);
    // no sample, nothing to run
    await expect(dialog.getByRole("button", { name: "Run test" })).toBeDisabled();
    await userEvent.type(dialog.getByLabelText("Sample text"), "write to a@b.io now");
    await userEvent.click(dialog.getByRole("button", { name: "Run test" }));

    const result = await dialog.findByRole("status");
    await expect(within(result).getByText("Redacted")).toBeVisible();
    await expect(within(result).getByText("1 match")).toBeVisible();
    await expect(within(result).getByText("a@b.io").tagName).toBe("MARK");
    await expect(within(result).getByText(/write to \[REDACTED:EMAIL\] now/)).toBeVisible();

    const sent = await trials.expectSentBody<Record<string, unknown>>("POST", "/rules/test");
    await expect(sent).toMatchObject({
      builtin: "email",
      pattern: null,
      action: "redact",
      stage: "pre_call",
      system: false,
      sample: "write to a@b.io now",
    });
    // nothing was published
    await expect(
      trials.calls.filter((call) => call.method === "POST" && !call.url.endsWith("/rules/test")),
    ).toHaveLength(0);

    // changing the draft retires an answer that no longer describes it
    await userEvent.type(dialog.getByLabelText("Sample text"), "!");
    await waitFor(() => expect(dialog.queryByRole("status")).toBeNull());
  },
};

/** a sample the rule does not match says so, and a system sample says it was not scanned */
export const SaysWhenARuleDoesNotMatch: Story = {
  render: () => {
    let n = 0;
    return (
      <Harness
        fetchStub={withPolicy(async (input) => {
          if (!isRuleTrial(input)) return json(RULES);
          n += 1;
          return n === 1
            ? json({ outcome: "no_match", match_count: 0, spans: [] })
            : json({
                outcome: "skipped",
                skip: "system_message",
                match_count: 0,
                spans: [],
              });
        })}
      />
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openTrial(canvasElement);
    await userEvent.type(dialog.getByLabelText("Sample text"), "nothing sensitive");
    await userEvent.click(dialog.getByRole("button", { name: "Run test" }));
    const result = await dialog.findByRole("status");
    await expect(within(result).getByText("No match, forwarded unchanged")).toBeVisible();
    await expect(within(result).getByText("0 matches")).toBeVisible();

    await userEvent.click(dialog.getByRole("switch", { name: "Treat as a system message" }));
    await userEvent.click(dialog.getByRole("button", { name: "Run test" }));
    await expect(await dialog.findByText("Not scanned")).toBeVisible();
    await expect(dialog.getByText(/does not inspect system messages/)).toBeVisible();
  },
};

/** the server's own message for a pattern the gateway would refuse, beside the panel */
export const TrialShowsAnInvalidPattern: Story = {
  render: () => (
    <Harness
      fetchStub={withPolicy(async (input) =>
        isRuleTrial(input)
          ? json(
              {
                error: {
                  message: "guardrail rule 'draft' has an invalid or unbounded pattern: unclosed",
                },
              },
              400,
            )
          : json(RULES),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openTrial(canvasElement);
    await pickOption(dialog.getByLabelText("Source"), "Custom regex");
    await userEvent.type(dialog.getByLabelText("Regular expression"), "[[");
    await userEvent.type(dialog.getByLabelText("Sample text"), "anything");
    await userEvent.click(dialog.getByRole("button", { name: "Run test" }));
    await expect(await dialog.findByRole("alert")).toHaveTextContent(/invalid or unbounded/);
    await expect(dialog.queryByRole("status")).toBeNull();
  },
};

/** the picker lines say what each choice does, and move with the choice */
export const PickersSayWhatTheyDo: Story = {
  render: () => <Harness fetchStub={withPolicy(async () => json(RULES))} />,
  play: async ({ canvasElement }) => {
    await userEvent.click(await within(canvasElement).findByRole("button", { name: /add rule/i }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await expect(dialog.getByLabelText("Detector")).toHaveAccessibleDescription(/name@domain\.tld/);
    await pickOption(dialog.getByLabelText("Detector"), "Payment card");
    await expect(dialog.getByLabelText("Detector")).toHaveAccessibleDescription(
      /does not check the card number/,
    );
    await pickOption(dialog.getByLabelText("Action"), "Annotate only");
    await expect(dialog.getByLabelText("Action")).toHaveAccessibleDescription(
      /records the rule's name on the request log/,
    );
  },
};

/**
 * The two pickers the others hang off say what they choose between in an (i)
 * (#487): where a rule looks for a match, and which leg of the request it
 * reads. The action and the detector already describe the chosen option under
 * the field.
 */
function explainsSourceAndStage(locale: "en" | "ru"): Story {
  const catalog = locale === "ru" ? ru : en;
  const r = catalog.pages.guardrailRules;
  const about = (label: string) => catalog.common.aboutField.replace("{{label}}", label);
  return {
    ...(locale === "ru" ? { globals: { locale: "ru" } } : {}),
    render: () => <Harness fetchStub={withPolicy(async () => json(RULES))} />,
    play: async ({ canvasElement }) => {
      await userEvent.click(await within(canvasElement).findByRole("button", { name: r.addRule }));
      const dialog = await within(document.body).findByRole("dialog", { name: r.dialogAddTitle });
      await expectFieldHint(dialog, about(r.fieldSource), r.sourceInfo);
      await expectFieldHint(dialog, about(r.fieldStage), r.stageInfo);
    },
  };
}
export const ExplainsSourceAndStageInAHint: Story = explainsSourceAndStage("en");
export const ExplainsSourceAndStageInAHintInRussian: Story = explainsSourceAndStage("ru");

/** the panel in Russian, with the plural the count needs */
export const TrialInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness
      fetchStub={withPolicy(async (input) =>
        isRuleTrial(input)
          ? json({
              outcome: "blocked",
              match_count: 2,
              spans: [
                { start: 0, end: 1 },
                { start: 2, end: 3 },
              ],
            })
          : json(RULES),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const copy = ru.pages.guardrailRules;
    // the locale decorator switches language from an effect, after first paint
    const add = await within(canvasElement).findByRole("button", { name: copy.addRule });
    await userEvent.click(add);
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.click(dialog.getByRole("button", { name: copy.trial.title }));
    await userEvent.type(dialog.getByLabelText(copy.trial.fieldSample), "a b");
    await userEvent.click(dialog.getByRole("button", { name: copy.trial.run }));
    await waitFor(() => expect(dialog.getByText("2 совпадения")).toBeVisible());
    await expect(dialog.getByText(copy.trial.outcome.blocked)).toBeVisible();
  },
};

const isPolicyTrial = (input: RequestInfo | URL) =>
  new URL(String(input), "http://localhost").pathname === "/api/v1/guardrails/policy/test";

const POLICY_RUN = {
  outcome: "redacted",
  steps: [
    {
      rule: "file-pager",
      stage: "pre_call",
      action: "annotate",
      outcome: "annotated",
      match_count: 1,
      source: "file",
    },
    {
      rule: "Redact customer email",
      stage: "pre_call",
      action: "redact",
      outcome: "redacted",
      match_count: 1,
      source: "dashboard",
    },
    {
      rule: "Block email",
      stage: "pre_call",
      action: "block",
      outcome: "no_match",
      match_count: 0,
      source: "dashboard",
    },
  ],
  redacted: "PAGER [REDACTED:EMAIL]",
};

const policyTrials = recording(
  withPolicy(async (input) => (isPolicyTrial(input) ? json(POLICY_RUN) : json(RULES))),
);

async function openPolicyTrial(canvasElement: HTMLElement) {
  await userEvent.click(
    await within(canvasElement).findByRole("button", { name: "Try the policy" }),
  );
  return within(await within(document.body).findByRole("dialog"));
}

/**
 * The whole policy is run on a sample (#3084): every rule shows in the order it
 * runs with where it comes from, a redaction explains why a later block did not
 * fire, and the text that would be forwarded is shown. The request carries the
 * sample and stage and no draft.
 */
export const TriesThePolicyOnASample: Story = {
  render: () => <Harness fetchStub={policyTrials.stub} />,
  play: async ({ canvasElement }) => {
    const dialog = await openPolicyTrial(canvasElement);
    await expect(dialog.getByRole("button", { name: "Run policy" })).toBeDisabled();
    await userEvent.type(dialog.getByLabelText("Sample text"), "PAGER a@b.io");
    await userEvent.click(dialog.getByRole("button", { name: "Run policy" }));

    const trace = within(await dialog.findByRole("list", { name: "Rules in the order they run" }));
    const steps = trace.getAllByRole("listitem");
    await expect(steps).toHaveLength(3);
    await expect(within(steps[0]).getByText("Config file")).toBeVisible();
    await expect(within(steps[0]).getByText("Annotated")).toBeVisible();
    await expect(within(steps[1]).getByText("Dashboard")).toBeVisible();
    await expect(within(steps[1]).getByText("Redacted")).toBeVisible();
    await expect(within(steps[2]).getByText("No match")).toBeVisible();
    await expect(dialog.getByText("Redacted, then forwarded")).toBeVisible();
    await expect(dialog.getByText(/PAGER \[REDACTED:EMAIL\]/)).toBeVisible();

    const sent = await policyTrials.expectSentBody<Record<string, unknown>>("POST", "/policy/test");
    await expect(sent).toEqual({ sample: "PAGER a@b.io", stage: "pre_call", system: false });

    // the stage is part of the question, so an answer to the other one is retired
    await userEvent.click(dialog.getByRole("radio", { name: "Response" }));
    await waitFor(() => expect(dialog.queryByRole("list", { name: /order they run/ })).toBeNull());
  },
};

/** the draft in the rule dialog is slotted into the policy at its own position */
export const TriesADraftInThePolicy: Story = {
  render: () => {
    const recorder = recording(
      withPolicy(async (input) =>
        isPolicyTrial(input)
          ? json({
              outcome: "blocked",
              blocked_by: "Draft block",
              steps: [
                {
                  rule: "Draft block",
                  stage: "pre_call",
                  action: "block",
                  outcome: "blocked",
                  match_count: 1,
                  source: "draft",
                },
                {
                  rule: "Redact customer email",
                  stage: "pre_call",
                  action: "redact",
                  outcome: "skipped",
                  skip: "after_block",
                  match_count: 0,
                  source: "dashboard",
                },
              ],
            })
          : json(RULES),
      ),
    );
    return <Harness fetchStub={recorder.stub} />;
  },
  play: async ({ canvasElement }) => {
    const dialog = await openTrial(canvasElement);
    await userEvent.type(dialog.getByLabelText("Rule name"), "Draft block");
    await userEvent.type(dialog.getByLabelText("Sample text"), "a@b.io");
    await userEvent.click(dialog.getByRole("button", { name: "Run in the whole policy" }));

    await expect(await dialog.findByText("Blocked by Draft block")).toBeVisible();
    const steps = dialog.getAllByRole("listitem");
    await expect(within(steps[0]).getByText("Draft")).toBeVisible();
    await expect(
      within(steps[1]).getByText("Not reached: an earlier rule blocked the request."),
    ).toBeVisible();
  },
};

/** a refusal from the endpoint is shown beside the panel, with no trace */
export const PolicyTrialShowsAnError: Story = {
  render: () => (
    <Harness
      fetchStub={withPolicy(async (input) =>
        isPolicyTrial(input)
          ? json({ error: { message: "sample must be at most 65536 bytes" } }, 400)
          : json(RULES),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openPolicyTrial(canvasElement);
    await userEvent.type(dialog.getByLabelText("Sample text"), "x");
    await userEvent.click(dialog.getByRole("button", { name: "Run policy" }));
    await expect(await dialog.findByRole("alert")).toHaveTextContent(/at most 65536 bytes/);
    await expect(dialog.queryByRole("list", { name: /order they run/ })).toBeNull();
  },
};

/** the panel in Russian */
export const PolicyTrialInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness
      fetchStub={withPolicy(async (input) =>
        isPolicyTrial(input) ? json({ ...POLICY_RUN, outcome: "redacted" }) : json(RULES),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const copy = ru.pages.guardrailRules;
    // the locale decorator switches language from an effect, after first paint
    await userEvent.click(
      await within(canvasElement).findByRole("button", { name: copy.policyTrial.open }),
    );
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.type(dialog.getByLabelText(copy.trial.fieldSample), "PAGER a@b.io");
    await userEvent.click(dialog.getByRole("button", { name: copy.policyTrial.run }));
    const verdict = await dialog.findByText(copy.policyTrial.verdict.redacted);
    await waitFor(() => expect(verdict).toBeVisible());
    await expect(dialog.getAllByText(copy.policyTrial.source.file).length).toBeGreaterThan(0);
  },
};
