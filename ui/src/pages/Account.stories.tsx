import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Account from "./Account";
import {
  Harness,
  NEEDS_MEMBER,
  answerSecretClosePrompt,
  cancelConfirmation,
  clickWhenEnabled,
  confirmDestructive,
  expectAllowed,
  expectAnalyticsUnavailable,
  expectClosesWithoutPrompting,
  expectLoadError,
  expectRefused,
  json,
  pending,
  pickOption,
  recording,
  scoped,
  secretClosePrompt,
  sheet,
  stubClipboard,
  answerDiscardPrompt,
  type FetchStub,
  expectEmptyState,
  expectInStatusRegion,
  expectNoFalseEmpty,
  expectNoUxEvent,
  expectSheetClosed,
  expectSkeleton,
  expectUxEvent,
  recordUxEvents,
  uxEvents,
  type Recorder,
} from "./story-harness";
import type {
  MfaStatus,
  MintedKey,
  MyUsageRow,
  OwnedKeyRow,
  ProviderRow,
  RouteRow,
} from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { atMobile, expectNoHorizontalOverflow } from "@/lib/story-viewport";
import { UxScreenProvider } from "@/lib/ux-react";

/**
 * No second factor and no policy asking for one — the default account.
 *
 * The panel's own states live in `Components/TwoFactorPanel`; what these
 * stories need from it is that it does not error out above the keys, which is
 * what an unanswered `/me/mfa` would look like (#1078).
 */
const MFA: MfaStatus = {
  enabled: false,
  enrolment_pending: false,
  recovery_codes_remaining: 0,
  policy: "off",
  required: false,
  enforce_after: null,
};

const KEYS: OwnedKeyRow[] = [
  {
    id: "vk-1",
    project_id: "project-1",
    project_name: "Gateway",
    org_name: "Rolter",
    key_prefix: "sk-rolter-laptop",
    name: "my laptop",
    models: ["gpt-4o"],
    disabled: false,
    expires_at: null,
    created_at: "2026-07-01T00:00:00Z",
  },
  {
    id: "vk-2",
    project_id: "project-1",
    project_name: "Gateway",
    org_name: "Rolter",
    key_prefix: "sk-rolter-retired",
    name: null,
    models: [],
    disabled: true,
    expires_at: null,
    created_at: "2026-06-01T00:00:00Z",
  },
];

const PROVIDERS: ProviderRow[] = [
  {
    id: "prov-1",
    org_id: "org-1",
    name: "OpenAI",
    slug: "openai",
    kind: "openai",
    api_base: "https://api.openai.com/v1",
    egress_proxies: [],
    created_at: "2026-01-01T00:00:00Z",
  },
];

/** the project's routes, which the model allow-list ticks off (#1345) */
const ROUTES: RouteRow[] = [
  {
    id: "route-1",
    project_id: "project-1",
    model: "gpt-4o",
    strategy: "round_robin",
    enabled: true,
    params: {},
    param_policy: {},
    advanced: {},
    created_at: "2026-01-02T00:00:00Z",
  },
  {
    id: "route-2",
    project_id: "project-1",
    model: "claude-sonnet",
    strategy: "round_robin",
    enabled: true,
    params: {},
    param_policy: {},
    advanced: {},
    created_at: "2026-01-03T00:00:00Z",
  },
];

const USAGE: MyUsageRow[] = [
  { virtual_key_id: "vk-1", requests: 1204, tokens: 903_112, cost_usd: "12.34", errors: 3 },
];

const MINTED: MintedKey = {
  id: "vk-3",
  project_id: "project-1",
  key_hash: "hash-3",
  key_prefix: "sk-rolter-ci-runner",
  name: "ci runner",
  models: [],
  providers: [],
  created_by: null,
  business_unit_id: null,
  customer_id: null,
  disabled: false,
  created_at: "2026-08-01T00:00:00Z",
  key: "sk-rolter-plaintext-shown-once",
};

/**
 * The screen runs two independent queries — keys and usage — and the usage one
 * is allowed to fail on its own, so every stub has to answer both.
 */
