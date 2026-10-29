import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Keys from "./Keys";
import {
  Harness,
  answerSecretClosePrompt,
  cancelConfirmation,
  clickWhenEnabled,
  confirmDestructive,
  expectClosesWithoutPrompting,
  expectNoUxEvent,
  expectSheetClosed,
  expectListTable,
  expectSkeleton,
  expectUxEvent,
  json,
  openOptions,
  pending,
  pickOption,
  recordUxEvents,
  recording,
  scoped,
  secretClosePrompt,
  StaleSession,
  sheet,
  stubClipboard,
  type FetchStub,
  type Recorder,
  answerDiscardPrompt,
  expectEmptyState,
  expectNoFalseEmpty,
  uxEvents,
  withDocsBase,
} from "./story-harness";
import type {
  BudgetRow,
  RateLimitRow,
  BusinessUnitRow,
  CustomerRow,
  ProviderRow,
  RouteRow,
  VirtualKeyRow,
} from "@/lib/api";
import { formattersFor } from "@/lib/i18n/format";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import {
  atMobile,
  atSplit,
  atTablet,
  expectInViewport,
  expectNoHorizontalOverflow,
  expectNotTruncated,
  phoneFits,
} from "@/lib/story-viewport";
import { UxScreenProvider } from "@/lib/ux-react";

const UNIT_ID = "aaaaaaaa-0000-0000-0000-000000000001";
const CUSTOMER_ID = "bbbbbbbb-0000-0000-0000-000000000001";

const UNITS: BusinessUnitRow[] = [
  {
    id: UNIT_ID,
    org_id: "org-1",
    name: "Platform Engineering",
    slug: "platform-engineering",
    retired_at: null,
    created_at: "2026-01-05T10:00:00Z",
  },
];

const CUSTOMERS: CustomerRow[] = [
  {
    id: CUSTOMER_ID,
    org_id: "org-1",
    business_unit_id: UNIT_ID,
    name: "Acme Corp",
    slug: "acme-corp",
    retired_at: null,
    created_at: "2026-02-05T10:00:00Z",
  },
  // owned by no unit, so it pairs with whichever unit is chosen
  {
    id: "bbbbbbbb-0000-0000-0000-000000000002",
    org_id: "org-1",
    business_unit_id: null,
    name: "Globex",
    slug: "globex",
    retired_at: null,
    created_at: "2026-04-05T10:00:00Z",
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
  {
    id: "prov-2",
    org_id: "org-1",
    name: "Anthropic",
    slug: "anthropic",
    kind: "anthropic",
    api_base: "https://api.anthropic.com",
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

/** the lookups the mint and attribution editors read */
const lookups = (url: string): Response | null => {
  if (url.includes("/business-units")) return json(UNITS);
  if (url.includes("/customers")) return json(CUSTOMERS);
  if (url.includes("/providers")) return json(PROVIDERS);
  if (url.includes("/routes")) return json(ROUTES);
  return null;
};

/**
 * A stub that answers the lookups and every mutation, and records what was
 * sent. The recorder is held in a module-scope slot the story's `play` reads:
 * `render` always runs first, and asserting on the request body is the only way
 * to tell "a PUT left" from "the right PUT left".
 */
let sent: Recorder;
const recorded = (row: VirtualKeyRow): FetchStub => {
  sent = recording(
    scoped(async (input, init) => {
      if (init?.method === "POST") return json({ ...row, key: "sk-rolter-once" }, 201);
      if (init?.method === "PUT") return json(row);
      return lookups(String(input)) ?? json(KEYS);
    }),
  );
  return sent.stub;
};

const KEYS: VirtualKeyRow[] = [
  {
    id: "vk-1",
    project_id: "project-1",
    key_prefix: "sk-rolter-backend",
    name: "backend service",
    models: ["gpt-4o", "claude-sonnet"],
    providers: ["openai"],
    created_by: null,
    business_unit_id: UNIT_ID,
    customer_id: null,
    disabled: false,
    expires_at: null,
    cache_enabled: null,
    created_at: "2026-07-01T00:00:00Z",
  },
  {
    id: "vk-2",
    project_id: "project-1",
    key_prefix: "sk-rolter-laptop",
    name: "revoked laptop",
    models: [],
    providers: [],
    created_by: null,
    business_unit_id: null,
    customer_id: null,
    disabled: true,
    expires_at: "2026-12-31T00:00:00Z",
    cache_enabled: false,
    created_at: "2026-06-01T00:00:00Z",
  },
];

/** a key the Playground minted for itself, which no operator created (#944) */
const PLAYGROUND_KEY: VirtualKeyRow = {
  id: "vk-3",
  project_id: "project-1",
  key_prefix: "sk-rolter-play",
  name: "Playground",
  models: ["gpt-4o"],
  providers: [],
  created_by: "user-1",
  business_unit_id: null,
  customer_id: null,
  disabled: false,
  expires_at: "2026-07-01T00:30:00Z",
  cache_enabled: null,
  purpose: "playground",
  created_at: "2026-07-01T00:00:00Z",
};

const withKeys = (keys: VirtualKeyRow[], status = 200): FetchStub =>
  scoped(
    async (input) =>
      lookups(String(input)) ??
      json(status === 200 ? keys : { error: { message: "forbidden" } }, status),
  );

const meta = {
  title: "Screens/Keys",
  component: Keys,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Keys>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={withKeys(KEYS)}>
      <Keys />
    </Harness>
  ),
  // the row's own `style` (opacity for a disabled key) must not replace the
  // grid template: it once did, and every cell stacked into one column
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const name = await canvas.findByText("backend service");
    const row = name.closest('[style*="grid-template-columns"]');
    await expect(row).not.toBeNull();
    await expect(getComputedStyle(row as HTMLElement).gridTemplateColumns).not.toBe("none");
    await expectListTable(canvasElement, "Virtual Keys");
  },
};

/**
 * The Playground's own key says what it is (#944).
 *
 * Every other row on this screen was created by somebody who chose its name,
 * its reach and its lifetime. This one was issued by a screen, scoped by the
 * server and expires by itself, and a reader who cannot tell the difference
 * reads a half-hour expiry as a mistake somebody made.
 */
export const PlaygroundKeyIsLabelled: Story = {
  render: () => (
    <Harness fetchStub={withKeys([...KEYS, PLAYGROUND_KEY])}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // by its explanation, not its text: the control plane names the key itself
    // "Playground" too, so matching on the word alone finds the name as well
    const badge = await canvas.findByTitle(/expires on its own/);
    await expect(badge).toHaveTextContent("Playground");

    // and only that row carries it: a badge on every key would say nothing
    await expect(canvas.getAllByTitle(/expires on its own/)).toHaveLength(1);
    // the expiry the badge is explaining is on the row itself, in the house
    // short date rather than the raw timestamp
    const row = badge.closest('[style*="grid-template-columns"]');
    const expires = formattersFor("en").date(PLAYGROUND_KEY.expires_at as string);
    await expect(within(row as HTMLElement).getByText(`expires ${expires}`)).toBeVisible();
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No virtual keys yet/);
  },
};

