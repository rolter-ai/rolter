import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Ldap from "./Ldap";
import {
  confirmation,
  confirmDestructive,
  clickWhenEnabled,
  expectAllowed,
  expectEmptyState,
  expectLoadError,
  expectNoFalseEmpty,
  expectRefused,
  expectSkeleton,
  Harness,
  json,
  NEEDS_ADMIN,
  ORG,
  pending,
  recording,
  scoped,
  sheet,
  type FetchStub,
} from "./story-harness";
import type { LdapDirectoryRow, LdapGroupMappingRow } from "@/lib/api";

// built at run time: a password-shaped literal in a fixture trips the secret scanners
const BIND_PASSWORD = ["bind", "fixture", "one"].join("-");
const ROTATED_PASSWORD = ["bind", "fixture", "two"].join("-");

const NOW = "2026-08-01T10:00:00Z";

const directory = (over: Partial<LdapDirectoryRow> = {}): LdapDirectoryRow => ({
  id: "ldap-1",
  org_id: ORG.id,
  name: "Corporate directory",
  url: "ldaps://dir.acme.example",
  bind_dn: "cn=svc-rolter,dc=acme,dc=example",
  has_bind_password: true,
  base_dn: "ou=people,dc=acme,dc=example",
  user_filter: "(|(uid={login})(mail={login}))",
  email_attr: "mail",
  display_name_attr: "cn",
  group_attr: "memberOf",
  require_tls: true,
  default_role: "member",
  enabled: true,
  created_at: NOW,
  ...over,
});

const MAPPINGS: LdapGroupMappingRow[] = [
  {
    id: "lmap-1",
    directory_id: "ldap-1",
    group_name: "cn=rolter-admins,ou=groups,dc=acme,dc=example",
    org_id: ORG.id,
    team_id: null,
    project_id: null,
    role: "admin",
    created_at: NOW,
  },
];

function api({
  directories = () => [directory()] as unknown,
  status = 200,
  write,
}: {
  directories?: () => unknown;
  status?: number;
  /** answers a POST or PUT of a directory; by default it echoes the body */
  write?: () => Response;
} = {}): FetchStub {
  return scoped(async (input, init) => {
    const url = String(input);
    const method = (init?.method ?? "GET").toUpperCase();
    if (url.includes("/group-mappings")) {
      if (method === "POST") return json(MAPPINGS[0], 201);
      return json(MAPPINGS, status);
    }
    if (url.includes("/ldap-group-mappings/")) return new Response(null, { status: 204 });
    if (url.includes("/ldap-directories")) {
      if (method === "DELETE") return new Response(null, { status: 204 });
      if (method === "POST" || method === "PUT") {
        if (write) return write();
        const body = JSON.parse(String(init?.body)) as Record<string, unknown>;
        return json(directory({ ...body, bind_password: undefined } as never), 200);
      }
      return json(directories(), status);
    }
    return json([]);
  });
}

const meta = {
  title: "Screens/Ldap",
  component: Ldap,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Ldap>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={api()}>
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Corporate directory")).toBeVisible());
    await expect(canvas.getByText("ldaps://dir.acme.example")).toBeVisible();
    await expect(canvas.getByText("Stored")).toBeVisible();
    await expect(canvas.queryByText("No TLS required")).toBeNull();
    // an org has one directory, so a second is not offered
    await expect(canvas.getByRole("button", { name: "Add directory" })).toBeDisabled();
    await waitFor(() =>
      expect(canvas.getByText("cn=rolter-admins,ou=groups,dc=acme,dc=example")).toBeVisible(),
    );
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No LDAP directory registered yet/);
  },
};

export const Empty: Story = {
  render: () => (
    <Harness fetchStub={api({ directories: () => [] })}>
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No LDAP directory registered yet/, /Add directory/);
  },
};

export const LoadFailed: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async () => json({ error: { message: "directory store is down" } }, 500))}
    >
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /directory store is down/);
    await expectNoFalseEmpty(canvasElement, /No LDAP directory registered yet/);
  },
};