const account = (
  keys: (init?: RequestInit) => Response,
  usage: () => Response = () => json({ data: USAGE }),
): FetchStub =>
  scoped(async (input, init) => {
    const url = String(input);
    // the mint sheet's provider picker reads this one, and answering it with
    // the key list gave it an option named `null` — a checkbox row with no
    // label at all, which is what axe reported as `button-name` (#1181)
    if (url.includes("/providers")) return json(PROVIDERS);
    // the mint sheet's model allow-list ticks the project's routes off (#1345)
    if (url.includes("/routes")) return json(ROUTES);
    if (url.includes("/me/usage")) return usage();
    // the second-factor panel sits above the keys on this screen
    if (url.includes("/me/mfa")) return json(MFA);
    return keys(init);
  });

const loaded = account(() => json(KEYS));

/** the key the Playground minted for itself, which nobody here created (#944) */
const PLAYGROUND_KEY: OwnedKeyRow = {
  id: "vk-3",
  project_id: "project-1",
  project_name: "Gateway",
  org_name: "Rolter",
  key_prefix: "sk-rolter-play",
  name: "Playground",
  models: ["gpt-4o"],
  disabled: false,
  expires_at: "2026-07-01T00:30:00Z",
  purpose: "playground",
  created_at: "2026-07-01T00:00:00Z",
};

const withPlaygroundKey = account(() => json([...KEYS, PLAYGROUND_KEY]));

// rotation is destructive — the old secret dies the moment the new one exists —
// so the confirmation is what stands between a stray click and a broken client
const rotations = recording(
  account((init) => (init?.method === "POST" ? json(MINTED) : json(KEYS))),
);

const meta = {
  title: "Screens/Account",
  component: Account,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Account>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("my laptop")).toBeInTheDocument();
    // the second factor comes first: it protects the session that reaches
    // every key below it
    await expect(canvas.getByText("Two-factor authentication")).toBeInTheDocument();
    // a key with no usage row still renders, with the window spelled out —
    // "nothing recorded" and "analytics is down" must not look the same
    await expect(canvas.getByText(/no usage in the last 7 days/i)).toBeInTheDocument();
    // the figure is spelled out like the rest of the dashboard: "requests" and
    // "the last 7 days", not "req" and "(7d)"
    await expect(
      await canvas.findByText("1,204 requests · $12.34 in the last 7 days"),
    ).toBeInTheDocument();
    await expect(canvas.queryByText(/\breq\b|\(7d\)/)).toBeNull();
  },
};

/** A usage row for `id`, for the stories that care how the figure is worded. */
const usageRow = (id: string, requests: number, cost: string): MyUsageRow => ({
  virtual_key_id: id,
  requests,
  tokens: requests * 100,
  cost_usd: cost,
  errors: 0,
});

/** three keys, so one screen can show every plural form the language has */
const THREE_KEYS = [...KEYS, PLAYGROUND_KEY];

/**
 * One request reads "1 request", not "1 requests": the count picks the form
 * and the figure beside it is still formatted by the locale.
 */
export const UsageAgreesWithItsCount: Story = {
  render: () => (
    <Harness
      fetchStub={account(
        () => json(KEYS),
        () => json({ data: [usageRow("vk-1", 1, "0.05"), usageRow("vk-2", 2, "1.5")] }),
      )}
    >
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByText("1 request · $0.05 in the last 7 days"),
    ).toBeInTheDocument();
    await expect(canvas.getByText("2 requests · $1.50 in the last 7 days")).toBeInTheDocument();
  },
};

/**
 * Russian needs four plural forms, and all three keys here land on a different
 * one: 1 204 is "запроса", 5 is "запросов", 21 is "запрос". The catalog used to
 * abbreviate to "запр." and "7 дн.", which is no form at all.
 */
export const LoadedInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness
      fetchStub={account(
        () => json(THREE_KEYS),
        () =>
          json({
            data: [
              usageRow("vk-1", 1204, "12.34"),
              usageRow("vk-2", 5, "0.4"),
              usageRow("vk-3", 21, "1"),
            ],
          }),
      )}
    >
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the figure is locale-formatted, so the grouping space is matched loosely
    await expect(
      await canvas.findByText(/^1\s204 запроса · .+ за последние 7 дней$/),
    ).toBeInTheDocument();
    await expect(canvas.getByText(/^5 запросов · .+ за последние 7 дней$/)).toBeInTheDocument();
    await expect(canvas.getByText(/^21 запрос · .+ за последние 7 дней$/)).toBeInTheDocument();
    await expect(canvas.queryByText(/запр\.|дн\./)).toBeNull();
  },
};