export const Empty: Story = {
  render: () => (
    <Harness fetchStub={withKeys([])}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No virtual keys yet/, /Add virtual key/);
  },
};

export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={withKeys([], 403)}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // a 403 is a permission problem, not a data problem (#962): the screen
    // must say so rather than blame the list it could not read
    await expect(
      await canvas.findByText(/do not have access to virtual keys/i),
    ).toBeInTheDocument();
    // and must not offer a retry that cannot possibly succeed
    await expect(canvas.queryByRole("button", { name: /try again/i })).toBeNull();
    await expectNoFalseEmpty(canvasElement, /No virtual keys yet/);
    // an export of a list that never loaded is a header line with no rows,
    // which reads as a project with no keys (#2056)
    await expect(canvas.getByRole("button", { name: "Export CSV" })).toBeDisabled();
  },
};

/** the file the export button hands over, read back from the Blob it made */
const exported: { name: string; text: Promise<string> }[] = [];

/** stands in for the download: records the file name and the Blob's text */
function captureDownloads() {
  exported.length = 0;
  const blobs = new Map<string, Blob>();
  const create = URL.createObjectURL;
  const revoke = URL.revokeObjectURL;
  const click = HTMLAnchorElement.prototype.click;
  URL.createObjectURL = (blob) => {
    const url = `blob:story-${blobs.size}`;
    blobs.set(url, blob as Blob);
    return url;
  };
  URL.revokeObjectURL = () => {};
  HTMLAnchorElement.prototype.click = function () {
    exported.push({ name: this.download, text: (blobs.get(this.href) as Blob).text() });
  };
  return () => {
    URL.createObjectURL = create;
    URL.revokeObjectURL = revoke;
    HTMLAnchorElement.prototype.click = click;
  };
}

const AWKWARD: VirtualKeyRow[] = [
  { ...KEYS[0], id: "vk-a", name: "billing, eu", key_prefix: "sk-rolter-aaa" },
  { ...KEYS[0], id: "vk-b", name: '=HYPERLINK("http://x")', key_prefix: "sk-rolter-bbb" },
  { ...KEYS[0], id: "vk-c", name: "plain", key_prefix: "sk-rolter-ccc" },
];

/**
 * The export writes a real CSV: a name with a comma stays one column, a quote is
 * doubled, and a name a spreadsheet would run as a formula is written as text.
 * With a search active it exports only those rows, and the button and the file
 * name say so (#2057).
 */
export const ExportQuotesAndNamesItsRows: Story = {
  beforeEach: captureDownloads,
  render: () => (
    <Harness fetchStub={withKeys(AWKWARD)}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("plain");
    await userEvent.click(canvas.getByRole("button", { name: en.pages.virtualKeys.exportCsv }));
    await expect(exported[0]?.name).toBe("virtual-keys.csv");
    const lines = (await exported[0].text).trimEnd().split("\r\n");
    await expect(lines).toHaveLength(4);
    await expect(lines[1]).toMatch(/^"billing, eu",sk-rolter-aaa,/);
    await expect(lines[2]).toMatch(/^"'=HYPERLINK\(""http:\/\/x""\)",sk-rolter-bbb,/);

    await userEvent.type(
      canvas.getByPlaceholderText(en.pages.virtualKeys.searchPlaceholder),
      "plain",
    );
    await waitFor(() => expect(canvas.queryByText("billing, eu")).toBeNull());
    await userEvent.click(
      canvas.getByRole("button", { name: en.pages.virtualKeys.exportCsvFiltered }),
    );
    await expect(exported[1]?.name).toBe("virtual-keys-filtered.csv");
    await expect((await exported[1].text).trimEnd().split("\r\n")).toHaveLength(2);
  },
};