export const WarnsWhenTlsIsNotRequired: Story = {
  render: () => (
    <Harness
      fetchStub={api({
        directories: () => [directory({ require_tls: false, url: "ldap://dir.acme.example" })],
      })}
    >
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expect(await within(canvasElement).findByText("No TLS required")).toBeVisible();
  },
};

export const NoDefaultRoleAndNoPassword: Story = {
  render: () => (
    <Harness
      fetchStub={api({
        directories: () => [directory({ default_role: null, has_bind_password: false })],
      })}
    >
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/No default role/)).toBeVisible();
    await expect(canvas.getByText("Not set (anonymous search)")).toBeVisible();
    await expect(
      canvas.queryByRole("button", { name: /Remove the stored bind password/ }),
    ).toBeNull();
  },
};

const blocked = recording(api({ directories: () => [] }));

/**
 * Name, URL and base DN are what Save waits for (#3242): a refused submit names
 * each of them instead of leaving Add directory quietly dead, and Enter in a
 * field submits once they are filled in.
 */
export const BlockedSubmitNamesTheMandatoryFields: Story = {
  render: () => (
    <Harness fetchStub={blocked.stub}>
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      (await within(canvasElement).findAllByRole("button", { name: /Add directory/ }))[0],
    );
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    for (const label of ["Name", "URL", "Base DN"]) {
      await expect(panel.getByLabelText(label)).toBeRequired();
    }
    await expect(panel.queryByText("Required")).toBeNull();

    await userEvent.click(panel.getByRole("button", { name: "Add directory" }));
    await waitFor(() => expect(panel.getAllByText("Required")).toHaveLength(3));
    await waitFor(() => expect(panel.getByLabelText("Name")).toHaveFocus());
    await expect(await panel.findByRole("status")).toHaveTextContent("3 fields need attention");
    blocked.expectNotSent("POST", "/orgs/org-1/ldap-directories");

    await userEvent.type(panel.getByLabelText("Name"), "Corporate directory");
    await userEvent.type(panel.getByLabelText("URL"), "ldaps://dir.acme.example");
    await userEvent.type(panel.getByLabelText("Base DN"), "ou=people,dc=acme,dc=example{Enter}");
    await expect(
      await blocked.expectSentBody("POST", "/orgs/org-1/ldap-directories"),
    ).toMatchObject({ name: "Corporate directory" });
  },
};

const registers = recording(api({ directories: () => [] }));

export const RegistersADirectory: Story = {
  render: () => (
    <Harness fetchStub={registers.stub}>
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      (await within(canvasElement).findAllByRole("button", { name: /Add directory/ }))[0],
    );
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    await userEvent.type(panel.getByLabelText("Name"), "Corporate directory");
    await userEvent.type(panel.getByLabelText("URL"), "ldaps://dir.acme.example");
    await userEvent.type(panel.getByLabelText("Bind DN"), "cn=svc,dc=acme,dc=example");
    await userEvent.type(panel.getByLabelText("Bind password"), BIND_PASSWORD);
    await userEvent.type(panel.getByLabelText("Base DN"), "ou=people,dc=acme,dc=example");
    await userEvent.click(panel.getByRole("button", { name: "Add directory" }));
    const body = await registers.expectSentBody<Record<string, unknown>>(
      "POST",
      "/orgs/org-1/ldap-directories",
    );
    await expect(body.name).toBe("Corporate directory");
    await expect(body.bind_password).toBe(BIND_PASSWORD);
    await expect(body.require_tls).toBe(true);
    // left empty, the attributes take the server's defaults rather than ""
    await expect(body.user_filter).toBeUndefined();
    await expect(body.default_role).toBeUndefined();
  },
};

