import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Users from "./Users";
import {
  Harness,
  LOADING_LABEL,
  Toasted,
  cancelConfirmation,
  clickWhenEnabled,
  confirmation,
  confirmDestructive,
  expectClosesWithoutPrompting,
  expectEmptyState,
  expectAllowed,
  expectGateAnswered,
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
import type { Invitation, MembershipRow, UserRow } from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";
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
const revokeInvitationName = (email: string) => `Revoke the invitation for ${email}`;

const DAY = 86_400_000;
const inDays = (days: number) => new Date(Date.now() + days * DAY).toISOString();

// what `GET /orgs/{org}/invitations` answers: every invitation of the org,
// newest first, the spent ones (accepted, revoked) among the pending. the
// screen lists only the four that have neither
const INVITATIONS: Invitation[] = [
  {
    id: "inv-1",
    org_id: "org-1",
    email: "newcomer@example.com",
    role: "member",
    team_id: null,
    project_id: "project-1",
    invited_by: "user-1",
    expires_at: inDays(5),
    accepted_at: null,
    revoked_at: null,
    created_at: "2026-09-25T00:00:00Z",
  },
  {
    id: "inv-2",
    org_id: "org-1",
    email: "lead@example.com",
    role: "admin",
    team_id: "team-1",
    project_id: null,
    invited_by: "user-2",
    expires_at: inDays(2),
    accepted_at: null,
    revoked_at: null,
    created_at: "2026-09-24T00:00:00Z",
  },
  {
    id: "inv-3",
    org_id: "org-1",
    email: "everyone@example.com",
    role: "viewer",
    team_id: null,
    project_id: null,
    invited_by: null,
    expires_at: inDays(6),
    accepted_at: null,
    revoked_at: null,
    created_at: "2026-09-23T00:00:00Z",
  },
  {
    id: "inv-4",
    org_id: "org-1",
    email: "stale@example.com",
    role: "member",
    team_id: null,
    project_id: null,
    invited_by: "user-9",
    expires_at: "2026-01-10T00:00:00Z",
    accepted_at: null,
    revoked_at: null,
    created_at: "2026-01-03T00:00:00Z",
  },
  {
    id: "inv-5",
    org_id: "org-1",
    email: "joined@example.com",
    role: "member",
    team_id: "team-1",
    project_id: null,
    invited_by: "user-1",
    expires_at: inDays(3),
    accepted_at: "2026-09-26T00:00:00Z",
    revoked_at: null,
    created_at: "2026-09-22T00:00:00Z",
  },
  {
    id: "inv-6",
    org_id: "org-1",
    email: "withdrawn@example.com",
    role: "viewer",
    team_id: null,
    project_id: "project-1",
    invited_by: "user-1",
    expires_at: inDays(4),
    accepted_at: null,
    revoked_at: "2026-09-26T00:00:00Z",
    created_at: "2026-09-21T00:00:00Z",
  },
];

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
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/invitations")) return json([]);
        return url.includes("/memberships") ? new Promise<Response>(() => {}) : json(USERS);
      })}
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
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/invitations")) return json([]);
        return url.includes("/memberships")
          ? json({ error: { message: "store unavailable" } }, 500)
          : json(USERS);
      })}
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