const MINTED_KEY = "sk-rolter-plaintext-shown-once";

/** a control plane that answers a mint with the plaintext key */
const minting = () =>
  scoped(async (input, init) =>
    init?.method === "POST"
      ? json({ ...KEYS[0], key: MINTED_KEY }, 201)
      : (lookups(String(input)) ?? json(KEYS)),
  );

/** Fill the add sheet in and create the key, returning the reveal dialog. */
async function mintAKey(canvasElement: HTMLElement) {
  await clickWhenEnabled(canvasElement, /add virtual key/i);
  const form = sheet();
  await userEvent.type(within(form).getByLabelText("Name"), "ci runner");
  await userEvent.click(within(form).getByRole("button", { name: "Create" }));
  return within(document.body).findByRole("dialog", { name: "Key created" });
}

export const CreatesAKey: Story = {
  render: () => (
    <Harness fetchStub={minting()}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await mintAKey(canvasElement);
    // the plaintext key is shown exactly once, right after creation — losing
    // that dialog means the caller never gets their secret
    await waitFor(() => expect(within(dialog).getByText(MINTED_KEY)).toBeInTheDocument());

    // and the step after it. with no public base URL known there is no
    // address to hand out: the /gw proxy needs a dashboard session an external
    // client lacks, so the step asks for a base URL instead (#2486)
    await expect(await within(dialog).findByRole("note")).toHaveTextContent(
      "Save your gateway base URL under Client Settings",
    );
    await expect(dialog.textContent ?? "").not.toContain("/gw/");
  },
};

/**
 * A superadmin who saved a public base URL on Client Settings hands out that
 * address, not the dashboard's proxy, in the next step (#2218).
 */
export const TheNextStepUsesTheSavedGatewayUrl: Story = {
  render: () => (
    <Harness
      role="superadmin"
      fetchStub={scoped(async (input, init) => {
        if (init?.method === "POST") return json({ ...KEYS[0], key: MINTED_KEY }, 201);
        if (String(input).includes("/client-settings")) {
          return json({ public_base_url: "https://llm.example.com" });
        }
        return lookups(String(input)) ?? json(KEYS);
      })}
    >
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await mintAKey(canvasElement);
    await waitFor(() =>
      expect(within(dialog).getByRole("region", { name: /Gateway URL/ })).toHaveTextContent(
        "https://llm.example.com/v1",
      ),
    );
    await expect(within(dialog).getByRole("region", { name: /First request/ })).toHaveTextContent(
      "curl https://llm.example.com/v1/chat/completions",
    );
    await expect(dialog.textContent ?? "").not.toContain("/gw/");
    // the key is referenced, never written out, and names the first model it may reach
    const request = within(dialog).getByRole("region", { name: /First request/ });
    await expect(request).toHaveTextContent(`"model":"gpt-4o"`);
    await expect(request).toHaveTextContent("$ROLTER_API_KEY");
    await expect(request).not.toHaveTextContent(MINTED_KEY);
  },
};

/**
 * An org admin cannot read client settings either, yet the saved public base
 * URL reaches the next step through `/auth/me` (#2512).
 */
export const TheNextStepUsesTheSavedUrlForAnAdmin: Story = {
  render: () => (
    <Harness
      role="admin"
      fetchStub={scoped(async (input, init) => {
        if (init?.method === "POST") return json({ ...KEYS[0], key: MINTED_KEY }, 201);
        if (String(input).includes("/auth/me")) {
          return json({
            user: { id: "u1", email: "anya@acme.co", is_superadmin: false },
            memberships: [],
            display_name_managed: false,
            gateway_base_url: "https://llm.example.com",
          });
        }
        return lookups(String(input)) ?? json(KEYS);
      })}
    >
      <StaleSession>
        <Keys />
      </StaleSession>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await mintAKey(canvasElement);
    await waitFor(() =>
      expect(within(dialog).getByRole("region", { name: /Gateway URL/ })).toHaveTextContent(
        "https://llm.example.com/v1",
      ),
    );
    await expect(dialog.textContent ?? "").not.toContain("/gw/");
  },
};

