import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import UserProvisioning from "./UserProvisioning";
import {
  expectLoadError,
  expectNoFalseEmpty,
  expectRefused,
  expectSkeleton,
  expectToast,
  Harness as ScreenHarness,
  json,
  openOptions,
  pickOption,
  Toasted,
  type FetchStub,
  type StoryRole,
} from "./story-harness";
import type { PublicUrl, ScimGroupMappingRow, ScimTokenRow } from "@/lib/api";
import { formattersFor } from "@/lib/i18n/format";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";

const NOW = new Date("2026-07-01T10:00:00Z").toISOString();

const ORG = { id: "org-1", name: "Acme", slug: "acme", created_at: NOW };

/**
 * The control plane's configured public url (#2079).
 *
 * Deliberately not the story's own origin: a strip or a reveal step that fell
 * back to `window.location` would show the storybook host and fail the
 * assertion, which is the bug the server-reported base exists to prevent.
 */
const PUBLIC_BASE = "https://rolter.acme.example";
const PUBLIC_URL: PublicUrl = { public_url: PUBLIC_BASE, configured: true };
const SCIM_URL = `${PUBLIC_BASE}/scim/v2`;
// `ROLTER_PUBLIC_URL` unset: the control plane falls back to its default
const DEFAULT_BASE = "http://localhost:4001";
const UNSET: PublicUrl = { public_url: DEFAULT_BASE, configured: false };

const token = (over: Partial<ScimTokenRow> = {}): ScimTokenRow => ({
  id: "tok-1",
  org_id: ORG.id,
  name: "Okta production",
  created_by: "user-1",
  created_at: NOW,
  last_used_at: new Date("2026-07-02T09:30:00Z").toISOString(),
  revoked_at: null,
  ...over,
});

const TOKENS: ScimTokenRow[] = [
  token(),
  token({ id: "tok-2", name: "Entra staging", last_used_at: null }),
  token({
    id: "tok-3",
    name: "Okta legacy",
    revoked_at: new Date("2026-07-03T12:00:00Z").toISOString(),
  }),
];

const mapping = (over: Partial<ScimGroupMappingRow> = {}): ScimGroupMappingRow => ({
  id: "map-1",
  org_id: ORG.id,
  group_name: "platform-engineering",
  team_id: null,
  project_id: null,
  role: "member",
  created_at: NOW,
  ...over,
});

const MAPPINGS: ScimGroupMappingRow[] = [
  mapping(),
  mapping({ id: "map-2", group_name: "sre-oncall", role: "admin", team_id: "team-1" }),
];

// two teams, and a project under each: the scope switcher only ever has one
// team selected, so "payments/checkout" is the project outside it that the
// mapping form still has to be able to name (#1249)
const TEAMS = [
  { id: "team-1", org_id: ORG.id, name: "core", created_at: NOW },
  { id: "team-2", org_id: ORG.id, name: "payments", created_at: NOW },
];

const PROJECTS: Record<
  string,
  { id: string; team_id: string; name: string; created_at: string }[]
> = {
  "team-1": [{ id: "proj-1", team_id: "team-1", name: "prod", created_at: NOW }],
  "team-2": [{ id: "proj-2", team_id: "team-2", name: "checkout", created_at: NOW }],
};

// the same projects as the org-wide endpoint returns them: one list, every row
// naming its owning team, which is what the mapping form's picker reads (#1357)
const ORG_PROJECTS = TEAMS.flatMap((team) =>
  (PROJECTS[team.id] ?? []).map((project) => ({ ...project, team_name: team.name })),
);