// two panels load here, and the second-factor one had a skeleton first — so
// this asserts the *keys* placeholder specifically, in the `role="status"`
// region that makes it audible, rather than anything skeleton-shaped (#1589)
/**
 * A card for a key the Playground issued says so (#944).
 *
 * Every other card here is a key this account minted on purpose. This one
 * arrived because somebody opened a screen, and it deletes itself — which the
 * card has to say, or its half-hour life reads as a mistake.
 */
export const PlaygroundKeyIsLabelled: Story = {
  render: () => (
    <Harness fetchStub={withPlaygroundKey}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // by its explanation rather than its text: the control plane names the key
    // "Playground" as well, so the word alone matches the card title too
    const badge = await canvas.findByTitle(/expires on its own/);
    await expect(badge).toHaveTextContent("Playground");
    // only the key that carries the purpose is labelled
    await expect(canvas.getAllByTitle(/expires on its own/)).toHaveLength(1);
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectInStatusRegion(canvasElement, "own-keys-loading");
    await expectNoFalseEmpty(canvasElement, /No virtual keys yet/);
  },
};

export const Empty: Story = {
  render: () => (
    <Harness fetchStub={account(() => json([]))}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    // the screen can mint a key, so the placeholder that says there are none
    // offers to — a grey sentence was the half #1180 was filed over
    await expectEmptyState(canvasElement, /No virtual keys yet/);
    // the CTA has to be *in* the placeholder: the toolbar above carries a
    // button with the same words, so a canvas-wide match proves nothing
    await waitFor(() => {
      const placeholder = within(within(canvasElement).getByTestId("own-keys-empty"));
      expect(placeholder.getByRole("button", { name: "Generate virtual key" })).toBeEnabled();
    });
  },
};

/** Both places the screen offers a mint: the toolbar, and the empty state repeating it. */
const GENERATE = "Generate virtual key";

/**
 * Minting takes `my_virtual_key:create`, the member role at the project, and a
 * viewer does not hold it (#2064). The button used to open the whole sheet and
 * leave the refusal to a raw server line at the end of it. Both Generate
 * buttons now refuse up front and name the role, the way the Playground's mint
 * does (#2061), and the placeholder says who mints keys here and whom to ask
 * instead of inviting a mint the button beside it refuses.
 */
export const RefusedToAViewer: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness fetchStub={account(() => json([]))} role="viewer">
      <UxScreenProvider screen="api-keys">
        <Account />
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectEmptyState(canvasElement, /No virtual keys yet/);
    // every match is refused, and there are two of them to match: a gate on
    // the toolbar alone would leave the placeholder's copy of it live
    await expectRefused(canvasElement, GENERATE, NEEDS_MEMBER);
    await expect(canvas.getAllByRole("button", { name: GENERATE })).toHaveLength(2);

    const placeholder = within(canvas.getByTestId("own-keys-empty"));
    await expect(placeholder.getByText(/cannot mint keys/)).toBeVisible();
    await expect(placeholder.getByText(/ask an admin of this project/)).toBeVisible();
    await expect(placeholder.queryByText(/Create one to start calling the gateway/)).toBeNull();

    // a reach for it is recorded, and no sheet opens behind the refusal
    await userEvent.click(placeholder.getByRole("button", { name: GENERATE }), {
      pointerEventsCheck: 0,
    });
    await expectUxEvent("refused_click", "account-key-mint-empty:my_virtual_key:create");
    await expect(within(document.body).queryByRole("dialog")).toBeNull();
  },
};

/** A member holds the pair, so both buttons stay live and the placeholder invites a mint. */
export const AllowedToAMember: Story = {
  render: () => (
    <Harness fetchStub={account(() => json([]))} role="member">
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectEmptyState(canvasElement, /No virtual keys yet/);
    await expectAllowed(canvasElement, GENERATE);
    await expect(canvas.getAllByRole("button", { name: GENERATE })).toHaveLength(2);

    const placeholder = within(canvas.getByTestId("own-keys-empty"));
    await expect(placeholder.getByText(/Create one to start calling the gateway/)).toBeVisible();
    await expect(placeholder.queryByText(/cannot mint keys/)).toBeNull();

    // the placeholder's copy of the action opens the same sheet the toolbar's does
    await userEvent.click(placeholder.getByRole("button", { name: GENERATE }));
    await waitFor(() =>
      expect(within(document.body).getByRole("dialog", { name: "New virtual key" })).toBeVisible(),
    );
  },
};

