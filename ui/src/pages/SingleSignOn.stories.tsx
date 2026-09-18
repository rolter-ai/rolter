import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import SingleSignOn from "./SingleSignOn";
import {
  cancelConfirmation,
  clickWhenEnabled,
  confirmation,
  confirmDestructive,
  expectRefused,
  openOptions,
  PROJECT,
  TEAM,
  expectToast,
  Harness,
  json,
  ORG,
  pending,
  pickOption,
  recording,
  scoped,
  sheet,
  Toasted,
  type FetchStub,
} from "./story-harness";
import type {
  OrgAuthPolicy,
  SsoGroupMappingRow,
  SsoProviderRow,
} from "@/lib/api";

const NOW = "2026-08-01T10:00:00Z";

const provider = (over: Partial<SsoProviderRow> = {}): SsoProviderRow => ({
  id: "sso-1",
  org_id: ORG.id,
  name: "Acme Okta",
  slug: "okta",
  issuer: "https://acme.okta.com",
  client_id: "0oa1b2c3d4",
  has_client_secret: true,
  scopes: ["openid", "email", "profile"],
  group_claim: "groups",
  default_role: "member",
  enabled: true,
  created_at: NOW,
  ...over,
});

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
  updated_at: NOW,
};

/**
 * The screen's three endpoints, routed by path.
 *
 * `/sso-providers/{id}/group-mappings` contains `sso-providers`, so the
 * mappings branch has to come first or the provider list answers it and every
 * card renders the provider array as its groups.
 */
function api({
  providers = () => PROVIDERS as unknown,
  policy = () => POLICY as unknown,
  status = 200,
}: {
  providers?: () => unknown;
  policy?: () => unknown;
  status?: number;
} = {}): FetchStub {
  // a 204 carries no body at all — `new Response(body, { status: 204 })` throws
  const noContent = () => new Response(null, { status: 204 });
  return scoped(async (input, init) => {
    const url = String(input);
    const method = (init?.method ?? "GET").toUpperCase();
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

    // the login URL is the thing an operator has to hand to the IdP, so it is
    // on the card and copyable rather than something to reconstruct by hand
    await expect(
      canvas.getByText(new RegExp("/auth/sso/okta/start")),
    ).toBeVisible();
    await expect(canvas.getByText("https://acme.okta.com")).toBeVisible();

    // a group mapping is the thing that grants a role. its own request is
    // separate from the provider list, so it settles after the card is drawn
    await waitFor(() =>
      expect(canvas.getByText("platform-engineering")).toBeVisible(),
    );

    // a provider with no default role refuses an unmapped user, and says so
    await expect(canvas.getByText(/No default role/)).toBeVisible();

    // the org policy is the same screen: both ways in are on here
    await expect(
      canvas.getByRole("switch", { name: "Password sign-in" }),
    ).toBeChecked();
    await expect(
      canvas.getByRole("switch", { name: "Single sign-on" }),
    ).toBeChecked();
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
    await waitFor(() =>
      expect(canvas.getByText("No client secret")).toBeVisible(),
    );
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
    await expect(
      canvas.queryByRole("button", { name: /Add provider/ }),
    ).not.toBeInTheDocument();
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
    await waitFor(() =>
      expect(canvas.getByText("No identity provider yet")).toBeVisible(),
    );
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
        status: 403,
      })}
    >
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(
        canvas.getByText("You do not have access to identity providers"),
      ).toBeVisible(),
    );
    await expect(
      canvas.getByText("You do not have access to the sign-in policy"),
    ).toBeVisible();
    // a 403 gets no retry button, and nothing to press that would 403 again
    await expect(canvas.queryByRole("button", { name: /Try again/ })).toBeNull();
    await expect(
      canvas.getByRole("button", { name: /Add provider/ }),
    ).toBeDisabled();
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
    await userEvent.type(
      panel.getByLabelText("Issuer URL"),
      "https://acme.okta.com",
    );
    await userEvent.type(panel.getByLabelText("Client ID"), "0oa1b2c3d4");
    await userEvent.type(panel.getByLabelText("Client secret"), "s3cr3t");

    // and the sheet is unambiguous that this is the only sighting of it
    await expect(panel.getByText(/never shown again/)).toBeVisible();

    await userEvent.click(panel.getByRole("button", { name: "Add provider" }));

    const created = await creates.expectSentBody(
      "POST",
      `/api/v1/orgs/${ORG.id}/sso-providers`,
    );
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
        (init?.method ?? "GET").toUpperCase() === "POST" &&
        String(input).includes("/sso-providers")
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
    await waitFor(() =>
      expect(within(document.body).getByRole("dialog")).toBeInTheDocument(),
    );
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
    await userEvent.click(
      canvas.getByLabelText("Remove the stored client secret for Acme Okta"),
    );
    await cancelConfirmation();
    clears.expectNotSent("PUT", "/api/v1/sso-providers/sso-1");

    await userEvent.click(
      canvas.getByLabelText("Remove the stored client secret for Acme Okta"),
    );
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
    await waitFor(() =>
      expect(canvas.getByText("No client secret")).toBeVisible(),
    );
    await expect(canvas.getByText("Not set")).toBeVisible();
    // with nothing stored, the control that removes one is gone
    await expect(
      canvas.queryByLabelText("Remove the stored client secret for Acme Okta"),
    ).toBeNull();
  },
};

