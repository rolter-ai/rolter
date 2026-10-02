import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import SingleSignOn from "./SingleSignOn";
import {
  cancelConfirmation,
  clickWhenEnabled,
  confirmation,
  confirmDestructive,
  expectAllowed,
  expectInStatusRegion,
  expectLoadError,
  expectNoFalseEmpty,
  expectNoUxEvent,
  expectRefused,
  expectSheetClosed,
  expectUxEvent,
  openOptions,
  PROJECT,
  TEAM,
  expectToast,
  Harness,
  json,
  NEEDS_ADMIN,
  ORG,
  pending,
  pickOption,
  recordUxEvents,
  recording,
  scoped,
  sheet,
  Toasted,
  type FetchStub,
  type Recorder,
} from "./story-harness";
import type {
  MembershipRow,
  OrgAuthPolicy,
  PublicUrl,
  SsoGroupMappingRow,
  SsoProviderRow,
} from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { atMobile } from "@/lib/story-viewport";
import { UxScreenProvider } from "@/lib/ux-react";

const NOW = "2026-08-01T10:00:00Z";

/**
 * The control plane's configured public url (#2083).
 *
 * Deliberately not the story's own origin: a card or a preview that fell back
 * to `window.location` would show the storybook host here and fail the
 * assertion, which is the bug the server-built urls exist to prevent.
 */
const PUBLIC_BASE = "https://rolter.acme.example";
const PUBLIC_URL: PublicUrl = { public_url: PUBLIC_BASE, configured: true };
// `ROLTER_PUBLIC_URL` unset: the control plane falls back to its default
const DEFAULT_BASE = "http://localhost:4001";
const UNSET: PublicUrl = { public_url: DEFAULT_BASE, configured: false };

const provider = (over: Partial<SsoProviderRow> = {}): SsoProviderRow => {
  const slug = over.slug ?? "okta";
  return {
    id: "sso-1",
    org_id: ORG.id,
    name: "Acme Okta",
    slug,
    issuer: "https://acme.okta.com",
    client_id: "0oa1b2c3d4",
    has_client_secret: true,
    scopes: ["openid", "email", "profile"],
    group_claim: "groups",
    default_role: "member",
    enabled: true,
    created_at: NOW,
    // built by the server from its public url, as `ProviderView` does
    redirect_uri: `${PUBLIC_BASE}/auth/sso/${slug}/callback`,
    login_url: `${PUBLIC_BASE}/auth/sso/${slug}/start`,
    ...over,
  };
};

const PROVIDERS: SsoProviderRow[] = [
  provider(),
  // no default role: a user in no mapped group is refused rather than let in
  // with an empty membership set, and the card has to say so
  provider({
    id: "sso-2",
    name: "Entra staging",
    slug: "entra",
    issuer: "https://login.microsoftonline.com/acme/v2.0",
    client_id: "b2c3d4e5",
    default_role: null,
    enabled: false,
  }),
];

const MAPPINGS: Record<string, SsoGroupMappingRow[]> = {
  "sso-1": [
    {
      id: "map-1",
      provider_id: "sso-1",
      group_name: "platform-engineering",
      org_id: ORG.id,
      team_id: null,
      project_id: null,
      role: "admin",
      created_at: NOW,
    },
    // #1234: a mapping the API can already write and the dashboard could not.
    // it grants inside one team, so the row has to say so — listed beside an
    // org-wide one, it would otherwise read as the same grant
    {
      id: "map-2",
      provider_id: "sso-1",
      group_name: "gateway-oncall",
      org_id: null,
      team_id: TEAM.id,
      project_id: null,
      role: "member",
      created_at: NOW,
    },
  ],
  "sso-2": [],
};

const POLICY: OrgAuthPolicy = {
  org_id: ORG.id,
  allow_password_login: true,
  allow_sso: true,
  mfa_policy: "off",
  mfa_enforce_after: null,
  updated_at: NOW,
};

// sign-in through an identity provider only: what makes the last enabled
// provider the last way in for everyone but a superadmin
const PASSWORDS_OFF: OrgAuthPolicy = { ...POLICY, allow_password_login: false };

// single sign-on already off: every provider of the org is refused at the
// callback, so turning it back on is what restores the way in
const SSO_OFF: OrgAuthPolicy = { ...POLICY, allow_sso: false };

/**
 * The screen's endpoints, routed by path.
 *
 * `/sso-providers/{id}/group-mappings` contains `sso-providers`, so the
 * mappings branch has to come first or the provider list answers it and every
 * card renders the provider array as its groups.
 */
function api({
  providers = () => PROVIDERS as unknown,
  policy = () => POLICY as unknown,
  memberships = () => [] as unknown,
  publicUrl = () => json(PUBLIC_URL),
  status = 200,
  putPolicy,
}: {
  providers?: () => unknown;
  policy?: () => unknown;
  /** answers the org's membership rows, one per grant, which the second-factor
   * confirmation counts */
  memberships?: () => unknown;
  /** answers `GET /api/v1/public-url`, which every signed-in caller may read,
   * so it keeps its own status rather than following `status` */
  publicUrl?: () => Response;
  status?: number;
  /** answers a policy save; by default it echoes the body back as saved */
  putPolicy?: () => Response;
} = {}): FetchStub {
  // a 204 carries no body at all — `new Response(body, { status: 204 })` throws
  const noContent = () => new Response(null, { status: 204 });
  return scoped(async (input, init) => {
    const url = String(input);
    const method = (init?.method ?? "GET").toUpperCase();
    if (url.includes("/api/v1/public-url")) return publicUrl();
    if (url.includes("/memberships")) return json(memberships(), status);
    if (url.includes("/group-mappings")) {
      if (method === "POST") return json(MAPPINGS["sso-1"][0], 201);
      const id = url.split("/sso-providers/")[1]?.split("/")[0] ?? "";
      return json(MAPPINGS[id] ?? [], status);
    }
    if (url.includes("/sso-group-mappings/")) return noContent();
    if (url.includes("/sso-providers")) {
      if (method === "POST") return json(provider({ id: "sso-new" }), 201);
      if (method === "DELETE") return noContent();
      // an update answers with the saved row, the way the control plane does,
      // so the screen re-renders from the server's version and not the draft
      if (method === "PUT") {
        const body = JSON.parse(String(init?.body)) as Record<string, unknown>;
        return json(provider({ ...body, id: "sso-1" }), 200);
      }
      return json(providers(), status);
    }
    if (url.includes("/auth-policy")) {
      if (method === "PUT" && putPolicy) return putPolicy();
      if (method === "PUT") {
        const next = JSON.parse(String(init?.body)) as Partial<OrgAuthPolicy>;
        return json({ ...POLICY, ...next }, 200);
      }
      return json(policy(), status);
    }
    return json([]);
  });
}

const meta = {
  title: "Screens/SingleSignOn",
  component: SingleSignOn,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof SingleSignOn>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={api()}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Acme Okta")).toBeVisible());

    // the card's addresses are the server's, on its configured public url
    await expect(canvas.getByText(`${PUBLIC_BASE}/auth/sso/okta/callback`)).toBeVisible();
    await expect(canvas.getByText(`${PUBLIC_BASE}/auth/sso/okta/start`)).toBeVisible();
    await expect(canvas.getByText("https://acme.okta.com")).toBeVisible();
    // and with the public url configured, nothing warns about it
    await expect(canvas.queryByText(/use the default address/)).toBeNull();

    // a group mapping is the thing that grants a role. its own request is
    // separate from the provider list, so it settles after the card is drawn
    await waitFor(() => expect(canvas.getByText("platform-engineering")).toBeVisible());

    // a provider with no default role refuses an unmapped user, and says so
    await expect(canvas.getByText(/No default role/)).toBeVisible();

    // the org policy is the same screen: both ways in are on here
    await expect(canvas.getByRole("switch", { name: "Password sign-in" })).toBeChecked();
    await expect(canvas.getByRole("switch", { name: "Single sign-on" })).toBeChecked();
  },
};

/**
 * #1231: a provider registered without a client secret — or one whose secret
 * was dropped because `ROLTER_KEK` was unset at the time — cannot complete the
 * token exchange. The list response says so with `has_client_secret`, so the
 * card can warn now instead of letting the first failed login be the signal.
 */
export const WarnsWhenNoClientSecretIsStored: Story = {
  render: () => (
    <Harness
      fetchStub={api({
        providers: () => [provider({ has_client_secret: false })],
      })}
    >
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("No client secret")).toBeVisible());
    await expect(canvas.getByText("Not set")).toBeVisible();
  },
};

/** The same card with a secret sealed: no warning, and the row reads "Stored". */
export const SaysWhenAClientSecretIsStored: Story = {
  render: () => (
    <Harness fetchStub={api({ providers: () => [provider()] })}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Stored")).toBeVisible());
    await expect(canvas.queryByText("No client secret")).toBeNull();
  },
};

/**
 * #2083: the card shows the redirect URI an identity provider asks for, next
 * to the login URL users follow, and each one says which it is.
 *
 * Before, the only copyable address was the login URL, so an admin looking for
 * "the URL to give the IdP" pasted that one where the callback belongs and the
 * IdP refused every sign-in with a redirect mismatch. Both come off the row,
 * built by the server from its configured public url, never from the
 * browser's own origin.
 */
export const ShowsTheRedirectUriToRegister: Story = {
  render: () => (
    <Harness fetchStub={api({ providers: () => [provider()] })}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const redirect = `${PUBLIC_BASE}/auth/sso/okta/callback`;
    const login = `${PUBLIC_BASE}/auth/sso/okta/start`;
    await waitFor(() => expect(canvas.getByText(redirect)).toBeVisible());

    // labelled: the redirect uri is what the identity provider is given, the
    // login url is what users follow
    await expect(canvas.getByText("Redirect URI")).toBeVisible();
    await expect(canvas.getByText("Register this in your identity provider.")).toBeVisible();
    await expect(canvas.getByText(login)).toBeVisible();
    await expect(canvas.getByText("What users follow to sign in.")).toBeVisible();

    // each copy button is named for the value it copies, so the two are never
    // mistaken for one another
    await expect(
      canvas.getByRole("button", { name: `Copy redirect URI: ${redirect}` }),
    ).toBeVisible();
    await expect(canvas.getByRole("button", { name: `Copy login URL: ${login}` })).toBeVisible();

    // neither was assembled from the page's own origin
    await expect(canvas.queryByText(new RegExp(window.location.origin))).toBeNull();
  },
};

/**
 * #2083: the identity provider wants the redirect URI before it issues the
 * client ID and secret this sheet asks for, so the add sheet builds it from the
 * slug as it is typed, on the public url the server reported.
 */
const previews = recording(api({ providers: () => [provider()] }));

