import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Users from "./Users";
import {
  Harness,
  Toasted,
  cancelConfirmation,
  clickWhenEnabled,
  confirmation,
  confirmDestructive,
  expectClosesWithoutPrompting,
  expectEmptyState,
  expectAllowed,
  expectLoadError,
  expectNoUxEvent,
  expectRefused,
  expectSheetClosed,
  expectListTable,
  expectNoFalseEmpty,
  expectSkeleton,
  expectToast,
  expectUxEvent,
  json,
  NEEDS_SUPERADMIN,
  openOptions,
  pending,
  pickOption,
  recordUxEvents,
  recording,
  routes,
  scoped,
  sheet,
  answerDiscardPrompt,
  uxEvents,
  type FetchStub,
  type Recorder,
} from "./story-harness";
import type { MembershipRow, UserRow } from "@/lib/api";
import { UxScreenProvider } from "@/lib/ux-react";

const USERS: UserRow[] = [
  {
    id: "user-1",
    email: "ada@example.com",
    is_superadmin: true,
    deactivated_at: null,
    created_at: "2026-01-04T00:00:00Z",
  },
  {
    id: "user-2",
    email: "grace@example.com",
    is_superadmin: false,
    deactivated_at: null,
    created_at: "2026-03-11T00:00:00Z",
  },
  {
    id: "user-3",
    email: "former@example.com",
    is_superadmin: false,
    deactivated_at: "2026-06-01T00:00:00Z",
    created_at: "2026-02-02T00:00:00Z",
  },
];

const MEMBERSHIPS: MembershipRow[] = [
  {
    id: "m-1",
    user_id: "user-1",
    org_id: "org-1",
    team_id: null,
    project_id: null,
    role: "admin",
    created_at: "2026-01-04T00:00:00Z",
  },
  {
    id: "m-2",
    user_id: "user-2",
    org_id: null,
    team_id: "team-1",
    project_id: null,
    role: "member",
    created_at: "2026-03-11T00:00:00Z",
  },
  {
    id: "m-3",
    user_id: "user-3",
    org_id: null,
    team_id: null,
    project_id: "project-1",
    role: "viewer",
    created_at: "2026-02-02T00:00:00Z",
  },
];

// the per-grant controls name the grant they act on (#1214, #2053)
const REVOKE_GRACE = "Revoke Member on the Platform team from grace@example.com";
const CHANGE_GRACE = "Change grace@example.com's Member role on the Platform team";

const loaded = routes([
  ["/memberships", () => MEMBERSHIPS],
  ["/users", () => USERS],
]);
const empty = routes([
  ["/memberships", () => []],
  ["/users", () => []],
]);

const meta = {
  title: "Screens/Users",
  component: Users,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Users>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // an active and a deactivated account render side by side; the status tabs
    // are the only place the counts are visible
    await expect(await canvas.findByText("ada@example.com")).toBeInTheDocument();
    await expect(canvas.getByText("former@example.com")).toBeInTheDocument();
    await expectListTable(canvasElement, "Users");

    // each grant reads as its role and its scope by name: the team and the
    // project, never the head of a uuid (#2053)
    const row = (email: string) =>
      within(canvas.getByText(email).closest('[role="row"]') as HTMLElement);
    await waitFor(() => expect(row("grace@example.com").getByText("Platform")).toBeVisible());
    await expect(row("grace@example.com").getByText("Member")).toBeVisible();
    await expect(row("former@example.com").getByText("Gateway")).toBeVisible();
    await expect(row("former@example.com").getByText("Viewer")).toBeVisible();
    await expect(row("ada@example.com").getByText("Whole organization")).toBeVisible();
    await expect(canvas.queryByText(/project-1|team-1/)).toBeNull();
  },
};

/**
 * The grants are a read of their own, so while it is still coming a row is not
 * a person with "no roles" (#2211).
 */
export const RolesStillLoading: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) =>
        String(input).includes("/memberships") ? new Promise<Response>(() => {}) : json(USERS),
      )}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("grace@example.com")).toBeInTheDocument();
    await expect(canvas.queryByText("no roles")).toBeNull();
  },
};

