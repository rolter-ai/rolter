import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Security from "./Security";
import {
  Harness,
  Toasted,
  cancelConfirmation,
  confirmDestructive,
  confirmation,
  expectLoadError,
  expectNoUxEvent,
  expectSkeleton,
  expectToast,
  expectUxEvent,
  json,
  recordUxEvents,
  recording,
} from "./story-harness";
import type { ClusterNodeRow, SecuritySettingsDto } from "@/lib/api";
import ru from "@/lib/i18n/locales/ru.json";
import { atMobile, atTablet, expectNoHorizontalOverflow } from "@/lib/story-viewport";
import { UxScreenProvider } from "@/lib/ux-react";

const BASE: SecuritySettingsDto = {
  allowed_origins: ["https://app.example.com"],
  allowed_headers: ["x-request-id"],
  required_headers: { "x-tenant": "acme" },
  auth_bypass_routes: ["/v1/models"],
  updated_at: "2026-08-01T09:00:00Z",
};

const meta = {
  title: "Screens/Security",
  component: Security,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Security>;

export default meta;
type Story = StoryObj<typeof meta>;

const SETTINGS = "/api/v1/security-settings";

const gateway = (over: Partial<ClusterNodeRow>): ClusterNodeRow => ({
  id: "gw-1",
  role: "gateway",
  build_version: "0.9.0",
  config_version: 12,
  desired_state: "active",
  state_changed_at: "2026-09-01T08:00:00Z",
  first_seen_at: "2026-09-01T08:00:00Z",
  last_seen_at: "2026-09-01T09:00:00Z",
  live: true,
  converged: true,
  ...over,
});

/**
 * The control plane as this screen talks to it: the settings read, a save that
 * echoes what was sent, and the cluster inventory the saved line asks about.
 * Recorded, so a story can assert what left the browser and what did not.
 */
function securityApi(
  base: SecuritySettingsDto = BASE,
  nodes: () => Response = () => json([gateway({ id: "gw-1" }), gateway({ id: "gw-2" })]),
  save: (body: Record<string, unknown>) => Response = (body) => json({ ...base, ...body }),
) {
  return recording(async (input, init) => {
    if (String(input).includes("/api/v1/cluster/nodes")) return nodes();
    if (init?.method === "PUT") return save(JSON.parse(String(init.body)));
    return json(base);
  });
}

// the body a save of `BASE` with nothing touched sends
const BASE_BODY = {
  allowed_origins: ["https://app.example.com"],
  allowed_headers: ["x-request-id"],
  required_headers: { "x-tenant": "acme" },
  auth_bypass_routes: ["/v1/models"],
};

const puts = (api: { calls: { method: string }[] }) =>
  api.calls.filter((call) => call.method === "PUT");

// replace a list, one entry per line: `{Enter}` is the line break
async function enter(field: HTMLElement, text: string) {
  await userEvent.clear(field);
  if (text) await userEvent.type(field, text);
}

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={async () => json(BASE)}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByLabelText("Allowed Origins")).toBeVisible();
    // the "enforce virtual keys" switch is gone (#2357): it only reached
    // managed gateways, which refuse a keyless request whatever it said
    await expect(canvas.queryByRole("switch")).toBeNull();
    await expect(canvas.queryByText(/Enforce Virtual Keys/)).toBeNull();
  },
};

// a settings form's honest placeholder is the shape of the panels it is about
// to render, not the word "Loading" on one line
export const Loading: Story = {
  render: () => (
    <Harness fetchStub={() => new Promise<Response>(() => {})}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

/**
 * The #962 failure this screen had verbatim: every cause rendered "Security
 * settings need superadmin access", so a 500 read as a permissions problem.
 * A 403 is the one case where that sentence was true, and it is now the only
 * case where it is said.
 */
export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={async () => json({ error: { message: "forbidden" } }, 403)}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /You do not have access to security settings/);
    // a 403 is not transient, so no retry is offered
    await expect(canvas.queryByRole("button", { name: /Try again/ })).not.toBeInTheDocument();
  },
};

export const Error_: Story = {
  name: "Error",
  render: () => (
    <Harness fetchStub={async () => json({ error: { message: "sqlx: pool timed out" } }, 500)}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /failed to return security settings/i);
    // the control plane's own words survive rather than being swallowed
    await expect(canvas.getByText(/pool timed out/)).toBeVisible();
    await expect(canvas.getByRole("button", { name: /Try again/ })).toBeInTheDocument();
  },
};