/** A key that reached the clipboard closes without a question. */
export const ACopiedKeyClosesWithoutAsking: Story = {
  beforeEach: stubClipboard(async () => {}),
  render: () => (
    <Harness fetchStub={minting()}>
      <Keys />
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
 * Escape used to close the reveal with the key uncopied and nothing to bring
 * it back (#2217). It asks now; cancelling keeps the key on screen, and the
 * explicit confirm closes.
 */
export const AnUncopiedKeyAsksBeforeClosing: Story = {
  render: () => (
    <Harness fetchStub={minting()}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await mintAKey(canvasElement);
    await userEvent.keyboard("{Escape}");
    const prompt = await secretClosePrompt();
    await expect(prompt).toHaveAccessibleDescription(en.common.secret.closeBody);
    await answerSecretClosePrompt(false);
    await expect(within(dialog).getByText(MINTED_KEY)).toBeVisible();

    await userEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    await answerSecretClosePrompt(true);
    await waitFor(() => expect(within(document.body).queryByRole("dialog")).toBeNull());
  },
};

/**
 * On a plain-http dashboard the clipboard is withheld. The copy says so in a
 * line that stays, and the key stays on screen to be copied by hand (#2327).
 */
export const AFailedCopyKeepsTheKeyAndSaysSo: Story = {
  beforeEach: stubClipboard(() => Promise.reject(new Error("denied"))),
  render: () => (
    <Harness fetchStub={minting()}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await mintAKey(canvasElement);
    await userEvent.click(await within(dialog).findByRole("button", { name: /^Copy: / }));
    const alert = await within(dialog).findByRole("alert");
    await expect(alert).toHaveTextContent(en.common.copyFailed);
    await expect(within(dialog).getByText(MINTED_KEY)).toBeVisible();
    await expect(window.getSelection()?.toString()).toBe(MINTED_KEY);
  },
};

/** 375 px and Russian: the reveal, its message and the snippet all fit the window. */
export const TheRevealFitsAPhoneInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  beforeEach: stubClipboard(() => Promise.reject(new Error("denied"))),
  render: () => (
    // a saved base URL, so the step shows the snippet whose width is under test
    <Harness
      role="superadmin"
      fetchStub={scoped(async (input, init) => {
        if (init?.method === "POST") return json({ ...KEYS[0], key: MINTED_KEY }, 201);
        if (String(input).includes("/client-settings")) {
          return json({ public_base_url: "https://llm.example.com" });
        }
        return lookups(String(input)) ?? json(KEYS);
      })}
    >
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, new RegExp(ru.pages.virtualKeys.add, "i"));
    const form = within(sheet());
    await userEvent.type(form.getByLabelText(ru.keyMint.name), "ci runner");
    await userEvent.click(form.getByRole("button", { name: ru.common.create }));
    const dialog = await within(document.body).findByRole("dialog", {
      name: ru.pages.virtualKeys.createdTitle,
    });
    await userEvent.click(await within(dialog).findByRole("button", { name: /^Копировать: / }));
    await within(dialog).findByRole("alert");
    await within(dialog).findByRole("region", {
      name: new RegExp(ru.common.secret.nextStep.request),
    });
    await expectInViewport(dialog);
    await expectInViewport(within(dialog).getByRole("button", { name: ru.common.secret.select }));
    // the snippet scrolls inside its own region instead of widening the panel
    const panel = dialog.getBoundingClientRect();
    for (const region of within(dialog).getAllByRole("region")) {
      await expect(region.getBoundingClientRect().right).toBeLessThanOrEqual(panel.right);
    }
  },
};

/**
 * The server refuses the mint (#1607).
 *
 * Losing the draft here costs the operator everything they typed, so the story
 * asserts the sheet is still open with the name intact — and that the refusal
 * is on screen rather than swallowed, which is the half that can rot silently.
 */
export const CreateRejectedByTheServer: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        init?.method === "POST"
          ? json({ error: { message: "the project has reached its virtual-key quota" } }, 409)
          : (lookups(String(input)) ?? json(KEYS)),
      )}
    >
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add virtual key/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Name"), "ci runner");
    await userEvent.click(within(form).getByRole("button", { name: "Create" }));
    await waitFor(() =>
      expect(within(form).getByText(/reached its virtual-key quota/)).toBeVisible(),
    );
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
    await expect(within(form).getByLabelText("Name")).toHaveValue("ci runner");
  },
};

/**
 * The dirty guard #868 introduced. A blank draft is *not* dirty, so closing it
 * must not prompt — a confirm on an untouched form trains people to click
 * through the one that matters.
 */
export const ClosesACleanDraftWithoutPrompting: Story = {
  render: () => (
    <Harness fetchStub={withKeys(KEYS)}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add virtual key/i);
    await expectClosesWithoutPrompting();
  },
};

export const KeepsADirtyDraftWhenDiscardIsDeclined: Story = {
  render: () => (
    <Harness fetchStub={withKeys(KEYS)}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add virtual key/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Name"), "half typed");

    await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
    // declining the discard keeps the sheet — and the typing — alive
    await answerDiscardPrompt(false);
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
    await expect(within(form).getByLabelText("Name")).toHaveValue("half typed");

    await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
    await answerDiscardPrompt(true);
    await expectSheetClosed();
  },
};

/**
 * #945 applies to the admin screen too. A rule enforced only on the
 * self-service panel is a rule with a way around it.
 */