/** The control plane refuses plain ldap:// while TLS is required; its words stay in the sheet. */
export const PlainLdapRefusedWhileTlsIsRequired: Story = {
  render: () => (
    <Harness
      fetchStub={api({
        directories: () => [],
        write: () =>
          json(
            {
              error: {
                message: "ldap:// is plain text; use ldaps:// or turn require_tls off",
                code: "invalid_field",
                field: "directory",
              },
            },
            400,
          ),
      })}
    >
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      (await within(canvasElement).findAllByRole("button", { name: /Add directory/ }))[0],
    );
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    await userEvent.type(panel.getByLabelText("Name"), "Corporate directory");
    await userEvent.type(panel.getByLabelText("URL"), "ldap://dir.acme.example");
    // the form says it first, before anything is sent
    await waitFor(() =>
      expect(panel.getByText(/plain ldap:\/\/, which the control plane refuses/)).toBeVisible(),
    );
    await userEvent.type(panel.getByLabelText("Bind DN"), "cn=svc,dc=acme,dc=example");
    await userEvent.type(panel.getByLabelText("Base DN"), "ou=people,dc=acme,dc=example");
    await userEvent.click(panel.getByRole("button", { name: "Add directory" }));
    await waitFor(() =>
      expect(
        panel.getAllByRole("alert").some((a) => /is plain text/.test(a.textContent ?? "")),
      ).toBe(true),
    );
    await expect(sheet()).toBeVisible();
  },
};

/** Two admins race: the second registration is a 409 and says so. */
export const ASecondDirectoryIsRefused: Story = {
  render: () => (
    <Harness
      fetchStub={api({
        directories: () => [],
        write: () =>
          json(
            {
              error: {
                message: "this organization already has an LDAP directory; edit or delete it first",
              },
            },
            409,
          ),
      })}
    >
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      (await within(canvasElement).findAllByRole("button", { name: /Add directory/ }))[0],
    );
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    await userEvent.type(panel.getByLabelText("Name"), "Second");
    await userEvent.type(panel.getByLabelText("URL"), "ldaps://other.example");
    await userEvent.type(panel.getByLabelText("Bind DN"), "cn=svc,dc=acme,dc=example");
    await userEvent.type(panel.getByLabelText("Base DN"), "ou=people,dc=acme,dc=example");
    await userEvent.click(panel.getByRole("button", { name: "Add directory" }));
    await waitFor(() =>
      expect(
        panel
          .getAllByRole("alert")
          .some((a) => /already has an LDAP directory/.test(a.textContent ?? "")),
      ).toBe(true),
    );
  },
};

const keeps = recording(api());

/** An untouched password field leaves the sealed password alone: the key is not sent. */
export const EditKeepsTheBindPassword: Story = {
  render: () => (
    <Harness fetchStub={keeps.stub}>
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", {
        name: "Edit directory Corporate directory",
      }),
    );
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    await expect(panel.getByLabelText("Bind password")).toHaveValue("");
    await expect(panel.getByLabelText("Bind password")).toHaveAccessibleDescription(
      /Leave empty to keep the stored password/,
    );
    const name = panel.getByLabelText("Name");
    await userEvent.clear(name);
    await userEvent.type(name, "Acme directory");
    await userEvent.click(panel.getByRole("button", { name: "Save directory" }));
    const body = await keeps.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/api/v1/ldap-directories/ldap-1",
    );
    await expect(body.name).toBe("Acme directory");
    await expect("bind_password" in body).toBe(false);
  },
};

const rotates = recording(api());

export const EditRotatesTheBindPassword: Story = {
  render: () => (
    <Harness fetchStub={rotates.stub}>
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", {
        name: "Edit directory Corporate directory",
      }),
    );
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    await userEvent.type(panel.getByLabelText("Bind password"), ROTATED_PASSWORD);
    await userEvent.click(panel.getByRole("button", { name: "Save directory" }));
    const body = await rotates.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/api/v1/ldap-directories/ldap-1",
    );
    await expect(body.bind_password).toBe(ROTATED_PASSWORD);
  },
};

const clears = recording(api());

/** Clearing is its own confirmed control and the only thing that sends "". */
export const ClearsTheBindPassword: Story = {
  render: () => (
    <Harness fetchStub={clears.stub}>
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", {
        name: "Remove the stored bind password for Corporate directory",
      }),
    );
    await confirmDestructive(
      /Remove the stored bind password of Corporate directory/,
      "Remove password",
    );
    const body = await clears.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/api/v1/ldap-directories/ldap-1",
    );
    await expect(body.bind_password).toBe("");
    await expect(body.name).toBe("Corporate directory");
  },
};

