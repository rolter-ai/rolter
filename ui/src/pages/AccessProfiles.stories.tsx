import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import AccessProfiles from "./AccessProfiles";
import {
  cancelConfirmation,
  clickWhenEnabled,
  adminOfProject,
  confirmation,
  confirmDestructive,
  expectAllowed,
  expectEmptyState,
  expectLoadError,
  expectRefused,
  expectClosesWithoutPrompting,
  expectNoFalseEmpty,
  expectNoUxEvent,
  expectSheetClosed,
  expectSkeleton,
  expectUxEvent,
  Harness,
  json,
  openOptions,
  pending,
  pickOption,
  PROJECT,
  recordUxEvents,
  recording,
  scoped,
  sheet,
  TEAM,
  Toasted,
  expectToast,
  uxEvents,
  type FetchStub,
} from "./story-harness";
import type {
  AccessProfileAssignmentRow,
  AccessProfileDetail,
  AccessProfileRow,
  CustomRoleRow,
  UserRow,
} from "@/lib/api";
import { AuthProvider } from "@/lib/auth";
import { UxScreenProvider } from "@/lib/ux-react";

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
      { id: "a-2", profile_id: "p-1", user_id: null, team_id: TEAM.id, created_at: "" },
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

// the accounts the org lists: `u-1` is already assigned to Support engineers,
// `u-2` is who the assignment stories hand On-call to
const USERS: UserRow[] = [
  {
    id: "u-1",
    email: "ada@example.com",
    display_name: "Ada Lovelace",
    is_superadmin: false,
    created_at: "2026-01-01T00:00:00Z",
  },
  {
    id: "u-2",
    email: "grace@example.com",
    display_name: "Grace Hopper",
    is_superadmin: false,
    created_at: "2026-01-01T00:00:00Z",
  },
];

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
 * What `GET /api/v1/orgs/{org}/access-profiles?include=roles,assignments,policy`
 * answers (#3301): each profile with what it carries. The screen reads this one
 * request instead of one detail per card.
 */
function embedded(
  rows: AccessProfileRow[],
  details: Record<string, AccessProfileDetail> = DETAILS,
): AccessProfileDetail[] {
  return rows.map((row) => ({
    ...row,
    roles: details[row.id]?.roles ?? [],
    assignments: details[row.id]?.assignments ?? [],
    policy: details[row.id]?.policy ?? null,
  }));
}

const INCLUDES_DETAIL = /[?&]include=[^&]*roles/;

/**
 * Route by URL.
 *
 * The detail path is tested before the list it is a prefix of: both contain
 * `/access-profiles`, and answering the detail with the list would leave an
 * edit seeded from a profile that carries nothing. A list read that asks for
 * `include` is answered with each profile's roles, assignments and policy.
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
    if (url.includes("/access-profiles")) {
      const answer = await profiles();
      if (!INCLUDES_DETAIL.test(url) || !answer.ok) return answer;
      return json(embedded((await answer.clone().json()) as AccessProfileRow[], details));
    }
    if (url.includes("/custom-roles")) return json(roles);
    if (/\/orgs\/[^/]+\/users/.test(url)) return json(USERS);
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
    await waitFor(() => expect(canvas.getByText("Support engineers")).toBeVisible());
    await expect(canvas.getByText("On-call")).toBeVisible();

    // a profile's reach is the thing that matters: one user plus one team
    await waitFor(() => expect(canvas.getByText("1 user · 1 team")).toBeVisible());
    // and a profile assigned to nobody says so plainly
    await expect(canvas.getByText("Not assigned to anyone yet")).toBeVisible();

    // the roles and the policy it carries, read back with the list — before
    // #1184 a policy could be written and never shown again
    await expect(canvas.getByText("1 custom role")).toBeVisible();
    await expect(canvas.getByText("3 policy patterns")).toBeVisible();
    await expect(canvas.getByText("No model or route policy")).toBeVisible();
    // a slug is an identifier someone types into the API: it keeps its case
    await expect(canvas.getByText("support-engineers")).toHaveStyle({ textTransform: "none" });
  },
};

const oneRead = recording(stub(async () => json(PROFILES)));

/**
 * #3301: the screen used to read each profile's detail once per card, so an org
 * with many profiles made one request per profile. The list now carries every
 * profile's roles, assignments and policy (`?include=roles,assignments,policy`),
 * so two profiles are one request and the cards are drawn from it.
 */
export const LoadsEveryProfileWithOneRequest: Story = {
  render: () => (
    <Harness fetchStub={oneRead.stub}>
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("1 user · 1 team")).toBeVisible());
    await expect(canvas.getByText("Not assigned to anyone yet")).toBeVisible();
    const reads = oneRead.calls.filter(
      (c) => c.method === "GET" && c.url.includes("/access-profiles"),
    );
    // one read of the list, asking for everything a card draws, and no read of
    // a single profile
    await expect(reads).toHaveLength(1);
    const url = new URL(reads[0].url, "http://localhost");
    await expect(url.pathname).toBe(`/api/v1/orgs/${ORG}/access-profiles`);
    await expect(url.searchParams.get("include")).toBe("roles,assignments,policy");
    oneRead.expectNotSent("GET", "/access-profiles/p-1");
    oneRead.expectNotSent("GET", "/access-profiles/p-2");
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
    await expectNoFalseEmpty(canvasElement, /No access profiles yet/);
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
    await clickWhenEnabled(canvasElement, "Add profile");

    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Name"), "Support");
    // the roles are a request of their own, so the checkbox is not there the
    // instant the sheet is (#1689)
    await userEvent.click(await form.findByRole("checkbox", { name: /Support engineer/ }));
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

// the matcher reads a `*` as a wildcard only at the end of a line, so a pattern like
// `o1-*-mini` is an exact name nobody serves, and in an allow-list it blocks
// every model for everyone the profile reaches
export const RefusesAStarThatIsNotTheLastCharacter: Story = {
  render: () => (
    <Harness fetchStub={stub(async () => json([]))}>
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Add profile");

    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Name"), "Support");
    const save = form.getByRole("button", { name: "Create profile" });
    await waitFor(() => expect(save).toBeEnabled());

    await userEvent.type(form.getByLabelText("Allowed models"), "gpt-4o\no1-*-mini");
    const refusal = await form.findByText(/o1-\*-mini: a \* only works as the last character/);
    await waitFor(() => expect(refusal).toBeVisible());
    await expect(form.getByLabelText("Allowed models")).toBeInvalid();
    await waitFor(() => expect(save).toHaveAttribute("aria-disabled", "true"));

    // a trailing glob is what the hint promises, and it saves
    await userEvent.clear(form.getByLabelText("Allowed models"));
    await userEvent.type(form.getByLabelText("Allowed models"), "gpt-4o\no1-*");
    await waitFor(() => expect(save).toBeEnabled());
    await expect(form.queryByText(/only works as the last character/)).toBeNull();
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
    await clickWhenEnabled(canvasElement, "Add profile");

    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Name"), "Support");
    await userEvent.click(await form.findByRole("checkbox", { name: /Support engineer/ }));
    await userEvent.click(form.getByRole("button", { name: "Create profile" }));

    await expectToast(canvasElement, /already taken/, "error");
    await waitFor(() => expect(within(document.body).getByRole("dialog")).toBeInTheDocument());
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
    // the list carried the roles and the policy, so there is something to seed
    // the sheet with as soon as the card is drawn
    await userEvent.click(edit);

    const form = within(sheet());
    // the sheet reads the role list itself, so the checkbox is a request behind
    // the dialog it sits in (#1689)
    await expect(await form.findByRole("checkbox", { name: /Support engineer/ })).toBeChecked();
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
    await waitFor(() => expect(canvas.getByText("Support engineers")).toBeVisible());

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
    await waitFor(() => expect(canvas.getByText("Support engineers")).toBeVisible());

    await userEvent.click(canvas.getByRole("button", { name: "Delete Support engineers" }));
    await cancelConfirmation();
    deletes.expectNotSent("DELETE", "/access-profiles/p-1");

    await userEvent.click(canvas.getByRole("button", { name: "Delete Support engineers" }));
    await confirmDestructive(/Support engineers/, /delete profile/i);
    await deletes.expectSent("DELETE", "/access-profiles/p-1");

    // the outcome, not just the request: the confirmation closes, the queue
    // announces it, and the row is gone from the list
    await expectSheetClosed();
    await expectToast(canvasElement, /Support engineers deleted/);
    await waitFor(() => expect(canvas.queryByText("Support engineers")).not.toBeInTheDocument());
  },
};

// profiles are org-scoped and admin-gated, so a viewer gets 403
export const Error_: Story = {
  name: "Error",
  render: () => (
    <Harness fetchStub={stub(async () => json({ error: { message: "forbidden" } }, 403))}>
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /You do not have access to access profiles/);
    await expectNoFalseEmpty(canvasElement, /No access profiles yet/);
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
    // the exact name, not a fragment: the toolbar carries `Add profile` and
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
    await waitFor(() => expect(canvas.getByText("Deploy admin on Gateway")).toBeVisible());
    await expect(canvas.queryByText(PROJECT.id)).not.toBeInTheDocument();
    await expect(canvas.queryByText("role-2")).not.toBeInTheDocument();
  },
};