// the screen resolves its org through useScope(), which fetches orgs, teams and
// projects before the token list is even enabled — so every stub has to route
// by url rather than answer one shape
function scoped(
  tokens: (init?: RequestInit) => Promise<Response>,
  mappings: (init?: RequestInit) => Promise<Response> = async () => json([]),
  chain: {
    teams?: () => Promise<Response>;
    projects?: () => Promise<Response>;
    orgProjects?: () => Promise<Response>;
    /** answers `GET /api/v1/public-url`, which every signed-in caller may read */
    publicUrl?: () => Promise<Response>;
  } = {},
): FetchStub {
  return async (input, init) => {
    const url = String(input);
    const path = new URL(url, "http://localhost").pathname;
    if (url.includes("scim-group-mappings")) return mappings(init);
    if (url.includes("scim-tokens")) return tokens(init);
    if (path === "/api/v1/orgs") return json([ORG]);
    if (path === "/api/v1/public-url") return (chain.publicUrl ?? (async () => json(PUBLIC_URL)))();
    // the projects route also contains "/teams", so it is matched first
    const projects = /^\/api\/v1\/teams\/([^/]+)\/projects$/.exec(path);
    if (projects) return (chain.projects ?? (async () => json(PROJECTS[projects[1]] ?? [])))();
    if (/^\/api\/v1\/orgs\/[^/]+\/projects$/.test(path)) {
      return (chain.orgProjects ?? (async () => json(ORG_PROJECTS)))();
    }
    if (/^\/api\/v1\/orgs\/[^/]+\/teams$/.test(path)) {
      return (chain.teams ?? (async () => json(TEAMS)))();
    }
    return json([]);
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
  toasted,
}: {
  fetchStub: FetchStub;
  role?: StoryRole;
  /** mount the shell's toast queue, for a story that asserts the outcome */
  toasted?: boolean;
}) {
  return (
    <ScreenHarness fetchStub={fetchStub} role={role}>
      {toasted ? (
        <Toasted>
          <UserProvisioning />
        </Toasted>
      ) : (
        <UserProvisioning />
      )}
    </ScreenHarness>
  );
}

/**
 * The cells of the token's table row in column order: token, status, last sync,
 * created, actions. Found by the row's name, so a story reads a column of one
 * token rather than counting cells across the table.
 */
async function cellsOf(canvasElement: HTMLElement, name: string): Promise<HTMLElement[]> {
  const row = await within(canvasElement).findByRole("row", { name: new RegExp(name) });
  return within(row).getAllByRole("cell");
}

const meta = {
  title: "Screens/UserProvisioning",
  component: UserProvisioning,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof UserProvisioning>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => <Harness fetchStub={scoped(async () => json(TOKENS))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Okta production")).toBeVisible());
    // a token an IdP has never presented is distinguishable from a live one
    await expect(canvas.getByText("never used")).toBeVisible();

    // the status reads from the catalog, and a revoked token keeps when it was
    // revoked on the badge
    const copy = en.pages.userProvisioning;
    const [, live] = await cellsOf(canvasElement, "Okta production");
    await expect(live.textContent).toBe(copy.statusActive);
    const [, revoked] = await cellsOf(canvasElement, "Okta legacy");
    await expect(revoked.textContent).toBe(copy.statusRevoked);
    await expect(within(revoked).getByText(copy.statusRevoked)).toHaveAttribute(
      "title",
      copy.revokedAt.replace("{{when}}", formattersFor("en").dateTime(TOKENS[2].revoked_at!)),
    );
  },
};

/**
 * "Last sync" and "Created" were two date styles in one table: a numeric stamp
 * with the clock beside a short date. Both columns show the day now, and the
 * whole stamp is the hover (#2080).
 */
export const DatesShareOneStyle: Story = {
  render: () => <Harness fetchStub={scoped(async () => json(TOKENS))} />,
  play: async ({ canvasElement }) => {
    const fmt = formattersFor("en");
    const [, , lastSync, created] = await cellsOf(canvasElement, "Okta production");
    const lastUsed = TOKENS[0].last_used_at!;
    await expect(lastSync.textContent).toBe(fmt.date(lastUsed));
    await expect(created.textContent).toBe(fmt.date(NOW));
    // the same shape in both: a short date, never the clock or the numeric stamp
    for (const cell of [lastSync, created]) {
      await expect(cell.textContent).toMatch(/^[A-Za-z]{3,} \d{1,2}, \d{4}$/);
    }
    // and the clock is one hover away, on the moment itself
    await expect(within(lastSync).getByText(fmt.date(lastUsed))).toHaveAttribute(
      "title",
      fmt.dateTime(lastUsed),
    );
    await expect(within(created).getByText(fmt.date(NOW))).toHaveAttribute(
      "title",
      fmt.dateTime(NOW),
    );
    // a token never presented has no date to style
    const [, , never] = await cellsOf(canvasElement, "Entra staging");
    await expect(never.textContent).toBe(en.pages.userProvisioning.neverUsed);
  },
};

/**
 * The status badges were the English words `ACTIVE` and `REVOKED` in every
 * locale (#2080). They read from the catalog, so the Russian dashboard says so
 * in Russian.
 */