export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={account(() => json({ error: { message: "forbidden" } }, 403))}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/do not have access to your keys/i)).toBeInTheDocument();
    await expectNoFalseEmpty(canvasElement, /No virtual keys yet/);
  },
};

/**
 * ClickHouse is optional, so `/me/usage` answering 503 is a supported
 * deployment rather than a fault. The keys must still render: losing the whole
 * self-service panel because the analytics store is absent would strand every
 * user who needs to rotate a key. The reason is said once, as the informational
 * `AnalyticsUnavailable` panel rather than the red alert a 500 gets, with no
 * retry to offer, and no card claims its key spent nothing (#1270, #2016).
 */
export const AnalyticsUnavailable: Story = {
  render: () => (
    <Harness
      fetchStub={account(
        () => json(KEYS),
        () => json({ error: { message: "analytics not configured" } }, 503),
      )}
    >
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("my laptop")).toBeInTheDocument();
    await expectAnalyticsUnavailable(
      canvasElement,
      en.account.keys.noAnalytics.title,
      "analytics not configured",
    );
    await expect(canvas.getAllByText("usage: unavailable")).toHaveLength(KEYS.length);
    await expect(canvas.queryByText(/no usage in the last 7 days/i)).toBeNull();
    // the keys stay as usable as they were: the card's own buttons are there
    await expect(canvas.getByRole("button", { name: "Rotate key my laptop" })).toBeVisible();
    await expect(
      canvas.getByRole("button", { name: "Rotate key sk-rolter-retired" }),
    ).toBeVisible();
  },
};

/**
 * The same panel at 375px in Russian, where the body is the longest copy on the
 * screen: it wraps inside the viewport, the cards keep saying the figure is
 * unavailable in Russian too, and the whole document fits. The key row used to
 * push it to 393px, because the Russian "create" button sat beside the count on
 * a row that could not wrap (#2352).
 */
export const AnalyticsUnavailableAtMobileInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => (
    <Harness
      fetchStub={account(
        () => json(KEYS),
        () => json({ error: { message: "analytics not configured" } }, 503),
      )}
    >
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("my laptop")).toBeInTheDocument();
    const panel = await expectAnalyticsUnavailable(
      canvasElement,
      ru.account.keys.noAnalytics.title,
      "analytics not configured",
    );
    await expect(canvas.getAllByText(ru.account.keys.card.usageUnavailable)).toHaveLength(
      KEYS.length,
    );
    await expect(panel.getBoundingClientRect().right).toBeLessThanOrEqual(window.innerWidth);
    await expectNoHorizontalOverflow();
  },
};

/**
 * The key row at 375px in Russian: the longest label the button has and the
 * longest count beside it. The row wraps, so the button drops under the count
 * with its whole box on screen and the document does not scroll sideways (#2352).
 */
export const KeyRowAtMobileInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => (
    <Harness fetchStub={loaded}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("my laptop");
    const generate = canvas.getByRole("button", { name: ru.account.keys.generate });
    await expect(generate.getBoundingClientRect().right).toBeLessThanOrEqual(window.innerWidth);
    await expectNoHorizontalOverflow();
  },
};

/**
 * A usage query that fails for any other reason used to fall through to "no
 * usage in the last 7 days" on every card — a failure dressed as a quiet week.
 */
export const UsageFailed: Story = {
  render: () => (
    <Harness
      fetchStub={account(
        () => json(KEYS),
        () => json({ error: { message: "analytics query failed" } }, 500),
      )}
    >
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("my laptop")).toBeInTheDocument();
    await expectLoadError(canvasElement, /your usage/);
    await expect(canvas.getByRole("button", { name: /try again/i })).toBeVisible();
    await expect(canvas.queryByText(/no usage in the last 7 days/i)).toBeNull();
  },
};

/** a control plane that answers a mint with the plaintext key */
const minting = () => account((init) => (init?.method === "POST" ? json(MINTED, 201) : json(KEYS)));