export const PreviewsTheRedirectUriFromTheSlug: Story = {
  render: () => (
    <Harness fetchStub={previews.stub}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Add provider/);
    // the sheet animates in, so visibility is polled once rather than read (#2287)
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    const preview = await panel.findByRole("group", { name: "Redirect URI" });

    // nothing to build from yet, and the row says what would fill it
    await expect(within(preview).getByText("Type a slug to see the redirect URI.")).toBeVisible();
    await expect(preview).toHaveAccessibleDescription(/when you create the application/);

    // it follows the slug keystroke by keystroke, with no request per key
    const reads = () => previews.calls.filter((c) => c.url.includes("/public-url")).length;
    const before = reads();
    const slug = panel.getByLabelText("Slug");
    await userEvent.type(slug, "ok");
    await expect(within(preview).getByText(`${PUBLIC_BASE}/auth/sso/ok/callback`)).toBeVisible();
    await userEvent.type(slug, "ta");
    const uri = `${PUBLIC_BASE}/auth/sso/okta/callback`;
    await expect(within(preview).getByText(uri)).toBeVisible();
    await expect(
      within(preview).getByRole("button", { name: `Copy redirect URI: ${uri}` }),
    ).toBeVisible();
    await expect(reads()).toBe(before);

    // a configured public url needs no warning
    await expect(within(preview).queryByText(/ROLTER_PUBLIC_URL/)).toBeNull();

    // and clearing the slug takes the uri away again rather than leaving a
    // stale one to copy
    await userEvent.clear(slug);
    await expect(within(preview).getByText("Type a slug to see the redirect URI.")).toBeVisible();
    await expect(within(preview).queryByRole("button")).toBeNull();
  },
};

// the slug is registered at the identity provider as part of the redirect uri,
// and the database refuses anything outside `^[a-z0-9][a-z0-9-]{0,62}$`. the
// sheet states that rule up front and marks a slug outside it before anything
// is saved, instead of letting the admin copy a uri the server would refuse
// (#2304)
const SLUG_RULE = /Lowercase letters, digits and hyphens.*up to 63 characters/;
const SLUG_INVALID =
  "Use only lowercase letters, digits and hyphens, and start with a letter or digit.";
const REDIRECT_URI_INVALID =
  "The slug above is not valid, so there is no redirect URI to register yet.";

/** the fields other than the slug, so the slug is the only thing that can block Save */
async function fillTheRest(panel: ReturnType<typeof within>): Promise<void> {
  await userEvent.type(panel.getByLabelText("Name"), "Acme Okta");
  await userEvent.type(panel.getByLabelText("Issuer URL"), "https://acme.okta.com");
  await userEvent.type(panel.getByLabelText("Client ID"), "0oa1b2c3d4");
}

const refusals = recording(api({ providers: () => [provider()] }));

export const RefusesASlugOutsideTheCharset: Story = {
  render: () => (
    <Harness fetchStub={refusals.stub}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Add provider/);
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    await fillTheRest(panel);
    const slug = panel.getByLabelText("Slug");
    const save = panel.getByRole("button", { name: "Add provider" });
    const preview = await panel.findByRole("group", { name: "Redirect URI" });

    // the rule is stated before a character is typed, and nothing is marked yet
    await expect(slug).toHaveAccessibleDescription(SLUG_RULE);
    await expect(slug).not.toHaveAttribute("aria-invalid");

    await userEvent.type(slug, "Acme Okta");

    // what was typed stays exactly as typed: the slug ends up at the identity
    // provider, so the admin has to see what will be saved, not a rewrite of it
    await expect(slug).toHaveValue("Acme Okta");
    await expect(slug).toHaveAttribute("aria-invalid", "true");
    const reason = `${SLUG_INVALID} Try acme-okta.`;
    await expect(panel.getByText(reason)).toBeVisible();
    // the field announces the rule and the reason together
    await expect(slug).toHaveAccessibleDescription(/up to 63 characters.*Try acme-okta\./);

    // there is nothing to copy for a slug the server would refuse
    await expect(within(preview).getByText(REDIRECT_URI_INVALID)).toBeVisible();
    await expect(within(preview).queryByRole("button")).toBeNull();
    await expect(panel.queryByText(/\/auth\/sso\//)).toBeNull();

    // and Save is blocked until it is fixed, so no request leaves
    await expect(save).toBeDisabled();
    refusals.expectNotSent("POST", "/sso-providers");
  },
};

/**
 * Each way a slug can miss the rule is named, and a corrected value is offered
 * when one can be worked out. The boundary is the store's: 63 characters pass,
 * 64 do not.
 */
export const NamesWhatIsWrongWithTheSlug: Story = {
  render: () => (
    <Harness fetchStub={api({ providers: () => [provider()] })}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Add provider/);
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    const slug = panel.getByLabelText("Slug");
    const preview = await panel.findByRole("group", { name: "Redirect URI" });

    // a leading hyphen is refused, and the suggestion drops it
    await userEvent.type(slug, "-okta");
    await expect(panel.getByText(`${SLUG_INVALID} Try okta.`)).toBeVisible();

    // nothing Latin to fold down to: the rule is all there is to say
    await userEvent.clear(slug);
    await userEvent.type(slug, "日本語");
    await expect(panel.getByText(SLUG_INVALID)).toBeVisible();
    await expect(panel.queryByText(/Try /)).toBeNull();
    await expect(slug).toHaveAttribute("aria-invalid", "true");

    // 64 characters is one too many, and the message counts them
    await userEvent.clear(slug);
    await userEvent.click(slug);
    await userEvent.paste("a".repeat(64));
    await expect(
      panel.getByText("A slug has at most 63 characters. This one has 64."),
    ).toBeVisible();
    await expect(within(preview).getByText(REDIRECT_URI_INVALID)).toBeVisible();

    // 63 is the longest the store accepts: no error, and the uri is offered
    await userEvent.clear(slug);
    await userEvent.click(slug);
    const longest = "a".repeat(63);
    await userEvent.paste(longest);
    await expect(panel.queryByText(/at most 63 characters\. This one/)).toBeNull();
    await expect(slug).not.toHaveAttribute("aria-invalid");
    await expect(
      within(preview).getByText(`${PUBLIC_BASE}/auth/sso/${longest}/callback`),
    ).toBeVisible();
  },
};

/**
 * Correcting the slug clears the mark and brings the preview and Save back; the
 * request then carries the slug exactly as typed.
 */
const corrected = recording(api({ providers: () => [provider()] }));

export const SavesTheSlugOnceItIsCorrected: Story = {
  render: () => (
    <Harness fetchStub={corrected.stub}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Add provider/);
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    await fillTheRest(panel);
    const slug = panel.getByLabelText("Slug");
    const save = panel.getByRole("button", { name: "Add provider" });
    const preview = await panel.findByRole("group", { name: "Redirect URI" });

    await userEvent.type(slug, "Okta");
    await expect(panel.getByText(`${SLUG_INVALID} Try okta.`)).toBeVisible();
    await expect(save).toBeDisabled();

    await userEvent.clear(slug);
    await userEvent.type(slug, "acme-okta");
    const uri = `${PUBLIC_BASE}/auth/sso/acme-okta/callback`;
    await expect(panel.queryByText(/Use only lowercase letters/)).toBeNull();
    await expect(slug).not.toHaveAttribute("aria-invalid");
    // the hint stays once the error is gone, and is what the field describes itself with
    await expect(slug).toHaveAccessibleDescription(SLUG_RULE);
    await expect(within(preview).getByText(uri)).toBeVisible();
    await expect(
      within(preview).getByRole("button", { name: `Copy redirect URI: ${uri}` }),
    ).toBeVisible();

    await waitFor(() => expect(save).toBeEnabled());
    await userEvent.click(save);
    const created = await corrected.expectSentBody<Record<string, unknown>>(
      "POST",
      `/api/v1/orgs/${ORG.id}/sso-providers`,
    );
    await expect(created.slug).toBe("acme-okta");
    await expect(created.name).toBe("Acme Okta");
  },
};

/**
 * The server's own 400 is shown in the sheet, with what was typed kept.
 *
 * The dashboard's copy of the rule can lag the control plane it talks to, so a
 * slug it lets through can still be refused. The message is the server's, which
 * states the rule, and the sheet stays open so nothing has to be typed again.
 */
export const ShowsTheServersRefusalOfASlug: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        (init?.method ?? "GET").toUpperCase() === "POST" && String(input).includes("/sso-providers")
          ? json(
              {
                error: {
                  message:
                    "config error: slug must be lowercase letters, digits and hyphens, start with a letter or digit, and be at most 63 characters",
                },
              },
              400,
            )
          : api({ providers: () => [provider()] })(input, init),
      )}
    >
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Add provider/);
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    await fillTheRest(panel);
    await userEvent.type(panel.getByLabelText("Slug"), "okta");
    await userEvent.click(panel.getByRole("button", { name: "Add provider" }));

    await waitFor(() =>
      expect(panel.getByRole("alert")).toHaveTextContent(
        /slug must be lowercase letters, digits and hyphens.*at most 63 characters/,
      ),
    );
    await expect(panel.getByLabelText("Slug")).toHaveValue("okta");
    await expect(panel.getByLabelText("Issuer URL")).toHaveValue("https://acme.okta.com");
  },
};

/**
 * The reason and the preview's note on a phone, in Russian, the longest copy
 * the sheet carries for this. Both read in Russian and stay inside the sheet's
 * width instead of running off it.
 */
export const ExplainsAnInvalidSlugInRussianOnAPhone: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => (
    <Harness fetchStub={api({ providers: () => [provider()] })}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const copy = ru.pages.sso.create;
    await clickWhenEnabled(canvasElement, new RegExp(ru.pages.sso.providers.add));
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    await userEvent.type(panel.getByLabelText(copy.slug), "Acme Okta");

    const reason = await panel.findByText(copy.slugSuggest.replace("{{suggestion}}", "acme-okta"));
    const note = panel.getByText(copy.redirectUriInvalid);
    // the sheet slides in from the edge, so it is measured once it has come to rest
    await waitFor(() =>
      expect(sheet().getBoundingClientRect().right).toBeLessThanOrEqual(window.innerWidth),
    );
    for (const text of [reason, note]) {
      await expect(text).toBeVisible();
      const box = text.getBoundingClientRect();
      await expect(box.left).toBeGreaterThanOrEqual(0);
      await expect(box.right).toBeLessThanOrEqual(window.innerWidth);
      await expect(text.scrollWidth).toBeLessThanOrEqual(text.clientWidth);
    }
    await expect(panel.getByLabelText(copy.slug)).toHaveAttribute("aria-invalid", "true");
  },
};

/**
 * #2083: with `ROLTER_PUBLIC_URL` unset the control plane builds every address
 * from its default, which an identity provider can only send a browser back to
 * on the control plane's own host. The screen says so once above the list, and
 * the sheet says so beside the preview, naming the address it will use.
 */
export const WarnsWhenThePublicUrlIsUnset: Story = {
  render: () => (
    <Harness
      fetchStub={api({
        publicUrl: () => json(UNSET),
        providers: () => [
          provider({
            redirect_uri: `${DEFAULT_BASE}/auth/sso/okta/callback`,
            login_url: `${DEFAULT_BASE}/auth/sso/okta/start`,
          }),
        ],
      })}
    >
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const notice = await canvas.findByRole("note");
    await expect(
      within(notice).getByText("The redirect and login URLs use the default address"),
    ).toBeVisible();
    await expect(notice).toHaveTextContent(`ROLTER_PUBLIC_URL is not set`);
    await expect(notice).toHaveTextContent(DEFAULT_BASE);
    // the card still shows what the server built, default and all
    await expect(canvas.getByText(`${DEFAULT_BASE}/auth/sso/okta/callback`)).toBeVisible();

    await clickWhenEnabled(canvasElement, /Add provider/);
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    const preview = await panel.findByRole("group", { name: "Redirect URI" });
    await userEvent.type(panel.getByLabelText("Slug"), "entra");
    await expect(
      within(preview).getByText(`${DEFAULT_BASE}/auth/sso/entra/callback`),
    ).toBeVisible();
    await expect(within(preview).getByText(/uses the default address/)).toBeVisible();
  },
};

