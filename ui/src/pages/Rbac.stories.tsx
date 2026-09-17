import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Rbac from "./Rbac";
import {
  cancelConfirmation,
  clickWhenEnabled,
  confirmDestructive,
  expectRefused,
  Harness,
  json,
  pending,
  recording,
  routes,
  scoped,
  sheet,
  Toasted,
  expectToast,
} from "./story-harness";
import type {
  AccessProfileDetail,
  CustomRoleRow,
  MembershipRow,
  RbacMatrix,
} from "@/lib/api";

// a slice of the real CAPABILITIES table, chosen for the four things a cell can
// say: a plain minimum role, a superadmin-only deployment setting, an action
// that does not exist for the resource at all, and one open to any
// authenticated caller. The absent actions are absent, not null — the control
// plane filters them out, which is exactly why they must not read as "denied".
const MATRIX: RbacMatrix = {
  roles: [
    { role: "viewer", rank: 0 },
    { role: "member", rank: 1 },
    { role: "admin", rank: 2 },
  ],
  resources: [
    {
      resource: "runtime_settings",
      scope: "deployment",
      actions: [
        { action: "read", minimum_role: null, superadmin_only: true, authenticated_only: false },
        { action: "update", minimum_role: null, superadmin_only: true, authenticated_only: false },
      ],
    },
    {
      resource: "org",
      scope: "org",
      actions: [
        { action: "read", minimum_role: "viewer", superadmin_only: false, authenticated_only: false },
        { action: "create", minimum_role: null, superadmin_only: true, authenticated_only: false },
        { action: "delete", minimum_role: "admin", superadmin_only: false, authenticated_only: false },
      ],
    },
    {
      resource: "provider",
      scope: "org",
      actions: [
        { action: "read", minimum_role: "viewer", superadmin_only: false, authenticated_only: false },
        { action: "create", minimum_role: "admin", superadmin_only: false, authenticated_only: false },
        { action: "update", minimum_role: "admin", superadmin_only: false, authenticated_only: false },
        { action: "delete", minimum_role: "admin", superadmin_only: false, authenticated_only: false },
      ],
    },
    {
      // append-only: it has a read and nothing else, for anyone
      resource: "audit_log",
      scope: "org",
      actions: [
        { action: "read", minimum_role: "admin", superadmin_only: false, authenticated_only: false },
      ],
    },
    {
      // a global catalog carrying no tenant's data (#766)
      resource: "provider_kind",
      scope: "org",
      actions: [
        { action: "read", minimum_role: null, superadmin_only: false, authenticated_only: true },
      ],
    },
    {
      resource: "project",
      scope: "team",
      actions: [
        { action: "read", minimum_role: "viewer", superadmin_only: false, authenticated_only: false },
        { action: "create", minimum_role: "admin", superadmin_only: false, authenticated_only: false },
        { action: "delete", minimum_role: "admin", superadmin_only: false, authenticated_only: false },
      ],
    },
    {
      resource: "virtual_key",
      scope: "project",
      actions: [
        { action: "read", minimum_role: "viewer", superadmin_only: false, authenticated_only: false },
        { action: "create", minimum_role: "member", superadmin_only: false, authenticated_only: false },
        { action: "update", minimum_role: "member", superadmin_only: false, authenticated_only: false },
        { action: "delete", minimum_role: "member", superadmin_only: false, authenticated_only: false },
      ],
    },
  ],
  custom_roles: [],
};

// a viewer that may also mint keys, plus a grant naming a resource this build
// retired — reported rather than hidden, because it silently allows nothing
const WITH_CUSTOM_ROLE: RbacMatrix = {
  ...MATRIX,
  custom_roles: [
    {
      id: "role-1",
      slug: "support-engineer",
      name: "Support engineer",
      description: "Read everything, mint keys for a customer",
      base_role: "viewer",
      base_rank: 0,
      grants: [{ resource: "virtual_key", action: "create" }],
      unknown_grants: [{ resource: "legacy_widget", action: "read" }],
    },
  ],
};

// the same role as the custom-roles tab lists it: the list endpoint carries the
// role, and the matrix carries the grants split into known and unknown
const CUSTOM_ROLES: CustomRoleRow[] = [
  {
    id: "role-1",
    org_id: "org-1",
    slug: "support-engineer",
    name: "Support engineer",
    description: "Read everything, mint keys for a customer",
    base_role: "viewer",
    created_at: "2026-08-01T10:00:00Z",
    updated_at: "2026-08-01T10:00:00Z",
  },
];

