import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, within } from "storybook/test";

import Users from "./Users";
import {
  Harness,
  clickWhenEnabled,
  expectClosesWithoutPrompting,
  expectSheetClosed,
  expectSkeleton,
  json,
  pickOption,
  pending,
  routes,
  scoped,
  sheet,
  withConfirm,
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

    await withConfirm(false, async () => {
      await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
      await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
    });
    await withConfirm(true, async () => {
      await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
      await expectSheetClosed();
    });
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