let invite: Recorder;
export const InvitesAUser: Story = {
  render: () => {
    invite = recording(
      scoped(async (input, init) => {
        const url = String(input);
        // the default method is an invitation link, so the screen calls
        // createInvitation and reads `accept_url` off the response
        if (init?.method === "POST") {
          return json({ id: "inv-1", accept_url: "https://rolter.local/invite/one-time" }, 201);
        }
        if (url.includes("/invitations")) return json([]);
        return url.includes("/memberships") ? json(MEMBERSHIPS) : json(USERS);
      }),
    );
    return (
      <Harness fetchStub={invite.stub}>
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Email"), "newcomer@example.com");
    // untouched, the scope is the org itself and the role field says so
    await expect(within(form).getByLabelText("Org role")).toBeInTheDocument();
    await userEvent.click(within(form).getByRole("button", { name: "Invite" }));
    const body = await invite.expectSentBody("POST", "/orgs/org-1/invitations");
    await expect(body).toEqual({
      email: "newcomer@example.com",
      role: "member",
      scope_type: "org",
      scope_id: "org-1",
    });
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
        const url = String(input);
        if (init?.method === "POST") {
          return json({ error: { message: "that address already has an invitation" } }, 409);
        }
        if (url.includes("/invitations")) return json([]);
        return url.includes("/memberships") ? json(MEMBERSHIPS) : json(USERS);
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

// ------------------------------------------ invitations at a scope, and pending (#2054)

/**
 * The org's invitations as the control plane holds them: the list answers every
 * one, a revoke stamps `revoked_at` and a create adds a row. `onRevoke` answers
 * a revoke first and may change the fixture or refuse, so a story can hold a
 * revoke in flight, refuse it or accept the invitation behind the screen's back.
 */
function invitationsApi(
  answers: {
    rows?: Invitation[];
    onRevoke?: (id: string, rows: Invitation[]) => Promise<Response | null> | Response | null;
  } = {},
): FetchStub {
  const rows = (answers.rows ?? INVITATIONS).map((row) => ({ ...row }));
  return scoped(async (input, init) => {
    const url = String(input);
    const method = (init?.method ?? "GET").toUpperCase();
    const revoked = url.match(/\/api\/v1\/invitations\/([^/?]+)$/);
    if (method === "DELETE" && revoked) {
      const refused = await answers.onRevoke?.(revoked[1], rows);
      if (refused) return refused;
      const row = rows.find((candidate) => candidate.id === revoked[1]);
      if (!row) return json({ error: { message: "not found" } }, 404);
      row.revoked_at = "2026-09-30T00:00:00Z";
      return json(row);
    }
    if (method === "POST" && url.includes("/invitations")) {
      return json({ accept_url: "https://rolter.local/invite/one-time" }, 201);
    }
    if (url.includes("/invitations")) return json(rows);
    if (url.includes("/memberships")) return json(MEMBERSHIPS);
    if (url.includes("/users")) return json(USERS);
    return json([]);
  });
}

/** the pending section, found by the heading that names it */
async function pendingSection(canvasElement: HTMLElement): Promise<HTMLElement> {
  return within(canvasElement).findByRole("region", { name: "Pending invitations" });
}

/**
 * The house date, written out the way `useFormat().date` renders it for `en`,
 * so the story asserts the expiry went through the formatter and not through a
 * bare `toLocaleDateString`.
 */
const expiryDate = (iso: string) =>
  new Intl.DateTimeFormat("en", { dateStyle: "medium" }).format(new Date(iso));

/**
 * The invite sheet picks its scope by name from the shared `OrgScopePicker`,
 * and the role field is named for the scope it lands in. The roles read as
 * Admin, Member and Viewer, never as the raw keys. The body sent carries the
 * project the operator picked, and the link dialog says what accepting it
 * grants.
 */
let projectInvite: Recorder;
export const InvitesToAProjectPickedByName: Story = {
  render: () => {
    projectInvite = recording(invitationsApi());
    return (
      <Harness fetchStub={projectInvite.stub}>
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Email"), "engineer@example.com");
    await pickOption(await form.findByLabelText("Scope"), "Gateway");

    // the role field follows the scope: it was "Org role" a moment ago
    await waitFor(() => expect(form.getByLabelText("Project role")).toBeInTheDocument());
    await expect(form.queryByLabelText("Org role")).toBeNull();
    const roles = await openOptions(form.getByLabelText("Project role"));
    await expect(
      within(roles)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual(["Admin", "Member", "Viewer"]);
    await userEvent.click(within(roles).getByRole("option", { name: "Viewer" }));

    await userEvent.click(form.getByRole("button", { name: "Invite" }));
    const body = await projectInvite.expectSentBody("POST", "/orgs/org-1/invitations");
    await expect(body).toEqual({
      email: "engineer@example.com",
      role: "viewer",
      scope_type: "project",
      scope_id: "project-1",
    });

    const dialog = within(await confirmation());
    await waitFor(() =>
      expect(dialog.getByText("https://rolter.local/invite/one-time")).toBeVisible(),
    );
    await waitFor(() =>
      expect(dialog.getByText("Accepting it grants Viewer on the Gateway project.")).toBeVisible(),
    );
  },
};

/** A team is a scope of its own: the body names the team and the field says "Team role". */
let teamInvite: Recorder;
export const InvitesToATeamPickedByName: Story = {
  render: () => {
    teamInvite = recording(invitationsApi());
    return (
      <Harness fetchStub={teamInvite.stub}>
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Email"), "lead@example.com");
    await pickOption(await form.findByLabelText("Scope"), "Platform");
    await waitFor(() => expect(form.getByLabelText("Team role")).toBeInTheDocument());
    await pickOption(form.getByLabelText("Team role"), "Admin");
    await userEvent.click(form.getByRole("button", { name: "Invite" }));
    const body = await teamInvite.expectSentBody("POST", "/orgs/org-1/invitations");
    await expect(body).toEqual({
      email: "lead@example.com",
      role: "admin",
      scope_type: "team",
      scope_id: "team-1",
    });
  },
};

/**
 * An account created with a password is granted its role on the org, since the
 * endpoint takes no scope. The picker is locked to the org and says why, and
 * nothing it held before is sent.
 */
let passwordInvite: Recorder;
export const APasswordAccountTakesItsRoleOnTheOrg: Story = {
  render: () => {
    passwordInvite = recording(invitationsApi());
    return (
      <Harness fetchStub={passwordInvite.stub}>
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Email"), "service@example.com");
    await pickOption(await form.findByLabelText("Scope"), "Platform");
    await pickOption(form.getByLabelText("Method"), "Set a password now");

    await expect(
      form.getByText(
        "An account created with a password takes its role on the whole organization. Invite by link to choose a team or a project.",
      ),
    ).toBeVisible();
    // story-wait-allow: disabled by the sheet's own method, which the pick above set
    await expect(form.getByLabelText("Scope")).toBeDisabled();
    await expect(form.getByLabelText("Scope")).toHaveValue("Whole organization");
    await expect(form.getByLabelText("Org role")).toBeInTheDocument();

    await userEvent.click(form.getByRole("button", { name: "Invite" }));
    const body = await passwordInvite.expectSentBody("POST", "/orgs/org-1/users");
    await expect(body).toEqual({ email: "service@example.com", role: "member" });
  },
};

/**
 * The scope switcher's gate answered for its own scope, so the control plane
 * can still refuse an invitation at the one picked. The sheet stays open with
 * the message verbatim and says what inviting there takes.
 */
export const InviteRefusedAtTheScopePicked: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        const url = String(input);
        if (init?.method === "POST") {
          return json({ error: { message: "insufficient role for this resource" } }, 403);
        }
        if (url.includes("/invitations")) return json([]);
        return url.includes("/memberships") ? json(MEMBERSHIPS) : json(USERS);
      })}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Email"), "newcomer@example.com");
    await pickOption(await form.findByLabelText("Scope"), "Platform");
    await userEvent.click(form.getByRole("button", { name: "Invite" }));
    await waitFor(() =>
      expect(
        form.getByText("Inviting into the Platform team takes Admin there or on a scope above it."),
      ).toBeVisible(),
    );
    await expect(form.getByText("insufficient role for this resource")).toBeVisible();
    await expect(form.getByLabelText("Email")).toHaveValue("newcomer@example.com");
  },
};

/** What the stub clipboard received, so a play can read it back. */
let copied: string[] = [];

/** A clipboard the story owns, put back when it ends. */
function stubClipboard(writeText: (value: string) => Promise<void>) {
  return () => {
    const original = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    return () => {
      if (original) Object.defineProperty(navigator, "clipboard", original);
      else Reflect.deleteProperty(navigator, "clipboard");
    };
  };
}

const INVITE_LINK = "https://rolter.local/invite/one-time";

/** The link dialog copies through the shared `CopyButton`, and what lands is the link itself. */
export const TheInviteLinkCopies: Story = {
  beforeEach: stubClipboard(async (value) => {
    copied = [...copied, value];
  }),
  render: () => (
    <Harness fetchStub={invitationsApi()}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    copied = [];
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Email"), "newcomer@example.com");
    await userEvent.click(form.getByRole("button", { name: "Invite" }));
    const dialog = within(await confirmation());
    await userEvent.click(await dialog.findByRole("button", { name: /Copy link/ }));
    await waitFor(() => expect(copied).toEqual([INVITE_LINK]));
    await waitFor(() =>
      expect(dialog.getByRole("button", { name: /Copy link/ })).toHaveAttribute(
        "title",
        en.common.copied,
      ),
    );
  },
};

/**
 * The clipboard is withheld on a plain-http dashboard, which is common on an
 * air-gapped LAN. The button used to say "Copied" whatever happened; now it
 * says the copy failed, and the link stays on screen until Done is pressed.
 */
export const TheInviteLinkCopyFails: Story = {
  beforeEach: stubClipboard(() => Promise.reject(new Error("denied"))),
  render: () => (
    <Harness fetchStub={invitationsApi()}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Email"), "newcomer@example.com");
    await userEvent.click(form.getByRole("button", { name: "Invite" }));
    const dialog = within(await confirmation());
    await waitFor(() => expect(dialog.getByText(INVITE_LINK)).toBeVisible());
    await userEvent.click(dialog.getByRole("button", { name: /Copy link/ }));
    await waitFor(() =>
      expect(dialog.getByRole("button", { name: /Copy link/ })).toHaveAttribute(
        "title",
        en.common.copyFailed,
      ),
    );
    await expect(dialog.queryByText(en.common.copied)).toBeNull();
    // the link is still there to copy by hand, and only Done takes it away
    await expect(dialog.getByText(INVITE_LINK)).toBeVisible();
    await userEvent.click(dialog.getByRole("button", { name: "Done" }));
    await expectSheetClosed();
  },
};

/**
 * The pending section lists what has neither been accepted nor revoked: the
 * address, the role by its translated name, the scope by name, who sent it
 * (resolved through the users list, never a uuid) and the expiry through
 * `fmt`. The accepted and the revoked invitation are not listed, and an
 * expired one is, marked, since it still holds its address.
 */
export const PendingInvitationsAreListed: Story = {
  render: () => (
    <Harness fetchStub={invitationsApi()}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const section = within(await pendingSection(canvasElement));
    await expectListTable(canvasElement, "Pending invitations");
    await expect(await section.findByText("newcomer@example.com")).toBeVisible();

    const row = (email: string) =>
      within(section.getByText(email).closest('[role="row"]') as HTMLElement);
    await expect(row("newcomer@example.com").getByText("Member")).toBeVisible();
    await expect(row("newcomer@example.com").getByText("Gateway")).toBeVisible();
    await expect(row("newcomer@example.com").getByText("ada@example.com")).toBeVisible();
    await expect(
      row("newcomer@example.com").getByText(expiryDate(INVITATIONS[0].expires_at)),
    ).toBeVisible();
    await expect(row("lead@example.com").getByText("Admin")).toBeVisible();
    await expect(row("lead@example.com").getByText("Platform")).toBeVisible();
    await expect(row("lead@example.com").getByText("grace@example.com")).toBeVisible();
    await expect(row("everyone@example.com").getByText("Viewer")).toBeVisible();
    await expect(row("everyone@example.com").getByText("Whole organization")).toBeVisible();
    await expect(row("everyone@example.com").getByText("not recorded")).toBeVisible();
    // a sender this account cannot name is unknown, not a uuid
    await expect(row("stale@example.com").getByText("unknown")).toBeVisible();
    await expect(row("stale@example.com").getByText("Expired")).toBeVisible();
    await expect(
      row("stale@example.com").getByText(expiryDate("2026-01-10T00:00:00Z")),
    ).toBeVisible();
    await expect(row("newcomer@example.com").queryByText("Expired")).toBeNull();

    await expect(section.queryByText("joined@example.com")).toBeNull();
    await expect(section.queryByText("withdrawn@example.com")).toBeNull();
    await expect(section.queryByText(/user-9|project-1|team-1/)).toBeNull();
    await expect(
      section.getByText("4 not accepted yet · a link works once and expires after seven days"),
    ).toBeVisible();
    // the users table above is a table of its own
    await expectListTable(canvasElement, "Users");
    await expect(canvas.getAllByRole("table")).toHaveLength(2);
  },
};

/** The list is a read of its own: while it is in flight the section is a skeleton, not "no invitations". */
export const PendingInvitationsLoading: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/invitations")) return new Promise<Response>(() => {});
        return url.includes("/memberships") ? json(MEMBERSHIPS) : json(USERS);
      })}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const section = await pendingSection(canvasElement);
    await waitFor(() =>
      expect(within(section).getAllByLabelText(LOADING_LABEL).length).toBeGreaterThan(0),
    );
    await expectNoFalseEmpty(section, /No pending invitations/);
  },
};