/**
 * A save that went through used to be a line of text in the footer that the
 * next keystroke wiped. It is a toast now, so it survives the sheet, the
 * scroll and the operator looking away (#1197).
 */
export const SavesChanges: Story = {
  render: () => (
    <Harness
      fetchStub={async (_input, init) =>
        init?.method === "PUT" ? json({ ...BASE, ...JSON.parse(String(init.body)) }) : json(BASE)
      }
    >
      <Toasted>
        <Security />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const origins = await canvas.findByDisplayValue("https://app.example.com");
    await userEvent.clear(origins);
    await userEvent.type(origins, "https://console.example.com");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /security settings updated/i);
  },
};

/**
 * The save is refused (#1607).
 *
 * This screen is a form, not a sheet, so "the draft survives" means the edited
 * field still holds what was typed rather than snapping back to the value the
 * server last confirmed — and the refusal reaches the toast queue, which is the
 * only place a rejected write is reported here.
 */
export const SaveRejectedByTheServer: Story = {
  render: () => (
    <Harness
      fetchStub={async (_input, init) =>
        init?.method === "PUT"
          ? json({ error: { message: "https://console.example.com is not a valid origin" } }, 422)
          : json(BASE)
      }
    >
      <Toasted>
        <Security />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const origins = await canvas.findByDisplayValue("https://app.example.com");
    await userEvent.clear(origins);
    await userEvent.type(origins, "https://console.example.com");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /is not a valid origin/, "error");
    await waitFor(() => expect(origins).toHaveValue("https://console.example.com"));
  },
};

/** The settings panels are one column below `sm`, and nothing spills (#1203). */
export const Mobile: Story = {
  ...atMobile,
  render: () => (
    <Harness fetchStub={async () => json(BASE)}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByLabelText("Allowed Origins")).toBeVisible();
    await expectNoHorizontalOverflow();
  },
};

export const Tablet: Story = {
  ...atTablet,
  render: () => (
    <Harness fetchStub={async () => json(BASE)}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByLabelText("Allowed Origins")).toBeVisible();
    await expectNoHorizontalOverflow();
  },
};

const fresh = securityApi({
  allowed_origins: [],
  allowed_headers: [],
  required_headers: {},
  auth_bypass_routes: [],
  updated_at: "2026-08-01T09:00:00Z",
});

/**
 * A deployment nobody has configured yet: every list is empty, so each field
 * shows the shape of its entries one per line, and there is nothing to save.
 */
export const FreshDeployment: Story = {
  render: () => (
    <Harness fetchStub={fresh.stub}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const origins = await canvas.findByLabelText("Allowed Origins");
    await expect(origins).toHaveValue("");
    await expect(origins).toHaveAttribute(
      "placeholder",
      "https://app.example.com\nhttps://console.example.com",
    );
    await expect(canvas.getByLabelText("Required Headers")).toHaveValue("");
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled();
    fresh.expectNotSent("PUT", SETTINGS);
  },
};

const pristine = securityApi();

/**
 * Save waits for an edit. A card says it was edited, the footer counts the
 * cards, a blank line is no edit, and Discard puts every field back to what
 * was loaded (#2103).
 */
export const SaveWaitsForAnEditAndDiscardRestores: Story = {
  render: () => (
    <Harness fetchStub={pristine.stub}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const origins = await canvas.findByLabelText("Allowed Origins");
    const save = canvas.getByRole("button", { name: "Save Changes" });
    const discard = canvas.getByRole("button", { name: "Discard changes" });
    await expect(save).toBeDisabled();
    await expect(discard).toBeDisabled();
    await expect(canvas.queryAllByText("Edited")).toHaveLength(0);

    // a blank line, or space around an entry, changes nothing that would be sent
    await userEvent.type(origins, "{Enter}{Enter}  ");
    await expect(save).toBeDisabled();
    await expect(canvas.queryAllByText("Edited")).toHaveLength(0);

    await userEvent.type(origins, "{Enter}https://console.example.com");
    await expect(save).toBeEnabled();
    await expect(discard).toBeEnabled();
    await expect(canvas.getAllByText("Edited")).toHaveLength(1);
    await expect(canvas.getByText("1 setting changed, not saved yet.")).toBeVisible();

    const names = canvas.getByLabelText("Allowed Headers");
    await userEvent.type(names, "{Enter}x-trace");
    await expect(canvas.getAllByText("Edited")).toHaveLength(2);
    await expect(canvas.getByText("2 settings changed, not saved yet.")).toBeVisible();

    await userEvent.click(discard);
    await expect(origins).toHaveValue("https://app.example.com");
    await expect(names).toHaveValue("x-request-id");
    await expect(canvas.queryAllByText("Edited")).toHaveLength(0);
    await expect(canvas.queryByText(/not saved yet/)).toBeNull();
    await expect(save).toBeDisabled();
    await expect(discard).toBeDisabled();
    pristine.expectNotSent("PUT", SETTINGS);
  },
};