// one profile composing that role, so the list can say what a delete would have
// to be detached from first
const PROFILE_DETAIL: AccessProfileDetail = {
  id: "p-1",
  org_id: "org-1",
  slug: "support",
  name: "Support",
  description: null,
  created_at: "2026-08-01T10:00:00Z",
  updated_at: "2026-08-01T10:00:00Z",
  roles: [
    {
      id: "pr-1",
      profile_id: "p-1",
      role_id: "role-1",
      org_id: "org-1",
      team_id: null,
      project_id: null,
      created_at: "2026-08-01T10:00:00Z",
    },
  ],
  assignments: [],
  policy: null,
};

const MEMBERSHIPS: MembershipRow[] = [
  { id: "m-1", user_id: "u-1", org_id: "org-1", team_id: null, project_id: null, role: "admin", created_at: "2026-01-01T00:00:00Z" },
  { id: "m-2", user_id: "u-2", org_id: "org-1", team_id: null, project_id: null, role: "admin", created_at: "2026-01-01T00:00:00Z" },
  { id: "m-3", user_id: "u-3", org_id: "org-1", team_id: null, project_id: null, role: "member", created_at: "2026-01-01T00:00:00Z" },
];

const stub = (matrix: RbacMatrix) =>
  routes([
    ["/rbac/matrix", () => matrix],
    ["/memberships", () => MEMBERSHIPS],
  ]);

/**
 * The custom-roles tab's own fan-out.
 *
 * Longest path first: `/access-profiles/p-1` has to answer the detail rather
 * than the list it is a prefix of, and both contain `/access-profiles`.
 */
const withRoles = (roles: CustomRoleRow[], matrix = WITH_CUSTOM_ROLE) =>
  routes([
    ["/rbac/matrix", () => matrix],
    ["/memberships", () => MEMBERSHIPS],
    ["/custom-roles", () => roles],
    ["/access-profiles/p-1", () => PROFILE_DETAIL],
    ["/access-profiles", () => [{ id: "p-1", name: "Support" }]],
  ]);

/** Open the org-defined half of the screen. */
async function openCustomTab(canvasElement: HTMLElement): Promise<void> {
  const canvas = within(canvasElement);
  await userEvent.click(await canvas.findByRole("tab", { name: /Custom roles/ }));
}

const meta = {
  title: "Screens/Rbac",
  component: Rbac,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Rbac>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={stub(MATRIX)}>
      <Rbac />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    // resources come from the server's table, grouped by the scope they live in
    await expect(await canvas.findByText("runtime_settings")).toBeVisible();
    await expect(canvas.getByText("virtual_key")).toBeVisible();
    await expect(canvas.getByText("Deployment-wide")).toBeVisible();
    await expect(canvas.getByText("Project")).toBeVisible();
    await expect(canvas.getByText("7 resources")).toBeVisible();

    // one column per built-in role, with the live membership count under it.
    // the counts are a second request, so they are awaited rather than assumed
    // to have landed with the matrix (#1266)
    await expect(canvas.getByText("admin")).toBeVisible();
    await expect(await canvas.findByText("2 members")).toBeVisible();
    await expect(await canvas.findByText("1 member")).toBeVisible();
    await expect(await canvas.findByText("0 members")).toBeVisible();
  },
};

// the distinction #1178 exists for: an action a resource does not have is "not
// applicable" for everyone, and a deployment setting is nobody's org role
export const MarksNotApplicableAndSuperadmin: Story = {
  render: () => (
    <Harness fetchStub={stub(MATRIX)}>
      <Rbac />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("audit_log");

    // an append-only log has no create/update/delete for any of the three roles
    await expect(canvas.getAllByTitle("Create — not applicable")).toHaveLength(9);
    // runtime settings are superadmin-only, so no org role reaches them
    await expect(canvas.getAllByTitle("Update — superadmin only")).toHaveLength(3);
    // and a member may not delete a provider, which *is* a denial
    await expect(canvas.getAllByTitle("Delete — denied")).not.toHaveLength(0);
    // a catalog open to any authenticated caller is allowed all the way down
    await expect(canvas.getAllByTitle("Read — allowed").length).toBeGreaterThan(3);
  },
};

