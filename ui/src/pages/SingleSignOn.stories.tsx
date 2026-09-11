import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import SingleSignOn from "./SingleSignOn";
import {
  Harness,
  ORG,
  Toasted,
  cancelConfirmation,
  clickWhenEnabled,
  confirmDestructive,
  expectToast,
  json,
  pending,
  recording,
  scoped,
  sheet,
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
  ],
  "sso-2": [],
};

const POLICY: OrgAuthPolicy = {
  org_id: ORG.id,
  allow_password_login: true,
  allow_sso: true,
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