const malformed = securityApi();

/**
 * The entry the old form dropped without a word (#2114). `x-tenant=acme` has
 * no colon: it stays in the field, the error names it and its line once the
 * caret leaves the field, the control points at the reason, and Save cannot
 * send it. Fixing the line is the way out, and the request that follows
 * carries the fixed pair.
 */
export const MalformedHeaderStaysAndBlocksSave: Story = {
  render: () => (
    <Harness fetchStub={malformed.stub}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const headers = await canvas.findByLabelText("Required Headers");
    await expect(headers).toHaveValue("x-tenant: acme");

    await enter(headers, "x-tenant=acme");
    // `h` on the way to `https://` is not a mistake yet: nothing is said while typing
    await expect(canvas.queryByText(/needs a name and a value/)).toBeNull();
    await userEvent.tab();

    const message = "Line 1: x-tenant=acme needs a name and a value, written as name: value.";
    await expect(await canvas.findByText(message)).toBeVisible();
    await expect(headers).toHaveValue("x-tenant=acme");
    await expect(headers).toBeInvalid();
    await expect(headers).toHaveAccessibleDescription(new RegExp(message.replace(".", "\\.")));
    await expect(canvas.getByText("Fix 1 entry to save.")).toBeVisible();
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled();
    malformed.expectNotSent("PUT", SETTINGS);

    await enter(headers, "x-tenant: globex");
    await waitFor(() => expect(canvas.queryByText(message)).toBeNull());
    await expect(headers).toBeValid();
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expect(await malformed.expectSentBody("PUT", SETTINGS)).toEqual({
      ...BASE_BODY,
      required_headers: { "x-tenant": "globex" },
    });
    await expect(puts(malformed)).toHaveLength(1);
  },
};

const problems = securityApi();

/**
 * Every list names its own kind of problem, each with its line, and the same
 * rules the control plane applies: a wildcard origin, a bypass path outside
 * `/v1/`, a header name with a space, a header given twice, several entries
 * typed on one line. Past three, the rest are counted rather than listed.
 */
export const EveryListNamesItsProblems: Story = {
  render: () => (
    <Harness fetchStub={problems.stub}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const origins = await canvas.findByLabelText("Allowed Origins");
    await enter(origins, "https://*.example.com{Enter}https://ok.example.com");
    const names = canvas.getByLabelText("Allowed Headers");
    await enter(names, "x-request-id{Enter}x tenant");
    const required = canvas.getByLabelText("Required Headers");
    await enter(required, "x-tenant: a{Enter}X-Tenant: b");
    const routes = canvas.getByLabelText("Auth Bypass Routes");
    await enter(routes, "/v1/models, /v1/ping");
    await userEvent.tab();
    await userEvent.click(canvas.getByText("Required Headers"));

    await expect(
      await canvas.findByText(
        "Line 1: https://*.example.com has a wildcard. List each origin exactly.",
      ),
    ).toBeVisible();
    await expect(
      canvas.getByText(
        "Line 2: x tenant is not a valid header name. Use letters, digits and ! # $ % & ' * + - . ^ _ ` | ~",
      ),
    ).toBeVisible();
    await expect(
      canvas.getByText(
        "Line 2: X-Tenant is already set on an earlier line. A header holds one value.",
      ),
    ).toBeVisible();
    await expect(
      canvas.getByText(
        "Line 1: /v1/models, /v1/ping looks like several entries. Put each on its own line.",
      ),
    ).toBeVisible();
    await expect(canvas.getByText("Fix 4 entries to save.")).toBeVisible();
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled();

    // a fifth and sixth problem in one list are counted, not listed
    await enter(
      origins,
      "ftp://a.example.com{Enter}b.example.com{Enter}https://c.example.com/x{Enter}https://d.example.com?q=1{Enter}https://e.example.com#top",
    );
    await userEvent.tab();
    await expect(await canvas.findByText("…and 2 more problems.")).toBeVisible();
    await expect(canvas.queryByText(/Line 5:/)).toBeNull();
    problems.expectNotSent("PUT", SETTINGS);
  },
};

