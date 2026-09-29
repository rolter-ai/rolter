import type { Meta, StoryObj } from "@storybook/react-vite";
import { MemoryRouter } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import GuardrailRules from "./GuardrailRules";
import {
  cancelConfirmation,
  confirmDestructive,
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
import type { GuardrailRuleRow } from "@/lib/api";
import type { EffectiveRule } from "@/lib/guardrail-policy";
import { atShort, expectInViewport } from "@/lib/story-viewport";

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

/**
 * `GET /api/v1/config` as the store builds it: the config file's rules first,
 * then the enabled rows whose names the file does not take, with
 * `guardrails.enabled` set from the flag (#2157).
 */
function effectiveConfig({
  rows = RULES,
  file = [],
  on = true,
  streaming = "reject",
}: {
  rows?: GuardrailRuleRow[];
  file?: EffectiveRule[];
  on?: boolean;
  streaming?: "reject" | "passthrough";
} = {}) {
  const taken = new Set(file.map((rule) => rule.name));
  return {
    providers: [],
    routes: [],
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
    await userEvent.type(
      within(dialog).getByLabelText("Regular expression"),
      "ignore previous instructions",
    );
    await expect(within(dialog).getByRole("button", { name: "Publish rule" })).toBeEnabled();
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
    await expect(within(document.body).getByLabelText("Rule name")).toHaveValue(
      "Redact customer email",
    );
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

    const paused = await ruleCard(canvasElement, /20 · Block override attempts/);
    await expectStatus(paused, "paused");
    await expect(paused).toHaveTextContent(
      "The config file also defines Block override attempts, so this rule would not run if resumed",
    );

    // the rule that runs instead is on the screen, read-only
    const file = await ruleCard(canvasElement, /^Redact customer email$/, 3);
    await expectStatus(file, "enforced");
    await expect(within(file).getByText("block")).toBeVisible();

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