/** A failed grants read says so on every row, under the screen's own `LoadError`. */
export const RolesFailedToLoad: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) =>
        String(input).includes("/memberships")
          ? json({ error: { message: "store unavailable" } }, 500)
          : json(USERS),
      )}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /store unavailable/);
    await waitFor(() => expect(canvas.getAllByText("roles not loaded")).toHaveLength(3));
    await expect(canvas.queryByText("no roles")).toBeNull();
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No users yet/);
  },
};

export const Empty: Story = {
  render: () => (
    <Harness fetchStub={empty}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No users yet/, /Invite user/);
  },
};

export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "forbidden" } }, 403))}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/do not have access to users/i)).toBeInTheDocument();
    await expectNoFalseEmpty(canvasElement, /No users yet/);
  },
};

export const FiltersToDeactivatedAccounts: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("ada@example.com")).toBeInTheDocument();
    await userEvent.click(canvas.getByRole("radio", { name: /deactivated/i }));
    await expect(canvas.getByText("former@example.com")).toBeInTheDocument();
    await expect(canvas.queryByText("ada@example.com")).not.toBeInTheDocument();
  },
};

export const InvitesAUser: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        // the default method is an invitation link, so the screen calls
        // createInvitation and reads `accept_url` off the response
        if (init?.method === "POST") {
          return json({ id: "inv-1", accept_url: "https://rolter.local/invite/one-time" }, 201);
        }
        return String(input).includes("/memberships") ? json(MEMBERSHIPS) : json(USERS);
      })}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Email"), "newcomer@example.com");
    await userEvent.click(within(form).getByRole("button", { name: "Invite" }));
    // the one-time link is shown once and never again; losing it means the
    // invited person can never accept
    await expect(
      await within(document.body).findByText("https://rolter.local/invite/one-time"),
    ).toBeInTheDocument();
  },
};

/**
 * The invite is refused (#1607).
 *
 * The sheet has to survive the refusal — closing it would drop the address and
 * the role the operator picked — and the refusal has to reach the toast queue,
 * which is the only place this screen reports a rejected write.
 */
export const InviteRejectedByTheServer: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        if (init?.method === "POST") {
          return json({ error: { message: "that address already has an invitation" } }, 409);
        }
        return String(input).includes("/memberships") ? json(MEMBERSHIPS) : json(USERS);
      })}
    >
      <Toasted>
        <Users />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Email"), "newcomer@example.com");
    await userEvent.click(within(form).getByRole("button", { name: "Invite" }));
    await expectToast(canvasElement, /already has an invitation/, "error");
    await waitFor(() => expect(within(document.body).getByRole("dialog")).toBeInTheDocument());
    await expect(within(form).getByLabelText("Email")).toHaveValue("newcomer@example.com");
  },
};

/** `Invite user` seeds role=member and method=link, so an untouched form is clean. */
export const AnUntouchedInviteFormClosesWithoutPrompting: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    await expectClosesWithoutPrompting();
  },
};

export const AnEditedInviteFormPromptsBeforeDiscarding: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Email"), "half@typed");

    await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
    await answerDiscardPrompt(false);
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();

    await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
    await answerDiscardPrompt(true);
    await expectSheetClosed();
  },
};

/** Switching method to `password` reveals the password field and marks dirty. */
export const ChoosingAPasswordRevealsTheField: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = sheet();
    await expect(within(form).queryByLabelText("Password (optional)")).not.toBeInTheDocument();
    await pickOption(within(form).getByLabelText("Method"), "Set a password now");
    await expect(within(form).getByLabelText("Password (optional)")).toBeInTheDocument();
  },
};