// stored by the old form, or by a seed file: a value that holds a comma, and
// lists of more than one entry
const OLD_FORM_VALUES: SecuritySettingsDto = {
  ...BASE,
  allowed_origins: ["https://app.example.com", "https://console.example.com"],
  allowed_headers: ["x-request-id", "X-Stainless-Timeout"],
  required_headers: { "x-trace": "a,b", "x-tenant": "acme" },
  auth_bypass_routes: ["/v1/models", "/v1/ping"],
};
const roundTrip = securityApi(OLD_FORM_VALUES);

/**
 * What was stored loads one entry to a line with nothing lost, a value that
 * holds commas included. Editing another field sends every entry back intact,
 * and a new value with commas in it goes out whole rather than cut at each one.
 */
export const ValuesWithCommasRoundTrip: Story = {
  render: () => (
    <Harness fetchStub={roundTrip.stub}>
      <Toasted>
        <Security />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const required = await canvas.findByLabelText("Required Headers");
    await expect(required).toHaveValue("x-trace: a,b\nx-tenant: acme");
    await expect(canvas.getByLabelText("Allowed Origins")).toHaveValue(
      "https://app.example.com\nhttps://console.example.com",
    );
    await expect(canvas.getByLabelText("Allowed Headers")).toHaveValue(
      "x-request-id\nX-Stainless-Timeout",
    );
    await expect(canvas.getByLabelText("Auth Bypass Routes")).toHaveValue("/v1/models\n/v1/ping");
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled();

    await userEvent.type(required, "{Enter}x-ids: 1,2,3");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expect(await roundTrip.expectSentBody("PUT", SETTINGS)).toEqual({
      ...BASE_BODY,
      allowed_origins: ["https://app.example.com", "https://console.example.com"],
      allowed_headers: ["x-request-id", "X-Stainless-Timeout"],
      required_headers: { "x-trace": "a,b", "x-tenant": "acme", "x-ids": "1,2,3" },
      auth_bypass_routes: ["/v1/models", "/v1/ping"],
    });
    await expectToast(canvasElement, /security settings updated/i);
    await waitFor(() => expect(required).toHaveValue("x-trace: a,b\nx-tenant: acme\nx-ids: 1,2,3"));
    await expect(canvas.queryAllByText("Edited")).toHaveLength(0);
  },
};

const legacy = securityApi({ ...BASE, auth_bypass_routes: ["/healthz", "/v1/models"] });

/**
 * A stored entry the control plane would now refuse is kept in the field and
 * named from the first paint, since nobody typed it. Nothing else can be saved
 * until it is fixed, which is the control plane's own answer to that save.
 */
export const StoredProblemIsShownFromTheStart: Story = {
  render: () => (
    <Harness fetchStub={legacy.stub}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const routes = await canvas.findByLabelText("Auth Bypass Routes");
    await expect(routes).toHaveValue("/healthz\n/v1/models");
    await expect(
      await canvas.findByText("Line 1: /healthz must be an exact path under /v1/."),
    ).toBeVisible();
    await expect(routes).toBeInvalid();
    await expect(canvas.getByText("Fix 1 entry to save.")).toBeVisible();
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled();
  },
};

const loosen = securityApi();

/**
 * Adding two bypass routes in one save. The dialog lists exactly those two,
 * says what each means, and nothing is sent until it is confirmed. One request
 * then carries the whole form (#2103).
 */