export const WithCustomRole: Story = {
  render: () => (
    <Harness fetchStub={withRoles(CUSTOM_ROLES)}>
      <Rbac />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    // the org-defined role sits beside the built-ins, ranked by its base role
    await expect(await canvas.findByText("Support engineer")).toBeVisible();
    await expect(canvas.getByText("granted by access profile")).toBeVisible();
    await expect(canvas.getByText("4 roles")).toBeVisible();

    // it is a viewer plus exactly one pair, shown as its own state. the column
    // arrives with the matrix, but the cells are re-derived once the roles land
    await waitFor(() =>
      expect(
        canvas.getAllByTitle("Create — granted by this custom role"),
      ).toHaveLength(1),
    );

    // a grant naming a resource this build retired is surfaced, not hidden
    await expect(canvas.getByText(/does not define/)).toBeVisible();
  },
};

// the org-defined half (#1184): what the deployment's roles are, what each one
// grants on top of its base, and which profiles would block a delete
export const CustomRolesList: Story = {
  render: () => (
    <Harness fetchStub={withRoles(CUSTOM_ROLES)}>
      <Rbac />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await openCustomTab(canvasElement);

    await expect(await canvas.findByText("support-engineer")).toBeVisible();
    await expect(canvas.getByText("extends viewer")).toBeVisible();
    // the known grant and the retired one both count: both are stored on the role
    await expect(canvas.getByText("2 grants")).toBeVisible();
    // derived from the profiles that compose it, which is what a 409 would name.
    // those are their own two requests — the profile list, then a detail call
    // per profile — so this is awaited rather than read off the roles' settle
    // point, and the row says nothing about composition until they answer (#1266)
    await expect(await canvas.findByText("Composed into Support")).toBeVisible();
  },
};

export const CustomRolesEmpty: Story = {
  render: () => (
    <Harness fetchStub={withRoles([], MATRIX)}>
      <Rbac />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await openCustomTab(canvasElement);

    await expect(await canvas.findByText("No custom roles yet")).toBeVisible();
    await expect(
      canvas.getAllByRole("button", { name: /New role/ }).length,
    ).toBeGreaterThan(0);
  },
};

// the grid is the matrix, not a list written out in the dashboard: every row is
// a resource the deployment guards, a pair it does not define has no checkbox
// at all, and a superadmin-only pair has one that cannot be ticked
const creates = recording(withRoles([], MATRIX));

export const CreatesACustomRole: Story = {
  render: () => (
    <Harness fetchStub={creates.stub}>
      <Rbac />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await openCustomTab(canvasElement);
    // the button stays disabled until the org/team/project chain has resolved
    await clickWhenEnabled(canvasElement, "+ New role");

    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Name"), "Support engineer");

    // an org's create is reserved for a superadmin, so the cell exists and is
    // refused rather than silently missing
    await expect(form.getByLabelText("Create on org")).toBeDisabled();
    // an audit log has no update at all: no checkbox, only the dash
    await expect(form.queryByLabelText("Update on audit_log")).toBeNull();

    await userEvent.click(form.getByLabelText("Create on virtual_key"));
    await userEvent.click(form.getByRole("button", { name: "Create role" }));

    const body = (await creates.expectSentBody("POST", "/custom-roles")) as {
      name: string;
      base_role: string;
      grants: { resource: string; action: string }[];
    };
    expect(body.name).toBe("Support engineer");
    expect(body.base_role).toBe("viewer");
    expect(body.grants).toEqual([{ resource: "virtual_key", action: "create" }]);
  },
};

/**
 * The role is refused (#1607).
 *
 * A grid of ticked capability cells is expensive to rebuild, so the sheet has to
 * survive a rejected save with every tick still set — and the refusal has to be
 * announced, since this screen reports one only through the toast queue.
 */
export const CreateRejectedByTheServer: Story = {
  render: () => (
    <Harness
      fetchStub={async (input, init) =>
        init?.method === "POST"
          ? json({ error: { message: "a role with that slug already exists" } }, 409)
          : withRoles([], MATRIX)(input, init)
      }
    >
      <Toasted>
        <Rbac />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await openCustomTab(canvasElement);
    await clickWhenEnabled(canvasElement, "+ New role");

    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Name"), "Support engineer");
    await userEvent.click(form.getByLabelText("Create on virtual_key"));
    await userEvent.click(form.getByRole("button", { name: "Create role" }));

    await expectToast(canvasElement, /slug already exists/, "error");
    await waitFor(() =>
      expect(within(document.body).getByRole("dialog")).toBeInTheDocument(),
    );
    await expect(form.getByLabelText("Name")).toHaveValue("Support engineer");
    await expect(form.getByLabelText("Create on virtual_key")).toBeChecked();
  },
};