/**
 * The public url could not be read. The sheet shows the path it can vouch for
 * and says what goes in front of it, rather than guessing a host from the
 * browser; the provider cards are unaffected, since each row carries its own.
 */
export const PreviewsOnlyThePathWhenThePublicUrlIsUnreadable: Story = {
  render: () => (
    <Harness
      fetchStub={api({
        publicUrl: () => json({ error: { message: "upstream unavailable" } }, 502),
        providers: () => [provider()],
      })}
    >
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText(`${PUBLIC_BASE}/auth/sso/okta/callback`)).toBeVisible(),
    );
    // an unread public url is not an unset one
    await expect(canvas.queryByRole("note")).toBeNull();

    await clickWhenEnabled(canvasElement, /Add provider/);
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    const preview = await panel.findByRole("group", { name: "Redirect URI" });
    await userEvent.type(panel.getByLabelText("Slug"), "okta");
    await expect(within(preview).getByText("/auth/sso/okta/callback")).toBeVisible();
    await expect(within(preview).getByText(/only the path is shown/)).toBeVisible();
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // skeletons only: nothing claims the org has no provider before the answer
    // has arrived
    await expect(canvas.queryByRole("button", { name: /Add provider/ })).not.toBeInTheDocument();
    await expect(canvas.queryByText(/No identity provider yet/)).toBeNull();
  },
};

// the state every deployment starts in: the control plane has carried OIDC
// since #240 and nobody has registered a provider yet
export const Empty: Story = {
  render: () => (
    <Harness fetchStub={api({ providers: () => [] })}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("No identity provider yet")).toBeVisible());
    // the empty state says what SSO buys them and carries the action
    await expect(canvas.getByText(/company account they already have/)).toBeVisible();
    await expect(canvas.getAllByRole("button", { name: /Add provider/ })).toHaveLength(2);
  },
};

// every endpoint here is org-admin gated (`sso_provider`, `org_auth_policy`),
// so a member gets 403 — a permission, not a failure, and retrying will not fix
// it
export const Forbidden: Story = {
  render: () => (
    <Harness
      fetchStub={api({
        providers: () => ({ error: { message: "forbidden" } }),
        policy: () => ({ error: { message: "forbidden" } }),
        // readable by anyone signed in, and unset here, so the notice below
        // is withheld by the refused list and not by a missing answer
        publicUrl: () => json(UNSET),
        status: 403,
      })}
    >
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText("You do not have access to identity providers")).toBeVisible(),
    );
    await expect(canvas.getByText("You do not have access to the sign-in policy")).toBeVisible();
    // a 403 gets no retry button, and nothing to press that would 403 again
    await expect(canvas.queryByRole("button", { name: /Try again/ })).toBeNull();
    await expect(canvas.getByRole("button", { name: /Add provider/ })).toBeDisabled();
    await expectNoFalseEmpty(canvasElement, /No identity provider yet/);
    // a caller refused the providers has no URL here to be warned about
    await expect(canvas.queryByText(/use the default address/)).toBeNull();
  },
};

// the create body is the assertion: the client secret is write-only and never
// comes back, so a screen that dropped it would look like it worked
const creates = recording(api({ providers: () => [provider()] }));

export const CreatesAProvider: Story = {
  render: () => (
    <Harness fetchStub={creates.stub}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Add provider/);

    const panel = within(sheet());
    await userEvent.type(panel.getByLabelText("Name"), "Acme Okta");
    await userEvent.type(panel.getByLabelText("Slug"), "okta");
    await userEvent.type(panel.getByLabelText("Issuer URL"), "https://acme.okta.com");
    await userEvent.type(panel.getByLabelText("Client ID"), "0oa1b2c3d4");
    await userEvent.type(panel.getByLabelText("Client secret"), "s3cr3t");

    // and the sheet is unambiguous that this is the only sighting of it
    await expect(panel.getByText(/never shown again/)).toBeVisible();

    await userEvent.click(panel.getByRole("button", { name: "Add provider" }));

    const created = await creates.expectSentBody("POST", `/api/v1/orgs/${ORG.id}/sso-providers`);
    await expect(created).toEqual({
      name: "Acme Okta",
      slug: "okta",
      issuer: "https://acme.okta.com",
      client_id: "0oa1b2c3d4",
      client_secret: "s3cr3t",
      // omitted optionals are left out entirely, so the server applies its own
      // defaults rather than being handed an empty string
    });
  },
};

/**
 * The provider is refused (#1607).
 *
 * The client secret is typed once and never comes back from the server, so a
 * sheet that closed on a rejected save would make the operator fetch it from
 * the identity provider again. It stays, and the refusal is announced.
 */
export const CreateRejectedByTheServer: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        (init?.method ?? "GET").toUpperCase() === "POST" && String(input).includes("/sso-providers")
          ? json({ error: { message: "the issuer did not answer its discovery document" } }, 502)
          : api({ providers: () => [provider()] })(input, init),
      )}
    >
      <Toasted>
        <SingleSignOn />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Add provider/);

    const panel = within(sheet());
    await userEvent.type(panel.getByLabelText("Name"), "Acme Okta");
    await userEvent.type(panel.getByLabelText("Slug"), "okta");
    await userEvent.type(panel.getByLabelText("Issuer URL"), "https://acme.okta.com");
    await userEvent.type(panel.getByLabelText("Client ID"), "0oa1b2c3d4");
    await userEvent.type(panel.getByLabelText("Client secret"), "s3cr3t");
    await userEvent.click(panel.getByRole("button", { name: "Add provider" }));

    await expectToast(canvasElement, /discovery document/, "error");
    await waitFor(() => expect(within(document.body).getByRole("dialog")).toBeInTheDocument());
    await expect(panel.getByLabelText("Client secret")).toHaveValue("s3cr3t");
    await expect(panel.getByLabelText("Issuer URL")).toHaveValue("https://acme.okta.com");
  },
};

// #1233: editing in place. before this, rotating a secret or fixing a typo
// meant deleting the provider and registering it again, which dropped every
// group mapping and changed the id in the audit trail
const edits = recording(api({ providers: () => [provider()] }));

export const EditsAProviderInPlace: Story = {
  render: () => (
    <Harness fetchStub={edits.stub}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Edit provider Acme Okta/);

    const panel = within(sheet());
    // the slug is in the login url, so it is shown but not editable
    await expect(panel.getByLabelText("Slug")).toBeDisabled();
    await expect(panel.getByText(/cannot be changed/)).toBeVisible();
    // and it is not held to the charset rule again: it was accepted once
    await expect(panel.getByLabelText("Slug")).not.toHaveAttribute("aria-invalid");
    // and the redirect uri beside it is the saved provider's own, from the row
    const preview = await panel.findByRole("group", { name: "Redirect URI" });
    await waitFor(() =>
      expect(within(preview).getByText(`${PUBLIC_BASE}/auth/sso/okta/callback`)).toBeVisible(),
    );

    // the sealed secret is not readable, so the field starts empty and an
    // empty field must mean "keep", never "clear"
    await expect(panel.getByLabelText("Client secret")).toHaveValue("");
    await expect(panel.getByText(/Leave empty to keep/)).toBeVisible();

    await userEvent.clear(panel.getByLabelText("Client ID"));
    await userEvent.type(panel.getByLabelText("Client ID"), "0oa-rotated");
    await userEvent.click(panel.getByRole("button", { name: "Save changes" }));

    const body = await edits.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/api/v1/sso-providers/sso-1",
    );
    await expect(body.client_id).toBe("0oa-rotated");
    await expect(body.name).toBe("Acme Okta");
    // the untouched secret field sends nothing at all, which is what tells the
    // server to leave the sealed one alone
    await expect("client_secret" in body).toBe(false);
    // and the slug is never sent, so it cannot be changed by accident
    await expect("slug" in body).toBe(false);
  },
};

// #1293: a stray space in the secret field used to trim to "" and reach the
// server as the wire's "clear it", so the provider silently lost its sealed
// secret and the next login failed at token exchange. whitespace alone is now
// the same as untouched: nothing is sent
const whitespace = recording(api({ providers: () => [provider()] }));

export const TreatsAWhitespaceOnlySecretAsUntouched: Story = {
  render: () => (
    <Harness fetchStub={whitespace.stub}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Edit provider Acme Okta/);

    const panel = within(sheet());
    await userEvent.type(panel.getByLabelText("Client secret"), "   ");
    await userEvent.click(panel.getByRole("button", { name: "Save changes" }));

    const body = await whitespace.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/api/v1/sso-providers/sso-1",
    );
    // the assertion that matters: not an empty string, not present at all
    await expect("client_secret" in body).toBe(false);
    await expect(body.client_id).toBe("0oa1b2c3d4");
  },
};

/**
 * #1293: clearing is its own control now, and it is the only thing that sends
 * the empty string.
 *
 * The stub models the server rather than a fixture: the PUT flips the stored
 * flag, so the badge the card draws afterwards comes from a refetched list and
 * not from the story asserting on its own constant.
 */
function clearingApi(): FetchStub {
  let stored = true;
  const row = () => provider({ has_client_secret: stored });
  return scoped(async (input, init) => {
    const url = String(input);
    const method = (init?.method ?? "GET").toUpperCase();
    if (url.includes("/group-mappings")) return json([], 200);
    if (url.includes("/sso-providers")) {
      if (method === "PUT") {
        const body = JSON.parse(String(init?.body)) as Record<string, unknown>;
        if (body.client_secret === "") stored = false;
        return json(row(), 200);
      }
      return json([row()], 200);
    }
    if (url.includes("/auth-policy")) return json(POLICY, 200);
    return json([]);
  });
}

const clears = recording(clearingApi());

export const RemovesTheStoredSecretWithConfirmation: Story = {
  render: () => (
    <Harness fetchStub={clears.stub}>
      <Toasted>
        <SingleSignOn />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Stored")).toBeVisible());

    // cancelling sends nothing: a secret that cannot be read back must not be
    // droppable by one stray click
    await userEvent.click(canvas.getByLabelText("Remove the stored client secret for Acme Okta"));
    await cancelConfirmation();
    clears.expectNotSent("PUT", "/api/v1/sso-providers/sso-1");

    await userEvent.click(canvas.getByLabelText("Remove the stored client secret for Acme Okta"));
    await confirmDestructive(/Acme Okta/, "Remove secret");

    const body = await clears.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/api/v1/sso-providers/sso-1",
    );
    // the empty string is the wire's third value, and only this path sends it
    await expect(body.client_secret).toBe("");
    // the rest of the row rides along unchanged
    await expect(body.name).toBe("Acme Okta");
    await expect(body.enabled).toBe(true);

    // and the badge flips off the refetched list
    await waitFor(() => expect(canvas.getByText("No client secret")).toBeVisible());
    await expect(canvas.getByText("Not set")).toBeVisible();
    // with nothing stored, the control that removes one is gone
    await expect(
      canvas.queryByLabelText("Remove the stored client secret for Acme Okta"),
    ).toBeNull();
  },
};

// a provider is taken out of service with a switch instead of a delete, and the
// switch confirms first: people signing in through it lose that route at once
const toggles = recording(api({ providers: () => [provider()] }));