export const LooseningIsConfirmedBeforeItIsSent: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness fetchStub={loosen.stub}>
      <UxScreenProvider screen="security">
        <Security />
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(
      await canvas.findByLabelText("Auth Bypass Routes"),
      "{Enter}/v1/ping{Enter}/v1/embeddings",
    );
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));

    const dialogElement = await confirmation();
    await waitFor(() => expect(dialogElement).toBeVisible());
    const dialog = within(dialogElement);
    await expect(
      dialog.getByRole("heading", { name: "Save changes that loosen security?" }),
    ).toBeInTheDocument();
    await expect(dialogElement).toHaveTextContent("This save loosens security in 2 ways");
    const rows = await dialog.findAllByRole("listitem");
    await expect(rows).toHaveLength(2);
    await expect(rows[0]).toHaveTextContent("Auth bypass route added:");
    await expect(within(rows[0]).getByText("/v1/ping")).toBeInTheDocument();
    await expect(rows[0]).toHaveTextContent("no budget or per-key rate limit applies to it");
    await expect(rows[1]).toHaveTextContent("Auth bypass route added:");
    await expect(within(rows[1]).getByText("/v1/embeddings")).toBeInTheDocument();
    await expect(puts(loosen)).toHaveLength(0);
    expectNoUxEvent("form_submit", "security-loosen");

    await confirmDestructive("Save changes that loosen security?", "Save changes");
    await expect(await loosen.expectSentBody("PUT", SETTINGS)).toEqual({
      ...BASE_BODY,
      auth_bypass_routes: ["/v1/models", "/v1/ping", "/v1/embeddings"],
    });
    await expect(puts(loosen)).toHaveLength(1);
    await expectUxEvent("form_submit", "security-loosen");
    await expectUxEvent("save_confirmed", "security-loosen");
    await waitFor(() => expect(within(document.body).queryByRole("dialog")).toBeNull());
    await expect(canvas.queryAllByText("Edited")).toHaveLength(0);
  },
};

const cancelled = securityApi();

/**
 * Backing out of the confirmation sends nothing and takes nothing away: the
 * edit is still in the form, still marked, and Save is still there for a second
 * thought.
 */
export const CancelledLooseningSendsNothing: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness fetchStub={cancelled.stub}>
      <UxScreenProvider screen="security">
        <Security />
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const routes = await canvas.findByLabelText("Auth Bypass Routes");
    await userEvent.type(routes, "{Enter}/v1/ping");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expect(await confirmation()).toBeInTheDocument();

    await cancelConfirmation();
    cancelled.expectNotSent("PUT", SETTINGS);
    expectNoUxEvent("form_submit", "security-loosen");
    await expectUxEvent("form_abandon", "security-loosen");
    await expect(routes).toHaveValue("/v1/models\n/v1/ping");
    await expect(canvas.getAllByText("Edited")).toHaveLength(1);
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeEnabled();
  },
};

const refused = securityApi(BASE, undefined, () =>
  json({ error: { message: "security settings could not be saved" } }, 422),
);

/**
 * The control plane refuses the confirmed save. The dialog stays open with its
 * own words on a line beside the button that caused it, the toast carries them
 * too, and the edit is still in the form.
 */
export const RefusedLooseningKeepsTheDialogOpen: Story = {
  render: () => (
    <Harness fetchStub={refused.stub}>
      <Toasted>
        <Security />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await canvas.findByLabelText("Auth Bypass Routes"), "{Enter}/v1/ping");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    const dialogElement = await confirmation();
    await userEvent.click(within(dialogElement).getByRole("button", { name: "Save changes" }));

    await waitFor(() =>
      expect(within(dialogElement).getByRole("alert")).toHaveTextContent(
        "security settings could not be saved",
      ),
    );
    await expectToast(canvasElement, /security settings could not be saved/, "error");
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
    await expect(canvas.getAllByText("Edited")).toHaveLength(1);
  },
};

const tightening = securityApi();

/**
 * A save that only closes things goes straight out: a bypass route taken away
 * and a header required. No dialog stands in front of it,
 * and one request is all that is sent.
 */
export const TighteningSavesWithoutAsking: Story = {
  render: () => (
    <Harness fetchStub={tightening.stub}>
      <Toasted>
        <Security />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.clear(await canvas.findByLabelText("Auth Bypass Routes"));
    await userEvent.type(canvas.getByLabelText("Required Headers"), "{Enter}x-mesh-id: edge");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));

    await expect(await tightening.expectSentBody("PUT", SETTINGS)).toEqual({
      ...BASE_BODY,
      required_headers: { "x-tenant": "acme", "x-mesh-id": "edge" },
      auth_bypass_routes: [],
    });
    await expectToast(canvasElement, /security settings updated/i);
    await expect(within(document.body).queryByRole("dialog")).toBeNull();
    await expect(puts(tightening)).toHaveLength(1);
  },
};

const pickedUp = securityApi();

/**
 * After a save the footer says when it landed and which gateways run it. The
 * control plane compares each node's config version with the one the save
 * produced, so "running it" is read, not promised.
 */