/** Fill the mint sheet in and submit it, returning the reveal dialog. */
async function mintAKey(canvasElement: HTMLElement) {
  await clickWhenEnabled(canvasElement, /generate virtual key/i);
  const form = sheet();
  await userEvent.type(within(form).getByLabelText("Name"), "ci runner");
  await userEvent.click(within(form).getByRole("button", { name: "Mint" }));
  return within(document.body).findByRole("dialog", { name: "Virtual key ready" });
}

export const MintsAKey: Story = {
  render: () => (
    <Harness fetchStub={minting()}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await mintAKey(canvasElement);
    // the plaintext is shown exactly once, right here; losing this dialog means
    // the user never gets the secret they just created
    await waitFor(() => expect(within(dialog).getByText(MINTED.key)).toBeInTheDocument());

    // the step after it (#2217). this key may reach every route, so the
    // request names the gateway's built-in model; the address is the
    // dashboard's own proxy, and the key is referenced, never written out
    const origin = window.location.origin;
    await expect(
      await within(dialog).findByRole("region", { name: /Gateway URL/ }),
    ).toHaveTextContent(`${origin}/gw/v1`);
    const request = within(dialog).getByRole("region", { name: /First request/ });
    await waitFor(() => expect(request).toHaveTextContent(`curl ${origin}/gw/v1/chat/completions`));
    await expect(request).toHaveTextContent(`"model":"fake-llm"`);
    await expect(request).not.toHaveTextContent(MINTED.key);

    // nobody copied the key, so closing asks, and only the confirm closes it
    await userEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    await answerSecretClosePrompt(true);
    await waitFor(() =>
      expect(within(document.body).queryByText(MINTED.key)).not.toBeInTheDocument(),
    );
  },
};

/** A key that reached the clipboard closes without a question. */
export const ACopiedKeyClosesWithoutAsking: Story = {
  beforeEach: stubClipboard(async () => {}),
  render: () => (
    <Harness fetchStub={minting()}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await mintAKey(canvasElement);
    const copy = await within(dialog).findByRole("button", { name: /^Copy: / });
    await userEvent.click(copy);
    await waitFor(() => expect(copy).toHaveAttribute("title", en.common.copied));
    await userEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(within(document.body).queryByRole("dialog")).toBeNull());
  },
};

/**
 * Escape, the scrim and the close button used to drop the key with nothing to
 * bring it back; each asks now, and cancelling keeps the key on screen (#2217).
 */
export const AnUncopiedKeyAsksBeforeClosing: Story = {
  render: () => (
    <Harness fetchStub={minting()}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await mintAKey(canvasElement);
    await userEvent.keyboard("{Escape}");
    await expect(await secretClosePrompt()).toHaveAccessibleDescription(en.common.secret.closeBody);
    await answerSecretClosePrompt(false);
    await expect(within(dialog).getByText(MINTED.key)).toBeVisible();

    await userEvent.click(within(dialog).getByRole("button", { name: en.common.close }));
    await answerSecretClosePrompt(true);
    await waitFor(() => expect(within(document.body).queryByRole("dialog")).toBeNull());
  },
};

/**
 * A plain-http dashboard has no clipboard: the copy says so in a line that
 * stays, and the key stays selectable on screen (#2327).
 */
export const AFailedCopyKeepsTheKeyAndSaysSo: Story = {
  beforeEach: stubClipboard(() => Promise.reject(new Error("denied"))),
  render: () => (
    <Harness fetchStub={minting()}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await mintAKey(canvasElement);
    await userEvent.click(await within(dialog).findByRole("button", { name: /^Copy: / }));
    await expect(await within(dialog).findByRole("alert")).toHaveTextContent(en.common.copyFailed);
    await expect(within(dialog).getByText(MINTED.key)).toBeVisible();
    await expect(window.getSelection()?.toString()).toBe(MINTED.key);
  },
};

/** The reveal, its question and the next step follow the locale. */
export const TheRevealIsInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={minting()}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, new RegExp(ru.account.keys.generate, "i"));
    const form = sheet();
    await userEvent.type(within(form).getByLabelText(ru.keyMint.name), "ci runner");
    await userEvent.click(within(form).getByRole("button", { name: ru.account.keys.mint.save }));
    const dialog = await within(document.body).findByRole("dialog", {
      name: ru.account.keys.revealed.title,
    });
    await expect(
      await within(dialog).findByRole("heading", { name: ru.common.secret.nextStep.title }),
    ).toBeVisible();
    await userEvent.click(
      within(dialog).getByRole("button", { name: ru.account.keys.revealed.done }),
    );
    const prompt = await within(document.body).findByRole("dialog", {
      name: ru.common.secret.closeTitle,
    });
    await expect(within(prompt).getByText(ru.common.secret.closeBody)).toBeVisible();
  },
};