// the two lists `useOrgScope` reads, answered 500. matched before `scoped()`,
// which is what resolves the chain for every other story here
const scopeListsFail =
  (inner: FetchStub): FetchStub =>
  async (input, init) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (/^\/api\/v1\/orgs\/[^/]+\/(teams|projects)$/.test(path)) {
      return json({ error: { message: "scope unavailable" } }, 500);
    }
    return inner(input, init);
  };

/**
 * A scope the card cannot name says so, instead of drawing its uuid (#1677).
 *
 * The card is read-only: there is no picker under it, so the `LoadError` and
 * the retry the picker owns are nowhere in sight, and `Deploy admin on
 * 0f3a1c8e-…` reads exactly like a project that happens to be named that.
 */
export const ScopeThatCannotBeResolved: Story = {
  render: () => (
    <Harness fetchStub={scopeListsFail(narrow())}>
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the role is still named — only the scope half failed to resolve
    await waitFor(() => expect(canvas.getByText("Deploy admin on Unresolved scope")).toBeVisible());
    await expect(canvas.queryByText(PROJECT.id)).not.toBeInTheDocument();
    // the id stays quotable in a support conversation
    const chip = canvas.getByTitle(/could not be matched/);
    await expect(chip.getAttribute("title")).toContain(PROJECT.id);
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
    await clickWhenEnabled(canvasElement, "Add profile");

    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Name"), "Platform support");

    // the scope is only a question once the role is actually composed, so an
    // unchecked role carries no picker to answer
    await expect(form.queryByLabelText("Where Support engineer applies")).not.toBeInTheDocument();
    await userEvent.click(await form.findByRole("checkbox", { name: /Support engineer/ }));

    const picker = await form.findByLabelText("Where Support engineer applies");
    // the org is the default, which is the scope the control plane would have
    // defaulted to anyway: ignoring the picker writes what the sheet wrote
    // before this
    await expect(picker).toHaveValue("Whole organization");

    // the whole org is on offer, not just the team the scope switcher holds
    const listbox = await openOptions(picker);
    await expect(within(listbox).getByRole("option", { name: TEAM.name })).toBeVisible();
    await expect(within(listbox).getByRole("option", { name: PROJECT.name })).toBeVisible();
    await userEvent.click(within(listbox).getByRole("option", { name: TEAM.name }));

    await userEvent.click(form.getByRole("button", { name: "Create profile" }));

    const body = (await composesAtTeamScope.expectSentBody("POST", "/access-profiles")) as {
      roles: Record<string, string>[];
    };
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
    await expect(await form.findByRole("checkbox", { name: /Deploy admin/ })).toBeChecked();
    // the stored scope comes back into the picker rather than resetting to the
    // org
    await expect(await form.findByLabelText("Where Deploy admin applies")).toHaveValue(
      PROJECT.name,
    );

    await userEvent.click(form.getByRole("button", { name: "Save profile" }));

    const body = (await keepsTheScopeOnEdit.expectSentBody("PUT", "/access-profiles/p-3")) as {
      roles: Record<string, string>[];
    };
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
    await clickWhenEnabled(canvasElement, "Add profile");

    const form = within(sheet());
    // the role list is a request of its own (`/custom-roles`), fired as the
    // sheet opens: `getByRole` does not retry, so this found a checkbox only
    // while the stub answered inside the same tick (#1689). And it is looked up
    // again for every act rather than held, since checking a role grows the
    // scope row beside it and re-renders the list
    // story-wait-allow: a thunk, not a read - the line below is the waiter, and
    // every call of it happens after that
    const role = () => form.getByRole("checkbox", { name: /Support engineer/ });
    await form.findByRole("checkbox", { name: /Support engineer/ });
    await userEvent.click(role());
    await waitFor(() =>
      expect(form.getByLabelText("Where Support engineer applies")).toBeVisible(),
    );
    await userEvent.click(role());
    await waitFor(() => expect(role()).not.toBeChecked());

    await expectClosesWithoutPrompting();
  },
};

