import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import AccessProfiles from "./AccessProfiles";
import {
  cancelConfirmation,
  clickWhenEnabled,
  confirmDestructive,
  expectEmptyState,
  expectLoadError,
  expectRefused,
  expectSkeleton,
  Harness,
  json,
  pending,
  recording,
  scoped,
  sheet,
  type FetchStub,
} from "./story-harness";
import type {
  AccessProfileDetail,
  AccessProfileRow,
  CustomRoleRow,
} from "@/lib/api";

const ORG = "org-1";

const profile = (over: Partial<AccessProfileRow> = {}): AccessProfileRow => ({
  id: "p-1",
  org_id: ORG,
  slug: "support-engineers",
  name: "Support engineers",
  description: "Read-only access to logs and analytics",
  created_at: "2026-08-01T10:00:00Z",
  updated_at: "2026-08-01T10:00:00Z",
  ...over,
});

const PROFILES: AccessProfileRow[] = [
  profile(),
  // a profile that reaches nobody grants nothing — the screen says so rather
  // than showing "0 users · 0 teams"
  profile({
    id: "p-2",
    slug: "oncall",
    name: "On-call",
    description: null,
  }),
];

// `GET /api/v1/access-profiles/{id}` is the only call that answers what a
// profile carries: the roles composed into it, everyone it reaches, and the
// model policy — which has no list endpoint at all (#1184)
const DETAILS: Record<string, AccessProfileDetail> = {
  "p-1": {
    ...profile(),
    roles: [
      {
        id: "pr-1",
        profile_id: "p-1",
        role_id: "role-1",
        org_id: ORG,
        team_id: null,
        project_id: null,
        created_at: "2026-08-01T10:00:00Z",
      },
    ],
    assignments: [
      { id: "a-1", profile_id: "p-1", user_id: "u-1", team_id: null, created_at: "" },
      { id: "a-2", profile_id: "p-1", user_id: null, team_id: "t-1", created_at: "" },
    ],
    policy: {
      profile_id: "p-1",
      allowed_models: ["gpt-4o", "claude-*"],
      denied_models: ["o1-preview"],
      allowed_routes: [],
      denied_routes: [],
      updated_at: "2026-08-01T10:00:00Z",
    },
  },
  "p-2": {
    ...profile({ id: "p-2", slug: "oncall", name: "On-call", description: null }),
    roles: [],
    assignments: [],
    policy: null,
  },
};

const ROLES: CustomRoleRow[] = [
  {
    id: "role-1",
    org_id: ORG,
    slug: "support-engineer",
    name: "Support engineer",
    description: null,
    base_role: "viewer",
    created_at: "2026-08-01T10:00:00Z",
    updated_at: "2026-08-01T10:00:00Z",
  },
  {
    id: "role-2",
    org_id: ORG,
    slug: "deploy-admin",
    name: "Deploy admin",
    description: null,
    base_role: "member",
    created_at: "2026-08-01T10:00:00Z",
    updated_at: "2026-08-01T10:00:00Z",
  },
];

/**
 * Route by URL.
 *
 * The detail path is tested before the list it is a prefix of: both contain
 * `/access-profiles`, and answering the detail with the list would leave the
 * card reporting a profile that carries nothing.
 */
function stub(profiles: () => Promise<Response>, roles = ROLES): FetchStub {
  return scoped(async (input) => {
    const url = String(input);
    const detail = /\/access-profiles\/([^/?]+)$/.exec(url);
    if (detail) return json(DETAILS[detail[1]] ?? {});
    if (url.includes("/access-profiles")) return profiles();
    if (url.includes("/custom-roles")) return json(roles);
    return json([]);
  });
}

const meta = {
  title: "Screens/AccessProfiles",
  component: AccessProfiles,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof AccessProfiles>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={stub(async () => json(PROFILES))}>
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText("Support engineers")).toBeVisible(),
    );
    await expect(canvas.getByText("On-call")).toBeVisible();

    // a profile's reach is the thing that matters: one user plus one team
    await waitFor(() =>
      expect(canvas.getByText("1 user · 1 team")).toBeVisible(),
    );
    // and a profile assigned to nobody says so plainly
    await expect(canvas.getByText("Not assigned to anyone yet")).toBeVisible();

    // the roles and the policy it carries, read back from the detail — before
    // #1184 a policy could be written and never shown again
    await expect(canvas.getByText("1 custom role")).toBeVisible();
    await expect(canvas.getByText("3 policy patterns")).toBeVisible();
    await expect(canvas.getByText("No model or route policy")).toBeVisible();
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

// the state every deployment starts in: the backend has shipped for a while,
// but nobody has created a profile yet
export const Empty: Story = {
  render: () => (
    <Harness fetchStub={stub(async () => json([]))}>
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No access profiles yet/, /Add profile/);
  },
};

// a profile is created whole: the roles it composes and the policy it carries
// go in the same request, so it is never assignable half-written
const creates = recording(stub(async () => json([])));

export const CreatesAProfileWithRolesAndPolicy: Story = {
  render: () => (
    <Harness fetchStub={creates.stub}>
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    // the button stays disabled until the org/team/project chain has resolved
    await clickWhenEnabled(canvasElement, "+ Add profile");

    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Name"), "Support");
    await userEvent.click(form.getByRole("checkbox", { name: /Support engineer/ }));
    await userEvent.type(form.getByLabelText("Allowed models"), "gpt-4o\nclaude-*");
    await userEvent.type(form.getByLabelText("Denied models"), "o1-preview");
    await userEvent.click(form.getByRole("button", { name: "Create profile" }));

    const body = (await creates.expectSentBody("POST", "/access-profiles")) as {
      name: string;
      roles: { role_id: string }[];
      policy: Record<string, string[]>;
    };
    expect(body.name).toBe("Support");
    // scope is left to the server, which defaults a composition to the
    // profile's own org — the whole tenant, which is what the sheet promises
    expect(body.roles).toEqual([{ role_id: "role-1" }]);
    expect(body.policy).toEqual({
      allowed_models: ["gpt-4o", "claude-*"],
      denied_models: ["o1-preview"],
      allowed_routes: [],
      denied_routes: [],
    });
  },
};

