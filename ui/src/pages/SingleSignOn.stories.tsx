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

/** every request the stub saw, with its parsed JSON body */
interface Sent {
  method: string;
  url: string;
  body?: unknown;
}

/**
 * Record what actually left, bodies included.
 *
 * The shared `recording` helper keeps method and URL, which answers "did the
 * DELETE leave". These stories also have to answer "did the create send the
 * *right* provider", and the write-only client secret is exactly the field a
 * screen could plausibly drop on the floor — so the body is kept too.
 */
function record(handler: FetchStub): { stub: FetchStub; calls: Sent[] } {
  const calls: Sent[] = [];
  return {
    calls,
    stub: async (input, init) => {
      calls.push({
        method: (init?.method ?? "GET").toUpperCase(),
        url: String(input),
        body: init?.body ? JSON.parse(String(init.body)) : undefined,
      });
      return handler(input, init);
    },
  };
}

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
const creates = record(api({ providers: () => [provider()] }));

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

    await waitFor(() => {
      const post = creates.calls.find(
        (c) => c.method === "POST" && c.url.includes("/sso-providers"),
      );
      expect(post).toBeDefined();
      expect(post?.url).toContain(`/api/v1/orgs/${ORG.id}/sso-providers`);
      expect(post?.body).toEqual({
        name: "Acme Okta",
        slug: "okta",
        issuer: "https://acme.okta.com",
        client_id: "0oa1b2c3d4",
        client_secret: "s3cr3t",
        // omitted optionals are left out entirely, so the server applies its
        // own defaults rather than being handed an empty string
      });
    });
  },
};

// #1233: editing in place. before this, rotating a secret or fixing a typo
// meant deleting the provider and registering it again, which dropped every
// group mapping and changed the id in the audit trail
const edits = record(api({ providers: () => [provider()] }));

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

    await waitFor(() => {
      const put = edits.calls.find((c) => c.method === "PUT");
      expect(put).toBeDefined();
      expect(put?.url).toContain("/api/v1/sso-providers/sso-1");
      const body = put?.body as Record<string, unknown>;
      expect(body.client_id).toBe("0oa-rotated");
      expect(body.name).toBe("Acme Okta");
      // the untouched secret field sends nothing at all, which is what tells
      // the server to leave the sealed one alone
      expect("client_secret" in body).toBe(false);
      // and the slug is never sent, so it cannot be changed by accident
      expect("slug" in body).toBe(false);
    });
  },
};

// a provider is taken out of service with a switch instead of a delete
const toggles = record(api({ providers: () => [provider()] }));

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

    await waitFor(() => {
      const put = toggles.calls.find((c) => c.method === "PUT");
      expect(put).toBeDefined();
      const body = put?.body as Record<string, unknown>;
      expect(body.enabled).toBe(false);
      // everything else rides along unchanged, and the secret is untouched
      expect(body.name).toBe("Acme Okta");
      expect("client_secret" in body).toBe(false);
    });
    // nothing was deleted: the group mappings that hang off this provider are
    // exactly what delete-and-recreate used to destroy
    await expect(
      toggles.calls.some((c) => c.method === "DELETE"),
    ).toBe(false);
  },
};

// deleting a provider takes a whole sign-in route away, so it is named and
// confirmed before anything leaves (#1179)
const deletes = record(api({ providers: () => [provider()] }));

export const DeletesWithConfirmation: Story = {
  render: () => (
    <Harness fetchStub={deletes.stub}>
      <SingleSignOn />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Acme Okta")).toBeVisible());
    const sent = () =>
      deletes.calls.some(
        (c) => c.method === "DELETE" && c.url.includes("/sso-providers/sso-1"),
      );

    await userEvent.click(canvas.getByLabelText("Delete provider Acme Okta"));
    await cancelConfirmation();
    expect(sent()).toBe(false);

    await userEvent.click(canvas.getByLabelText("Delete provider Acme Okta"));
    await confirmDestructive(/Acme Okta/, "Delete provider");
    await waitFor(() => expect(sent()).toBe(true));
  },
};

// both flags travel together because the control plane refuses the combination,
// not the field
const policySave = record(api({ providers: () => [provider()] }));

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

    await waitFor(() => {
      const put = policySave.calls.find((c) => c.method === "PUT");
      expect(put?.url).toContain(`/api/v1/orgs/${ORG.id}/auth-policy`);
      expect(put?.body).toEqual({
        allow_password_login: false,
        allow_sso: true,
      });
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