// a provider is taken out of service with a switch instead of a delete
const toggles = recording(api({ providers: () => [provider()] }));

export const DisablesAProviderWithoutDeletingIt: Story = {
  render: () => (
    <Harness fetchStub={toggles.stub}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const toggle = await canvas.findByRole("switch", {
      name: "Enable provider Acme Okta",
    });
    await userEvent.click(toggle);

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
    await cancelConfirmation();
    deletes.expectNotSent("DELETE", "/sso-providers/sso-1");

    await userEvent.click(canvas.getByLabelText("Delete provider Acme Okta"));
    await confirmDestructive(/Acme Okta/, "Delete provider");
    await deletes.expectSent("DELETE", "/sso-providers/sso-1");
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

    await userEvent.click(
      canvas.getByRole("switch", { name: "Password sign-in" }),
    );
    await waitFor(() => expect(save).toBeEnabled());
    await userEvent.click(save);

    await expect(
      await policySave.expectSentBody(
        "PUT",
        `/api/v1/orgs/${ORG.id}/auth-policy`,
      ),
    ).toEqual({
      allow_password_login: false,
      allow_sso: true,
      // the second-factor policy travels with the two flags even when it is
      // the one thing that did not change: omitting it would be read as "keep
      // the current value", which is right, but sending it is what makes this
      // assertion prove the field is wired at all (#1078)
      mfa_policy: "off",
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

    await userEvent.click(
      canvas.getByRole("switch", { name: "Password sign-in" }),
    );
    await userEvent.click(canvas.getByRole("switch", { name: "Single sign-on" }));

    await waitFor(() =>
      expect(canvas.getByText(/At least one sign-in method/)).toBeVisible(),
    );
    await expect(save).toBeDisabled();
  },
};

// --- the second-factor policy (#1078) -------------------------------------

const mfaSave = recording(api({ providers: () => [provider()] }));

/**
 * Tightening `mfa_policy` is the one setting on this card that can lock people
 * out without being wrong, so it confirms first — and the confirmation names
 * the way back in rather than only the consequence.
 */
export const RequiringASecondFactorWarnsAboutTheLockout: Story = {
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
    await expect(
      within(dialog).getByText(/cannot set one up without signing in first/i),
    ).toBeVisible();
    await expect(within(dialog).getByText(/rolter mfa reset/)).toBeVisible();

    // backing out sends nothing: the policy is unchanged until it is confirmed
    await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    mfaSave.expectNotSent("PUT", "/auth-policy");

    await userEvent.click(canvas.getByRole("button", { name: "Save policy" }));
    await confirmDestructive(/cannot set one up/, "Require it");
    await expect(
      await mfaSave.expectSentBody("PUT", `/api/v1/orgs/${ORG.id}/auth-policy`),
    ).toEqual({
      allow_password_login: true,
      allow_sso: true,
      mfa_policy: "required_all",
    });
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
    await expectRefused(
      canvasElement,
      "Remove the stored client secret for Acme Okta",
    );
    await expectRefused(
      canvasElement,
      "Remove the mapping for platform-engineering",
    );
    // #1234: writing a mapping at a narrower scope is the same capability as
    // writing an org-wide one, so the new scope select must not come with a
    // create button that only fails on submit
    await expectRefused(canvasElement, "Map a group in Acme Okta");
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
    await expect(
      within(listbox).getByRole("option", { name: TEAM.name }),
    ).toBeVisible();
    // a project in that team is reachable without moving the scope switcher
    await expect(
      within(listbox).getByRole("option", { name: PROJECT.name }),
    ).toBeVisible();
    await userEvent.click(
      within(listbox).getByRole("option", { name: TEAM.name }),
    );

    await userEvent.type(canvas.getByLabelText("IdP group"), "gateway-oncall");
    await clickWhenEnabled(canvasElement, "Map a group in Acme Okta");

    const body = await teamScoped.expectSentBody<Record<string, unknown>>(
      "POST",
      "/api/v1/sso-providers/sso-1/group-mappings",
    );
    await expect(body.group_name).toBe("gateway-oncall");
    // the narrower scope is the whole point: `team_id` set, and `project_id`
    // left out entirely rather than sent as null, which the server would read
    // as the more specific scope
    await expect(body.team_id).toBe(TEAM.id);
    await expect(body.project_id).toBeUndefined();
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
    await userEvent.click(
      canvas.getByLabelText("Remove the mapping for gateway-oncall"),
    );

    const dialog = within(await confirmation());
    await expect(dialog.getByText(new RegExp(TEAM.name))).toBeVisible();
    await cancelConfirmation();
  },
};