/**
 * Opening the profile editor and giving up is a `form_abandon` — on this
 * screen, the one that used to be lost (#1739).
 *
 * This screen holds the draft in state and mounts `ProfileSheet` only while it
 * exists, so cancelling *unmounts* the sheet rather than closing it and there
 * is no `open` falling to false anywhere. Instrumentation that only read that
 * edge reported this screen, and four others shaped like it, as zero
 * abandonments — which reads as "nobody ever gives up here" rather than as a
 * gap. The screen key comes from the shell in `App.tsx`, so the provider is
 * spelled out here the way the route supplies it.
 */
export const AbandoningTheEditorIsReported: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness fetchStub={stub(async () => json([]))}>
      <UxScreenProvider screen="access-profiles">
        <AccessProfiles />
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Add profile");
    await within(sheet()).findByLabelText("Name");

    await expectClosesWithoutPrompting();

    const event = await expectUxEvent("form_abandon", "access-profile-create");
    await expect(event.screen).toBe("access-profiles");
    await expect(typeof event.duration_ms).toBe("number");
    // and it is an abandonment, not a save that got mislabelled
    expectNoUxEvent("form_submit", "access-profile-create");
    await expect(uxEvents().filter((e) => e.action === "form_abandon")).toHaveLength(1);
  },
};