export const AdminCreateAlsoRequiresANameAndAnExpiry: Story = {
  render: () => (
    <Harness fetchStub={withKeys(KEYS)}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add virtual key/i);
    const form = sheet();
    await expect(within(form).getByRole("button", { name: "Create" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await userEvent.type(within(form).getByLabelText("Name"), "backend service");
    await expect(within(form).getByRole("button", { name: "Create" })).toBeEnabled();
    // the finite default is the same one the self-service sheet offers
    await expect(within(form).getByLabelText("Expires")).toHaveValue("In 30 days");
    await expect(within(form).getByText(/^Until /)).toBeInTheDocument();
  },
};

/**
 * #1182: the row said `expires 05.10.2026` while the sheet previewed `Until
 * 10/5/2026` — two formats for one date, on one screen. Both go through
 * `useFormat().date` now, so they cannot disagree.
 */
export const RowExpiryUsesTheSameDateFormatAsTheMintPreview: Story = {
  render: () => (
    <Harness fetchStub={withKeys(KEYS)}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const fmt = formattersFor("en");
    await expect(
      await canvas.findByText(`expires ${fmt.date("2026-12-31T00:00:00Z")}`),
    ).toBeInTheDocument();

    await clickWhenEnabled(canvasElement, /add virtual key/i);
    const form = sheet();
    // the default is 30 days out; the preview formats it exactly as the row does
    const until = within(form).getByText(/^Until /);
    const previewed = (until.textContent ?? "").replace(/^Until /, "");
    await expect(previewed).toBe(fmt.date(new Date(Date.now() + 30 * 86_400_000)));
  },
};

/**
 * The key list scrolls inside its border rather than dragging the page with
 * it. #1193 added a seventh column (attribution), so the row is wider than the
 * viewport at 375px by design — the assertion is that the *page* does not
 * scroll, not that the table fits.
 */
export const Mobile: Story = {
  ...atMobile,
  render: () => (
    <Harness fetchStub={withKeys(KEYS)}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("backend service");
    await expectNoHorizontalOverflow();
  },
};

export const Tablet: Story = {
  ...atTablet,
  render: () => (
    <Harness fetchStub={withKeys(KEYS)}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("backend service");
    await expectNoHorizontalOverflow();
  },
};

/**
 * #1193: the row is where an operator sees whether a key's spend has a home.
 * The two dimensions are badged separately because they answer different
 * questions — which part of us spent it, and which customer it was for.
 */
export const RowsShowWhereSpendIsCharged: Story = {
  render: () => (
    <Harness fetchStub={withKeys(KEYS)}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("Platform Engineering")).toBeVisible();
    // the unattributed key is not badged "Unattributed": attribution is
    // optional, and a chip on every row would read as a warning about it
    await expect(canvas.queryByText("Unattributed")).toBeNull();
  },
};

/**
 * The PUT the whole feature turns on. Asserting the body rather than that a
 * request left is the point: an editor that sends the wrong ids satisfies every
 * url-only assertion while charging the spend to the wrong unit.
 */
export const EditingAKeySendsTheAttributionPut: Story = {
  render: () => (
    <Harness fetchStub={recorded(KEYS[1])}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("revoked laptop");
    await userEvent.click(await canvas.findByRole("button", { name: /Edit key revoked laptop/i }));
    const form = sheet();
    // save stays disabled until something actually moves — re-saving an
    // untouched sheet must not write or audit anything
    await expect(within(form).getByRole("button", { name: "Save" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await pickOption(within(form).getByLabelText("Business unit"), "Platform Engineering");
    await pickOption(within(form).getByLabelText("Customer"), "Acme Corp");
    await userEvent.click(within(form).getByRole("button", { name: "Save" }));

    await expect(await sent.expectSentBody("PUT", "/attribution")).toEqual({
      business_unit_id: UNIT_ID,
      customer_id: CUSTOMER_ID,
    });
    // the allow-list never moved, so its endpoint is left alone
    sent.expectNotSent("PUT", "/providers");
  },
};

/**
 * Clearing an attribution has to be expressible. The server reads an omitted
 * field as "leave unchanged", so the editor always sends both dimensions and
 * `null` is how an operator says "charge this to nobody".
 */
export const ClearingAnAttributionSendsNull: Story = {
  render: () => (
    <Harness fetchStub={recorded(KEYS[0])}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("backend service");
    await userEvent.click(await canvas.findByRole("button", { name: /Edit key backend service/i }));
    const form = sheet();
    // seeded from the row, so the editor opens on the truth rather than blank
    // a combobox reads as the option's label; the id is what goes on the wire
    await expect(within(form).getByLabelText("Business unit")).toHaveValue("Platform Engineering");
    await pickOption(within(form).getByLabelText("Business unit"), "Unattributed");
    await userEvent.click(within(form).getByRole("button", { name: "Save" }));

    await expect(await sent.expectSentBody("PUT", "/attribution")).toEqual({
      business_unit_id: null,
      customer_id: null,
    });
  },
};

/** the provider allow-list goes to its own endpoint, and only when it moved */
export const NarrowingTheProviderAllowListSendsItsOwnPut: Story = {
  render: () => (
    <Harness fetchStub={recorded(KEYS[1])}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("revoked laptop");
    await userEvent.click(await canvas.findByRole("button", { name: /Edit key revoked laptop/i }));
    const form = sheet();
    // the provider list is the sheet's own request, so the checkbox arrives
    // after the dialog does (#1689)
    await userEvent.click(await within(form).findByRole("checkbox", { name: "Anthropic" }));
    await userEvent.click(within(form).getByRole("button", { name: "Save" }));

    await expect(await sent.expectSentBody("PUT", "/providers")).toEqual({
      providers: ["anthropic"],
    });
    sent.expectNotSent("PUT", "/attribution");
  },
};

/**
 * The create sheet carries the same controls. `POST /virtual-keys` takes the
 * allow-list but not the attribution, so a key created with one is pointed at
 * it by a follow-up PUT rather than losing the choice silently.
 */
export const CreatingWithAnAttributionFollowsUpWithThePut: Story = {
  render: () => (
    <Harness fetchStub={recorded(KEYS[0])}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add virtual key/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Name"), "ci runner");
    await userEvent.click(within(form).getByRole("checkbox", { name: "OpenAI" }));
    await pickOption(within(form).getByLabelText("Business unit"), "Platform Engineering");
    await userEvent.click(within(form).getByRole("button", { name: "Create" }));

    const posted = (await sent.expectSentBody("POST", "/virtual-keys")) as {
      providers: string[];
    };
    await expect(posted.providers).toEqual(["openai"]);
    await expect(await sent.expectSentBody("PUT", "/attribution")).toEqual({
      business_unit_id: UNIT_ID,
      customer_id: null,
    });
  },
};

/**
 * The server rejects a customer already owned by a different business unit, so
 * the editor only ever offers pairings it will accept. A customer that belongs
 * to no unit fits under any of them.
 */
export const OnlyCustomersThatFitTheChosenUnitAreOffered: Story = {
  render: () => (
    <Harness fetchStub={withKeys(KEYS)}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("revoked laptop");
    await userEvent.click(await canvas.findByRole("button", { name: /Edit key revoked laptop/i }));
    const form = sheet();
    const customer = within(form).getByLabelText("Customer");
    // with no unit chosen, every customer is reachable
    await expect(
      within(await openOptions(customer)).getByRole("option", { name: "Acme Corp" }),
    ).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    await pickOption(within(form).getByLabelText("Business unit"), "Platform Engineering");
    // Acme belongs to that unit and Globex to none, so both still fit
    const offered = within(await openOptions(customer));
    await expect(offered.getByRole("option", { name: "Acme Corp" })).toBeInTheDocument();
    await expect(offered.getByRole("option", { name: "Globex" })).toBeInTheDocument();
  },
};

/**
 * The model allow-list is the project's routes, ticked off rather than typed
 * (#1345). The wire format is unchanged — the same array of addresses — but a
 * typo can no longer produce an allow-list that matches nothing.
 */
export const TheAllowListOffersTheProjectsRoutes: Story = {
  render: () => (
    <Harness fetchStub={recorded(KEYS[0])}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add virtual key/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Name"), "ci runner");
    await waitFor(() =>
      expect(within(form).getByRole("checkbox", { name: "gpt-4o" })).toBeVisible(),
    );
    await userEvent.click(within(form).getByRole("checkbox", { name: "gpt-4o" }));
    // an address no route serves is still allowed through free-form entry
    await userEvent.type(within(form).getByLabelText(/model allow-list/i), "legacy-davinci");
    await userEvent.click(within(form).getByRole("button", { name: "Add" }));
    await userEvent.click(within(form).getByRole("button", { name: "Create" }));

    const posted = (await sent.expectSentBody("POST", "/virtual-keys")) as {
      models: string[];
    };
    await expect(posted.models).toEqual(["gpt-4o", "legacy-davinci"]);
  },
};

/** The explainer carries a link into `security/which-key` when docs exist (#1164). */
export const ExplainerLinksToTheDocs: Story = {
  beforeEach: withDocsBase("https://docs.example.com"),
  render: () => (
    <Harness fetchStub={withKeys(KEYS)}>
      <Keys />
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
    <Harness fetchStub={withKeys(KEYS)}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the explainer itself is still there — only the link is suppressed
    await canvas.findByText(/Virtual keys are the credential clients send/);
    await expect(canvas.queryByRole("link", { name: /Which key do I need/ })).toBeNull();
  },
};

/**
 * Deleting a key confirms through the shared `ConfirmDialog` (#1738): the
 * dialog names the key, a cancel sends nothing, and both outcomes reach the UX
 * stream under the `virtual-key-delete` key the hand-rolled dialog emitted, so
 * the series reads on across the swap.
 */
let deletes: Recorder;
export const DeleteIsConfirmedAndReported: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    deletes = recording(
      scoped(async (input, init) =>
        init?.method === "DELETE"
          ? new Response(null, { status: 204 })
          : (lookups(String(input)) ?? json(KEYS)),
      ),
    );
    return (
      <Harness fetchStub={deletes.stub}>
        <UxScreenProvider screen="keys">
          <Keys />
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Delete key backend service");
    await expect(
      await within(document.body).findByRole("heading", {
        name: "Delete virtual key backend service?",
      }),
    ).toBeInTheDocument();
    await cancelConfirmation();
    deletes.expectNotSent("DELETE", "/virtual-keys/");
    const abandon = await expectUxEvent("form_abandon", "virtual-key-delete");
    await expect(abandon.screen).toBe("keys");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "virtual-key-delete");

    await clickWhenEnabled(canvasElement, "Delete key backend service");
    // the body carries the prefix, which is what a client actually sends
    await confirmDestructive(/sk-rolter-backend/, "Delete key");
    await deletes.expectSent("DELETE", "/virtual-keys/vk-1");
    const submit = await expectUxEvent("form_submit", "virtual-key-delete");
    await expect(submit.outcome).toBe("ok");
    await expectSheetClosed();
    // the confirmed delete is not also an abandon on its way out
    await expect(
      uxEvents().filter((e) => e.action === "form_abandon" && e.target === "virtual-key-delete"),
    ).toHaveLength(1);
  },
};

/**
 * Rotating a shared key (#1837) is a confirmation that carries the overlap
 * choice: the request names it in seconds, the new secret is shown once, and a
 * rotation with the default choice keeps no old secret alive.
 */
let rotations: Recorder;
export const RotatingAKeySendsTheChosenOverlapAndRevealsTheSecret: Story = {
  render: () => {
    rotations = recording(
      scoped(async (input, init) =>
        init?.method === "POST"
          ? json({ ...KEYS[0], key: "sk-rolter-rotated-shown-once" }, 200)
          : (lookups(String(input)) ?? json(KEYS)),
      ),
    );
    return (
      <Harness fetchStub={rotations.stub}>
        <Keys />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Rotate key backend service");
    const dialog = await within(document.body).findByRole("dialog", {
      name: "Rotate virtual key backend service?",
    });
    await pickOption(within(dialog).getByRole("combobox"), "24 hours");
    await userEvent.click(within(dialog).getByRole("button", { name: "Rotate key" }));

    await expect(await rotations.expectSentBody("POST", "/virtual-keys/vk-1/rotate")).toEqual({
      overlap_seconds: 86400,
    });
    const reveal = await within(document.body).findByRole("dialog", { name: "Key rotated" });
    await waitFor(() =>
      expect(within(reveal).getByText("sk-rolter-rotated-shown-once")).toBeInTheDocument(),
    );
  },
};

/**
 * A key mid-overlap names the old secret and the moment it stops working, and
 * offers to end it early: the revoke is its own confirmation and its own
 * `DELETE`, and a cancelled one sends nothing.
 */
let revokes: Recorder;
export const AKeyInOverlapNamesTheOldSecretAndRevokesItEarly: Story = {
  render: () => {
    revokes = recording(
      scoped(async (input, init) =>
        init?.method === "DELETE"
          ? json({ ...KEYS[0], previous_key_prefix: null, previous_expires_at: null })
          : (lookups(String(input)) ??
            json([
              {
                ...KEYS[0],
                previous_key_prefix: "sk-rolter-oldold",
                previous_expires_at: "2099-01-01T12:00:00Z",
              },
              KEYS[1],
            ])),
      ),
    );
    return (
      <Harness fetchStub={revokes.stub}>
        <Keys />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/old secret sk-rolter-oldold… valid until/)).toBeVisible();
    // only the key mid-overlap offers it
    await expect(canvas.getAllByRole("button", { name: /Revoke the old secret/ })).toHaveLength(1);

    await clickWhenEnabled(canvasElement, "Revoke the old secret of backend service now");
    await cancelConfirmation();
    revokes.expectNotSent("DELETE", "/previous-secret");

    await clickWhenEnabled(canvasElement, "Revoke the old secret of backend service now");
    await confirmDestructive(/sk-rolter-oldold/, "Revoke old secret");
    await revokes.expectSent("DELETE", "/virtual-keys/vk-1/previous-secret");
    await expectSheetClosed();
  },
};

// the same screen at a phone's width in both languages: Russian runs a third
// longer than English and overflowed twice as many screens (#2004)
const keysFit = phoneFits({
  render: () => (
    <Harness fetchStub={withKeys(KEYS)}>
      <Keys />
    </Harness>
  ),
  ready: (canvas) => canvas.findByText("backend service"),
});
export const MobileInRussian: Story = keysFit("mobile", "ru");
export const SmallPhone: Story = keysFit("small", "en");
export const SmallPhoneInRussian: Story = keysFit("small", "ru");

/**
 * The create sheet's subtitle is the warning that the key is shown once, and at
 * a 1024px window it was cut to "…copy" (#2812). It wraps, in both locales, so
 * the sentence is on screen to its last word.
 */
function createSubtitleIsWhole(locale: "en" | "ru") {
  const cat = locale === "ru" ? ru : en;
  return {
    ...atSplit,
    globals: { ...atSplit.globals, locale },
    render: () => (
      <Harness fetchStub={withKeys(KEYS)}>
        <Keys />
      </Harness>
    ),
    play: async ({ canvasElement }: { canvasElement: HTMLElement }) => {
      await clickWhenEnabled(canvasElement, new RegExp(cat.pages.virtualKeys.add, "i"));
      const dialog = within(sheet());
      const subtitle = await dialog.findByText(cat.pages.virtualKeys.createSubtitle);
      await expectNotTruncated(subtitle);
      await expectInViewport(subtitle);
    },
  };
}

export const CreateSubtitleIsWholeAt1024: Story = createSubtitleIsWhole("en");
export const CreateSubtitleIsWholeAt1024InRussian: Story = createSubtitleIsWhole("ru");

/** the caps the org-wide reads answer for a key (#1171) */
const KEY_BUDGET: BudgetRow = {
  id: "budget-1",
  scope_type: "virtual_key",
  scope_id: "vk-1",
  limit_usd: "50.0000",
  period: "monthly",
  unpriced_policy: null,
  created_at: "2026-07-01T00:00:00Z",
};

const KEY_RATE: RateLimitRow = {
  id: "rate-1",
  scope_type: "virtual_key",
  scope_id: "vk-1",
  rpm: 60,
  tpm: 40000,
  created_at: "2026-07-01T00:00:00Z",
};

/**
 * A row states its key's cap and window from the org-wide reads, one request
 * each however many keys there are, and what is left of the cap from the one
 * org-wide spend read (#3128). A budget on another scope is not this key's.
 */
export const RowStatesItsCap: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/budgets/spend")) {
          return json({
            status: "ok",
            budgets: [
              { id: "budget-1", spent_usd: "12.5000", resets_at: "2026-08-01T00:00:00Z" },
              { id: "budget-2", spent_usd: "1.0000", resets_at: null },
            ],
          });
        }
        if (url.includes("/budgets")) {
          return json([
            KEY_BUDGET,
            { ...KEY_BUDGET, id: "budget-2", scope_type: "project", scope_id: "vk-2" },
          ]);
        }
        if (url.includes("/rate-limits")) return json([KEY_RATE]);
        return lookups(url) ?? json(KEYS);
      })}
    >
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/Spend cap \$50\.00 \(monthly\)/)).toBeVisible();
    await expect(await canvas.findByText(/\$12\.50 spent · \$37\.50 left/)).toBeVisible();
    await expect(canvas.getByText("Rate cap 60 rpm · 40,000 tpm")).toBeVisible();
    await expect(canvas.getAllByText(/Spend cap/)).toHaveLength(1);
    await expect(canvas.getAllByText(/left/)).toHaveLength(1);
  },
};

