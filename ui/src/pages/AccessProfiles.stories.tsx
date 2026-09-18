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
  expectClosesWithoutPrompting,
  expectSheetClosed,
  expectSkeleton,
  Harness,
  json,
  openOptions,
  pending,
  pickOption,
  PROJECT,
  recording,
  scoped,
  sheet,
  TEAM,
  Toasted,
  expectToast,
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
function stub(
  profiles: () => Promise<Response>,
  roles = ROLES,
  details: Record<string, AccessProfileDetail> = DETAILS,
): FetchStub {
  return scoped(async (input) => {
    const url = String(input);
    const detail = /\/access-profiles\/([^/?]+)$/.exec(url);
    if (detail) return json(details[detail[1]] ?? {});
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

/**
 * The profile is refused (#1607).
 *
 * A composed profile carries roles and two model lists, so a sheet that closed
 * on a rejected save would cost all of it — it stays, and the toast queue is
 * where the refusal is reported.
 */
export const CreateRejectedByTheServer: Story = {
  render: () => (
    <Harness
      fetchStub={async (input, init) =>
        init?.method === "POST"
          ? json({ error: { message: "slug support is already taken" } }, 409)
          : stub(async () => json([]))(input, init)
      }
    >
      <Toasted>
        <AccessProfiles />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "+ Add profile");

    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Name"), "Support");
    await userEvent.click(form.getByRole("checkbox", { name: /Support engineer/ }));
    await userEvent.click(form.getByRole("button", { name: "Create profile" }));

    await expectToast(canvasElement, /already taken/, "error");
    await waitFor(() =>
      expect(within(document.body).getByRole("dialog")).toBeInTheDocument(),
    );
    await expect(form.getByLabelText("Name")).toHaveValue("Support");
    await expect(form.getByRole("checkbox", { name: /Support engineer/ })).toBeChecked();
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
//
// the list shrinks once the DELETE lands, so the story can assert the outcome
// — the toast, the row gone — rather than that the request left. A stub that
// answers the full list forever passes either way, which is how a 204 fixture
// that threw went unnoticed (#1260)
let profileDeleted = false;
const deletes = recording(async (input, init) => {
  if (init?.method === "DELETE") {
    profileDeleted = true;
    return json({}, 204);
  }
  const remaining = async () =>
    json(profileDeleted ? PROFILES.filter((row) => row.id !== "p-1") : PROFILES);
  return stub(remaining)(input, init);
});

export const ConfirmsBeforeDeletingAProfile: Story = {
  render: () => {
    profileDeleted = false;
    return (
      <Harness fetchStub={deletes.stub}>
        <Toasted>
          <AccessProfiles />
        </Toasted>
      </Harness>
    );
  },
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

    // the outcome, not just the request: the confirmation closes, the queue
    // announces it, and the row is gone from the list
    await expectSheetClosed();
    await expectToast(canvasElement, /Support engineers deleted/);
    await waitFor(() =>
      expect(canvas.queryByText("Support engineers")).not.toBeInTheDocument(),
    );
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

// ---------------------------------------------------------------------------
// #1251: a composition is a `(role, scope)` pair, not just a role
//
// `ProfileRoleBody` has taken `org_id`/`team_id`/`project_id` since #534 and
// `GET /api/v1/access-profiles/{id}` has always answered with them. The sheet
// sent `roles: [{ role_id }]` and read the scope back only to discard it, so
// "auditor across the org, and deploy admin on one project" — the shape
// `user-docs/security/rbac.mdx` documents with curl — could not be written or
// even seen from the dashboard.

const NARROW_PROFILE = profile({
  id: "p-3",
  slug: "deploy-admins",
  name: "Deploy admins",
  description: "Deploy admin on the gateway project, and nothing org-wide",
});

const NARROW_DETAIL: AccessProfileDetail = {
  ...NARROW_PROFILE,
  roles: [
    {
      id: "pr-3",
      profile_id: "p-3",
      role_id: "role-2",
      org_id: null,
      team_id: null,
      project_id: PROJECT.id,
      created_at: "2026-08-01T10:00:00Z",
    },
  ],
  assignments: [],
  policy: null,
};

// its own profile and detail map rather than a third card in `PROFILES`: the
// loaded story reads several of its counts with `getByText`, and a second
// unassigned, policy-less profile would make those ambiguous
const narrow = (): FetchStub =>
  stub(async () => json([NARROW_PROFILE]), ROLES, { "p-3": NARROW_DETAIL });

/**
 * A role pinned to one project is *drawn* as such.
 *
 * "1 custom role" is true of an org-wide composition and of a project-scoped
 * one alike, so without the chip the card cannot tell an operator that this
 * profile grants nothing outside Gateway.
 */
export const ShowsAProjectScopedRoleOnTheCard: Story = {
  render: () => (
    <Harness fetchStub={narrow()}>
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the role's name and the project's name, both resolved — a raw uuid on a
    // card is not an answer to "what does this profile grant"
    await waitFor(() =>
      expect(canvas.getByText("Deploy admin on Gateway")).toBeVisible(),
    );
    await expect(canvas.queryByText(PROJECT.id)).not.toBeInTheDocument();
    await expect(canvas.queryByText("role-2")).not.toBeInTheDocument();
  },
};

// the create path: the scope picked beside the role reaches the POST body
const composesAtTeamScope = recording(stub(async () => json([])));

export const ComposesARoleAtTeamScope: Story = {
  render: () => (
    <Harness fetchStub={composesAtTeamScope.stub}>
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "+ Add profile");

    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Name"), "Platform support");

    // the scope is only a question once the role is actually composed, so an
    // unchecked role carries no picker to answer
    await expect(
      form.queryByLabelText("Where Support engineer applies"),
    ).not.toBeInTheDocument();
    await userEvent.click(form.getByRole("checkbox", { name: /Support engineer/ }));

    const picker = await form.findByLabelText("Where Support engineer applies");
    // the org is the default, which is the scope the control plane would have
    // defaulted to anyway: ignoring the picker writes what the sheet wrote
    // before this
    await expect(picker).toHaveValue("Whole organization");

    // the whole org is on offer, not just the team the scope switcher holds
    const listbox = await openOptions(picker);
    await expect(
      within(listbox).getByRole("option", { name: TEAM.name }),
    ).toBeVisible();
    await expect(
      within(listbox).getByRole("option", { name: PROJECT.name }),
    ).toBeVisible();
    await userEvent.click(
      within(listbox).getByRole("option", { name: TEAM.name }),
    );

    await userEvent.click(form.getByRole("button", { name: "Create profile" }));

    const body = (await composesAtTeamScope.expectSentBody(
      "POST",
      "/access-profiles",
    )) as { roles: Record<string, string>[] };
    // `team_id` set and `project_id` absent rather than null: the control plane
    // resolves the most specific id it is given, so a null project would still
    // be the narrower scope if it were sent
    expect(body.roles).toEqual([{ role_id: "role-1", team_id: TEAM.id }]);
  },
};

/**
 * The edit path, which is where discarding the scope actually did damage: the
 * sheet seeded `roles` from the detail and dropped the ids, so saving a
 * project-scoped profile after changing its *name* silently widened the role
 * to the whole org.
 */
const keepsTheScopeOnEdit = recording(narrow());

export const SeedsTheComposedScopeIntoTheSheet: Story = {
  render: () => (
    <Harness fetchStub={keepsTheScopeOnEdit.stub}>
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const edit = await canvas.findByRole("button", { name: "Edit Deploy admins" });
    await waitFor(() => expect(edit).toBeEnabled());
    await userEvent.click(edit);

    const form = within(sheet());
    await expect(form.getByRole("checkbox", { name: /Deploy admin/ })).toBeChecked();
    // the stored scope comes back into the picker rather than resetting to the
    // org
    await expect(
      await form.findByLabelText("Where Deploy admin applies"),
    ).toHaveValue(PROJECT.name);

    await userEvent.click(form.getByRole("button", { name: "Save profile" }));

    const body = (await keepsTheScopeOnEdit.expectSentBody(
      "PUT",
      "/access-profiles/p-3",
    )) as { roles: Record<string, string>[] };
    expect(body.roles).toEqual([{ role_id: "role-2", project_id: PROJECT.id }]);
  },
};

// narrowing an existing composition further, and back out to the org: the
// scope is editable, not just readable
const rescopes = recording(narrow());

export const WidensAComposedRoleBackToTheOrg: Story = {
  render: () => (
    <Harness fetchStub={rescopes.stub}>
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const edit = await canvas.findByRole("button", { name: "Edit Deploy admins" });
    await waitFor(() => expect(edit).toBeEnabled());
    await userEvent.click(edit);

    const form = within(sheet());
    await pickOption(
      await form.findByLabelText("Where Deploy admin applies"),
      "Whole organization",
    );
    await userEvent.click(form.getByRole("button", { name: "Save profile" }));

    const body = (await rescopes.expectSentBody("PUT", "/access-profiles/p-3")) as {
      roles: Record<string, string>[];
    };
    // neither id, which the control plane reads as the profile's own org
    expect(body.roles).toEqual([{ role_id: "role-2" }]);
  },
};

/**
 * Ticking a role and changing your mind leaves the draft where it started.
 *
 * The scope of a composition is kept beside `roleIds` rather than inside it, so
 * that unticking a role and ticking it again does not throw away a scope
 * somebody picked. The trap that buys is a map that grows on a toggle the
 * operator undid: the sheet compares the whole draft against the one it opened
 * with, so an entry left behind by a round trip through the checkbox reads as
 * an edit, and closing asks to discard changes nobody made. A prompt on a form
 * nobody touched is what teaches people to click through the one that matters.
 */
export const TogglingARoleOffLeavesTheDraftClean: Story = {
  render: () => (
    <Harness fetchStub={stub(async () => json([]))}>
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "+ Add profile");

    const form = within(sheet());
    const role = form.getByRole("checkbox", { name: /Support engineer/ });
    await userEvent.click(role);
    await expect(await form.findByLabelText("Where Support engineer applies")).toBeVisible();
    await userEvent.click(role);
    await expect(role).not.toBeChecked();

    await expectClosesWithoutPrompting();
  },
};