export const DisablesAProviderWithoutDeletingIt: Story = {
  render: () => (
    <Harness fetchStub={toggles.stub}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("switch", { name: "Enable provider Acme Okta" }));

    // it names the provider and says what stays. with password sign-in on
    // nobody is locked out, so nothing warns of it
    const dialogElement = await confirmation();
    await waitFor(() => expect(dialogElement).toBeVisible());
    const dialog = within(dialogElement);
    await expect(
      dialog.getByRole("heading", { name: "Take Acme Okta out of service?" }),
    ).toBeInTheDocument();
    await expect(dialog.getByText(/group mappings stay/)).toBeInTheDocument();
    await expect(dialog.queryByRole("note")).toBeNull();

    // backing out sends nothing, and the switch is still on
    await cancelConfirmation();
    toggles.expectNotSent("PUT", "/api/v1/sso-providers/sso-1");
    await expect(canvas.getByRole("switch", { name: "Enable provider Acme Okta" })).toBeChecked();

    await userEvent.click(canvas.getByRole("switch", { name: "Enable provider Acme Okta" }));
    await confirmDestructive(/Take Acme Okta out of service/, "Take out of service");

    const body = await toggles.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/api/v1/sso-providers/sso-1",
    );
    await expect(body.enabled).toBe(false);
    // everything else rides along unchanged, and the secret is untouched
    await expect(body.name).toBe("Acme Okta");
    await expect("client_secret" in body).toBe(false);
    // nothing was deleted: the group mappings that hang off this provider are
    // exactly what delete-and-recreate used to destroy
    toggles.expectNotSent("DELETE", "/sso-providers");
    await expectSheetClosed();
  },
};

// turning a provider back on restores a route, so it sends at once: a dialog
// in front of it would only slow down the way back in
const enables = recording(api({ providers: () => [provider({ enabled: false })] }));

export const EnablingAProviderDoesNotConfirm: Story = {
  render: () => (
    <Harness fetchStub={enables.stub}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("switch", { name: "Enable provider Acme Okta" }));
    const body = await enables.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/api/v1/sso-providers/sso-1",
    );
    await expect(body.enabled).toBe(true);
    await expect(within(document.body).queryByRole("dialog")).not.toBeInTheDocument();
  },
};

// the control plane refuses the update: the dialog stays open with its words,
// beside the button that caused it, and the provider stays on
export const DisableRefusedByTheServerStaysInTheDialog: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        (init?.method ?? "GET").toUpperCase() === "PUT" && String(input).includes("/sso-providers/")
          ? json({ error: { message: "the provider could not be updated" } }, 422)
          : api({ providers: () => [provider()] })(input, init),
      )}
    >
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("switch", { name: "Enable provider Acme Okta" }));
    const dialog = within(await confirmation());
    await userEvent.click(dialog.getByRole("button", { name: "Take out of service" }));
    await waitFor(() =>
      expect(dialog.getByRole("alert")).toHaveTextContent("the provider could not be updated"),
    );
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
    await expect(canvas.getByRole("switch", { name: "Enable provider Acme Okta" })).toBeChecked();
  },
};

// deleting a provider takes a whole sign-in route away, so it is named and
// confirmed before anything leaves (#1179)
const deletes = recording(api({ providers: () => [provider()] }));

export const DeletesWithConfirmation: Story = {
  render: () => (
    <Harness fetchStub={deletes.stub}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Acme Okta")).toBeVisible());

    await userEvent.click(canvas.getByLabelText("Delete provider Acme Okta"));
    // password sign-in is on, so the last provider going is no lockout and the
    // dialog carries no warning of one
    await expect(within(await confirmation()).queryByRole("note")).toBeNull();
    await cancelConfirmation();
    deletes.expectNotSent("DELETE", "/sso-providers/sso-1");

    await userEvent.click(canvas.getByLabelText("Delete provider Acme Okta"));
    await confirmDestructive(/Acme Okta/, "Delete provider");
    await deletes.expectSent("DELETE", "/sso-providers/sso-1");
  },
};

// --- a change that would shut members out (#2084, #2443) -------------------

const REASON =
  "Disabling or deleting this provider is unavailable: it is the last enabled one and password sign-in is off";

// password sign-in off and one enabled provider: the control plane refuses to
// take it away (409), so the card holds the controls back instead of asking
const lastOff = recording(api({ providers: () => [provider()], policy: () => PASSWORDS_OFF }));

/**
 * The last enabled provider with password sign-in off cannot be disabled or
 * deleted: both controls are disabled and a reason beside them says what to do
 * instead. Nothing is confirmed and nothing is sent.
 */
export const TheLastProviderCannotBeDisabledOrDeleted: Story = {
  render: () => (
    <Harness fetchStub={lastOff.stub}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const toggle = await canvas.findByRole("switch", { name: "Enable provider Acme Okta" });
    const remove = canvas.getByLabelText("Delete provider Acme Okta");
    // story-wait-allow: disabled by the card's own prop from the first paint, not by the gate
    await expect(toggle).toBeDisabled();
    // story-wait-allow: disabled by the card's own prop from the first paint, not by the gate
    await expect(remove).toBeDisabled();
    await expect(toggle).toHaveAttribute("title", expect.stringContaining(REASON));
    await expect(remove).toHaveAttribute("title", expect.stringContaining(REASON));

    const reason = canvas.getByText(/Enable password sign-in or another provider first/);
    await expect(reason).toBeVisible();
    await expect(toggle).toHaveAccessibleDescription(reason.textContent ?? "");
    await expect(remove).toHaveAccessibleDescription(reason.textContent ?? "");

    // disabled controls open no dialog and send nothing
    await userEvent.click(toggle);
    await userEvent.click(remove);
    await expect(within(document.body).queryByRole("alertdialog")).toBeNull();
    lastOff.expectNotSent("PUT", "/api/v1/sso-providers/sso-1");
    lastOff.expectNotSent("DELETE", "/sso-providers/sso-1");
  },
};

// the control plane answers 409 anyway: another admin turned the other
// provider off after this screen read the list
const raced = recording(
  scoped(async (input, init) => {
    const method = (init?.method ?? "GET").toUpperCase();
    if (method !== "GET" && String(input).includes("/sso-providers/")) {
      return json(
        {
          error: {
            message:
              "cannot disable the last enabled sso provider while password sign-in is off: no member could sign in. Enable password sign-in or another sso provider first",
          },
        },
        409,
      );
    }
    return api({
      providers: () => [
        provider(),
        provider({ id: "sso-2", name: "Entra staging", slug: "entra" }),
      ],
      policy: () => PASSWORDS_OFF,
    })(input, init);
  }),
);

/** A 409 from the server is shown in the dashboard's words, not the raw message. */
export const AServerRefusalIsShownInTheDashboardsWords: Story = {
  render: () => (
    <Harness fetchStub={raced.stub}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("switch", { name: "Enable provider Acme Okta" }));
    await confirmDestructive(/Take Acme Okta out of service/, "Take out of service");
    await raced.expectSent("PUT", "/api/v1/sso-providers/sso-1");

    const dialog = within(await confirmation());
    await expect(await dialog.findByText(/the control plane refused this one/)).toBeInTheDocument();
    await expect(dialog.queryByText(/cannot disable the last enabled sso provider/)).toBeNull();
    await cancelConfirmation();

    await userEvent.click(canvas.getByLabelText("Delete provider Acme Okta"));
    await confirmDestructive(/Delete provider Acme Okta\?/, "Delete provider");
    await expect(
      await within(await confirmation()).findByText(/the control plane refused this one/),
    ).toBeInTheDocument();
  },
};

// password sign-in off, but two providers enabled: either one can go
const oneOfTwo = recording(
  api({
    providers: () => [provider(), provider({ id: "sso-2", name: "Entra staging", slug: "entra" })],
    policy: () => PASSWORDS_OFF,
  }),
);

/**
 * The warning is for the last way in, not for every change while passwords are
 * off. With another provider enabled, members still have a route, and a
 * warning here would be the click-through the last one depends on being read.
 */
export const AnotherEnabledProviderLeavesTheControlsOn: Story = {
  render: () => (
    <Harness fetchStub={oneOfTwo.stub}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("switch", { name: "Enable provider Acme Okta" }));
    await expect(
      within(await confirmation()).getByRole("heading", { name: "Take Acme Okta out of service?" }),
    ).toBeInTheDocument();
    await expect(within(await confirmation()).queryByRole("note")).toBeNull();
    await cancelConfirmation();

    await userEvent.click(canvas.getByLabelText("Delete provider Entra staging"));
    await expect(
      within(await confirmation()).getByRole("heading", { name: "Delete provider Entra staging?" }),
    ).toBeInTheDocument();
    await expect(within(await confirmation()).queryByRole("note")).toBeNull();
    await cancelConfirmation();
    oneOfTwo.expectNotSent("PUT", "/sso-providers/");
    oneOfTwo.expectNotSent("DELETE", "/sso-providers/");
  },
};

const passwordOff = recording(api({ providers: () => [provider()] }));

/**
 * Turning password sign-in off confirms before it saves: from then on every
 * member but a superadmin signs in through a provider. Backing out sends
 * nothing; confirming sends the same body a plain save does.
 */
export const TurningPasswordSignInOffConfirmsFirst: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness fetchStub={passwordOff.stub}>
      <UxScreenProvider screen="sso">
        <SingleSignOn />
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("switch", { name: "Password sign-in" }));
    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));

    const dialogElement = await confirmation();
    await waitFor(() => expect(dialogElement).toBeVisible());
    const dialog = within(dialogElement);
    await expect(
      dialog.getByRole("heading", { name: "Turn off password sign-in?" }),
    ).toBeInTheDocument();
    await expect(dialogElement).toHaveTextContent(
      "Superadmins are exempt from this setting and can still sign in with a password",
    );
    // the one provider has a secret stored, so there is nothing to warn about
    await expect(dialog.queryByRole("note")).toBeNull();

    await userEvent.click(dialog.getByRole("button", { name: "Cancel" }));
    await expectSheetClosed();
    passwordOff.expectNotSent("PUT", "/auth-policy");
    expectNoUxEvent("form_submit", "sso-password-off");

    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));
    await confirmDestructive(/Turn off password sign-in/, "Turn it off");
    await expect(
      await passwordOff.expectSentBody("PUT", `/api/v1/orgs/${ORG.id}/auth-policy`),
    ).toEqual({
      allow_password_login: false,
      allow_sso: true,
      mfa_policy: "off",
      mfa_enforce_after: null,
    });
    await expectUxEvent("form_submit", "sso-password-off");
    await expectUxEvent("save_confirmed", "sso-password-off");
  },
};

const noSecretOff = recording(api({ providers: () => [provider({ has_client_secret: false })] }));

/**
 * The control plane refuses passwords-off with no enabled provider, and accepts
 * it when the only one that is enabled carries "No client secret". The
 * confirmation names it, and says that without a secret nobody could finish a
 * sign-in. It is a warning, not a block: a public client has none on purpose.
 */
export const PasswordsOffWarnsWhenTheOnlyProviderHasNoSecret: Story = {
  render: () => (
    <Harness fetchStub={noSecretOff.stub}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("switch", { name: "Password sign-in" }));
    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));

    const dialogElement = await confirmation();
    await waitFor(() => expect(dialogElement).toBeVisible());
    const notice = within(dialogElement).getByRole("note");
    await expect(notice).toHaveTextContent("1 enabled provider has no client secret");
    await expect(notice).toHaveTextContent("Acme Okta");
    await expect(within(notice).getByText("okta")).toBeInTheDocument();
    await expect(notice).toHaveTextContent(
      "If an identity provider expects a secret, sign-ins through it fail at the token exchange",
    );
    await expect(notice).toHaveTextContent("Every enabled provider is on this list");
    await expect(notice).toHaveTextContent("Superadmins could still sign in with a password");

    // the warning does not hold the save back
    await userEvent.click(within(dialogElement).getByRole("button", { name: "Turn it off" }));
    const body = await noSecretOff.expectSentBody<Record<string, unknown>>(
      "PUT",
      `/api/v1/orgs/${ORG.id}/auth-policy`,
    );
    await expect(body.allow_password_login).toBe(false);
  },
};