/** An unreadable counter is unknown, never a full cap. */
export const RowSaysSpendIsUnknown: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/budgets/spend")) {
          return json({
            status: "unavailable",
            budgets: [{ id: "budget-1", spent_usd: null, resets_at: null }],
          });
        }
        if (url.includes("/budgets")) return json([KEY_BUDGET]);
        if (url.includes("/rate-limits")) return json([]);
        return lookups(url) ?? json(KEYS);
      })}
    >
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("spend so far unknown")).toBeVisible();
    await expect(canvas.queryByText(/left/)).toBeNull();
  },
};

/** The sheet sends a cap only when it was switched on, and the summary says so. */
export const MintsWithASpendAndRateCap: Story = {
  render: () => {
    sentCaps = recording(
      scoped(async (input, init) =>
        init?.method === "POST"
          ? json({ ...KEYS[0], key: MINTED_KEY }, 201)
          : (lookups(String(input)) ?? json(KEYS)),
      ),
    );
    return (
      <Harness fetchStub={sentCaps.stub}>
        <Keys />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add virtual key/i);
    const form = sheet();
    const inForm = within(form);
    await expect(inForm.getByText("No spend cap of its own")).toBeVisible();
    await expect(inForm.getByText("No rate cap of its own")).toBeVisible();
    await userEvent.type(inForm.getByLabelText("Name"), "ci runner");
    await userEvent.click(inForm.getByRole("switch", { name: "Spend cap" }));
    await userEvent.type(inForm.getByLabelText(/^Cap \(USD\)/), "25");
    await expect(inForm.getByText(/Spend capped at \$25\.00 \(monthly\)/)).toBeVisible();
    await userEvent.click(inForm.getByRole("switch", { name: "Rate cap" }));
    await userEvent.type(inForm.getByLabelText("Tokens per minute"), "9000");
    await expect(inForm.getByText("Rate capped at 9,000 tpm")).toBeVisible();
    await userEvent.click(inForm.getByRole("button", { name: "Create" }));
    const body = await sentCaps.expectSentBody<{
      budget?: { limit_usd: string; period: string };
      rate_limit?: { rpm?: number; tpm?: number };
    }>("POST", "/virtual-keys");
    await expect(body.budget).toEqual({ limit_usd: "25", period: "monthly" });
    await expect(body.rate_limit).toEqual({ tpm: 9000 });
  },
};
let sentCaps: Recorder;

