import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import ApiTokens from "./ApiTokens";
import {
  answerSecretClosePrompt,
  clickWhenEnabled,
  confirmation,
  confirmDestructive,
  expectAllowed,
  expectEmptyState,
  expectListTable,
  expectLoadError,
  expectNoFalseEmpty,
  expectRefused,
  expectSkeleton,
  Harness,
  json,
  NEEDS_ADMIN,
  ORG,
  pending,
  pickOption,
  PROJECT,
  recording,
  scoped,
  sheet,
  stubClipboard,
  TEAM,
  type FetchStub,
} from "./story-harness";
import type { ApiTokenRow, MintedApiToken } from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";

const NOW = Date.now();
const DAY = 86_400_000;
const inDays = (days: number) => new Date(NOW + days * DAY).toISOString();

// built at run time: a literal token-shaped value would trip the secret scan
const PREFIX = "rolter_api_";

const token = (over: Partial<ApiTokenRow> = {}): ApiTokenRow => ({
  id: "tok-1",
  org_id: ORG.id,
  team_id: null,
  project_id: null,
  name: "ci-deploy",
  role: "member",
  custom_role_id: null,
  token_prefix: `${PREFIX}a1b2`,
  expires_at: inDays(40),
  created_by: "user-1",
  created_at: "2026-08-01T10:00:00Z",
  rotated_at: null,
  last_used_at: "2026-09-30T08:00:00Z",
  revoked_at: null,
  ...over,
});

const TOKENS: ApiTokenRow[] = [
  token(),
  token({
    id: "tok-2",
    name: "gitops-routes",
    team_id: TEAM.id,
    project_id: PROJECT.id,
    role: "admin",
    token_prefix: `${PREFIX}c3d4`,
    expires_at: null,
    last_used_at: null,
  }),
  token({
    id: "tok-3",
    name: "old-migration",
    role: null,
    custom_role_id: "role-1",
    token_prefix: `${PREFIX}e5f6`,
    expires_at: inDays(-3),
  }),
  token({
    id: "tok-4",
    name: "leaked-demo",
    token_prefix: `${PREFIX}0789`,
    revoked_at: "2026-09-01T00:00:00Z",
  }),
];

const SECRET = [PREFIX, "S3cretValueShownOnce"].join("");

const minted = (over: Partial<MintedApiToken> = {}): MintedApiToken => ({
  ...token({ id: "tok-new", name: "ci-new", token_prefix: `${PREFIX}n3w0` }),
  secret: SECRET,
  ...over,
});

function api({
  list = () => json(TOKENS),
  mint,
  rotate,
  revoke,
}: {
  list?: () => Response | Promise<Response>;
  mint?: (body: Record<string, unknown>) => Response | Promise<Response>;
  rotate?: () => Response | Promise<Response>;
  revoke?: () => Response | Promise<Response>;
} = {}): FetchStub {
  return scoped(async (input, init) => {
    const url = String(input);
    const method = (init?.method ?? "GET").toUpperCase();
    if (url.includes("/custom-roles")) {
      return json([
        {
          id: "role-1",
          org_id: ORG.id,
          slug: "key-rotator",
          name: "Key rotator",
          description: null,
          base_role: "member",
          created_at: "2026-01-01T00:00:00Z",
          updated_at: "2026-01-01T00:00:00Z",
        },
      ]);
    }
    if (url.includes("/rotate"))
      return rotate ? rotate() : json(minted({ id: "tok-1", name: "ci-deploy" }));
    if (url.includes("/api/v1/api-tokens/") && method === "DELETE") {
      return revoke ? revoke() : json(token({ id: "tok-1", revoked_at: new Date().toISOString() }));
    }
    if (url.includes("/api-tokens")) {
      if (method === "POST") {
        const body = JSON.parse(String(init?.body)) as Record<string, unknown>;
        return mint ? mint(body) : json(minted(), 201);
      }
      return list();
    }
    return json([]);
  });
}

const meta = {
  title: "Screens/ApiTokens",
  component: ApiTokens,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof ApiTokens>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={api()}>
      <ApiTokens />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectListTable(canvasElement, "API Tokens");
    await waitFor(() => expect(canvas.getByText("ci-deploy")).toBeVisible());
    await expect(canvas.getByText("rolter_api_a1b2…")).toBeVisible();
    // the custom role is named, not shown as an id
    await waitFor(() => expect(canvas.getByText("Key rotator")).toBeVisible());
    await expect(canvas.getByText("Never used")).toBeVisible();
    await expect(canvas.getAllByText("Active")).toHaveLength(2);
    await expect(canvas.getByText("Expired")).toBeVisible();
    await expect(canvas.getByText("Revoked")).toBeVisible();
    // a token with no expiry says so
    await expect(canvas.getByText("Never")).toBeVisible();
    // a dead token cannot be rotated, and a revoked one cannot be revoked twice
    await expect(canvas.getByRole("button", { name: "Rotate token old-migration" })).toBeDisabled();
    await expect(canvas.getByRole("button", { name: "Rotate token leaked-demo" })).toBeDisabled();
    await expect(canvas.getByRole("button", { name: "Revoke token leaked-demo" })).toBeDisabled();
    await expect(canvas.getByRole("button", { name: "Revoke token ci-deploy" })).toBeEnabled();
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <ApiTokens />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No API tokens yet/);
  },
};