export const ReadsInRussian: Story = {
  globals: { locale: "ru" },
  render: () => <Harness fetchStub={scoped(async () => json(TOKENS))} />,
  play: async ({ canvasElement }) => {
    const copy = ru.pages.userProvisioning;
    // the locale decorator switches language from an effect, after first paint
    await waitFor(async () => {
      const [, live] = await cellsOf(canvasElement, "Okta production");
      await expect(live.textContent).toBe(copy.statusActive);
    });
    const [, revoked] = await cellsOf(canvasElement, "Okta legacy");
    await expect(revoked.textContent).toBe(copy.statusRevoked);
    await expect(within(revoked).getByText(copy.statusRevoked)).toHaveAttribute(
      "title",
      copy.revokedAt.replace("{{when}}", formattersFor("ru").dateTime(TOKENS[2].revoked_at!)),
    );
    // no English status anywhere on the screen, in either case
    await expect(canvasElement.textContent).not.toMatch(/\b(active|revoked)\b/i);
  },
};

export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No provisioning tokens yet/);
  },
};

export const Empty: Story = {
  render: () => <Harness fetchStub={scoped(async () => json([]))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("No provisioning tokens yet")).toBeVisible());
  },
};

// a failed read is not an empty one (#2211): the table under the load error
// used to say "No provisioning tokens yet" beside a button to issue the first,
// and the lead to count "0 tokens". the lead keeps what it explains and drops
// the count it does not have
export const Error_: Story = {
  name: "Error",
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "boom" } }, 500))} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /failed to return provisioning tokens/i);
    await expect(canvas.getByText("/scim/v2/Users")).toBeVisible();
    await expectNoFalseEmpty(canvasElement, /No provisioning tokens yet/);
    // the connector address is stated beside a token list that was read, not
    // beside one that failed: a control plane with no store serves no SCIM at all
    await expect(canvas.queryByTestId("scim-base-url")).toBeNull();
  },
};

// listing, minting and revoking all need Admin on the org; a lesser principal
// gets a calm explanation and no mint button rather than a red error
export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "forbidden" } }, 403))} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText(/visible to org admins only/)).toBeVisible());
    await expect(canvas.getByRole("button", { name: /Issue token/ })).toBeDisabled();
    // a refused read is not a list of zero: the lead keeps what it explains and
    // states no count of tokens, and no empty table stands in for the refusal
    // (#2211, which #2080 asked for)
    await expect(canvas.getByText("/scim/v2/Users")).toBeVisible();
    await expect(canvasElement.textContent).not.toMatch(/\d+ tokens?\b/);
    await expectNoFalseEmpty(canvasElement, /No provisioning tokens yet/);
    await expect(canvas.queryByRole("table")).toBeNull();
    // a caller who may connect nothing is not handed the address to connect it to
    await expect(canvas.queryByTestId("scim-base-url")).toBeNull();
  },
};

// the whole point of the screen: the plaintext comes back once, from the create
// response, and the UI has to say so unmissably
export const IssueRevealsTheSecretOnce: Story = {
  render: () => {
    const stub = scoped(async (init) => {
      if (init?.method === "POST") {
        return json({
          ...token({ id: "tok-new", name: "Okta production" }),
          secret: "rolter_scim_deadbeef",
        });
      }
      return json([]);
    });
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the empty placeholder repeats the toolbar action, so both are on screen
    await userEvent.click((await canvas.findAllByRole("button", { name: /Issue token/ }))[0]);
    // sheets portal to document.body, so the panel is not under the canvas root
    const sheet = within(await within(document.body).findByRole("dialog"));
    await userEvent.type(sheet.getByPlaceholderText("Okta production"), "Okta production");
    await userEvent.click(sheet.getByRole("button", { name: /Issue token/ }));
    await waitFor(() =>
      expect(sheet.getByTestId("scim-token-secret")).toHaveTextContent("rolter_scim_deadbeef"),
    );
    await expect(sheet.getByText(/only time this token is shown/)).toBeVisible();
    // and it is copyable, because it can never be read back
    await expect(sheet.getByRole("button", { name: /Copy provisioning token/ })).toBeVisible();

    // the other value the connector needs is beside it (#2079): the control
    // plane's own address, never this page's origin, which is not the host the
    // identity provider has to call
    const base = await sheet.findByTestId("scim-base-url");
    await expect(base.textContent).toBe(SCIM_URL);
    await expect(base.textContent).not.toContain(window.location.origin);
    await expect(sheet.getByRole("group", { name: "SCIM base URL" })).toContainElement(base);
    await expect(
      sheet.getByRole("button", { name: `Copy SCIM base URL: ${SCIM_URL}` }),
    ).toBeVisible();
    // the hint no longer hands over a placeholder host to fill in by hand
    await expect(sheet.queryByText(/your-rolter-host/)).toBeNull();
    // a configured public url raises no warning
    await expect(sheet.queryByRole("note")).toBeNull();
  },
};