const someNoSecret = recording(
  api({
    providers: () => [
      provider(),
      provider({ id: "sso-2", name: "Entra staging", slug: "entra", has_client_secret: false }),
      // out of service and without a secret: not a route anybody has
      provider({
        id: "sso-3",
        name: "Legacy SAML bridge",
        slug: "legacy",
        enabled: false,
        has_client_secret: false,
      }),
    ],
  }),
);

/**
 * With another enabled provider that has its secret, members still have a
 * working route. The notice lists only the enabled provider without one, and
 * does not claim nobody could sign in.
 */
export const PasswordsOffNamesOnlyTheEnabledProvidersWithoutASecret: Story = {
  render: () => (
    <Harness fetchStub={someNoSecret.stub}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("switch", { name: "Password sign-in" }));
    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));

    const dialogElement = await confirmation();
    await waitFor(() => expect(dialogElement).toBeVisible());
    const notice = within(dialogElement).getByRole("note");
    await expect(notice).toHaveTextContent("1 enabled provider has no client secret");
    await expect(notice).toHaveTextContent("Entra staging");
    await expect(notice).not.toHaveTextContent("Acme Okta");
    await expect(notice).not.toHaveTextContent("Legacy SAML bridge");
    await expect(notice).not.toHaveTextContent("Every enabled provider is on this list");
    await cancelConfirmation();
    someNoSecret.expectNotSent("PUT", "/auth-policy");
  },
};

const alreadyOff = recording(api({ providers: () => [provider()], policy: () => PASSWORDS_OFF }));

/**
 * Only the switch going from on to off confirms. With passwords already off, a
 * save that changes something else takes nobody's route away.
 */
export const SavingOtherFieldsWithPasswordsAlreadyOffDoesNotConfirm: Story = {
  render: () => (
    <Harness fetchStub={alreadyOff.stub}>
      <Toasted>
        <SingleSignOn />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await pickOption(await canvas.findByLabelText("Second factor"), "Optional");
    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));
    await expect(
      await alreadyOff.expectSentBody("PUT", `/api/v1/orgs/${ORG.id}/auth-policy`),
    ).toEqual({
      allow_password_login: false,
      allow_sso: true,
      mfa_policy: "optional",
      mfa_enforce_after: null,
    });
    await expect(within(document.body).queryByRole("dialog")).not.toBeInTheDocument();
    await expectToast(canvasElement, /the sign-in policy updated/i);
  },
};

const both = recording(api({ providers: () => [provider()] }));

/**
 * One save that turns passwords off and requires a second factor raises both
 * confirmations, the password one first, and sends a single request after the
 * second.
 */
export const PasswordsOffAndASecondFactorAreConfirmedInTurn: Story = {
  render: () => (
    <Harness fetchStub={both.stub}>
      <Toasted>
        <SingleSignOn />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("switch", { name: "Password sign-in" }));
    await pickOption(canvas.getByLabelText("Second factor"), "Required for everyone");
    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));

    await confirmDestructive(/Turn off password sign-in/, "Turn it off");
    // the first answer only moves on: nothing is sent until the second
    await waitFor(async () =>
      expect(
        within(await confirmation()).getByRole("heading", {
          name: "Require a second factor to sign in?",
        }),
      ).toBeInTheDocument(),
    );
    both.expectNotSent("PUT", "/auth-policy");

    await confirmDestructive(/before they get a session/, "Require it");
    await expect(await both.expectSentBody("PUT", `/api/v1/orgs/${ORG.id}/auth-policy`)).toEqual({
      allow_password_login: false,
      allow_sso: true,
      mfa_policy: "required_all",
      mfa_enforce_after: null,
    });
    await expectToast(canvasElement, /the sign-in policy updated/i);
  },
};

const ssoOff = recording(api({ providers: () => [provider()] }));

/**
 * Turning single sign-on off while a provider is enabled confirms first and says
 * who it shuts out (#2326). An account a provider created has no password, so
 * password sign-in being on does not bring those members back; an account that
 * holds one keeps signing in. Backing out sends nothing, and confirming sends
 * the same body a plain save does.
 */
export const TurningSingleSignOnOffConfirmsFirst: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness fetchStub={ssoOff.stub}>
      <UxScreenProvider screen="sso">
        <SingleSignOn />
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("switch", { name: "Single sign-on" }));
    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));

    const dialogElement = await confirmation();
    // the dialog animates in, so visibility is polled rather than read once (#2287)
    await waitFor(() => expect(dialogElement).toBeVisible());
    const dialog = within(dialogElement);
    await expect(
      dialog.getByRole("heading", { name: "Turn off single sign-on?" }),
    ).toBeInTheDocument();
    await expect(dialogElement).toHaveTextContent(
      "provider buttons disappear from the sign-in screen and new sign-ins through its identity providers are refused at once",
    );
    await expect(dialogElement).toHaveTextContent(
      "already under way when they return from the provider",
    );
    const notice = dialog.getByRole("note");
    await expect(notice).toHaveTextContent(
      "Members who only sign in through a provider would be locked out",
    );
    await expect(notice).toHaveTextContent(
      "Accounts created through a provider have no password, so they cannot sign in until single sign-on is back on or a superadmin sets one",
    );
    // and who still gets in
    await expect(notice).toHaveTextContent(
      "Accounts that have a password, such as those created from an invitation, keep signing in with it",
    );

    // backing out sends nothing and is recorded as a cancel, not a decision
    await userEvent.click(dialog.getByRole("button", { name: "Cancel" }));
    await expectSheetClosed();
    ssoOff.expectNotSent("PUT", "/auth-policy");
    const abandon = await expectUxEvent("form_abandon", "sso-single-sign-on-off");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "sso-single-sign-on-off");

    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));
    await confirmDestructive(/Turn off single sign-on/, "Turn it off");
    await expect(await ssoOff.expectSentBody("PUT", `/api/v1/orgs/${ORG.id}/auth-policy`)).toEqual({
      allow_password_login: true,
      allow_sso: false,
      mfa_policy: "off",
      mfa_enforce_after: null,
    });
    await expectUxEvent("form_submit", "sso-single-sign-on-off");
    await expectUxEvent("save_confirmed", "sso-single-sign-on-off");
  },
};

/**
 * The play shared by the two cases with nobody to shut out: with no enabled
 * provider nobody signs in through one, so switching single sign-on off takes
 * nobody's route away and the save goes straight out.
 */
const savesSingleSignOnOffAtOnce =
  (sent: Recorder): NonNullable<Story["play"]> =>
  async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("switch", { name: "Single sign-on" }));
    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));
    await expect(await sent.expectSentBody("PUT", `/api/v1/orgs/${ORG.id}/auth-policy`)).toEqual({
      allow_password_login: true,
      allow_sso: false,
      mfa_policy: "off",
      mfa_enforce_after: null,
    });
    await expect(within(document.body).queryByRole("dialog")).not.toBeInTheDocument();
    await expectToast(canvasElement, /the sign-in policy updated/i);
  };

// a provider that is out of service has no `/start`, so it is not a route either
const ssoOffParked = recording(api({ providers: () => [provider({ enabled: false })] }));

export const TurningSingleSignOnOffWithOnlyParkedProvidersDoesNotConfirm: Story = {
  render: () => (
    <Harness fetchStub={ssoOffParked.stub}>
      <Toasted>
        <SingleSignOn />
      </Toasted>
    </Harness>
  ),
  play: savesSingleSignOnOffAtOnce(ssoOffParked),
};

const ssoOffBare = recording(api({ providers: () => [] }));

export const TurningSingleSignOnOffWithNoProviderDoesNotConfirm: Story = {
  render: () => (
    <Harness fetchStub={ssoOffBare.stub}>
      <Toasted>
        <SingleSignOn />
      </Toasted>
    </Harness>
  ),
  play: savesSingleSignOnOffAtOnce(ssoOffBare),
};

const ssoOn = recording(api({ providers: () => [provider()], policy: () => SSO_OFF }));

/** Turning it back on restores a route, so it saves at once. */
export const TurningSingleSignOnOnDoesNotConfirm: Story = {
  render: () => (
    <Harness fetchStub={ssoOn.stub}>
      <Toasted>
        <SingleSignOn />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const sso = await canvas.findByRole("switch", { name: "Single sign-on" });
    await expect(sso).not.toBeChecked();
    await userEvent.click(sso);
    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));
    await expect(await ssoOn.expectSentBody("PUT", `/api/v1/orgs/${ORG.id}/auth-policy`)).toEqual({
      allow_password_login: true,
      allow_sso: true,
      mfa_policy: "off",
      mfa_enforce_after: null,
    });
    await expect(within(document.body).queryByRole("dialog")).not.toBeInTheDocument();
    await expectToast(canvasElement, /the sign-in policy updated/i);
  },
};

const ssoOffAndMfa = recording(api({ providers: () => [provider()] }));

/**
 * One save that turns single sign-on off and requires a second factor raises
 * both confirmations, the single sign-on one first, and sends a single request
 * after the second. Only the last confirmation reports a landing.
 */
export const SingleSignOnOffAndASecondFactorAreConfirmedInTurn: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness fetchStub={ssoOffAndMfa.stub}>
      <UxScreenProvider screen="sso">
        <Toasted>
          <SingleSignOn />
        </Toasted>
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("switch", { name: "Single sign-on" }));
    await pickOption(canvas.getByLabelText("Second factor"), "Required for everyone");
    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));

    await confirmDestructive(/Turn off single sign-on/, "Turn it off");
    // the first answer only moves on: nothing is sent until the second
    await waitFor(async () =>
      expect(
        within(await confirmation()).getByRole("heading", {
          name: "Require a second factor to sign in?",
        }),
      ).toBeInTheDocument(),
    );
    ssoOffAndMfa.expectNotSent("PUT", "/auth-policy");

    await confirmDestructive(/before they get a session/, "Require it");
    await expect(
      await ssoOffAndMfa.expectSentBody("PUT", `/api/v1/orgs/${ORG.id}/auth-policy`),
    ).toEqual({
      allow_password_login: true,
      allow_sso: false,
      mfa_policy: "required_all",
      mfa_enforce_after: null,
    });
    await expectToast(canvasElement, /the sign-in policy updated/i);

    // one request for two answers, and the landing belongs to the one that sent it
    const puts = ssoOffAndMfa.calls.filter(
      (call) => call.method === "PUT" && call.url.includes("/auth-policy"),
    );
    await expect(puts).toHaveLength(1);
    await expectUxEvent("form_submit", "sso-single-sign-on-off");
    await expectUxEvent("save_confirmed", "sso-mfa-policy");
    expectNoUxEvent("save_confirmed", "sso-single-sign-on-off");
  },
};

// both flags travel together because the control plane refuses the combination,
// not the field
const policySave = recording(api({ providers: () => [provider()] }));