// The two gates here take different authorities (#1606).
//
// Inviting someone is `invitation:create`, which is admin, but editing an
// account — the name, the active flag — is `user:update`, which the table
// marks superadmin-only because an account is deployment-wide and not the
// org's to rewrite. So an admin is offered the invite and refused the edit.
export const RefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="viewer">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Invite user");
    await expectRefused(canvasElement, "Grant a role to ada@example.com");
    await expectRefused(canvasElement, REVOKE_GRACE);
    await expectRefused(canvasElement, CHANGE_GRACE);
    await expectRefused(canvasElement, "Edit ada@example.com", NEEDS_SUPERADMIN);
    await expectRefused(canvasElement, "Deactivate ada@example.com", NEEDS_SUPERADMIN);
  },
};

export const EditRefusedToAnAdmin: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="admin">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Edit ada@example.com", NEEDS_SUPERADMIN);
    // the invitation half of the screen is still theirs, asserted through
    // `expectAllowed` so it is the gate's answer being read and not the
    // enabled state the button was in before it (#1707)
    await expectAllowed(canvasElement, "Invite user");
    // granting, changing and revoking a role are the org admin's own (#2053)
    await expectAllowed(canvasElement, REVOKE_GRACE);
    await expectAllowed(canvasElement, CHANGE_GRACE);
  },
};

// ------------------------------------------------ revoking and changing a role (#2053)

/**
 * The org as the control plane holds it, for the stories that change it: a
 * revoke takes the grant out of the list, a grant adds one, and the users list
 * is everyone still holding a role in the org, the way `list_in_org` builds it.
 * `onDelete` and `onPost` answer a write before the fixture changes, so a
 * story can hold one in flight or refuse it.
 */
function directory(
  answers: {
    onDelete?: (id: string) => Promise<Response | null> | Response | null;
    onPost?: () => Promise<Response | null> | Response | null;
  } = {},
): FetchStub {
  let grants = [...MEMBERSHIPS];
  let created = 0;
  return scoped(async (input, init) => {
    const url = String(input);
    const method = (init?.method ?? "GET").toUpperCase();
    const revoked = url.match(/\/api\/v1\/memberships\/([^/?]+)$/);
    if (method === "DELETE" && revoked) {
      const refused = await answers.onDelete?.(revoked[1]);
      if (refused) return refused;
      grants = grants.filter((g) => g.id !== revoked[1]);
      return json(null, 204);
    }
    if (method === "POST" && url.includes("/memberships")) {
      const refused = await answers.onPost?.();
      if (refused) return refused;
      const body = JSON.parse(String(init?.body)) as {
        user_id: string;
        scope_type: string;
        scope_id: string;
        role: string;
      };
      const grant: MembershipRow = {
        id: `m-new-${++created}`,
        user_id: body.user_id,
        org_id: body.scope_type === "org" ? body.scope_id : null,
        team_id: body.scope_type === "team" ? body.scope_id : null,
        project_id: body.scope_type === "project" ? body.scope_id : null,
        role: body.role,
        created_at: "2026-09-30T00:00:00Z",
      };
      grants = [...grants, grant];
      return json(grant);
    }
    if (url.includes("/memberships")) return json(grants);
    if (url.includes("/users")) {
      return json(USERS.filter((u) => grants.some((g) => g.user_id === u.id)));
    }
    return json([]);
  });
}

/**
 * Revoke goes through `ConfirmDialog`: the title names the person, the role
 * and the scope, the body says what they lose. A cancel sends nothing and is
 * an abandon; a confirm is on the wire with both buttons out of reach until
 * it lands, then the dialog closes, the toast says what went, and a person
 * whose last role it was leaves the list.
 */