/** A failed read says so under the section's own `LoadError`, and claims neither none nor a count. */
export const PendingInvitationsFailToLoad: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/invitations")) {
          return json({ error: { message: "invitation store unavailable" } }, 500);
        }
        return url.includes("/memberships") ? json(MEMBERSHIPS) : json(USERS);
      })}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /invitation store unavailable/);
    const section = await pendingSection(canvasElement);
    await expectNoFalseEmpty(section, /No pending invitations/);
    // the users list above is unaffected
    await expect(await within(canvasElement).findByText("grace@example.com")).toBeVisible();
  },
};

/**
 * Nothing pending says so, and offers the invite. An accepted and a revoked
 * invitation do not make a pending list: they are spent.
 */
export const NoPendingInvitations: Story = {
  render: () => (
    <Harness fetchStub={invitationsApi({ rows: [INVITATIONS[4], INVITATIONS[5]] })}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const section = await pendingSection(canvasElement);
    await expectEmptyState(section, /No pending invitations/, /Invite user/);
    await expect(within(section).queryByText("joined@example.com")).toBeNull();
    await expect(within(section).queryByText("withdrawn@example.com")).toBeNull();
  },
};

/** The invitations are the org admin's to read: a viewer has no section and no request. */
let viewerReads: Recorder;
export const PendingInvitationsHiddenFromAViewer: Story = {
  render: () => {
    viewerReads = recording(invitationsApi());
    return (
      <Harness fetchStub={viewerReads.stub} role="viewer">
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("grace@example.com")).toBeInTheDocument();
    await expectGateAnswered();
    await expect(canvas.queryByRole("region", { name: "Pending invitations" })).toBeNull();
    viewerReads.expectNotSent("GET", "/invitations");
  },
};