/**
 * #2079: the screen states the address an identity provider's SCIM connector is
 * pointed at, under the line that explains what the provider drives.
 *
 * It was a placeholder in the reveal step's hint and only a path in the lead, so
 * the operator worked out the public host by hand. The value is what the control
 * plane reports as its public url plus `/scim/v2`, and the story's own origin
 * is nowhere in it.
 */
export const ShowsTheScimBaseUrl: Story = {
  render: () => <Harness fetchStub={scoped(async () => json(TOKENS))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const group = await canvas.findByRole("group", { name: "SCIM base URL" });
    const base = await within(group).findByTestId("scim-base-url");
    await expect(base.textContent).toBe(SCIM_URL);
    await expect(base.textContent).not.toContain(window.location.origin);
    await expect(
      within(group).getByRole("button", { name: `Copy SCIM base URL: ${SCIM_URL}` }),
    ).toBeVisible();
    await expect(within(group).getByText(/same for every token in this org/)).toBeVisible();
    // a configured public url raises no warning
    await expect(canvas.queryByRole("note")).toBeNull();
  },
};

/**
 * #2079: with `ROLTER_PUBLIC_URL` unset the control plane's base is its default,
 * which an identity provider can only reach from the control plane's own host.
 * Both places the address appears say so, the same way the Single Sign-On screen
 * does, rather than leaving the provider's test console to say it later.
 */
export const WarnsWhenThePublicUrlIsUnset: Story = {
  render: () => {
    const stub = scoped(
      async (init) => {
        if (init?.method === "POST") {
          return json({
            ...token({ id: "tok-new", name: "Okta production" }),
            secret: "rolter_scim_deadbeef",
          });
        }
        return json(TOKENS);
      },
      undefined,
      { publicUrl: async () => json(UNSET) },
    );
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const group = await canvas.findByRole("group", { name: "SCIM base URL" });
    // the default address is still shown and copyable: it is what the control
    // plane will answer on, from its own host
    const base = await within(group).findByTestId("scim-base-url");
    await expect(base.textContent).toBe(`${DEFAULT_BASE}/scim/v2`);
    const notice = within(group).getByRole("note");
    await expect(notice).toHaveTextContent("ROLTER_PUBLIC_URL is not set");
    await expect(notice).toHaveTextContent(DEFAULT_BASE);
    await expect(notice).toHaveTextContent(/restart the control plane/);

    // and again in the reveal step, where the operator is about to paste it
    await userEvent.click(await canvas.findByRole("button", { name: /Issue token/ }));
    const sheet = within(await within(document.body).findByRole("dialog"));
    await userEvent.type(sheet.getByPlaceholderText("Okta production"), "Okta production");
    await userEvent.click(sheet.getByRole("button", { name: /Issue token/ }));
    const revealed = await sheet.findByTestId("scim-base-url");
    await expect(revealed.textContent).toBe(`${DEFAULT_BASE}/scim/v2`);
    await expect(sheet.getByRole("note")).toHaveTextContent("ROLTER_PUBLIC_URL is not set");
  },
};

/**
 * The public url is still in flight. The screen does not wait on it: the token
 * list is on screen, and the address holds its space as a labelled skeleton
 * rather than claiming a value or a failure.
 */
export const WaitsForThePublicUrl: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async () => json(TOKENS), undefined, {
        publicUrl: () => new Promise<Response>(() => {}),
      })}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Okta production")).toBeVisible());
    await expectSkeleton(canvasElement);
    await expect(canvas.queryByTestId("scim-base-url")).toBeNull();
    await expect(canvas.queryByRole("note")).toBeNull();
  },
};

/**
 * The public url could not be read. The address is not guessed from the
 * browser: the screen says what failed, offers a retry, and the address
 * appears once the retry lands.
 */