// an edit seeds from the detail and replaces both wholesale, which is also how
// a role is detached before it can be deleted
const edits = recording(stub(async () => json(PROFILES)));

export const EditsAProfile: Story = {
  render: () => (
    <Harness fetchStub={edits.stub}>
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const edit = await canvas.findByRole("button", { name: "Edit Support engineers" });
    // the button waits for the detail: there is nothing to seed the sheet with
    // until the roles and the policy have landed
    await waitFor(() => expect(edit).toBeEnabled());
    await userEvent.click(edit);

    const form = within(sheet());
    await expect(form.getByRole("checkbox", { name: /Support engineer/ })).toBeChecked();
    await expect(form.getByLabelText("Allowed models")).toHaveValue("gpt-4o\nclaude-*");

    // detach the role, keep the policy
    await userEvent.click(form.getByRole("checkbox", { name: /Support engineer/ }));
    await userEvent.click(form.getByRole("button", { name: "Save profile" }));

    const body = (await edits.expectSentBody("PUT", "/access-profiles/p-1")) as {
      roles: unknown[];
      policy: Record<string, string[]>;
    };
    expect(body.roles).toEqual([]);
    expect(body.policy.allowed_models).toEqual(["gpt-4o", "claude-*"]);
  },
};

// the delete mutation is shared by every card, so `isPending` alone marked the
// whole grid busy. the pending row is the one the mutation was given, and this
// story is what stops that regression coming back: one row spins, the other
// stays clickable.
export const Deleting: Story = {
  render: () => (
    <Harness
      fetchStub={async (input, init) => {
        // hang the delete so the pending state stays on screen
        if (init?.method === "DELETE") return new Promise<Response>(() => {});
        return stub(async () => json(PROFILES))(input, init);
      }}
    >
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText("Support engineers")).toBeVisible(),
    );

    const target = canvas.getByRole("button", { name: "Delete Support engineers" });
    const other = canvas.getByRole("button", { name: "Delete On-call" });
    await expect(target).toBeEnabled();
    await expect(other).toBeEnabled();

    await userEvent.click(target);
    // the delete only leaves once the confirmation is answered (#1179)
    await confirmDestructive(/Support engineers/, /delete profile/i);

    // the clicked row goes busy...
    await waitFor(() => expect(target).toBeDisabled());
    // ...and the sibling row is untouched
    await expect(other).toBeEnabled();
  },
};

// a profile reaches whole teams, so removing one changes what a group of people
// can do — it is named and the consequence stated before anything leaves
const deletes = recording(async (input, init) => {
  if (init?.method === "DELETE") return json({}, 204);
  return stub(async () => json(PROFILES))(input, init);
});

export const ConfirmsBeforeDeletingAProfile: Story = {
  render: () => (
    <Harness fetchStub={deletes.stub}>
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText("Support engineers")).toBeVisible(),
    );

    await userEvent.click(
      canvas.getByRole("button", { name: "Delete Support engineers" }),
    );
    await cancelConfirmation();
    deletes.expectNotSent("DELETE", "/access-profiles/p-1");

    await userEvent.click(
      canvas.getByRole("button", { name: "Delete Support engineers" }),
    );
    await confirmDestructive(/Support engineers/, /delete profile/i);
    await deletes.expectSent("DELETE", "/access-profiles/p-1");
  },
};

// profiles are org-scoped and admin-gated, so a viewer gets 403
export const Error_: Story = {
  name: "Error",
  render: () => (
    <Harness
      fetchStub={stub(async () => json({ error: { message: "forbidden" } }, 403))}
    >
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /You do not have access to access profiles/);
  },
};

// What a viewer is offered, which is nothing (#1606).
//
// `access_profile` is admin at every action, so a viewer must find the create
// refused and the per-card edit and delete refused with it. Without a role
// these stories would render under no `CapabilityProvider` at all, `can()`
// would answer "unknown", and every one of those controls would come up
// enabled — which is why a screen with no role story cannot catch a dropped
// gate.
export const RefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={stub(async () => json(PROFILES))} role="viewer">
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, /add profile/i);
    await expectRefused(canvasElement, "Edit Support engineers");
    await expectRefused(canvasElement, "Delete Support engineers");
  },
};

// a member outranks a viewer and is still short of admin, so the answer is
// the same sentence rather than a softer one
export const RefusedToAMember: Story = {
  render: () => (
    <Harness fetchStub={stub(async () => json(PROFILES))} role="member">
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, /add profile/i);
    await expectRefused(canvasElement, "Delete Support engineers");
  },
};

// the empty state repeats the create control, so it needs the gate too — an
// operator who may not create should not be invited to
export const RefusedToAViewerWhenEmpty: Story = {
  render: () => (
    <Harness fetchStub={stub(async () => json([]))} role="viewer">
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    // the exact name, not a fragment: the toolbar carries `+ Add profile` and
    // the placeholder `Add profile`, and a loose regex would match both and
    // fail on the ambiguity rather than on the gate
    await expectRefused(canvasElement, "Add profile");
  },
};