/**
 * A list of keys is N identical pairs of buttons unless each one says which key
 * it belongs to (#1896): a screen reader tabbing through hears the name, and a
 * story can reach the button it means without counting. An unnamed key is named
 * by its prefix, since "unnamed key" would be the same on every such card.
 */
export const EveryCardControlNamesItsKey: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("my laptop");
    for (const name of ["my laptop", "sk-rolter-retired"]) {
      await expect(canvas.getByRole("button", { name: `Rotate key ${name}` })).toBeVisible();
      await expect(canvas.getByRole("button", { name: `Delete key ${name}` })).toBeVisible();
    }
    // no two controls share a name, and no card is left with a bare one
    const names = canvas
      .getAllByRole("button", { name: /^(Rotate|Delete) key / })
      .map((b) => b.getAttribute("aria-label"));
    await expect(new Set(names).size).toBe(names.length);
    await expect(names).toHaveLength(KEYS.length * 2);
    await expect(canvas.queryByRole("button", { name: "Rotate" })).toBeNull();
  },
};

/**
 * The names follow the locale, and still begin with the visible word on the
 * Rotate button, so a voice-control user saying what they see is understood.
 */
export const CardControlNamesInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={loaded}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("my laptop");
    const rotate = canvas.getByRole("button", { name: "Ротировать ключ my laptop" });
    await expect(rotate).toHaveTextContent(ru.account.keys.card.rotate);
    await expect(canvas.getByRole("button", { name: "Удалить ключ my laptop" })).toBeVisible();
    await expect(
      canvas.getByRole("button", { name: "Удалить ключ sk-rolter-retired" }),
    ).toBeVisible();
  },
};

/**
 * Rotation reaches the same reveal dialog by a different path — the card's own
 * mutation rather than the mint sheet — and that second entry point is the one
 * a refactor of the sheet would quietly drop.
 */
export const RotatingAKeyRevealsTheNewSecret: Story = {
  render: () => (
    <Harness fetchStub={account((init) => (init?.method === "POST" ? json(MINTED) : json(KEYS)))}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Rotate key my laptop" }));
    // rotation kills the old secret the instant the new one is issued, so it
    // now asks first, naming the key it is about to invalidate (#1179)
    await confirmDestructive(/my laptop/, /rotate key/i);
    await waitFor(() => expect(within(document.body).getByText(MINTED.key)).toBeInTheDocument());
  },
};

// backing out of the rotation must leave the key working: no POST is issued
export const CancellingARotationLeavesTheKeyAlone: Story = {
  render: () => (
    <Harness fetchStub={rotations.stub}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const rotate = await canvas.findByRole("button", { name: "Rotate key my laptop" });
    await userEvent.click(rotate);
    await cancelConfirmation();
    rotations.expectNotSent("POST", "/me/virtual-keys/vk-1/rotate");

    await userEvent.click(rotate);
    await confirmDestructive(/my laptop/, /rotate key/i);
    await rotations.expectSent("POST", "/me/virtual-keys/vk-1/rotate");
  },
};

/**
 * Deleting a key confirms through the shared `ConfirmDialog` (#1760): the
 * title names the key, a cancel sends nothing and is an abandon, and a confirm
 * sends the DELETE and lands as `save_confirmed`.
 */