export const PublicUrlUnreadable: Story = {
  render: () => {
    let reads = 0;
    const stub = scoped(async () => json(TOKENS), undefined, {
      publicUrl: async () =>
        ++reads === 1
          ? json({ error: { message: "upstream unavailable" } }, 502)
          : json(PUBLIC_URL),
    });
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /failed to return the public URL/i);
    await expect(canvas.queryByTestId("scim-base-url")).toBeNull();
    // the rest of the screen is unaffected: the tokens were read
    await expect(canvas.getByText("Okta production")).toBeVisible();

    await userEvent.click(canvas.getByRole("button", { name: "Try again" }));
    const base = await canvas.findByTestId("scim-base-url");
    await expect(base.textContent).toBe(SCIM_URL);
  },
};

const revokedTokens: string[] = [];

/**
 * The mint is refused (#1607).
 *
 * `IssueRevealsTheSecretOnce` covers the answer; this covers the other one. The
 * sheet reports the control plane's own message and stays open with the name
 * typed, because closing it would drop the draft on a token that was never
 * issued.
 */
export const IssueRejectedByTheServer: Story = {
  render: () => {
    const stub = scoped(async (init) => {
      if (init?.method === "POST") {
        return json({ error: { message: "this org already has 10 provisioning tokens" } }, 409);
      }
      return json([]);
    });
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click((await canvas.findAllByRole("button", { name: /Issue token/ }))[0]);
    const panel = within(await within(document.body).findByRole("dialog"));
    const name = panel.getByPlaceholderText("Okta production");
    await userEvent.type(name, "Okta production");
    await userEvent.click(panel.getByRole("button", { name: /Issue token/ }));

    await waitFor(() =>
      expect(panel.getByText(/already has 10 provisioning tokens/)).toBeVisible(),
    );
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
    await expect(name).toHaveValue("Okta production");
  },
};

// revoking is immediate and does not touch the accounts already provisioned —
// the confirmation has to say that before the operator commits. the row stays
// in the list as revoked, so the toast says revoked and not deleted (#2080)
export const RevokeExplainsWhatItDoesNotDo: Story = {
  render: () => {
    revokedTokens.length = 0;
    const stub = scoped(async (init) => {
      if (init?.method === "DELETE") {
        revokedTokens.push("tok-1");
        return json(token({ revoked_at: NOW }));
      }
      return json([revokedTokens.length ? token({ revoked_at: NOW }) : token()]);
    });
    return <Harness fetchStub={stub} toasted />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // by name, not by position: the row control names the token (#1214)
    await userEvent.click(
      await canvas.findByRole("button", {
        name: "Revoke provisioning token Okta production",
      }),
    );
    const modal = within(await within(document.body).findByRole("dialog"));
    await expect(modal.getByText(/nobody is deactivated or logged out/)).toBeVisible();
    await userEvent.click(modal.getByRole("button", { name: "Revoke" }));
    // the DELETE itself, not just the badge: the row re-renders off a fixture
    // this story controls, so "Revoked" on screen would pass a screen that
    // never sent the request (#1607)
    await waitFor(() => expect(revokedTokens).toEqual(["tok-1"]));
    await waitFor(async () => {
      const [, status] = await cellsOf(canvasElement, "Okta production");
      await expect(status.textContent).toBe(en.pages.userProvisioning.statusRevoked);
    });
    // the toast describes what happened to the row that is still there
    await expectToast(canvasElement, /Okta production revoked/);
    await expect(canvas.queryByText(/deleted/i)).toBeNull();
  },
};

// the second half of the screen (#1186): the tokens decide who exists, the
// mappings decide what they may do. a mapping names its scope, so a team-scoped
// grant is distinguishable from an org-wide one at a glance
export const GroupMappingsListed: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(
        async () => json(TOKENS),
        async () => json(MAPPINGS),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("platform-engineering")).toBeVisible());
    // asserted per row rather than per string: the add form's own selects carry
    // the same scope and role labels as options
    await expect(canvas.getByText("platform-engineering").closest("li")).toHaveTextContent(
      "Whole organization",
    );
    const scoped = canvas.getByText("sre-oncall").closest("li");
    // team-1 is "core" in the scope stub, so the stored id is shown as its name
    await expect(scoped).toHaveTextContent("core");
    await expect(scoped).toHaveTextContent("Admin");
  },
};