// ---------------------------------------------------------------------------
// #2073: handing a profile to people
//
// A profile that reaches nobody grants nothing, and the card could only count
// who it reached. The assignments are listed by name, added from a sheet and
// removed behind the shared confirmation.

/**
 * An assignment the stub holds, so the card shows the outcome of a write: the
 * detail answers what was posted and drops what was deleted, which a fixed
 * fixture would not (#1260).
 */
function assignable() {
  let extra: AccessProfileAssignmentRow[] = [];
  let removed = new Set<string>();
  const rec = recording(async (input, init) => {
    const url = String(input);
    if (init?.method === "POST" && url.includes("/access-profiles/p-2/assignments")) {
      const body = JSON.parse(String(init.body)) as { user_id?: string; team_id?: string };
      const row = {
        id: `a-new-${extra.length}`,
        profile_id: "p-2",
        user_id: body.user_id ?? null,
        team_id: body.team_id ?? null,
        created_at: "",
      };
      extra = [...extra, row];
      return json(row);
    }
    if (init?.method === "DELETE" && url.includes("/access-profile-assignments/")) {
      removed = new Set([...removed, url.split("/").pop() as string]);
      return json({}, 204);
    }
    // what each profile carries now, after the writes the story has made
    const current: Record<string, AccessProfileDetail> = {
      ...DETAILS,
      "p-2": { ...DETAILS["p-2"], assignments: extra.filter((a) => !removed.has(a.id)) },
      "p-1": {
        ...DETAILS["p-1"],
        assignments: DETAILS["p-1"].assignments.filter((a) => !removed.has(a.id)),
      },
    };
    const detail = /\/access-profiles\/([^/?]+)$/.exec(url);
    if (detail && current[detail[1]]) return json(current[detail[1]]);
    return stub(async () => json(PROFILES), ROLES, current)(input, init);
  });
  return rec;
}

const assigning = assignable();

export const NamesWhoAProfileReaches: Story = {
  render: () => (
    <Harness fetchStub={assigning.stub} role="admin">
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const list = await canvas.findByRole("list", { name: "Who Support engineers reaches" });
    // by name, not by id: the user from the org's list and the team from the scope
    await waitFor(() => expect(within(list).getByText("Ada Lovelace")).toBeVisible());
    await expect(within(list).getByText("Platform")).toBeVisible();
    await expectAllowed(canvasElement, "Assign Support engineers");
  },
};

const userAssign = assignable();