export const SavesPolicy: Story = {
  render: () => (
    <Harness fetchStub={policySave.stub}>
      <Toasted>
        <SingleSignOn />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const save = await canvas.findByRole("button", { name: "Save policy" });
    // nothing changed yet, so there is nothing to save
    await waitFor(() => expect(save).toBeDisabled());

    await userEvent.click(canvas.getByRole("switch", { name: "Password sign-in" }));
    await waitFor(() => expect(save).toBeEnabled());
    await userEvent.click(save);
    // password sign-in going off is the one flag here that confirms (#2084)
    await confirmDestructive(/Turn off password sign-in/, "Turn it off");

    await expect(
      await policySave.expectSentBody("PUT", `/api/v1/orgs/${ORG.id}/auth-policy`),
    ).toEqual({
      allow_password_login: false,
      allow_sso: true,
      // the second-factor policy travels with the two flags even when it is
      // the one thing that did not change: omitting it would be read as "keep
      // the current value", which is right, but sending it is what makes this
      // assertion prove the field is wired at all (#1078)
      mfa_policy: "off",
      // and the grace window travels with it, null under `off` (#1852)
      mfa_enforce_after: null,
    });
    await expectToast(canvasElement, /the sign-in policy updated/i);
  },
};

// turning both off is an outage rather than a policy; the screen says so before
// the round trip instead of making the operator read a 409
export const RefusesToDisableEverySignIn: Story = {
  render: () => (
    <Harness fetchStub={api({ providers: () => [provider()] })}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const save = await canvas.findByRole("button", { name: "Save policy" });

    await userEvent.click(canvas.getByRole("switch", { name: "Password sign-in" }));
    await userEvent.click(canvas.getByRole("switch", { name: "Single sign-on" }));

    await waitFor(() => expect(canvas.getByText(/At least one sign-in method/)).toBeVisible());
    await expect(save).toBeDisabled();
  },
};

// --- the second-factor policy (#1078) -------------------------------------

const mfaSave = recording(api({ providers: () => [provider()] }));

/**
 * Tightening `mfa_policy` changes what every member meets at sign-in, so it
 * confirms first — the confirmation says an unenrolled member sets a factor
 * up on the way in (#1852) rather than being refused, and names the way back
 * in for a lost device.
 */
export const RequiringASecondFactorConfirmsFirst: Story = {
  render: () => (
    <Harness fetchStub={mfaSave.stub}>
      <Toasted>
        <SingleSignOn />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const select = await canvas.findByLabelText("Second factor");
    await pickOption(select, "Required for everyone");
    // the hint under the control changes with the value: the two `required_*`
    // options differ in who they bind, which is the whole decision
    await expect(canvas.getByText(/Superadmins included/)).toBeVisible();

    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));
    const dialog = await within(document.body).findByRole("dialog");
    // the dialog animates in, so visibility is polled rather than read once (#2287)
    await waitFor(() => expect(dialog).toBeVisible());
    await expect(
      within(dialog).getByText(/next password sign-in, before they get a session/i),
    ).toBeVisible();
    await expect(within(dialog).getByText(/rolter mfa reset/)).toBeVisible();
    // no window was picked, so there is no date to announce
    await expect(within(dialog).queryByText(/It starts on/)).not.toBeInTheDocument();

    // backing out sends nothing: the policy is unchanged until it is confirmed
    await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    mfaSave.expectNotSent("PUT", "/auth-policy");

    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));
    await confirmDestructive(/before they get a session/, "Require it");
    await expect(await mfaSave.expectSentBody("PUT", `/api/v1/orgs/${ORG.id}/auth-policy`)).toEqual(
      {
        allow_password_login: true,
        allow_sso: true,
        mfa_policy: "required_all",
        mfa_enforce_after: null,
      },
    );
  },
};

/** One grant of `role` to `user`, at the org or at a team inside it. */
const grant = (
  id: string,
  user: string,
  role: string,
  at: Pick<MembershipRow, "org_id" | "team_id">,
): MembershipRow => ({
  id,
  user_id: user,
  org_id: null,
  team_id: null,
  project_id: null,
  role,
  created_at: NOW,
  ...at,
});

// the org's memberships are one row per grant anywhere in its tree: ada holds
// a role on the org and another on a team, grace holds one. three rows, two people
const GRANTS: MembershipRow[] = [
  grant("m-1", "user-ada", "admin", { org_id: ORG.id }),
  grant("m-2", "user-ada", "member", { team_id: TEAM.id }),
  grant("m-3", "user-grace", "member", { org_id: ORG.id }),
];

/**
 * "This organization has N members" counts people, not grants. A person with a
 * role on the org and another on a team is two rows and one member, and the
 * number sits in the dialog an admin reads before requiring a second factor.
 */
export const TheSecondFactorConfirmationCountsEachPersonOnce: Story = {
  render: () => (
    <Harness fetchStub={api({ providers: () => [provider()], memberships: () => GRANTS })}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await pickOption(await canvas.findByLabelText("Second factor"), "Required for everyone");
    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));

    const dialog = await confirmation();
    // the count is its own request, so it settles after the dialog is up
    await waitFor(() => expect(dialog).toHaveTextContent("This organization has 2 members."));
    await expect(dialog).not.toHaveTextContent("3 members");
  },
};

const graceSave = recording(api({ providers: () => [provider()] }));

/**
 * An org can announce the requirement before it bites (#1852): a grace window
 * picked beside the policy, said in the confirmation as a date, and sent as
 * that many days from the moment of saving.
 */
export const AGraceWindowAnnouncesTheRequirement: Story = {
  render: () => (
    <Harness fetchStub={graceSave.stub}>
      <Toasted>
        <SingleSignOn />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the window only exists while a factor is required
    await canvas.findByLabelText("Second factor");
    await expect(canvas.queryByLabelText("Start requiring it")).not.toBeInTheDocument();
    await pickOption(canvas.getByLabelText("Second factor"), "Required for everyone");
    await pickOption(await canvas.findByLabelText("Start requiring it"), "In 14 days");

    const before = Date.now();
    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));
    const dialog = await confirmation();
    await waitFor(() => expect(within(dialog).getByText(/It starts on/)).toBeVisible());
    await userEvent.click(within(dialog).getByRole("button", { name: "Require it" }));

    const body = await graceSave.expectSentBody<{ mfa_policy: string; mfa_enforce_after: string }>(
      "PUT",
      `/api/v1/orgs/${ORG.id}/auth-policy`,
    );
    await expect(body.mfa_policy).toBe("required_all");
    const days = (Date.parse(body.mfa_enforce_after) - before) / 86_400_000;
    await expect(days).toBeGreaterThan(13.99);
    await expect(days).toBeLessThan(14.01);
  },
};

/**
 * A window already announced is offered back as itself, dated, so saving an
 * unrelated switch does not quietly restart the clock — and pulling it in to
 * "at their next sign-in" is a tightening, so that confirms.
 */
const graceKeep = recording(
  api({
    providers: () => [provider()],
    policy: () => ({
      ...POLICY,
      mfa_policy: "required_all",
      mfa_enforce_after: new Date(Date.now() + 5 * 86_400_000).toISOString(),
    }),
  }),
);

export const AnAnnouncedWindowIsKeptUntilChanged: Story = {
  render: () => (
    <Harness fetchStub={graceKeep.stub}>
      <Toasted>
        <SingleSignOn />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const grace = await canvas.findByLabelText("Start requiring it");
    await waitFor(() => expect((grace as HTMLInputElement).value).toMatch(/as announced/));

    await pickOption(grace, "At their next sign-in");
    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));
    await confirmDestructive(/before they get a session/, "Require it");
    await expect(
      await graceKeep.expectSentBody("PUT", `/api/v1/orgs/${ORG.id}/auth-policy`),
    ).toEqual({
      allow_password_login: true,
      allow_sso: true,
      mfa_policy: "required_all",
      mfa_enforce_after: null,
    });
  },
};

/** An announced window, `days` from now, on a `required_all` policy. */
const announced = (days: number) =>
  recording(
    api({
      providers: () => [provider()],
      policy: () => ({
        ...POLICY,
        mfa_policy: "required_all",
        mfa_enforce_after: new Date(Date.now() + days * 86_400_000).toISOString(),
      }),
    }),
  );

const pulledIn = announced(20);

/**
 * Moving an announced window to a nearer preset binds members sooner than
 * they were told, just as "at their next sign-in" does, so it confirms too.
 */
export const PullingAnAnnouncedWindowInConfirms: Story = {
  render: () => (
    <Harness fetchStub={pulledIn.stub}>
      <Toasted>
        <SingleSignOn />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const grace = await canvas.findByLabelText("Start requiring it");
    await waitFor(() => expect((grace as HTMLInputElement).value).toMatch(/as announced/));
    await pickOption(grace, "In 7 days");
    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));

    const dialog = await confirmation();
    await waitFor(() => expect(within(dialog).getByText(/It starts on/)).toBeVisible());
    // nothing is sent until the admin confirms the earlier date
    pulledIn.expectNotSent("PUT", "/auth-policy");
    await userEvent.click(within(dialog).getByRole("button", { name: "Require it" }));
    const body = await pulledIn.expectSentBody<{ mfa_enforce_after: string }>(
      "PUT",
      `/api/v1/orgs/${ORG.id}/auth-policy`,
    );
    const days = (Date.parse(body.mfa_enforce_after) - Date.now()) / 86_400_000;
    await expect(days).toBeGreaterThan(6.9);
    await expect(days).toBeLessThan(7.01);
  },
};

const pushedOut = announced(5);

/**
 * Moving an announced window later binds nobody sooner, so it saves at once,
 * like any other relaxation.
 */
export const MovingAnAnnouncedWindowLaterSavesWithoutAConfirmation: Story = {
  render: () => (
    <Harness fetchStub={pushedOut.stub}>
      <Toasted>
        <SingleSignOn />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const grace = await canvas.findByLabelText("Start requiring it");
    await waitFor(() => expect((grace as HTMLInputElement).value).toMatch(/as announced/));
    await pickOption(grace, "In 30 days");
    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));
    // sent straight away: a confirmation in front of it would hold the PUT
    await pushedOut.expectSentBody("PUT", `/api/v1/orgs/${ORG.id}/auth-policy`);
    await expect(within(document.body).queryByRole("dialog")).not.toBeInTheDocument();
    await expectToast(canvasElement, /the sign-in policy updated/i);
  },
};

const KEK_REFUSAL =
  "a required second factor needs ROLTER_KEK set on the control plane: without it nobody " +
  "can enrol, so every account this policy binds would be locked out. Set ROLTER_KEK and " +
  "restart the control plane, or choose optional";

/**
 * A control plane with no `ROLTER_KEK` refuses a `required_*` policy with a
 * 409, since nobody on it could enrol. The confirmation stays open and shows
 * the refusal in the control plane's own words, beside the button that caused
 * it, so the admin reads why rather than a promise that did not hold.
 */
export const WithoutAKeyARequirementIsRefused: Story = {
  render: () => (
    <Harness
      fetchStub={api({
        providers: () => [provider()],
        putPolicy: () => json({ error: { message: KEK_REFUSAL, code: "conflict" } }, 409),
      })}
    >
      <Toasted>
        <SingleSignOn />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await pickOption(await canvas.findByLabelText("Second factor"), "Required for everyone");
    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));
    const dialog = await confirmation();
    await userEvent.click(within(dialog).getByRole("button", { name: "Require it" }));
    await waitFor(() => expect(within(dialog).getByText(/ROLTER_KEK/)).toBeVisible());
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
  },
};

const mfaRelax = recording(
  api({ providers: () => [provider()], policy: () => ({ ...POLICY, mfa_policy: "required_all" }) }),
);

/**
 * Relaxing it does not confirm. A dialog in front of a change that locks
 * nobody out is the click-through that teaches people to dismiss the one that
 * matters.
 */