/**
 * A scope whose lists failed to load says so, rather than printing the uuid it
 * could not name (#1671).
 *
 * The mapping row is read-only: there is no `LoadError` and no retry beside it,
 * so a chip reading `0f3a1c8e-…` is indistinguishable from a team that happens
 * to be called that, and the card quietly claims to answer "what does this
 * grant" while the answer is an id.
 */
export const GroupMappingScopeUnresolved: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(
        async () => json(TOKENS),
        async () => json(MAPPINGS),
        {
          teams: async () => json({ error: { message: "teams unavailable" } }, 500),
          orgProjects: async () => json({ error: { message: "projects unavailable" } }, 500),
        },
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const row = await waitFor(() => {
      const li = canvas.getByText("sre-oncall").closest("li");
      expect(li).not.toBeNull();
      return li as HTMLElement;
    });
    // the chip names the failure instead of the id
    await waitFor(() => expect(row).toHaveTextContent("Unresolved scope"));
    await expect(row).not.toHaveTextContent("team-1");
    // and the id is still quotable, in the tooltip
    const chip = within(row).getByTitle(/could not be matched/);
    await expect(chip).toHaveTextContent("Unresolved scope");
    await expect(chip.getAttribute("title")).toContain("team-1");
    // an org-wide mapping is untouched: it never had a scope to resolve
    await expect(canvas.getByText("platform-engineering").closest("li")).toHaveTextContent(
      "Whole organization",
    );
  },
};

// an org with tokens but no mappings is the trap the screen has to name: the
// IdP syncs happily and everyone it provisions can still do nothing
export const GroupMappingsEmpty: Story = {
  render: () => <Harness fetchStub={scoped(async () => json(TOKENS))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText(/every provisioned account joins as a viewer/)).toBeVisible(),
    );
  },
};

// what the stub recorded, asserted in `play`. a module-level sink rather than a
// second fetch wrapper: the stub is already the only thing the screen talks to
const postedMappings: unknown[] = [];

// the scope select is why this is more than a group/role pair — a team-scoped
// grant has to send the team id, and only the team id
export const MapGroupPostsTheScopedRole: Story = {
  render: () => {
    postedMappings.length = 0;
    const stub = scoped(
      async () => json(TOKENS),
      async (init) => {
        if (init?.method === "POST") {
          postedMappings.push(JSON.parse(String(init.body)));
          return json(mapping({ id: "map-new", group_name: "sre-oncall", role: "admin" }));
        }
        return json(
          postedMappings.length
            ? [mapping({ id: "map-new", group_name: "sre-oncall", role: "admin" })]
            : [],
        );
      },
    );
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await canvas.findByLabelText("IdP group"), "sre-oncall");
    await pickOption(canvas.getByLabelText("Where the role applies"), "core");
    await pickOption(canvas.getByLabelText("Role to grant"), "Admin");
    await userEvent.click(canvas.getByRole("button", { name: "Map group" }));
    await waitFor(() => expect(postedMappings).toHaveLength(1));
    await expect(postedMappings[0]).toEqual({
      group_name: "sre-oncall",
      role: "admin",
      team_id: "team-1",
    });
    await waitFor(() => expect(canvas.getByText("sre-oncall")).toBeVisible());
  },
};

/**
 * The mapping is refused (#1607).
 *
 * This form clears the group name on success only, so a rejected POST has to
 * leave it standing — and the refusal is rendered inline beside the form that
 * caused it rather than in the toast queue.
 */
export const MapGroupRejectedByTheServer: Story = {
  render: () => {
    const stub = scoped(
      async () => json(TOKENS),
      async (init) => {
        if (init?.method === "POST") {
          return json({ error: { message: "sre-oncall is already mapped" } }, 409);
        }
        return json([]);
      },
    );
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const group = await canvas.findByLabelText("IdP group");
    await userEvent.type(group, "sre-oncall");
    await pickOption(canvas.getByLabelText("Role to grant"), "Admin");
    await userEvent.click(canvas.getByRole("button", { name: "Map group" }));

    await waitFor(() =>
      expect(
        canvas.getAllByRole("alert").some((a) => /already mapped/.test(a.textContent ?? "")),
      ).toBe(true),
    );
    await expect(group).toHaveValue("sre-oncall");
  },
};

const deletedMappings: string[] = [];