/** An org admin reads the list and may revoke from it. */
export const PendingInvitationsOpenToAnAdmin: Story = {
  render: () => (
    <Harness fetchStub={invitationsApi()} role="admin">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const section = await pendingSection(canvasElement);
    await expectAllowed(section, revokeInvitationName("newcomer@example.com"));
  },
};

/**
 * A 403 from the list is an answer, not an outage: a caller the gate could not
 * refuse first (a role held below the org) gets no section instead of a red
 * "you do not have access to pending invitations" on a screen they may open.
 */
export const PendingInvitationsRefusedByTheServer: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/invitations")) {
          return json({ error: { message: "insufficient role for this resource" } }, 403);
        }
        return url.includes("/memberships") ? json(MEMBERSHIPS) : json(USERS);
      })}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("grace@example.com")).toBeInTheDocument();
    await waitFor(() =>
      expect(canvas.queryByRole("region", { name: "Pending invitations" })).toBeNull(),
    );
    await expect(canvas.queryByRole("alert")).toBeNull();
  },
};

/**
 * Revoke goes through `ConfirmDialog`: the title names the address and the body
 * says what the link can no longer do. A cancel sends nothing and is an
 * abandon; a confirm is on the wire with both buttons out of reach until it
 * lands, then the dialog closes, the toast says what went and the row leaves.
 */