export const RelaxingThePolicySavesWithoutAConfirmation: Story = {
  render: () => (
    <Harness fetchStub={mfaRelax.stub}>
      <Toasted>
        <SingleSignOn />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const select = await canvas.findByLabelText("Second factor");
    // a combobox reads as the option's label; `required_all` is the stored value
    await waitFor(() => expect(select).toHaveValue("Required for everyone"));
    await pickOption(select, "Optional");
    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));
    await expect(
      await mfaRelax.expectSentBody("PUT", `/api/v1/orgs/${ORG.id}/auth-policy`),
    ).toEqual({
      allow_password_login: true,
      allow_sso: true,
      mfa_policy: "optional",
      mfa_enforce_after: null,
    });
    await expectToast(canvasElement, /the sign-in policy updated/i);
  },
};

/**
 * A member may read the sign-in policy and change none of it.
 *
 * `Save policy` was an ungated `Button` until this PR gated it on
 * `org_auth_policy:update` — the same capability the control plane checks —
 * so a member used to be able to press it and collect a 403. The refusal now
 * names the role that would allow it, because "disabled" alone is the same
 * non-answer the 403 was.
 */
export const AsMemberThePolicyIsReadOnly: Story = {
  render: () => (
    <Harness fetchStub={api({ providers: () => [provider()] })} role="member">
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the control is there and readable: the value is information a member is
    // allowed to have, and hiding it would leave them guessing why sign-in
    // asks for a code
    await expect(await canvas.findByLabelText("Second factor")).toBeVisible();
    await pickOption(canvas.getByLabelText("Second factor"), "Optional");
    await expectRefused(canvasElement, "Save policy");
  },
};

// Registering and editing an identity provider is `sso_provider`, and mapping
// its groups to roles is `sso_group_mapping` — admin at every action (#1606).
//
// The screen's own read is `admin`-gated too, so a viewer sees the list fail;
// what these stories pin is that the controls say what they would take rather
// than offering a click the control plane is going to refuse.
export const RefusedToAMember: Story = {
  render: () => (
    <Harness fetchStub={api()} role="member">
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Add provider");
    await expectRefused(canvasElement, "Delete provider Acme Okta");
    await expectRefused(canvasElement, "Remove the stored client secret for Acme Okta");
    await expectRefused(canvasElement, "Remove the mapping for platform-engineering");
    // #1234: writing a mapping at a narrower scope is the same capability as
    // writing an org-wide one, so the new scope select must not come with a
    // create button that only fails on submit
    await expectRefused(canvasElement, "Map a group in Acme Okta");
    // #2084: the card's switch and pencil are updates, the same capability as
    // clearing the secret beside them, and were live for a member
    await expectRefused(canvasElement, "Enable provider Acme Okta", NEEDS_ADMIN, "switch");
    await expectRefused(canvasElement, "Edit provider Acme Okta");
    // a refused switch opens no confirmation to be refused again after it
    await userEvent.click(
      within(canvasElement).getByRole("switch", { name: "Enable provider Acme Okta" }),
    );
    await expect(within(document.body).queryByRole("dialog")).not.toBeInTheDocument();
  },
};

export const RefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={api()} role="viewer">
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Add provider");
    await expectRefused(canvasElement, "Delete provider Acme Okta");
    await expectRefused(canvasElement, "Enable provider Acme Okta", NEEDS_ADMIN, "switch");
    await expectRefused(canvasElement, "Edit provider Acme Okta");
  },
};

/**
 * The same two controls are offered to an admin: the gate repeats the control
 * plane's `sso_provider:update`, so over-gating would lock out the people the
 * screen is for.
 */
export const AsAdminTheProviderSwitchAndEditAreOffered: Story = {
  render: () => (
    <Harness fetchStub={api()} role="admin">
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, "Enable provider Acme Okta", "switch");
    await expectAllowed(canvasElement, "Edit provider Acme Okta");
  },
};

/**
 * #1234: a mapping that is narrower than the org says so on its row.
 *
 * `POST .../group-mappings` has always taken `team_id`/`project_id`, and the
 * list has always answered with them — the screen read those fields and threw
 * them away, so a team-scoped mapping written through the API was listed as if
 * it granted across the whole org.
 */
export const ShowsTheScopeOfANarrowMapping: Story = {
  render: () => (
    <Harness fetchStub={api()}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const row = (await canvas.findByText("gateway-oncall")).closest("li");
    if (!row) throw new Error("the mapping is not rendered as a row");
    // the team's name, resolved org-wide, not the raw uuid the row carries
    await waitFor(() => expect(within(row).getByText(TEAM.name)).toBeVisible());
    await expect(within(row).queryByText(TEAM.id)).toBeNull();

    // and the org-wide one beside it is still marked as org-wide, or "narrower
    // than the others" would be the absence of a chip rather than a statement
    const orgRow = canvas.getByText("platform-engineering").closest("li");
    if (!orgRow) throw new Error("the mapping is not rendered as a row");
    await expect(within(orgRow).getByText("Whole organization")).toBeVisible();
  },
};

// the two lists `useOrgScope` reads, answered 500. matched before `scoped()`
// gets to them, since that helper *is* what resolves the chain for every other
// story here
const scopeListsFail =
  (inner: FetchStub): FetchStub =>
  async (input, init) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (/^\/api\/v1\/orgs\/[^/]+\/(teams|projects)$/.test(path)) {
      return json({ error: { message: "scope unavailable" } }, 500);
    }
    return inner(input, init);
  };

/**
 * A scope the screen cannot name says so, instead of drawing its uuid (#1677).
 *
 * The mapping row is read-only — no picker under it, so no `LoadError` and no
 * retry — and a chip reading `team-1` there is indistinguishable from a team
 * actually called that.
 */
export const ScopeThatCannotBeResolved: Story = {
  render: () => (
    <Harness fetchStub={scopeListsFail(api())}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const row = (await canvas.findByText("gateway-oncall")).closest("li");
    if (!row) throw new Error("the mapping is not rendered as a row");
    await waitFor(() => expect(within(row).getByText("Unresolved scope")).toBeVisible());
    await expect(within(row).queryByText(TEAM.id)).toBeNull();
    // the id stays quotable in a support conversation
    const chip = within(row).getByTitle(/could not be matched/);
    await expect(chip.getAttribute("title")).toContain(TEAM.id);

    // an org-wide mapping never had a scope to resolve, so it is untouched
    const orgRow = canvas.getByText("platform-engineering").closest("li");
    if (!orgRow) throw new Error("the mapping is not rendered as a row");
    await expect(within(orgRow).getByText("Whole organization")).toBeVisible();
  },
};

/**
 * #1234: the scope select offers the whole org — every team, and every project
 * in any of those teams — and the id it picks reaches the create body.
 */
const teamScoped = recording(api({ providers: () => [provider()] }));

export const MapsAGroupToATeam: Story = {
  render: () => (
    <Harness fetchStub={teamScoped.stub}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Acme Okta")).toBeVisible());

    const picker = await canvas.findByLabelText("Where the role applies");
    // the org is the default, so an operator who ignores the select writes the
    // same org-wide mapping the screen wrote before this
    await expect(picker).toHaveValue("Whole organization");
    const listbox = await openOptions(picker);
    await expect(within(listbox).getByRole("option", { name: TEAM.name })).toBeVisible();
    // a project in that team is reachable without moving the scope switcher
    await expect(within(listbox).getByRole("option", { name: PROJECT.name })).toBeVisible();
    await userEvent.click(within(listbox).getByRole("option", { name: TEAM.name }));

    await userEvent.type(canvas.getByLabelText("IdP group"), "gateway-oncall");
    await clickWhenEnabled(canvasElement, "Map a group in Acme Okta");

    const body = await teamScoped.expectSentBody<Record<string, unknown>>(
      "POST",
      "/api/v1/sso-providers/sso-1/group-mappings",
    );
    await expect(body.group_name).toBe("gateway-oncall");
    // the role is the one the row starts on: nothing was picked, so nothing
    // more powerful than a viewer was granted (#2078)
    await expect(body.role).toBe("viewer");
    // the narrower scope is the whole point: `team_id` set, and `project_id`
    // left out entirely rather than sent as null, which the server would read
    // as the more specific scope
    await expect(body.team_id).toBe(TEAM.id);
    await expect(body.project_id).toBeUndefined();
    // a viewer on one team is the narrowest grant there is, so it saves at
    // once, with no dialog between the press and the request
    await expect(within(document.body).queryByRole("dialog")).toBeNull();
  },
};

/** The same picker, one level narrower: a project id, and no team id. */
const projectScoped = recording(api({ providers: () => [provider()] }));

export const MapsAGroupToAProject: Story = {
  render: () => (
    <Harness fetchStub={projectScoped.stub}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Acme Okta")).toBeVisible());

    const picker = await canvas.findByLabelText("Where the role applies");
    await pickOption(picker, PROJECT.name);
    await userEvent.type(canvas.getByLabelText("IdP group"), "gateway-deployers");
    await clickWhenEnabled(canvasElement, "Map a group in Acme Okta");

    const body = await projectScoped.expectSentBody<Record<string, unknown>>(
      "POST",
      "/api/v1/sso-providers/sso-1/group-mappings",
    );
    await expect(body.project_id).toBe(PROJECT.id);
    await expect(body.team_id).toBeUndefined();
  },
};

/**
 * #1234: removing a mapping takes access away from everyone in that group, so
 * the prompt names *what* is being withdrawn — the scope as well as the role.
 * "member" and "member on Platform" are very different removals.
 */
export const NamesTheScopeWhenRemovingAMapping: Story = {
  render: () => (
    <Harness fetchStub={api()}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("gateway-oncall")).toBeVisible());
    await userEvent.click(canvas.getByLabelText("Remove the mapping for gateway-oncall"));

    const dialog = within(await confirmation());
    await expect(dialog.getByText(new RegExp(TEAM.name))).toBeVisible();
    await cancelConfirmation();
  },
};

// --- the add row: what it starts on, and what it asks before it grants (#2078) ---

// the stub of the story being played, so `play` reads what `render` was given
let sent: Recorder;

const GROUP_MAPPINGS_URL = "/api/v1/sso-providers/sso-1/group-mappings";

type Posted = { group_name: string; role: string; team_id?: string; project_id?: string };

const reasonAdmin = en.groupMappings.grant.reasonAdmin;
const reasonOrg = en.groupMappings.grant.reasonOrg;

// the create is held until the story lets it land, so the dialog can be read
// while the request is on the wire
let releaseGrant: () => void = () => {};
const heldGrant =
  (inner: FetchStub): FetchStub =>
  async (input, init) => {
    if (
      (init?.method ?? "GET").toUpperCase() === "POST" &&
      String(input).includes("/group-mappings")
    ) {
      await new Promise<void>((resolve) => {
        releaseGrant = resolve;
      });
    }
    return inner(input, init);
  };

// every mapping read answered 500, and nothing else
const mappingsFail =
  (inner: FetchStub): FetchStub =>
  async (input, init) =>
    String(input).includes("/group-mappings") && (init?.method ?? "GET").toUpperCase() === "GET"
      ? json({ error: { message: "mappings unavailable" } }, 500)
      : inner(input, init);

/**
 * A new mapping starts on the least powerful role. The row used to preselect
 * `admin` at the whole organization, so typing a name and pressing the button
 * made everyone in the group an org admin. It also had no visible labels, and
 * a placeholder that was the name of a mapping already listed.
 */