// removing a mapping withdraws a role from everyone in the group, so it goes
// through ConfirmDialog and says so before the DELETE goes out (#1179)
export const RemoveMappingConfirmsFirst: Story = {
  render: () => {
    deletedMappings.length = 0;
    const stub = scoped(
      async () => json([]),
      async (init) => {
        if (init?.method === "DELETE") {
          deletedMappings.push("deleted");
          return new Response(null, { status: 204 });
        }
        return json(deletedMappings.length ? [] : [mapping()]);
      },
    );
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", {
        name: "Remove the mapping for platform-engineering",
      }),
    );
    const modal = within(await within(document.body).findByRole("dialog"));
    await expect(modal.getByText(/loses Member straight away/)).toBeVisible();
    await userEvent.click(modal.getByRole("button", { name: "Remove mapping" }));
    await waitFor(() => expect(deletedMappings).toHaveLength(1));
    await waitFor(() => expect(canvas.queryByText("platform-engineering")).toBeNull());
  },
};

// the bug #1249 was filed for: the scope select used to list only the projects
// of the team the switcher had selected, so a mapping onto a project in another
// team could not be written without moving the switcher first. the picker now
// groups every project under its own team, and posts that project's id
export const MapGroupToAProjectInAnotherTeam: Story = {
  render: () => {
    postedMappings.length = 0;
    const stub = scoped(
      async () => json(TOKENS),
      async (init) => {
        if (init?.method === "POST") {
          postedMappings.push(JSON.parse(String(init.body)));
          return json(
            mapping({
              id: "map-new",
              group_name: "checkout-oncall",
              role: "member",
              project_id: "proj-2",
            }),
          );
        }
        return json(
          postedMappings.length
            ? [
                mapping({
                  id: "map-new",
                  group_name: "checkout-oncall",
                  role: "member",
                  project_id: "proj-2",
                }),
              ]
            : [],
        );
      },
    );
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const scope = await canvas.findByLabelText("Where the role applies");
    // grouped by team, so two teams may each have a "prod" without the reader
    // having to guess which one an option means
    await waitFor(async () =>
      expect(
        within(await openOptions(scope)).getByRole("group", { name: "Projects in payments" }),
      ).toBeInTheDocument(),
    );
    await userEvent.keyboard("{Escape}");
    await userEvent.type(await canvas.findByLabelText("IdP group"), "checkout-oncall");
    await pickOption(scope, "checkout");
    await pickOption(canvas.getByLabelText("Role to grant"), "Member");
    await userEvent.click(canvas.getByRole("button", { name: "Map group" }));
    await waitFor(() => expect(postedMappings).toHaveLength(1));
    await expect(postedMappings[0]).toEqual({
      group_name: "checkout-oncall",
      role: "member",
      project_id: "proj-2",
    });
    // and the listed mapping names that project, not its raw id
    await waitFor(() =>
      expect(canvas.getByText("checkout-oncall").closest("li")).toHaveTextContent("checkout"),
    );
  },
};

// an org with no teams can only be mapped org-wide, and the form says so rather
// than offering a select with one option and no explanation
export const ScopePickerHasNoTeams: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(
        async () => json(TOKENS),
        async () => json([]),
        { teams: async () => json([]) },
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText(/no teams yet/)).toBeVisible());
  },
};

// the team list the picker reads can fail on its own: the narrower scopes go
// away, the org-wide mapping the operator was probably writing does not
export const ScopePickerCannotListTeams: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(
        async () => json(TOKENS),
        async () => json([]),
        { teams: async () => json({ error: { message: "boom" } }, 500) },
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /teams and projects/);
    await expect(canvas.getByLabelText("Where the role applies")).toBeVisible();
  },
};

// A SCIM token is what an IdP presents to provision accounts, and a group
// mapping is what turns a directory group into a role — both `admin` at every
// action (#1606).
export const RefusedToAViewer: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(
        async () => json(TOKENS),
        async () => json(MAPPINGS),
      )}
      role="viewer"
    />
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Issue token");
    await expectRefused(canvasElement, "Revoke provisioning token Okta production");
    await expectRefused(canvasElement, "Map group");
    await expectRefused(canvasElement, "Remove the mapping for platform-engineering");
  },
};

export const RefusedToAMember: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(
        async () => json(TOKENS),
        async () => json(MAPPINGS),
      )}
      role="member"
    />
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Issue token");
    await expectRefused(canvasElement, "Map group");
  },
};
