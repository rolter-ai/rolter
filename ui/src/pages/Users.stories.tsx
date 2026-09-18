import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Users from "./Users";
import {
  Harness,
  Toasted,
  clickWhenEnabled,
  expectClosesWithoutPrompting,
  expectEmptyState,
  expectRefused,
  expectSheetClosed,
  expectSkeleton,
  expectToast,
  json,
  NEEDS_SUPERADMIN,
  pending,
  pickOption,
  routes,
  scoped,
  sheet,
  answerDiscardPrompt,
} from "./story-harness";
import type { MembershipRow, UserRow } from "@/lib/api";

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
    await userEvent.click(canvas.getByRole("button", { name: /deactivated/i }));
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
    await waitFor(() =>
      expect(within(document.body).getByRole("dialog")).toBeInTheDocument(),
    );
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
    const canvas = within(canvasElement);
    await expectRefused(canvasElement, "Edit ada@example.com", NEEDS_SUPERADMIN);
    // the invitation half of the screen is still theirs
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: "Invite user" })).toBeEnabled(),
    );
  },
};