let keyDeletes: Recorder;
export const DeletingAKeyIsConfirmedAndReported: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    keyDeletes = recording(
      account((init) => (init?.method === "DELETE" ? json(null, 204) : json(KEYS))),
    );
    return (
      <Harness fetchStub={keyDeletes.stub}>
        <UxScreenProvider screen="api-keys">
          <Account />
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const remove = await canvas.findByRole("button", { name: "Delete key my laptop" });
    await userEvent.click(remove);
    await expect(
      await within(document.body).findByRole("heading", { name: "Delete key my laptop?" }),
    ).toBeVisible();
    await cancelConfirmation();
    keyDeletes.expectNotSent("DELETE", "/me/virtual-keys/");
    const abandon = await expectUxEvent("form_abandon", "account-key-delete");
    await expect(abandon.screen).toBe("api-keys");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "account-key-delete");

    await userEvent.click(remove);
    // the body carries the prefix, which is what a client config shows
    await confirmDestructive(/sk-rolter-laptop/, "Delete key");
    await keyDeletes.expectSent("DELETE", "/me/virtual-keys/vk-1");
    await expectSheetClosed();
    const submit = await expectUxEvent("form_submit", "account-key-delete");
    await expect(submit.outcome).toBe("ok");
    await expectUxEvent("save_confirmed", "account-key-delete");
  },
};

/**
 * An unnamed key is named by its prefix in the title, and a refused delete
 * keeps the dialog open over the control plane's reason, reported beside the
 * press even though the stub answers in the same tick (#1761).
 */
export const DeletingAKeyRefused: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness
      fetchStub={account((init) =>
        init?.method === "DELETE"
          ? json({ error: { message: "the key belongs to a project you left" } }, 403)
          : json(KEYS),
      )}
    >
      <UxScreenProvider screen="api-keys">
        <Account />
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the unnamed key is reached by its prefix, the same name its dialog carries
    await userEvent.click(
      await canvas.findByRole("button", { name: "Delete key sk-rolter-retired" }),
    );
    await expect(
      await within(document.body).findByRole("heading", { name: "Delete key sk-rolter-retired?" }),
    ).toBeVisible();
    // anchored: the title carries the prefix too, the body's mono span alone
    await confirmDestructive(/^sk-rolter-retired…$/, "Delete key");
    await waitFor(() =>
      expect(
        uxEvents()
          .filter((e) => e.action === "form_submit" && e.target === "account-key-delete")
          .map((e) => e.outcome),
      ).toEqual(["ok", "error"]),
    );
    expectNoUxEvent("save_confirmed", "account-key-delete");
    const dialog = within(await within(document.body).findByRole("dialog"));
    await expect(await dialog.findByRole("alert")).toHaveTextContent("a project you left");
  },
};

/** The dirty guard from #868: an untouched draft closes without a confirm. */
export const AnUntouchedMintFormClosesWithoutPrompting: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /generate virtual key/i);
    await expectClosesWithoutPrompting();
  },
};

export const AnEditedMintFormPromptsBeforeDiscarding: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /generate virtual key/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Name"), "half typed");

    await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
    // declining keeps the sheet, and the typing, alive
    await answerDiscardPrompt(false);
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
    await expect(within(form).getByLabelText("Name")).toHaveValue("half typed");
  },
};

/**
 * The mint sheet's whole point after #945 is that the two insecure choices are
 * no longer the ones you get by doing nothing: the key must be named, and it
 * expires in 30 days unless you say otherwise.
 */
export const MintRequiresANameAndDefaultsToAFiniteLife: Story = {
  render: () => (
    <Harness fetchStub={account(() => json(KEYS))}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /generate virtual key/i);
    const form = sheet();
    // nothing typed: minting is refused before a round trip is spent
    await expect(within(form).getByRole("button", { name: "Mint" })).toBeDisabled();
    // whitespace is not a name either
    await userEvent.type(within(form).getByLabelText("Name"), "   ");
    await expect(within(form).getByRole("button", { name: "Mint" })).toBeDisabled();
    await userEvent.type(within(form).getByLabelText("Name"), "ci runner");
    await expect(within(form).getByRole("button", { name: "Mint" })).toBeEnabled();

    // the default expiry is finite, and the reach panel says so in a date
    await expect(within(form).getByLabelText("Expires")).toHaveValue("In 30 days");
    await expect(
      within(form).getByText(/every model this project can route to/i),
    ).toBeInTheDocument();
    await expect(within(form).getByText(/^Until /)).toBeInTheDocument();
  },
};

/**
 * "Never expires" stays available — it is a legitimate choice for a key an
 * operator has other controls over — but it has to be picked, and picking it
 * says what it costs.
 */