export const AssignsAUser: Story = {
  render: () => (
    <Harness fetchStub={userAssign.stub} role="admin">
      <Toasted>
        <AccessProfiles />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectAllowed(canvasElement, "Assign On-call");
    await userEvent.click(canvas.getByRole("button", { name: "Assign On-call" }));

    const form = within(sheet());
    // an already-assigned account is not offered twice, and the save waits for a pick
    // story-wait-allow: disabled by the sheet's own empty pick, not by the gate
    await expect(form.getByRole("button", { name: "Assign profile" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    const listbox = await openOptions(await form.findByRole("combobox", { name: "User" }));
    await waitFor(() =>
      expect(within(listbox).getByRole("option", { name: /Grace Hopper/ })).toBeVisible(),
    );
    await userEvent.click(within(listbox).getByRole("option", { name: /Grace Hopper/ }));
    await userEvent.click(form.getByRole("button", { name: "Assign profile" }));

    const body = await userAssign.expectSentBody("POST", "/access-profiles/p-2/assignments");
    expect(body).toEqual({ user_id: "u-2" });

    // the outcome: the sheet closes and the card now names who it reaches
    await expectSheetClosed();
    await expectToast(canvasElement, /On-call assigned/);
    const list = await canvas.findByRole("list", { name: "Who On-call reaches" });
    await expect(within(list).getByText("Grace Hopper")).toBeVisible();
  },
};

/** a press on Assign with nobody picked names the picker instead of doing nothing (#3242) */
export const AssignWithoutAPickNamesTheField: Story = {
  render: () => (
    <Harness fetchStub={userAssign.stub} role="admin">
      <Toasted>
        <AccessProfiles />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectAllowed(canvasElement, "Assign On-call");
    await userEvent.click(canvas.getByRole("button", { name: "Assign On-call" }));

    const form = within(sheet());
    const picker = await form.findByRole("combobox", { name: "User" });
    await expect(picker).toBeRequired();
    await expect(form.queryByText("Required")).toBeNull();
    await userEvent.click(form.getByRole("button", { name: "Assign profile" }));
    await waitFor(async () => expect(await form.findByText("Required")).toBeVisible());
    await waitFor(() => expect(picker).toHaveFocus());
    await expect(await form.findByRole("status")).toHaveTextContent("1 field needs attention");
  },
};

const teamAssign = assignable();

export const AssignsATeam: Story = {
  render: () => (
    <Harness fetchStub={teamAssign.stub} role="admin">
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectAllowed(canvasElement, "Assign On-call");
    await userEvent.click(canvas.getByRole("button", { name: "Assign On-call" }));

    const form = within(sheet());
    await userEvent.click(await form.findByRole("radio", { name: "Team" }));
    await pickOption(await form.findByRole("combobox", { name: "Team" }), /Platform/);
    await userEvent.click(form.getByRole("button", { name: "Assign profile" }));

    const body = await teamAssign.expectSentBody("POST", "/access-profiles/p-2/assignments");
    expect(body).toEqual({ team_id: TEAM.id });
    await expectSheetClosed();
    const list = await canvas.findByRole("list", { name: "Who On-call reaches" });
    await expect(within(list).getByText("Platform")).toBeVisible();
  },
};

// a refused save keeps the sheet and what was picked, and says why
export const AssignRejectedByTheServer: Story = {
  render: () => (
    <Harness
      role="admin"
      fetchStub={async (input, init) =>
        init?.method === "POST"
          ? json(
              {
                error: {
                  message: "an access profile may only be assigned to a team in its own org",
                },
              },
              400,
            )
          : stub(async () => json(PROFILES))(input, init)
      }
    >
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectAllowed(canvasElement, "Assign On-call");
    await userEvent.click(canvas.getByRole("button", { name: "Assign On-call" }));

    const form = within(sheet());
    await pickOption(await form.findByRole("combobox", { name: "User" }), /Grace Hopper/);
    await userEvent.click(form.getByRole("button", { name: "Assign profile" }));

    await waitFor(() => expect(form.getByText("Could not assign the profile")).toBeVisible());
    await expect(form.getByText(/only be assigned to a team in its own org/)).toBeVisible();
    await expect(form.getByRole("combobox", { name: "User" })).toHaveValue("Grace Hopper");
  },
};

const unassigning = assignable();

export const RemovesAnAssignmentAfterConfirming: Story = {
  render: () => (
    <Harness fetchStub={unassigning.stub} role="admin">
      <Toasted>
        <AccessProfiles />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const remove = await canvas.findByRole("button", {
      name: "Remove Ada Lovelace from Support engineers",
    });
    await expectAllowed(canvasElement, "Remove Ada Lovelace from Support engineers");

    await userEvent.click(remove);
    await cancelConfirmation();
    unassigning.expectNotSent("DELETE", "/access-profile-assignments/a-1");

    await userEvent.click(remove);
    await confirmDestructive(/Remove Ada Lovelace from Support engineers\?/, "Remove assignment");
    await unassigning.expectSent("DELETE", "/access-profile-assignments/a-1");

    await expectSheetClosed();
    await expectToast(canvasElement, /Ada Lovelace removed/);
    await waitFor(() => expect(canvas.queryByText("Ada Lovelace")).not.toBeInTheDocument());
  },
};

// the delete confirmation says how many people lose the profile
export const DeleteConfirmationNamesTheReach: Story = {
  render: () => (
    <Harness fetchStub={assigning.stub} role="admin">
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("1 user · 1 team")).toBeVisible());
    await userEvent.click(canvas.getByRole("button", { name: "Delete Support engineers" }));
    const dialog = await confirmation();
    await waitFor(() =>
      expect(within(dialog).getByText(/It reaches 1 user and 1 team\./)).toBeVisible(),
    );
    await cancelConfirmation();

    // the reach arrived with the list
    await expect(await canvas.findByText("Not assigned to anyone yet")).toBeVisible();
    await userEvent.click(canvas.getByRole("button", { name: "Delete On-call" }));
    await waitFor(async () =>
      expect(within(await confirmation()).getByText(/not assigned to anyone yet/)).toBeVisible(),
    );
    await cancelConfirmation();
  },
};

// assigning and unassigning are both `access_profile_assignment` writes, which a
// viewer reads and may not change; a member is short of admin as well
export const AssignmentRefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={assigning.stub} role="viewer">
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Assign Support engineers");
    await expectRefused(canvasElement, "Assign On-call");
    await expectRefused(canvasElement, "Remove Ada Lovelace from Support engineers");
  },
};