export const Empty: Story = {
  render: () => (
    <Harness fetchStub={api({ list: () => json([]) })}>
      <ApiTokens />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No API tokens yet/, /Mint token/);
  },
};

export const LoadFailed: Story = {
  render: () => (
    <Harness
      fetchStub={api({ list: () => json({ error: { message: "token store is down" } }, 500) })}
    >
      <ApiTokens />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /token store is down/);
    await expectNoFalseEmpty(canvasElement, /No API tokens yet/);
  },
};

const blockedMint = recording(api());

/** A token without a name is refused by naming the field; Enter mints once it has one (#3242). */
export const BlockedSubmitNamesTheTokenName: Story = {
  beforeEach: stubClipboard(async () => {}),
  render: () => (
    <Harness fetchStub={blockedMint.stub}>
      <ApiTokens />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Mint token/);
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    const name = panel.getByLabelText("Name");
    await expect(name).toBeRequired();
    await expect(panel.queryByText("Required")).toBeNull();

    await userEvent.click(panel.getByRole("button", { name: "Mint token" }));
    await waitFor(async () => expect(await panel.findByText("Required")).toBeVisible());
    await waitFor(() => expect(name).toHaveFocus());
    await expect(await panel.findByRole("status")).toHaveTextContent("1 field needs attention");
    blockedMint.expectNotSent("POST", `/orgs/${ORG.id}/api-tokens`);

    await userEvent.type(name, "ci-new{Enter}");
    await expect(
      await blockedMint.expectSentBody("POST", `/orgs/${ORG.id}/api-tokens`),
    ).toMatchObject({ name: "ci-new" });
  },
};

const mints = recording(api());

/** The body carries the picked scope, role and expiry, and the secret is shown once. */
export const MintsATokenAndShowsTheSecretOnce: Story = {
  beforeEach: stubClipboard(async () => {}),
  render: () => (
    <Harness fetchStub={mints.stub}>
      <ApiTokens />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Mint token/);
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    await userEvent.type(panel.getByLabelText("Name"), "ci-new");
    await pickOption(panel.getByLabelText("Role"), "Viewer");
    await pickOption(panel.getByLabelText("Expires"), "In 30 days");
    await userEvent.click(panel.getByRole("button", { name: "Mint token" }));

    const body = await mints.expectSentBody<Record<string, unknown>>(
      "POST",
      `/orgs/${ORG.id}/api-tokens`,
    );
    await expect(body.name).toBe("ci-new");
    await expect(body.role).toBe("viewer");
    await expect(body.expires_in_days).toBe(30);
    await expect(body.never_expires).toBeUndefined();
    await expect(body.custom_role_id).toBeUndefined();
    await expect(body.team_id).toBeUndefined();

    const dialog = await within(document.body).findByRole("dialog", {
      name: /Token ci-new is ready/,
    });
    await waitFor(() => expect(within(dialog).getByText(SECRET)).toBeVisible());
    const copy = await within(dialog).findByRole("button", { name: /^Copy token/ });
    await userEvent.click(copy);
    await userEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(within(document.body).queryByText(SECRET)).toBeNull());
  },
};

const customMint = recording(api());

export const MintsWithACustomRoleAndNoExpiry: Story = {
  render: () => (
    <Harness fetchStub={customMint.stub}>
      <ApiTokens />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Mint token/);
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    await userEvent.type(panel.getByLabelText("Name"), "rotator");
    await pickOption(panel.getByLabelText("Role"), /Key rotator/);
    await pickOption(panel.getByLabelText("Expires"), "Never expires");
    await waitFor(() =>
      expect(panel.getByText(/stays valid until someone revokes it/)).toBeVisible(),
    );
    await userEvent.click(panel.getByRole("button", { name: "Mint token" }));

    const body = await customMint.expectSentBody<Record<string, unknown>>(
      "POST",
      `/orgs/${ORG.id}/api-tokens`,
    );
    await expect(body.custom_role_id).toBe("role-1");
    await expect(body.role).toBeUndefined();
    await expect(body.never_expires).toBe(true);
    await expect(body.expires_in_days).toBeUndefined();
  },
};