let revokeInvite: Recorder;
let releaseInvite: () => void = () => {};
export const RevokingAPendingInvitationIsConfirmed: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    revokeInvite = recording(
      invitationsApi({
        onRevoke: () =>
          new Promise<null>((resolve) => {
            releaseInvite = () => resolve(null);
          }),
      }),
    );
    return (
      <Harness fetchStub={revokeInvite.stub}>
        <UxScreenProvider screen="gov-users">
          <Toasted>
            <Users />
          </Toasted>
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const section = within(await pendingSection(canvasElement));
    const revoke = revokeInvitationName("newcomer@example.com");
    await userEvent.click(await section.findByRole("button", { name: revoke }));
    await expect(
      await within(document.body).findByRole("heading", { name: `${revoke}?` }),
    ).toBeInTheDocument();
    await cancelConfirmation();
    revokeInvite.expectNotSent("DELETE", "/invitations/");
    const abandon = await expectUxEvent("form_abandon", "invitation-revoke");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "invitation-revoke");

    await userEvent.click(section.getByRole("button", { name: revoke }));
    await confirmDestructive(
      /The link stops working straight away, so newcomer@example\.com cannot join the Gateway project as Member with it\./,
      "Revoke invitation",
    );
    await revokeInvite.expectSent("DELETE", "/invitations/inv-1");

    // in flight: the request is on the wire, so neither button can be pressed
    const dialog = within(await confirmation());
    await waitFor(() =>
      expect(dialog.getByRole("button", { name: "Revoke invitation" })).toBeDisabled(),
    );
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeDisabled();

    releaseInvite();
    await expectSheetClosed();
    await expectToast(canvasElement, /Revoked the invitation for newcomer@example\.com/);
    await waitFor(() => expect(section.queryByText("newcomer@example.com")).toBeNull());
    await expect(section.getByText("lead@example.com")).toBeVisible();
    const submit = await expectUxEvent("form_submit", "invitation-revoke");
    await expect(submit.outcome).toBe("ok");
    await expectUxEvent("save_confirmed", "invitation-revoke");
  },
};