// an edit replaces the grants wholesale, so the pairs this build no longer
// defines have to ride along — they are not on the grid to be re-ticked, and
// dropping them would quietly rewrite a role during an unrelated edit
const edits = recording(withRoles(CUSTOM_ROLES));

export const EditsACustomRole: Story = {
  render: () => (
    <Harness fetchStub={edits.stub}>
      <Rbac />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await openCustomTab(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Edit Support engineer" }),
    );

    const form = within(sheet());
    // seeded from the stored role: the pair it already grants is ticked
    await waitFor(() =>
      expect(form.getByLabelText("Create on virtual_key")).toBeChecked(),
    );
    // and the retired pair is accounted for rather than silently dropped
    await expect(form.getByText(/does not define/)).toBeVisible();
    // the slug is the role's stable handle, so an edit does not offer it
    await expect(form.queryByLabelText("Slug")).toBeNull();

    await userEvent.click(form.getByLabelText("Update on virtual_key"));
    await userEvent.click(form.getByRole("button", { name: "Save role" }));

    const body = (await edits.expectSentBody("PUT", "/custom-roles/role-1")) as {
      grants: { resource: string; action: string }[];
    };
    expect(body.grants).toEqual([
      { resource: "virtual_key", action: "create" },
      { resource: "virtual_key", action: "update" },
      { resource: "legacy_widget", action: "read" },
    ]);
  },
};

// a role reaches everyone holding it through a profile, so removing one changes
// what a group of people can do — it is named, and the profile blocking it too
const deletes = recording(async (input, init) => {
  if (init?.method === "DELETE") return json({}, 204);
  return withRoles(CUSTOM_ROLES)(input, init);
});

export const ConfirmsBeforeDeletingARole: Story = {
  render: () => (
    <Harness fetchStub={deletes.stub}>
      <Rbac />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await openCustomTab(canvasElement);

    await userEvent.click(
      await canvas.findByRole("button", { name: "Delete Support engineer" }),
    );
    await cancelConfirmation();
    deletes.expectNotSent("DELETE", "/custom-roles/role-1");

    await userEvent.click(canvas.getByRole("button", { name: "Delete Support engineer" }));
    await confirmDestructive(/Support engineer/, /delete role/i);
    await deletes.expectSent("DELETE", "/custom-roles/role-1");
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <Rbac />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    // the placeholder is the shape of the table that is coming, not a spinner
    await expect(canvasElement.querySelectorAll(".rl-skeleton").length).toBeGreaterThan(10);
  },
};

// any authenticated principal may read the matrix, so the failure worth showing
// is the org half: custom roles need a membership in the org being asked about
export const Forbidden: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) =>
        String(input).includes("/rbac/matrix")
          ? json({ error: { message: "forbidden" } }, 403)
          : json([]),
      )}
    >
      <Rbac />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByText(/do not have access to the permission matrix/i),
    ).toBeVisible();
  },
};

// the matrix is readable by anyone, the org's roles are not: the tab reports
// its own failure without taking the matrix down with it
export const CustomRolesForbidden: Story = {
  render: () => (
    <Harness
      fetchStub={async (input, init) =>
        String(input).includes("/custom-roles")
          ? json({ error: { message: "forbidden" } }, 403)
          : withRoles([], MATRIX)(input, init)
      }
    >
      <Rbac />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await openCustomTab(canvasElement);
    await waitFor(() =>
      expect(
        canvas.getAllByRole("alert").some((a) => /custom roles/.test(a.textContent ?? "")),
      ).toBe(true),
    );
  },
};

// A custom role is an org's own grant table, so writing one is `custom_role`
// — admin at every action (#1606). The built-in matrix above stays readable to
// a viewer: it is the published rules, not this caller's answer.
export const RefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={withRoles(CUSTOM_ROLES)} role="viewer">
      <Rbac />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await openCustomTab(canvasElement);
    await expectRefused(canvasElement, /new role/i);
    await expectRefused(canvasElement, "Edit Support engineer");
    await expectRefused(canvasElement, "Delete Support engineer");
  },
};

export const RefusedToAMember: Story = {
  render: () => (
    <Harness fetchStub={withRoles(CUSTOM_ROLES)} role="member">
      <Rbac />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await openCustomTab(canvasElement);
    await expectRefused(canvasElement, /new role/i);
    await expectRefused(canvasElement, "Delete Support engineer");
  },
};