const disables = recording(api());

export const TakingItOutOfServiceConfirms: Story = {
  render: () => (
    <Harness fetchStub={disables.stub}>
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("switch", {
        name: "Enable directory Corporate directory",
      }),
    );
    disables.expectNotSent("PUT", "/api/v1/ldap-directories/ldap-1");
    await confirmDestructive(/Take Corporate directory out of service/, "Take out of service");
    const body = await disables.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/api/v1/ldap-directories/ldap-1",
    );
    await expect(body.enabled).toBe(false);
    await expect("bind_password" in body).toBe(false);
  },
};

const deletes = recording(api());

export const DeletesTheDirectory: Story = {
  render: () => (
    <Harness fetchStub={deletes.stub}>
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", {
        name: "Delete directory Corporate directory",
      }),
    );
    const dialog = within(await confirmation());
    await waitFor(() =>
      expect(dialog.getByText(/Delete directory Corporate directory\?/)).toBeVisible(),
    );
    await userEvent.click(dialog.getByRole("button", { name: "Delete directory" }));
    await deletes.expectSent("DELETE", "/api/v1/ldap-directories/ldap-1");
  },
};

export const RefusedToAMember: Story = {
  render: () => (
    <Harness fetchStub={api()} role="member">
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Edit directory Corporate directory");
    await expectRefused(canvasElement, "Delete directory Corporate directory");
    await expectRefused(
      canvasElement,
      "Enable directory Corporate directory",
      NEEDS_ADMIN,
      "switch",
    );
    await expectRefused(canvasElement, "Remove the stored bind password for Corporate directory");
  },
};

export const RefusedToAMemberWhenEmpty: Story = {
  render: () => (
    <Harness fetchStub={api({ directories: () => [] })} role="member">
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Add directory");
  },
};

export const AllowedToAnAdmin: Story = {
  render: () => (
    <Harness fetchStub={api()} role="admin">
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, "Edit directory Corporate directory");
    await expectAllowed(canvasElement, "Delete directory Corporate directory");
    await expectAllowed(canvasElement, "Enable directory Corporate directory", "switch");
    await waitFor(() =>
      expect(
        within(canvasElement).getByRole("button", {
          name: "Remove the mapping for cn=rolter-admins,ou=groups,dc=acme,dc=example",
        }),
      ).toBeVisible(),
    );
  },
};

/** The members' own read is admin-gated, so a viewer sees the list refused, not an empty org. */
export const ReadRefusedToAViewer: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async () => json({ error: { message: "requires the admin role" } }, 403))}
      role="viewer"
    >
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /admin role/);
    await expectNoFalseEmpty(canvasElement, /No LDAP directory registered yet/);
  },
};

const maps = recording(api());

export const MapsAGroupToARole: Story = {
  render: () => (
    <Harness fetchStub={maps.stub}>
      <Ldap />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Corporate directory")).toBeVisible());
    // the form settles once the org's scope resolves; typing earlier is lost
    await waitFor(() =>
      expect(canvas.getByLabelText("Where the role applies")).toHaveValue("Whole organization"),
    );
    await userEvent.type(
      await canvas.findByLabelText("IdP group"),
      "cn=devs,ou=groups,dc=acme,dc=example",
    );
    await clickWhenEnabled(canvasElement, "Map a group in Corporate directory");
    // a mapping onto the whole organization is confirmed first
    await confirmDestructive(/Map cn=devs,ou=groups,dc=acme,dc=example to Viewer\?/, "Map group");
    const body = await maps.expectSentBody<Record<string, unknown>>(
      "POST",
      "/api/v1/ldap-directories/ldap-1/group-mappings",
    );
    await expect(body.group_name).toBe("cn=devs,ou=groups,dc=acme,dc=example");
    await expect(body.role).toBe("viewer");
  },
};