export const NeverExpiringIsADeliberateChoice: Story = {
  render: () => (
    <Harness
      fetchStub={account((init) => {
        if (init?.method !== "POST") return json(KEYS);
        // the body is what the assertion is really about: no TTL at all,
        // rather than a zero or an empty string the server would reject
        const body = JSON.parse(String(init.body));
        if (body.expires_in_days !== undefined)
          return json({ error: { message: "sent a ttl" } }, 400);
        if (!body.name) return json({ error: { message: "sent no name" } }, 400);
        return json(MINTED, 201);
      })}
    >
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /generate virtual key/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Name"), "build box");
    await pickOption(within(form).getByLabelText("Expires"), "Never expires");
    await expect(within(form).getByText(/until someone revokes it/i)).toBeInTheDocument();
    await expect(within(form).getByText(/forever, until revoked/i)).toBeInTheDocument();
    await userEvent.click(within(form).getByRole("button", { name: "Mint" }));
    await waitFor(() => expect(within(document.body).getByText(MINTED.key)).toBeInTheDocument());
  },
};

/**
 * A server-side rejection has to land on the sheet rather than vanishing — the
 * name rule is enforced in two places and the operator must see which one spoke.
 */
export const MintRejectionIsShownOnTheSheet: Story = {
  render: () => (
    <Harness
      fetchStub={account((init) =>
        init?.method === "POST"
          ? json({ error: { message: "virtual key name is required" } }, 400)
          : json(KEYS),
      )}
    >
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /generate virtual key/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Name"), "rejected");
    await userEvent.click(within(form).getByRole("button", { name: "Mint" }));
    // the lead is the dashboard's own, translated line; the control plane's
    // words follow it as the detail rather than standing in for it
    const alert = await within(form).findByRole("alert");
    await expect(within(alert).getByText("Could not create the key")).toBeInTheDocument();
    await expect(within(alert).getByText("virtual key name is required")).toBeInTheDocument();
    // and the sheet stays open, so the operator can fix it in place
    await expect(within(form).getByLabelText("Name")).toBeInTheDocument();
  },
};

/**
 * The same refusal in Russian. The server answers in English whatever the
 * locale and there is no table to translate it with, so the lead is Russian and
 * the detail stays as the server wrote it, instead of the whole line being a
 * lowercase English sentence in a Russian sheet.
 */
export const MintRejectionLeadsInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness
      fetchStub={account((init) =>
        init?.method === "POST"
          ? json({ error: { message: "virtual key name is required" } }, 400)
          : json(KEYS),
      )}
    >
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, ru.account.keys.generate);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText(ru.keyMint.name), "rejected");
    await userEvent.click(within(form).getByRole("button", { name: ru.account.keys.mint.save }));
    const alert = await within(form).findByRole("alert");
    await expect(within(alert).getByText(ru.account.keys.mint.failed)).toBeInTheDocument();
    await expect(within(alert).getByText("virtual key name is required")).toBeInTheDocument();
    await expect(within(form).getByLabelText(ru.keyMint.name)).toBeInTheDocument();
  },
};

/**
 * Sets the control plane's injected documentation base for one story and puts
 * it back afterwards, so the two states below cannot leak into each other.
 */
function withDocsBase(base: string | undefined) {
  return () => {
    const before = window.__ROLTER_CONFIG__;
    window.__ROLTER_CONFIG__ = base === undefined ? {} : { ...before, docsBaseUrl: base };
    return () => {
      window.__ROLTER_CONFIG__ = before;
    };
  };
}

/** The explainer carries a link into `security/which-key` when docs exist (#1164). */
export const ExplainerLinksToTheDocs: Story = {
  beforeEach: withDocsBase("https://docs.example.com"),
  render: () => (
    <Harness fetchStub={loaded}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const link = await canvas.findByRole("link", { name: /Which key do I need/ });
    await expect(link).toHaveAttribute("href", "https://docs.example.com/security/which-key");
  },
};

/**
 * The air-gapped default: no documentation host, so the explainer stands alone
 * and there is no link to click into nothing.
 */
export const ExplainerHasNoLinkWithoutADocsHost: Story = {
  beforeEach: withDocsBase(undefined),
  render: () => (
    <Harness fetchStub={loaded}>
      <Account />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the explainer itself is still there — only the link is suppressed
    await canvas.findByText(/These are rolter virtual keys/);
    await expect(canvas.queryByRole("link", { name: /Which key do I need/ })).toBeNull();
  },
};