/** A dead link is revoked only to free its address, and the body says so. */
let revokeExpired: Recorder;
export const RevokingAnExpiredInvitationFreesTheAddress: Story = {
  render: () => {
    revokeExpired = recording(invitationsApi());
    return (
      <Harness fetchStub={revokeExpired.stub}>
        <Toasted>
          <Users />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const section = within(await pendingSection(canvasElement));
    await userEvent.click(
      await section.findByRole("button", { name: revokeInvitationName("stale@example.com") }),
    );
    await confirmDestructive(
      /This invitation has already expired and its link no longer works\. Revoking it removes it from the list and lets you invite stale@example\.com again\./,
      "Revoke invitation",
    );
    await revokeExpired.expectSent("DELETE", "/invitations/inv-4");
    await expectSheetClosed();
    await waitFor(() => expect(section.queryByText("stale@example.com")).toBeNull());
  },
};

/**
 * The control plane refuses the revoke. The dialog stays open with its message
 * verbatim and a line on what revoking at that scope takes: the gate answered
 * for the scope the switcher is on, which is not always the invitation's.
 */
export const RevokingAnInvitationRefusedByTheServer: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness
      fetchStub={invitationsApi({
        onRevoke: () => json({ error: { message: "insufficient role for this resource" } }, 403),
      })}
    >
      <UxScreenProvider screen="gov-users">
        <Users />
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const section = within(await pendingSection(canvasElement));
    await userEvent.click(
      await section.findByRole("button", { name: revokeInvitationName("lead@example.com") }),
    );
    const dialog = within(await confirmation());
    await userEvent.click(dialog.getByRole("button", { name: "Revoke invitation" }));
    await waitFor(() =>
      expect(dialog.getByRole("alert")).toHaveTextContent("insufficient role for this resource"),
    );
    await waitFor(() =>
      expect(
        dialog.getByText(
          "Revoking an invitation into the Platform team takes Admin there or on a scope above it.",
        ),
      ).toBeVisible(),
    );
    expectNoUxEvent("save_confirmed", "invitation-revoke");
    await expect(section.getByText("lead@example.com")).toBeVisible();
  },
};

/**
 * The invitee accepted while the dialog was open. The revoke only touches
 * unaccepted invitations, so it answers 404: the dialog says the invitation is
 * no longer pending, and the list is read again and drops the row.
 */
let revokeAccepted: Recorder;
export const RevokingAnInvitationAcceptedMeanwhile: Story = {
  render: () => {
    revokeAccepted = recording(
      invitationsApi({
        onRevoke: (id, rows) => {
          const row = rows.find((candidate) => candidate.id === id);
          if (row) row.accepted_at = "2026-09-30T00:00:00Z";
          return json({ error: { message: `pending invitation ${id} not found` } }, 404);
        },
      }),
    );
    return (
      <Harness fetchStub={revokeAccepted.stub}>
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const section = within(await pendingSection(canvasElement));
    await userEvent.click(
      await section.findByRole("button", { name: revokeInvitationName("lead@example.com") }),
    );
    const dialog = within(await confirmation());
    await userEvent.click(dialog.getByRole("button", { name: "Revoke invitation" }));
    await waitFor(() =>
      expect(
        dialog.getByText(
          "This invitation is no longer pending, most likely because it was just accepted. The list has been refreshed.",
        ),
      ).toBeVisible(),
    );
    await waitFor(() => expect(section.queryByText("lead@example.com")).toBeNull());
    await expect(
      revokeAccepted.calls.filter((c) => c.method === "GET" && c.url.includes("/invitations"))
        .length,
    ).toBeGreaterThan(1);
  },
};