export const SavedLineSaysEveryGatewayRunsIt: Story = {
  render: () => (
    <Harness fetchStub={pickedUp.stub}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const origins = await canvas.findByLabelText("Allowed Origins");
    await enter(origins, "https://console.example.com");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expect(
      await canvas.findByText(/^Saved at .+\. All 2 live gateways are running it\.$/),
    ).toBeVisible();
    await pickedUp.expectSent("GET", "/api/v1/cluster/nodes");
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled();
  },
};

const lagging = securityApi(BASE, () =>
  json([
    gateway({ id: "gw-1" }),
    gateway({ id: "gw-2", converged: false, config_version: 11 }),
    // stopped polling: not serving, so not waited on
    gateway({ id: "gw-3", live: false, converged: false, config_version: 9 }),
  ]),
);

/** A gateway that has not polled since the save is counted as not running it yet. */
export const SavedLineCountsAGatewayThatLags: Story = {
  render: () => (
    <Harness fetchStub={lagging.stub}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await enter(await canvas.findByLabelText("Allowed Origins"), "https://console.example.com");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expect(
      await canvas.findByText(
        /^Saved at .+\. 1 of 2 live gateways are running it; the rest pick it up on their next config poll\.$/,
      ),
    ).toBeVisible();
  },
};

const silent = securityApi(BASE, () => json([]));

/**
 * No gateway has ever identified itself to this control plane, so the screen
 * cannot say any of them picked the save up, and says when one will.
 */
export const SavedLineAdmitsWhenNoGatewayHasReported: Story = {
  render: () => (
    <Harness fetchStub={silent.stub}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await enter(await canvas.findByLabelText("Allowed Origins"), "https://console.example.com");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expect(
      await canvas.findByText(
        /^Saved at .+\. No live gateway has reported to this control plane, so pickup cannot be confirmed\. A gateway applies it on its next config poll, 5 s by default\.$/,
      ),
    ).toBeVisible();
  },
};

const unreadable = securityApi(BASE, () => json({ error: { message: "cluster store down" } }, 500));

/** The inventory read fails: the save still stands, and the line says it could not check. */
export const SavedLineAdmitsWhenTheGatewaysCannotBeRead: Story = {
  render: () => (
    <Harness fetchStub={unreadable.stub}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await enter(await canvas.findByLabelText("Allowed Origins"), "https://console.example.com");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expect(
      await canvas.findByText(
        /^Saved at .+\. The live gateways could not be read, so pickup cannot be confirmed\./,
      ),
    ).toBeVisible();
  },
};

const russian = securityApi();

/** The errors, the footer and the buttons in Russian, with the entry quoted as typed. */
export const ValidationInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={russian.stub}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const copy = ru.pages.security;
    const canvas = within(canvasElement);
    // the locale decorator switches language from an effect, after first paint
    const origins = await canvas.findByLabelText(copy.allowedOrigins);
    await enter(origins, "ftp://x.example.com");
    await userEvent.tab();
    await expect(
      await canvas.findByText(
        copy.errors.originScheme
          .replace("{{line}}", "1")
          .replace("{{entry}}", "ftp://x.example.com"),
      ),
    ).toBeVisible();
    await expect(canvas.getByText(copy.status.invalid_one.replace("{{count}}", "1"))).toBeVisible();
    await expect(canvas.getByRole("button", { name: copy.discard })).toBeEnabled();
    await expect(canvas.getByRole("button", { name: "Сохранить изменения" })).toBeDisabled();
    russian.expectNotSent("PUT", SETTINGS);
  },
};

/** The footer, the status line and an error line share 375px without spilling (#1203). */
export const MobileKeepsErrorsAndFooterInView: Story = {
  ...atMobile,
  render: () => (
    <Harness fetchStub={securityApi().stub}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const headers = await canvas.findByLabelText("Required Headers");
    await enter(
      headers,
      "x-tenant=acme-with-a-rather-long-value-that-should-wrap-somewhere-sensible",
    );
    await userEvent.tab();
    await expect(await canvas.findByText(/needs a name and a value/)).toBeVisible();
    await expect(canvas.getByText("Fix 1 entry to save.")).toBeVisible();
    await expect(canvas.getByRole("button", { name: "Discard changes" })).toBeVisible();
    await expectNoHorizontalOverflow();
  },
};