/** A mint with no cap chosen sends neither field: the default is no cap. */
export const MintsWithoutACapByDefault: Story = {
  render: () => {
    sentCaps = recording(
      scoped(async (input, init) =>
        init?.method === "POST"
          ? json({ ...KEYS[0], key: MINTED_KEY }, 201)
          : (lookups(String(input)) ?? json(KEYS)),
      ),
    );
    return (
      <Harness fetchStub={sentCaps.stub}>
        <Keys />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await mintAKey(canvasElement);
    const body = await sentCaps.expectSentBody<Record<string, unknown>>("POST", "/virtual-keys");
    await expect(body).not.toHaveProperty("budget");
    await expect(body).not.toHaveProperty("rate_limit");
  },
};

/** A cap left wrong names the field and keeps Create refused. */
export const CapValidationErrors: Story = {
  render: () => (
    <Harness fetchStub={withKeys(KEYS)}>
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add virtual key/i);
    const form = sheet();
    const inForm = within(form);
    await userEvent.type(inForm.getByLabelText("Name"), "ci runner");
    await userEvent.click(inForm.getByRole("switch", { name: "Spend cap" }));
    await expect(await inForm.findByText("Enter the amount this key may spend")).toBeVisible();
    await userEvent.type(inForm.getByLabelText(/^Cap \(USD\)/), "1e9");
    await expect(inForm.getByText(/Enter a plain number/)).toBeVisible();
    await userEvent.click(inForm.getByRole("switch", { name: "Rate cap" }));
    await expect(inForm.getByText(/Fill in requests per minute/)).toBeVisible();
    await expect(inForm.getByRole("button", { name: "Create" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  },
};

/**
 * An overlap whose date has passed is over: the row stops naming the old secret
 * (and offering to revoke it) even though the list still carries the columns.
 */
export const AnOverlapThatHasEndedIsNotShown: Story = {
  render: () => (
    <Harness
      fetchStub={withKeys([
        {
          ...KEYS[0],
          previous_key_prefix: "sk-rolter-oldold",
          previous_expires_at: "2020-01-01T12:00:00Z",
        },
      ])}
    >
      <Keys />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("backend service");
    await expect(canvas.queryByText(/old secret/)).toBeNull();
    await expect(canvas.queryByRole("button", { name: /Revoke the old secret/ })).toBeNull();
  },
};