// `access_profile_assignment` is an org-scoped row: the control plane checks it
// at the profile's org, where an admin of one project holds nothing. The gate
// answers for the same chain, so the buttons are refused rather than offered
// and answered with a 403 (rbac-gating.md, "What `effective` answers per scope")
export const AssignmentRefusedToAProjectAdmin: Story = {
  render: () => (
    <Harness fetchStub={assigning.stub} role={adminOfProject(PROJECT.id)}>
      <AccessProfiles />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Assign Support engineers");
    await expectRefused(canvasElement, "Assign On-call");
    await expectRefused(canvasElement, "Remove Ada Lovelace from Support engineers");
  },
};

// ---------------------------------------------------------------------------
// #3010: a superadmin's unassigned accounts in the picker

const SEEDED_ADMIN: UserRow = {
  id: "u-root",
  email: "root@example.com",
  is_superadmin: true,
  deactivated_at: null,
  created_at: "2026-01-01T00:00:00Z",
};

function SignedInAsSuperadmin({ children }: { children: React.ReactNode }) {
  React.useState(() => {
    localStorage.setItem("rolter.session.email", SEEDED_ADMIN.email);
    localStorage.setItem("rolter.session.user", JSON.stringify(SEEDED_ADMIN));
    localStorage.removeItem("rolter.session.token");
  });
  React.useEffect(
    () => () => {
      localStorage.removeItem("rolter.session.email");
      localStorage.removeItem("rolter.session.user");
    },
    [],
  );
  return <AuthProvider>{children}</AuthProvider>;
}

// the accounts with no membership anywhere are in no org's list, so only the
// request that asks for them can offer them
const unassignedPicker = recording(
  scoped(async (input) => {
    const url = String(input);
    const detail = /\/access-profiles\/([^/?]+)$/.exec(url);
    if (detail) return json(DETAILS[detail[1]] ?? {});
    if (url.includes("/access-profiles")) {
      return json(INCLUDES_DETAIL.test(url) ? embedded(PROFILES) : PROFILES);
    }
    if (/\/orgs\/[^/]+\/users/.test(url)) {
      return json(url.includes("include_unassigned=true") ? [SEEDED_ADMIN, ...USERS] : USERS);
    }
    return json([]);
  }),
);

export const SuperadminPicksAnUnassignedAccount: Story = {
  render: () => (
    <Harness fetchStub={unassignedPicker.stub} role="superadmin">
      <SignedInAsSuperadmin>
        <AccessProfiles />
      </SignedInAsSuperadmin>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectAllowed(canvasElement, "Assign On-call");
    await userEvent.click(canvas.getByRole("button", { name: "Assign On-call" }));

    const form = within(sheet());
    const listbox = await openOptions(await form.findByRole("combobox", { name: "User" }));
    await waitFor(() =>
      expect(within(listbox).getByRole("option", { name: /root@example.com/ })).toBeVisible(),
    );
    await unassignedPicker.expectSent("GET", "/orgs/org-1/users?include_unassigned=true");
  },
};