let revokes: Recorder;
let releaseRevoke: () => void = () => {};
export const RevokeIsConfirmedThenLands: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    revokes = recording(
      directory({
        onDelete: () =>
          new Promise<null>((resolve) => {
            releaseRevoke = () => resolve(null);
          }),
      }),
    );
    return (
      <Harness fetchStub={revokes.stub}>
        <UxScreenProvider screen="gov-users">
          <Toasted>
            <Users />
          </Toasted>
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: REVOKE_GRACE }));
    await expect(
      await within(document.body).findByRole("heading", { name: `${REVOKE_GRACE}?` }),
    ).toBeInTheDocument();
    await cancelConfirmation();
    revokes.expectNotSent("DELETE", "/memberships/");
    const abandon = await expectUxEvent("form_abandon", "user-role-revoke");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "user-role-revoke");

    await userEvent.click(canvas.getByRole("button", { name: REVOKE_GRACE }));
    // what they lose: nothing else reaches the team, and it is their last
    // role in the org, so they leave this list
    await confirmDestructive(
      /no other grant of theirs reaches it\. It is their last role in this organization/,
      "Revoke role",
    );
    await revokes.expectSent("DELETE", "/memberships/m-2");

    // in flight: the request is on the wire, so neither button can be pressed
    const dialog = within(await confirmation());
    await waitFor(() => expect(dialog.getByRole("button", { name: "Revoke role" })).toBeDisabled());
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeDisabled();

    releaseRevoke();
    await expectSheetClosed();
    await expectToast(canvasElement, /Revoked Member on the Platform team from grace@example\.com/);
    await waitFor(() => expect(canvas.queryByText("grace@example.com")).toBeNull());
    const submit = await expectUxEvent("form_submit", "user-role-revoke");
    await expect(submit.outcome).toBe("ok");
    await expectUxEvent("save_confirmed", "user-role-revoke");
  },
};

/**
 * The control plane refuses the revoke. The dialog stays open with its message
 * verbatim and a line on what revoking at that scope takes: the gate answered
 * for the scope the switcher is on, which is not always the grant's.
 */
export const RevokeRefusedByTheServer: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness
      fetchStub={directory({
        onDelete: () => json({ error: { message: "requires admin at this scope" } }, 403),
      })}
    >
      <UxScreenProvider screen="gov-users">
        <Users />
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", {
        name: "Revoke Admin on the whole organization from ada@example.com",
      }),
    );
    const dialog = within(await confirmation());
    await userEvent.click(dialog.getByRole("button", { name: "Revoke role" }));
    await waitFor(() =>
      expect(dialog.getByRole("alert")).toHaveTextContent("requires admin at this scope"),
    );
    await expect(
      dialog.getByText(
        "Revoking a role on the whole organization takes Admin there or on a scope above it.",
      ),
    ).toBeVisible();
    await waitFor(() =>
      expect(
        uxEvents()
          .filter((e) => e.action === "form_submit" && e.target === "user-role-revoke")
          .map((e) => e.outcome),
      ).toEqual(["ok", "error"]),
    );
    expectNoUxEvent("save_confirmed", "user-role-revoke");
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
    await expect(canvas.getByText("ada@example.com")).toBeInTheDocument();
  },
};

/**
 * A role changes in one confirmed step. The control plane has no update for a
 * membership, so the new role is granted first and the old one revoked after:
 * at one scope the higher role applies, so the moment both exist grants
 * nothing beyond one of the two, and the person is never without a role.
 */
let changes: Recorder;
export const ChangesARoleGrantingFirst: Story = {
  render: () => {
    changes = recording(directory());
    return (
      <Harness fetchStub={changes.stub}>
        <Toasted>
          <Users />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: CHANGE_GRACE }));
    const dialog = within(await confirmation());
    await expect(
      dialog.getByRole("heading", {
        name: "Change grace@example.com's role on the Platform team?",
      }),
    ).toBeInTheDocument();
    // story-wait-allow: disabled by its own confirmDisabled until a role is picked
    await expect(dialog.getByRole("button", { name: "Change role" })).toBeDisabled();

    // the role they already hold is not offered as the one to change to
    const list = await openOptions(dialog.getByLabelText("New role"));
    await expect(within(list).queryByRole("option", { name: "Member" })).toBeNull();
    await userEvent.click(within(list).getByRole("option", { name: "Viewer" }));
    await userEvent.click(dialog.getByRole("button", { name: "Change role" }));

    const body = await changes.expectSentBody("POST", "/orgs/org-1/memberships");
    await expect(body).toEqual({
      user_id: "user-2",
      scope_type: "team",
      scope_id: "team-1",
      role: "viewer",
    });
    await changes.expectSent("DELETE", "/memberships/m-2");
    await expect(changes.calls.filter((c) => c.method !== "GET").map((c) => c.method)).toEqual([
      "POST",
      "DELETE",
    ]);
    await expectSheetClosed();
    await expectToast(canvasElement, /grace@example\.com is now Viewer on the Platform team/);
    await waitFor(() =>
      expect(
        canvas.getByRole("button", {
          name: "Revoke Viewer on the Platform team from grace@example.com",
        }),
      ).toBeInTheDocument(),
    );
  },
};