export const NewMappingStartsOnViewer: Story = {
  render: () => (
    <Harness fetchStub={api({ providers: () => [provider()] })}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByLabelText("Role to grant")).toHaveValue("Viewer");
    // the picker is a skeleton until the org's teams and projects answer
    await expect(await canvas.findByLabelText("Where the role applies")).toHaveValue(
      "Whole organization",
    );

    // each control carries a label you can read, not only one a screen reader is told
    for (const label of ["IdP group", "Where the role applies", "Role to grant"]) {
      await expect(canvas.getByText(label, { selector: "label" })).toBeVisible();
    }

    // and the empty field cannot be taken for a filled one: the example is marked as
    // one, and is not the name of a mapping that is listed
    const group = canvas.getByLabelText("IdP group");
    await expect(group).toHaveValue("");
    const placeholder = group.getAttribute("placeholder");
    await expect(placeholder).toBe(en.groupMappings.groupPlaceholder);
    await expect(placeholder).toMatch(/^e\.g\. /);
    await waitFor(() => expect(canvas.getByText("platform-engineering")).toBeVisible());
    await expect(MAPPINGS["sso-1"].map((m) => m.group_name)).not.toContain(placeholder);
  },
};

/**
 * Admin is asked about first, and the question names the group, the role and
 * the scope. Cancelling sends nothing and keeps what was typed; confirming
 * sends exactly the body the dialog described, and the form starts over on
 * Viewer rather than carrying the admin grant into the next mapping.
 */
export const AdminGrantAsksFirst: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    sent = recording(heldGrant(api({ providers: () => [provider()] })));
    return (
      <Harness fetchStub={sent.stub}>
        <UxScreenProvider screen="sso">
          <SingleSignOn />
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const group = await canvas.findByLabelText("IdP group");
    await userEvent.type(group, "gateway-deployers");
    await pickOption(await canvas.findByLabelText("Where the role applies"), TEAM.name);
    await pickOption(canvas.getByLabelText("Role to grant"), "Admin");
    await clickWhenEnabled(canvasElement, "Map a group in Acme Okta");

    const dialogElement = await confirmation();
    // the dialog animates in, so visibility is polled rather than read once (#2287)
    await waitFor(() => expect(dialogElement).toBeVisible());
    const dialog = within(dialogElement);
    await expect(dialog.getByText("Map gateway-deployers to Admin?")).toBeVisible();
    await expect(dialog.getByText(new RegExp(`gets Admin on ${TEAM.name}\\.`))).toBeVisible();
    // this screen grants at sign-in, and says so rather than promising a sync
    await expect(dialog.getByText(/at their next sign-in/)).toBeVisible();
    // admin is the only reason: the scope is one team, so nothing says "whole organization"
    await expect(dialog.getByText(reasonAdmin)).toBeVisible();
    await expect(dialog.queryByText(reasonOrg)).toBeNull();
    sent.expectNotSent("POST", GROUP_MAPPINGS_URL);

    // backing out sends nothing and is recorded as a cancel, not a decision
    await cancelConfirmation();
    sent.expectNotSent("POST", GROUP_MAPPINGS_URL);
    const abandon = await expectUxEvent("form_abandon", "sso-group-mapping-grant");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "sso-group-mapping-grant");
    await expect(group).toHaveValue("gateway-deployers");
    await expect(canvas.getByLabelText("Role to grant")).toHaveValue("Admin");

    await clickWhenEnabled(canvasElement, "Map a group in Acme Okta");
    await confirmDestructive("Map gateway-deployers to Admin?", "Map group");
    await expect(await sent.expectSentBody<Posted>("POST", GROUP_MAPPINGS_URL)).toEqual({
      group_name: "gateway-deployers",
      role: "admin",
      team_id: TEAM.id,
    });

    // in flight: the request is on the wire, so neither button can be pressed
    const inFlight = within(await confirmation());
    await waitFor(() => {
      expect(inFlight.getByRole("button", { name: "Map group" })).toBeDisabled();
      expect(inFlight.getByRole("button", { name: "Cancel" })).toBeDisabled();
    });

    releaseGrant();
    await expectSheetClosed();
    const submit = await expectUxEvent("form_submit", "sso-group-mapping-grant");
    await expect(submit.outcome).toBe("ok");
    await expectUxEvent("save_confirmed", "sso-group-mapping-grant");
    await waitFor(() => expect(group).toHaveValue(""));
    await expect(canvas.getByLabelText("Role to grant")).toHaveValue("Viewer");
    await expect(canvas.getByLabelText("Where the role applies")).toHaveValue("Whole organization");
  },
};

/**
 * A role across the whole organization is asked about too, however small the
 * role: the form starts there, so a viewer is the first thing an operator can
 * grant org-wide by pressing one button. The dialog says it is the scope that
 * raised it, and the request names no team or project.
 */
export const WholeOrgGrantAsksFirst: Story = {
  render: () => {
    sent = recording(api({ providers: () => [provider()] }));
    return (
      <Harness fetchStub={sent.stub}>
        <SingleSignOn />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await canvas.findByLabelText("IdP group"), "ops-readers");
    await clickWhenEnabled(canvasElement, "Map a group in Acme Okta");

    const dialog = within(await confirmation());
    await expect(dialog.getByText("Map ops-readers to Viewer?")).toBeVisible();
    await expect(dialog.getByText(/gets Viewer on the whole organization\./)).toBeVisible();
    await expect(dialog.getByText(reasonOrg)).toBeVisible();
    await expect(dialog.queryByText(reasonAdmin)).toBeNull();

    await cancelConfirmation();
    sent.expectNotSent("POST", GROUP_MAPPINGS_URL);

    await clickWhenEnabled(canvasElement, "Map a group in Acme Okta");
    await confirmDestructive("Map ops-readers to Viewer?", "Map group");
    // an org-wide grant names no scope at all, rather than an empty one
    await expect(await sent.expectSentBody<Posted>("POST", GROUP_MAPPINGS_URL)).toEqual({
      group_name: "ops-readers",
      role: "viewer",
    });
  },
};

// the two reasons stack: admin across the whole organization is the widest grant
// there is, and the dialog says both rather than picking one
export const AdminOnTheWholeOrgNamesBothReasons: Story = {
  render: () => {
    sent = recording(api({ providers: () => [provider()] }));
    return (
      <Harness fetchStub={sent.stub}>
        <SingleSignOn />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await canvas.findByLabelText("IdP group"), "platform-admins");
    await pickOption(canvas.getByLabelText("Role to grant"), "Admin");
    await clickWhenEnabled(canvasElement, "Map a group in Acme Okta");

    const dialog = within(await confirmation());
    await expect(dialog.getByText(/gets Admin on the whole organization\./)).toBeVisible();
    await expect(dialog.getByText(reasonAdmin)).toBeVisible();
    await expect(dialog.getByText(reasonOrg)).toBeVisible();

    await confirmDestructive("Map platform-admins to Admin?", "Map group");
    await expect(await sent.expectSentBody<Posted>("POST", GROUP_MAPPINGS_URL)).toEqual({
      group_name: "platform-admins",
      role: "admin",
    });
  },
};

// the confirmation does not close itself: a refusal stays beside the button that
// caused it, and cancelling does not leave it standing under the next attempt
export const MapGroupRefusedInsideTheConfirmation: Story = {
  render: () => (
    <Harness
      fetchStub={async (input, init) =>
        String(input).includes("/group-mappings") &&
        (init?.method ?? "GET").toUpperCase() === "POST"
          ? json({ error: { message: "platform-admins is already mapped" } }, 409)
          : api({ providers: () => [provider()] })(input, init)
      }
    >
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const group = await canvas.findByLabelText("IdP group");
    await userEvent.type(group, "platform-admins");
    await pickOption(canvas.getByLabelText("Role to grant"), "Admin");
    await clickWhenEnabled(canvasElement, "Map a group in Acme Okta");
    await confirmDestructive("Map platform-admins to Admin?", "Map group");

    const dialog = within(await confirmation());
    await waitFor(() => expect(dialog.getByRole("alert")).toHaveTextContent(/already mapped/));
    // said once: behind the dialog the form does not repeat it
    await expect(canvas.queryAllByRole("alert")).toHaveLength(0);

    await cancelConfirmation();
    await expect(canvas.queryAllByRole("alert")).toHaveLength(0);
    await expect(group).toHaveValue("platform-admins");
  },
};

// a list that could not be read is a LoadError with the retry a read has, where
// it used to be a line of red text with nothing to press
export const GroupMappingsCannotLoad: Story = {
  render: () => (
    <Harness fetchStub={mappingsFail(api({ providers: () => [provider()] }))}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /failed to return group mappings/);
    // the control plane's own words stay under the summary
    await expect(canvas.getByText("mappings unavailable")).toBeVisible();
    await expect(canvas.getByRole("button", { name: en.errors.load.retry })).toBeVisible();
    // a list that could not be read is not a list of nothing
    await expect(canvas.queryByText(/everyone signing in through this provider gets/)).toBeNull();
    // the form is still there: writing a mapping does not need the list
    await expect(canvas.getByLabelText("IdP group")).toBeVisible();
  },
};

export const GroupMappingsLoading: Story = {
  render: () => (
    <Harness
      fetchStub={async (input, init) =>
        String(input).includes("/group-mappings")
          ? new Promise<Response>(() => {})
          : api({ providers: () => [provider()] })(input, init)
      }
    >
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectInStatusRegion(canvasElement, "group-mappings-loading");
    await expect(canvas.queryByText(/everyone signing in through this provider gets/)).toBeNull();
  },
};

/**
 * The add row on a phone, in Russian. The group name shrank to three
 * characters and the role read "Администрато" because four controls shared one
 * wrapping line. Each control has its own line now, labelled, and all of them
 * are as wide as the card.
 */
export const AddRowFitsAPhoneInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => (
    <Harness fetchStub={api({ providers: () => [provider()] })}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const copy = ru.groupMappings;
    const canvas = within(canvasElement);
    const group = await canvas.findByLabelText(copy.groupLabel);
    const scope = await canvas.findByLabelText(copy.scopeLabel);
    const role = canvas.getByLabelText(copy.roleLabel);
    const add = canvas.getByRole("button", {
      name: ru.pages.sso.mappings.addNamed.replace("{{provider}}", "Acme Okta"),
    });
    // the row reads in Russian, and the default survives the language
    await waitFor(() => expect(role).toHaveValue(ru.shell.roles.viewer));
    await expect(canvas.getByText(copy.groupLabel, { selector: "label" })).toBeVisible();

    // the name field is a field, not a sliver: it spans the card
    await expect(group.getBoundingClientRect().width).toBeGreaterThan(240);
    // every control sits inside the phone's width. the page is not asked: the
    // provider cards have their own phone-width problems (#2090)
    for (const control of [group, scope, role, add]) {
      const box = control.getBoundingClientRect();
      await expect(box.left).toBeGreaterThanOrEqual(0);
      await expect(box.right).toBeLessThanOrEqual(window.innerWidth);
    }

    // a listed mapping keeps its name too: it was squeezed out by the chips on its row
    const listed = await canvas.findByText("platform-engineering");
    await expect(listed.getBoundingClientRect().width).toBeGreaterThan(100);
    await expect(listed.scrollWidth).toBeLessThanOrEqual(listed.clientWidth);

    // the widest role is spelled out in full, in the control that names it
    await pickOption(role, ru.shell.roles.admin);
    await waitFor(() => expect(role).toHaveValue(ru.shell.roles.admin));
    await expect(role.scrollWidth).toBeLessThanOrEqual(role.clientWidth);
  },
};
