import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Keys from "./Keys";
import {
  Harness,
  clickWhenEnabled,
  expectClosesWithoutPrompting,
  expectSheetClosed,
  expectSkeleton,
  json,
  pending,
  recording,
  scoped,
  sheet,
  type FetchStub,
  type Recorder,
  withConfirm,
} from "./story-harness";
import type {
  BusinessUnitRow,
  CustomerRow,
  ProviderRow,
  RouteRow,
  VirtualKeyRow,
} from "@/lib/api";
import { formattersFor } from "@/lib/i18n/format";
import { atMobile, atTablet, expectNoHorizontalOverflow } from "@/lib/story-viewport";

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
    key_hash: "hash-1",
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
    key_hash: "hash-2",
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
  },
};

export const Empty: Story = {
  render: () => (
    <Harness fetchStub={withKeys([])}>
      <Keys />
    </Harness>
  ),
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
  },
};

export const CreatesAKey: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        init?.method === "POST"
          ? json({ ...KEYS[0], key: "sk-rolter-plaintext-shown-once" }, 201)
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
    // the plaintext key is shown exactly once, right after creation — losing
    // that dialog means the caller never gets their secret
    await waitFor(() =>
      expect(within(document.body).getByText("sk-rolter-plaintext-shown-once")).toBeInTheDocument(),
    );
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

    await withConfirm(false, async () => {
      await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
      // declining the discard keeps the sheet — and the typing — alive
      await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
      await expect(within(form).getByLabelText("Name")).toHaveValue("half typed");
    });

    await withConfirm(true, async () => {
      await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
      await expectSheetClosed();
    });
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
    await expect(within(form).getByRole("button", { name: "Create" })).toBeDisabled();
    await userEvent.type(within(form).getByLabelText("Name"), "backend service");
    await expect(within(form).getByRole("button", { name: "Create" })).toBeEnabled();
    // the finite default is the same one the self-service sheet offers
    await expect(within(form).getByLabelText("Expires")).toHaveValue("30");
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
    await userEvent.click(
      await canvas.findByRole("button", { name: /Edit key revoked laptop/i }),
    );
    const form = sheet();
    // save stays disabled until something actually moves — re-saving an
    // untouched sheet must not write or audit anything
    await expect(within(form).getByRole("button", { name: "Save" })).toBeDisabled();
    await userEvent.selectOptions(within(form).getByLabelText("Business unit"), UNIT_ID);
    await userEvent.selectOptions(within(form).getByLabelText("Customer"), CUSTOMER_ID);
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
    await userEvent.click(
      await canvas.findByRole("button", { name: /Edit key backend service/i }),
    );
    const form = sheet();
    // seeded from the row, so the editor opens on the truth rather than blank
    await expect(within(form).getByLabelText("Business unit")).toHaveValue(UNIT_ID);
    await userEvent.selectOptions(
      within(form).getByLabelText("Business unit"),
      "__unattributed__",
    );
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
    await userEvent.click(
      await canvas.findByRole("button", { name: /Edit key revoked laptop/i }),
    );
    const form = sheet();
    await userEvent.click(within(form).getByRole("checkbox", { name: "Anthropic" }));
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
    await userEvent.selectOptions(within(form).getByLabelText("Business unit"), UNIT_ID);
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
    await userEvent.click(
      await canvas.findByRole("button", { name: /Edit key revoked laptop/i }),
    );
    const form = sheet();
    const customer = within(form).getByLabelText("Customer");
    // with no unit chosen, every customer is reachable
    await expect(within(customer).getByText("Acme Corp")).toBeInTheDocument();
    await userEvent.selectOptions(within(form).getByLabelText("Business unit"), UNIT_ID);
    // Acme belongs to that unit and Globex to none, so both still fit
    await expect(within(customer).getByText("Acme Corp")).toBeInTheDocument();
    await expect(within(customer).getByText("Globex")).toBeInTheDocument();
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
    await userEvent.type(
      within(form).getByLabelText(/model allow-list/i),
      "legacy-davinci",
    );
    await userEvent.click(within(form).getByRole("button", { name: "Add" }));
    await userEvent.click(within(form).getByRole("button", { name: "Create" }));

    const posted = (await sent.expectSentBody("POST", "/virtual-keys")) as {
      models: string[];
    };
    await expect(posted.models).toEqual(["gpt-4o", "legacy-davinci"]);
  },
};