/**
 * The grant landed and the revoke did not. The dialog says the state the
 * person is in, both roles and which one applies meanwhile, keeps the control
 * plane's message, and its confirm becomes a retry of the revoke alone: the
 * grant is never sent twice.
 */
let partial: Recorder;
let failedRevokes = 0;
export const ChangeWhoseRevokeFailsSaysSoAndRetries: Story = {
  render: () => {
    failedRevokes = 0;
    partial = recording(
      directory({
        onDelete: () =>
          failedRevokes++ === 0 ? json({ error: { message: "store unavailable" } }, 500) : null,
      }),
    );
    return (
      <Harness fetchStub={partial.stub}>
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: CHANGE_GRACE }));
    const dialog = within(await confirmation());
    await pickOption(dialog.getByLabelText("New role"), "Viewer");
    await userEvent.click(dialog.getByRole("button", { name: "Change role" }));

    await waitFor(() =>
      expect(
        dialog.getByText(
          "Viewer was granted, but Member could not be revoked. grace@example.com holds both roles on the Platform team, and Member applies there until Member is revoked.",
        ),
      ).toBeVisible(),
    );
    await expect(dialog.getByRole("alert")).toHaveTextContent("store unavailable");
    // the row shows the state the person is in: both grants
    await waitFor(() =>
      expect(
        canvas.getByRole("button", {
          name: "Revoke Viewer on the Platform team from grace@example.com",
        }),
      ).toBeInTheDocument(),
    );

    await userEvent.click(dialog.getByRole("button", { name: "Retry revoking Member" }));
    await expectSheetClosed();
    await expect(partial.calls.filter((c) => c.method === "POST")).toHaveLength(1);
    await expect(partial.calls.filter((c) => c.method === "DELETE")).toHaveLength(2);
    await waitFor(() => expect(canvas.queryByRole("button", { name: REVOKE_GRACE })).toBeNull());
  },
};

/**
 * The grant sheet picks its scope by name from the shared `OrgScopePicker`,
 * where it used to ask for a pasted project uuid, and posts the project it
 * names.
 */
let grantsSent: Recorder;
export const GrantsAProjectRolePickedByName: Story = {
  render: () => {
    grantsSent = recording(directory());
    return (
      <Harness fetchStub={grantsSent.stub}>
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Grant a role to grace@example.com");
    const form = within(sheet());
    await pickOption(await form.findByLabelText("Scope"), "Gateway");
    await pickOption(form.getByLabelText("Role"), "Viewer");
    await expect(form.queryByPlaceholderText(/0000/)).toBeNull();
    await userEvent.click(form.getByRole("button", { name: "Grant" }));
    const body = await grantsSent.expectSentBody("POST", "/orgs/org-1/memberships");
    await expect(body).toEqual({
      user_id: "user-2",
      scope_type: "project",
      scope_id: "project-1",
      role: "viewer",
    });
    await expectSheetClosed();
  },
};

/** A role the person already holds at that scope is refused before any request. */
export const AGrantAlreadyHeldIsRefusedBeforeSending: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Grant a role to grace@example.com");
    const form = within(sheet());
    await pickOption(await form.findByLabelText("Scope"), "Platform");
    await expect(
      form.getByText("grace@example.com already holds Member on the Platform team."),
    ).toBeVisible();
    // story-wait-allow: disabled by the sheet's own canSave, which the held grant sets
    await expect(form.getByRole("button", { name: "Grant" })).toBeDisabled();
  },
};