/** An uncopied secret asks before it is dropped. */
export const AnUncopiedSecretAsksBeforeClosing: Story = {
  render: () => (
    <Harness fetchStub={api()}>
      <ApiTokens />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Mint token/);
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    await userEvent.type(panel.getByLabelText("Name"), "ci-new");
    await userEvent.click(panel.getByRole("button", { name: "Mint token" }));
    const dialog = await within(document.body).findByRole("dialog", {
      name: /Token ci-new is ready/,
    });
    await userEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    await answerSecretClosePrompt(false);
    await waitFor(() => expect(within(dialog).getByText(SECRET)).toBeVisible());
    await userEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    await answerSecretClosePrompt(true);
    await waitFor(() => expect(within(document.body).queryByText(SECRET)).toBeNull());
  },
};

export const MintRefusedByTheServerStaysInTheSheet: Story = {
  render: () => (
    <Harness
      fetchStub={api({
        mint: () =>
          json(
            {
              error: {
                message: "role must be one of admin, member, viewer",
                code: "invalid_field",
                field: "role",
              },
            },
            400,
          ),
      })}
    >
      <ApiTokens />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Mint token/);
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    await userEvent.type(panel.getByLabelText("Name"), "ci-new");
    await userEvent.click(panel.getByRole("button", { name: "Mint token" }));
    await waitFor(() =>
      expect(
        panel.getAllByRole("alert").some((a) => /role must be one of/.test(a.textContent ?? "")),
      ).toBe(true),
    );
    await expect(sheet()).toBeVisible();
  },
};

const rotates = recording(api());

export const RotatesAfterAConfirmationAndShowsTheNewSecret: Story = {
  render: () => (
    <Harness fetchStub={rotates.stub}>
      <ApiTokens />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", { name: "Rotate token ci-deploy" }),
    );
    rotates.expectNotSent("POST", "/rotate");
    await confirmDestructive(/Rotate token ci-deploy\?/, "Rotate token");
    await rotates.expectSent("POST", "/api/v1/api-tokens/tok-1/rotate");
    const dialog = await within(document.body).findByRole("dialog", {
      name: /Token ci-deploy was rotated/,
    });
    await waitFor(() => expect(within(dialog).getByText(SECRET)).toBeVisible());
  },
};

/** The token went inactive since the list was read: the server's 409 stays in the dialog. */
export const RotatingAnInactiveTokenIsRefused: Story = {
  render: () => (
    <Harness
      fetchStub={api({
        rotate: () =>
          json(
            {
              error: {
                message: "the control plane's own english, not shown once the code is known",
                code: "api_token_inactive",
              },
            },
            409,
          ),
      })}
    >
      <ApiTokens />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", { name: "Rotate token ci-deploy" }),
    );
    await confirmDestructive(/Rotate token ci-deploy\?/, "Rotate token");
    const dialog = await confirmation();
    await waitFor(() =>
      expect(within(dialog).getByRole("alert")).toHaveTextContent(
        /revoked or has expired, so there is nothing to rotate/,
      ),
    );
  },
};

const revokes = recording(api());

export const RevokesAfterAConfirmation: Story = {
  render: () => (
    <Harness fetchStub={revokes.stub}>
      <ApiTokens />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", { name: "Revoke token ci-deploy" }),
    );
    revokes.expectNotSent("DELETE", "/api/v1/api-tokens/");
    await confirmDestructive(/Revoke token ci-deploy\?/, "Revoke token");
    await revokes.expectSent("DELETE", "/api/v1/api-tokens/tok-1");
  },
};

export const RefusedToAMember: Story = {
  render: () => (
    <Harness fetchStub={api()} role="member">
      <ApiTokens />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, /Mint token/);
    await expectRefused(canvasElement, "Rotate token ci-deploy", NEEDS_ADMIN);
    await expectRefused(canvasElement, "Revoke token ci-deploy", NEEDS_ADMIN);
  },
};

export const AllowedToAnAdmin: Story = {
  render: () => (
    <Harness fetchStub={api()} role="admin">
      <ApiTokens />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, /Mint token/);
    await expectAllowed(canvasElement, "Rotate token ci-deploy");
    await expectAllowed(canvasElement, "Revoke token ci-deploy");
  },
};

/** A plain-http dashboard has no clipboard: the failure stays on screen and the secret stays selectable. */
export const AFailedCopyKeepsTheSecretAndSaysSo: Story = {
  beforeEach: stubClipboard(() => Promise.reject(new Error("denied"))),
  render: () => (
    <Harness fetchStub={api()}>
      <ApiTokens />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /Mint token/);
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    await userEvent.type(panel.getByLabelText("Name"), "ci-new");
    await userEvent.click(panel.getByRole("button", { name: "Mint token" }));
    const dialog = await within(document.body).findByRole("dialog", {
      name: /Token ci-new is ready/,
    });
    await userEvent.click(await within(dialog).findByRole("button", { name: /^Copy token/ }));
    await expect(await within(dialog).findByRole("alert")).toHaveTextContent(en.common.copyFailed);
    await waitFor(() => expect(within(dialog).getByText(SECRET)).toBeVisible());
    await expect(window.getSelection()?.toString()).toBe(SECRET);
  },
};
