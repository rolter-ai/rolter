import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Teams from "./Teams";
import {
  clickWhenEnabled,
  confirmDestructive,
  expectAllowed,
  expectSheetClosed,
  openOptions,
  recording,
  expectEmptyState,
  expectLoadError,
  expectNoFalseEmpty,
  expectRefused,
  expectSkeleton,
  expectToast,
  Harness,
  json,
  ORG,
  sheet,
  Toasted,
  type FetchStub,
} from "./story-harness";
import type { BudgetRow, MembershipRow, OrgProjectRow, TeamRow, UserRow } from "@/lib/api";
import { resolveColorToken } from "@/lib/story-tokens";

const TEAMS: TeamRow[] = [
  { id: "team-1", org_id: "org-1", name: "Platform", created_at: "2026-01-04T00:00:00Z" },
  { id: "team-2", org_id: "org-1", name: "Research", created_at: "2026-02-11T00:00:00Z" },
];

/**
 * This screen reads the same `/orgs/{id}/teams` the sidebar scope switcher
 * does, so the shared `scoped()` stub cannot be used: it would answer the
 * screen's own query with the scope fixture and no story could show an org
 * with no teams. Only `/orgs` is fixed here; everything else is per-story.
 */
const orgThen =
  (teams: () => Response | Promise<Response>): FetchStub =>
  async (input) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (path === "/api/v1/orgs") return json([ORG]);
    if (/\/teams$/.test(path)) return teams();
    return json([]);
  };

const meta = {
  title: "Screens/Teams",
  component: Teams,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Teams>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={orgThen(() => json(TEAMS))}>
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Platform")).toBeVisible());
    await expect(canvas.getByText("Research")).toBeVisible();
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={orgThen(() => new Promise<Response>(() => {}))}>
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No teams yet/);
  },
};

// an org with no teams yet: the placeholder says what a team buys and offers
// the only control that makes one
export const Empty: Story = {
  render: () => (
    <Harness fetchStub={orgThen(() => json([]))}>
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No teams yet/, /New team/);
  },
};

export const Error_: Story = {
  name: "Error",
  render: () => (
    <Harness fetchStub={orgThen(() => json({ error: { message: "boom" } }, 500))}>
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return teams/i);
    await expectNoFalseEmpty(canvasElement, /No teams yet/);
  },
};

// retrying a 403 cannot work, so LoadError withholds the retry and names who can
export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={orgThen(() => json({ error: { message: "forbidden" } }, 403))}>
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /You do not have access to teams/);
    await expectNoFalseEmpty(canvasElement, /No teams yet/);
  },
};

/**
 * The team is refused (#1607).
 *
 * `onError` only pushes a toast — the sheet is left standing on purpose, so the
 * name survives a duplicate-slug refusal and can be corrected in place.
 */
export const CreateRejectedByTheServer: Story = {
  render: () => (
    <Harness
      fetchStub={async (input, init) => {
        const path = new URL(String(input), "http://localhost").pathname;
        if (init?.method === "POST") {
          return json({ error: { message: "a team called Platform already exists" } }, 409);
        }
        if (path === "/api/v1/orgs") return json([ORG]);
        if (/\/teams$/.test(path)) return json(TEAMS);
        return json([]);
      }}
    >
      <Toasted>
        <Teams />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /new team/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Team name"), "Platform");
    await userEvent.click(within(form).getByRole("button", { name: "Create" }));
    await expectToast(canvasElement, /already exists/, "error");
    await waitFor(() => expect(within(document.body).getByRole("dialog")).toBeInTheDocument());
    await expect(within(form).getByLabelText("Team name")).toHaveValue("Platform");
  },
};

// Creating a team is admin, and the control is the only one on the screen —
// so the gate has exactly one story to lose it from (#1606).
export const RefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={orgThen(() => json(TEAMS))} role="viewer">
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, /new team/i);
  },
};

// the empty state repeats the create control, and this is the case #1623
// thought could not be gated: the screen's team list *is* the scope's, so an
// org with no teams was believed to leave `useScope` loading forever, the
// effective-permissions query unasked, and every control on the screen enabled.
// It does not — a disabled query is settled, not pending — and this story is
// what keeps it that way
export const RefusedToAMember: Story = {
  render: () => (
    <Harness fetchStub={orgThen(() => json([]))} role="member">
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, /new team/i);
  },
};

// ---------------------------------------------------------------------------
// #3010: the access profiles a team holds, and handing one out from here

const PROFILE = (id: string, name: string) => ({
  id,
  org_id: "org-1",
  slug: name.toLowerCase().replace(/\s+/g, "-"),
  name,
  description: null,
  created_at: "",
  updated_at: "",
});
const SUPPORT = PROFILE("p-1", "Support engineers");
const ONCALL = PROFILE("p-2", "On-call");

/** the org's teams and profiles, holding what posts and deletes changed */
function teamProfiles(initial: Record<string, string[]> = { "p-1": ["team-1"], "p-2": [] }) {
  const held = new Map(Object.entries(initial).map(([id, teams]) => [id, [...teams]]));
  return recording(async (input, init) => {
    const url = String(input);
    const path = new URL(url, "http://localhost").pathname;
    if (path === "/api/v1/orgs") return json([ORG]);
    if (/\/teams$/.test(path)) return json(TEAMS);
    const post = /\/access-profiles\/([^/]+)\/assignments$/.exec(path);
    if (init?.method === "POST" && post) {
      const body = JSON.parse(String(init.body)) as { team_id: string };
      held.set(post[1], [...(held.get(post[1]) ?? []), body.team_id]);
      return json({ id: "a-new", profile_id: post[1], created_at: "", user_id: null, ...body });
    }
    const del = /\/access-profile-assignments\/a-(p-\d)-(team-\d)$/.exec(path);
    if (init?.method === "DELETE" && del) {
      held.set(
        del[1],
        (held.get(del[1]) ?? []).filter((x) => x !== del[2]),
      );
      return json({}, 204);
    }
    if (/\/access-profile-assignments$/.test(path)) {
      return json(
        [...held].flatMap(([profileId, teams]) =>
          teams.map((team_id) => ({
            id: `a-${profileId}-${team_id}`,
            profile_id: profileId,
            user_id: null,
            team_id,
            created_at: "",
          })),
        ),
      );
    }
    const detail = /\/access-profiles\/(p-\d)$/.exec(path);
    if (detail) {
      return json({
        ...(detail[1] === "p-1" ? SUPPORT : ONCALL),
        roles: [],
        policy: null,
        assignments: (held.get(detail[1]) ?? []).map((team_id) => ({
          id: `a-${detail[1]}-${team_id}`,
          profile_id: detail[1],
          user_id: null,
          team_id,
          created_at: "",
        })),
      });
    }
    if (/\/access-profiles$/.test(path)) return json([SUPPORT, ONCALL]);
    return json([]);
  });
}

const listsTeamProfiles = teamProfiles();
export const ListsTheProfilesATeamHolds: Story = {
  render: () => (
    <Harness fetchStub={listsTeamProfiles.stub} role="admin">
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const list = await canvas.findByRole("list", { name: "Access profiles held by Platform" });
    await expect(within(list).getByText("Support engineers")).toBeVisible();
    // the assignments come in one org-wide read, not a detail per profile (#3025)
    await listsTeamProfiles.expectSent("GET", "/access-profile-assignments");
    listsTeamProfiles.expectNotSent("GET", "/access-profiles/p-1");
    // a team that holds none reads as such
    await waitFor(() => expect(canvas.getByText("no access profile")).toBeVisible());
  },
};

export const ProfilesFailedToLoad: Story = {
  render: () => (
    <Harness
      role="admin"
      fetchStub={async (input) => {
        const path = new URL(String(input), "http://localhost").pathname;
        if (path === "/api/v1/orgs") return json([ORG]);
        if (/\/teams$/.test(path)) return json(TEAMS);
        if (path.includes("/access-profile")) return json({ error: { message: "boom" } }, 500);
        return json([]);
      }}
    >
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("profiles not loaded")).toHaveLength(2));
    await expect(canvas.queryByText("no access profile")).toBeNull();
  },
};

const blockedAssign = teamProfiles();

/** Assigning with no profile picked names the picker instead of doing nothing (#3242). */
export const AssignWithoutAPickNamesTheField: Story = {
  render: () => (
    <Harness fetchStub={blockedAssign.stub} role="admin">
      <Toasted>
        <Teams />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("Research");
    await expectAllowed(canvasElement, "Assign an access profile to Research");
    await userEvent.click(
      canvas.getByRole("button", { name: "Assign an access profile to Research" }),
    );

    const form = within(sheet());
    const picker = await form.findByRole("combobox", { name: "Access profile" });
    await expect(picker).toBeRequired();
    await expect(form.queryByText("Required")).toBeNull();
    await userEvent.click(form.getByRole("button", { name: "Assign profile" }));
    await waitFor(async () => expect(await form.findByText("Required")).toBeVisible());
    await waitFor(() => expect(picker).toHaveFocus());
    await expect(await form.findByRole("status")).toHaveTextContent("1 field needs attention");
    blockedAssign.expectNotSent("POST", "/access-profiles/p-2/assignments");
  },
};

const teamAssign = teamProfiles();
export const AssignsAProfileToATeam: Story = {
  render: () => (
    <Harness fetchStub={teamAssign.stub} role="admin">
      <Toasted>
        <Teams />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("Research");
    await expectAllowed(canvasElement, "Assign an access profile to Research");
    await userEvent.click(
      canvas.getByRole("button", { name: "Assign an access profile to Research" }),
    );

    const form = within(sheet());
    const listbox = await openOptions(
      await form.findByRole("combobox", { name: "Access profile" }),
    );
    await waitFor(() =>
      expect(within(listbox).getByRole("option", { name: /On-call/ })).toBeVisible(),
    );
    await userEvent.click(within(listbox).getByRole("option", { name: /On-call/ }));
    await userEvent.click(form.getByRole("button", { name: "Assign profile" }));

    const body = await teamAssign.expectSentBody("POST", "/access-profiles/p-2/assignments");
    expect(body).toEqual({ team_id: "team-2" });
    await expectSheetClosed();
    await expectToast(canvasElement, /On-call assigned to Research/);
    const list = await canvas.findByRole("list", { name: "Access profiles held by Research" });
    await waitFor(() => expect(within(list).getByText("On-call")).toBeVisible());
  },
};

const teamRemove = teamProfiles();
export const RemovesAProfileFromATeam: Story = {
  render: () => (
    <Harness fetchStub={teamRemove.stub} role="admin">
      <Toasted>
        <Teams />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("Platform");
    await expectAllowed(canvasElement, "Remove Support engineers from Platform");
    await userEvent.click(
      canvas.getByRole("button", { name: "Remove Support engineers from Platform" }),
    );
    await confirmDestructive(/Remove Platform from Support engineers/, "Remove assignment");

    await teamRemove.expectSent("DELETE", "/access-profile-assignments/a-p-1-team-1");
    await expectSheetClosed();
    await waitFor(() => expect(canvas.getAllByText("no access profile")).toHaveLength(2));
  },
};

export const ProfileAssignmentRefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={teamProfiles().stub} role="viewer">
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("Platform");
    await expectRefused(canvasElement, "Assign an access profile to Platform");
    await expectRefused(canvasElement, "Remove Support engineers from Platform");
  },
};

// ---------------------------------------------------------------------------
// #2066 / #2072: who the cards name and count, and the team's detail sheet

const user = (id: string, email: string): UserRow => ({
  id,
  email,
  display_name: null,
  is_superadmin: false,
  created_at: "2026-01-01T00:00:00Z",
});
const PEOPLE = [
  user("u-ada", "ada@example.com"),
  user("u-bob", "bob@example.com"),
  user("u-cy", "cy@example.com"),
  user("u-dee", "dee@example.com"),
];
const grant = (
  id: string,
  user_id: string,
  role: string,
  at: Partial<MembershipRow>,
): MembershipRow => ({ id, user_id, role, created_at: "2026-01-01T00:00:00Z", ...at });
// Platform: ada and gil administer it, bob also works on one project; Research:
// its only person holds a role on a project and none on the team itself
const GRANTS = [
  grant("m1", "u-ada", "admin", { team_id: "team-1" }),
  grant("m2", "u-bob", "member", { team_id: "team-1" }),
  grant("m3", "u-bob", "viewer", { project_id: "proj-1" }),
  grant("m4", "u-cy", "member", { project_id: "proj-1" }),
  grant("m5", "u-dee", "member", { project_id: "proj-3" }),
  grant("m6", "u-ada", "admin", { org_id: "org-1" }),
];
const project = (id: string, team_id: string, name: string): OrgProjectRow => ({
  id,
  team_id,
  name,
  team_name: team_id === "team-1" ? "Platform" : "Research",
  created_at: "2026-01-01T00:00:00Z",
});
const PROJECTS = [
  project("proj-1", "team-1", "prod"),
  project("proj-2", "team-1", "staging"),
  project("proj-3", "team-2", "evals"),
];
const budget = (id: string, scope_id: string, limit_usd: string, period: string): BudgetRow => ({
  id,
  scope_type: "team",
  scope_id,
  limit_usd,
  period,
  unpriced_policy: null,
  created_at: "2026-01-01T00:00:00Z",
});
const BUDGETS: Record<string, BudgetRow[]> = {
  "team-1": [budget("b1", "team-1", "500", "monthly"), budget("b2", "team-1", "50", "daily")],
  "team-2": [],
};

type Part = "memberships" | "users" | "projects" | "budgets";

/** the org's teams with the reads each card and the sheet draw on; `fail` breaks one of them */
function teamsApi(fail: Partial<Record<Part, "pending" | "error">> = {}) {
  let deleted = false;
  let renamed: string | null = null;
  return recording(async (input, init) => {
    const url = new URL(String(input), "http://localhost");
    const path = url.pathname;
    const answer = (part: Part, body: unknown) =>
      fail[part] === "pending"
        ? new Promise<Response>(() => {})
        : fail[part] === "error"
          ? json({ error: { message: "boom" } }, 500)
          : json(body);
    if (path === "/api/v1/orgs") return json([ORG]);
    if (init?.method === "DELETE" && path === "/api/v1/teams/team-1") {
      deleted = true;
      return json({}, 204);
    }
    if (init?.method === "PUT" && path === "/api/v1/teams/team-1") {
      const { name } = JSON.parse(String(init.body)) as { name: string };
      if (name === "Research") {
        return json(
          { error: { code: "name_taken", message: "team 'Research' already exists" } },
          409,
        );
      }
      renamed = name;
      return json({ ...TEAMS[0], name });
    }
    if (/\/teams$/.test(path)) {
      const rows = renamed ? [{ ...TEAMS[0], name: renamed }, ...TEAMS.slice(1)] : TEAMS;
      return json(deleted ? rows.slice(1) : rows);
    }
    if (/\/memberships$/.test(path)) return answer("memberships", GRANTS);
    if (/\/users$/.test(path)) return answer("users", PEOPLE);
    if (/\/orgs\/org-1\/projects$/.test(path)) return answer("projects", PROJECTS);
    if (/\/orgs\/org-1\/budgets$/.test(path))
      return answer("budgets", Object.values(BUDGETS).flat());
    return json([]);
  });
}

const cardOf = async (canvas: ReturnType<typeof within>, name: string) =>
  within(await canvas.findByRole("group", { name }));

// the numbers say what they count: people across the team and its projects (not
// the team-scoped grants alone), the admin by email, and a second budget as "+1"
const cards = teamsApi();
export const NamesAdminsAndCountsPeople: Story = {
  render: () => (
    <Harness fetchStub={cards.stub}>
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const platform = await cardOf(canvas, "Platform");
    // ada, bob and cy: bob holds two grants and counts once, ada's org-wide one is no team's
    await waitFor(() => expect(platform.getByText("3")).toBeVisible());
    await expect(platform.getByText("2")).toBeVisible();
    await expect(platform.getByText("ada@example.com")).toBeVisible();
    await expect(await platform.findByText(/\$500/)).toBeVisible();
    await expect(platform.getByText("+1 more")).toBeVisible();
    // a team whose only person works at project scope still has one
    const research = await cardOf(canvas, "Research");
    await waitFor(() => expect(research.getAllByText("1")).toHaveLength(2));
    await expect(research.getByText("none yet")).toBeVisible();
    await expect(research.getByText("—")).toBeVisible();
    // two teams, one budgets request (#1996)
    await expect(cards.calls.filter((c) => c.url.includes("budgets"))).toHaveLength(1);
  },
};

// a budget that could not be read is not a team with no budget
const budgetDown = teamsApi({ budgets: "error" });
export const BudgetFailedToLoad: Story = {
  render: () => (
    <Harness fetchStub={budgetDown.stub}>
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("unavailable")).toHaveLength(2));
    const [unavailable] = canvas.getAllByText("unavailable");
    await expect(getComputedStyle(unavailable).color).toBe(
      resolveColorToken("--status-danger-text"),
    );
    await expect(canvas.queryByText("—")).toBeNull();
  },
};

// counts the screen has not read are never drawn as zero
const readsPending = teamsApi({ memberships: "pending", projects: "pending" });
export const CountsWaitForTheirReads: Story = {
  render: () => (
    <Harness fetchStub={readsPending.stub}>
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole("group", { name: "Platform" });
    await expectNoFalseEmpty(canvasElement, /No teams yet/);
    await expect(canvas.queryByText("none yet")).toBeNull();
  },
};

const detail = teamsApi();
export const OpensATeamToSeeItsProjectsAndPeople: Story = {
  render: () => (
    <Harness fetchStub={detail.stub} role="admin">
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Open Platform" }));
    const panel = within(sheet());
    const onTeam = await panel.findByRole("list", { name: "Roles on the team" });
    await expect(within(onTeam).getByText("ada@example.com")).toBeVisible();
    await expect(within(onTeam).getByText("Admin")).toBeVisible();
    await expect(within(onTeam).getByText("bob@example.com")).toBeVisible();
    const projects = panel.getByRole("list", { name: "Projects" });
    await expect(within(projects).getByText("prod")).toBeVisible();
    await expect(within(projects).getByText("staging")).toBeVisible();
    // another team's project is not listed
    await expect(within(projects).queryByText("evals")).toBeNull();
    const onProd = panel.getByRole("list", { name: "Roles on prod" });
    await expect(within(onProd).getByText("cy@example.com")).toBeVisible();
    await waitFor(() =>
      expect(panel.getByText("No one holds a role on this project directly.")).toBeVisible(),
    );
    await expect(panel.getByRole("link", { name: "Users" })).toHaveAttribute(
      "href",
      "/gov-users?team=team-1",
    );
    await expect(panel.getByRole("link", { name: /Budgets & Limits of Platform/ })).toHaveAttribute(
      "href",
      "/budgets?scope_type=team&scope_id=team-1",
    );
    const deleteBtn = panel.getByRole("button", { name: "Delete Platform…" });
    await expect(getComputedStyle(deleteBtn).color).toBe(resolveColorToken("--status-danger-text"));
  },
};

// a team with nothing under it says so in both lists
const emptyApi = recording(async (input) => {
  const path = new URL(String(input), "http://localhost").pathname;
  if (path === "/api/v1/orgs") return json([ORG]);
  if (/\/teams$/.test(path)) return json([TEAMS[1]]);
  return json([]);
});
export const OpenedTeamWithNothingInIt: Story = {
  render: () => (
    <Harness fetchStub={emptyApi.stub}>
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", { name: "Open Research" }),
    );
    const panel = within(sheet());
    const none = await panel.findByText(/No one holds a role on this team itself/);
    await waitFor(() => expect(none).toBeVisible());
    await waitFor(() => expect(panel.getByText(/This team has no projects yet/)).toBeVisible());
  },
};

const sheetLoading = teamsApi({ memberships: "pending", projects: "pending" });
export const OpenedTeamLoading: Story = {
  render: () => (
    <Harness fetchStub={sheetLoading.stub}>
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", { name: "Open Platform" }),
    );
    await expectSkeleton(sheet());
    await expect(within(sheet()).queryByText(/No one holds a role/)).toBeNull();
    await expect(within(sheet()).queryByText(/no projects yet/)).toBeNull();
  },
};

const sheetBroken = teamsApi({ memberships: "error", projects: "error" });
export const OpenedTeamFailedToLoad: Story = {
  render: () => (
    <Harness fetchStub={sheetBroken.stub}>
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", { name: "Open Platform" }),
    );
    await expectLoadError(sheet(), /failed to return role grants/i);
    await expectLoadError(sheet(), /failed to return projects/i);
    await expect(within(sheet()).queryByText(/No one holds a role/)).toBeNull();
    await expect(within(sheet()).queryByText(/no projects yet/)).toBeNull();
  },
};

const removal = teamsApi();
export const DeletesATeamAfterNamingWhatGoesWithIt: Story = {
  render: () => (
    <Harness fetchStub={removal.stub} role="admin">
      <Toasted>
        <Teams />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Open Platform" }));
    await expectAllowed(document.body, "Delete Platform…");
    await userEvent.click(within(sheet()).getByRole("button", { name: "Delete Platform…" }));
    // the dialog names the team and how many projects go with it
    // the sheet makes way for the question
    await confirmDestructive(/Delete team Platform\?/, "Delete team");
    await removal.expectSent("DELETE", "/api/v1/teams/team-1");
    await expectToast(canvasElement, /Platform deleted/);
    await expectSheetClosed();
    await waitFor(() => expect(canvas.queryByRole("group", { name: "Platform" })).toBeNull());
  },
};

const confirmText = teamsApi();
export const DeleteConfirmationCountsTheProjects: Story = {
  render: () => (
    <Harness fetchStub={confirmText.stub} role="admin">
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", { name: "Open Platform" }),
    );
    await userEvent.click(await within(sheet()).findByRole("button", { name: "Delete Platform…" }));
    await expect(
      await within(document.body).findByText(/Its 2 projects and everything scoped/),
    ).toBeVisible();
  },
};

// deleting a team is an org admin's: a member opens the sheet and is refused the delete
export const DeleteRefusedToAMember: Story = {
  render: () => (
    <Harness fetchStub={teamsApi().stub} role="member">
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", { name: "Open Platform" }),
    );
    await expectRefused(document.body, "Delete Platform…");
  },
};

// #3126: the name at the top of the sheet renames the team
const renaming = teamsApi();
export const RenamesATeam: Story = {
  render: () => (
    <Harness fetchStub={renaming.stub} role="admin">
      <Toasted>
        <Teams />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Open Platform" }));
    const panel = within(sheet());
    const field = panel.getByLabelText("Team name");
    // announced as mandatory without flagging the untouched name (#3451)
    await expect(field).toHaveAttribute("aria-required", "true");
    await expect(field).not.toHaveAttribute("required");
    await expect(field).toBeValid();
    // nothing to send until the name differs
    // story-wait-allow: disabled by the unchanged name from its first paint, not by the gate
    await expect(panel.getByRole("button", { name: "Rename" })).toBeDisabled();
    await userEvent.clear(field);
    await userEvent.type(field, "Core platform");
    await userEvent.click(panel.getByRole("button", { name: "Rename" }));
    await expect(await renaming.expectSentBody("PUT", "/api/v1/teams/team-1")).toEqual({
      name: "Core platform",
    });
    await expectToast(canvasElement, /Core platform updated/);
    await canvas.findByRole("group", { name: "Core platform" });
    await expect(canvas.queryByRole("group", { name: "Platform" })).toBeNull();
  },
};

// a name another team holds is pinned under the field, not shown in a toast
export const RenameToATakenNameIsPinnedToTheField: Story = {
  render: () => (
    <Harness fetchStub={teamsApi().stub} role="admin">
      <Toasted>
        <Teams />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", { name: "Open Platform" }),
    );
    const panel = within(sheet());
    const field = panel.getByLabelText("Team name");
    await userEvent.clear(field);
    await userEvent.type(field, "Research");
    await userEvent.click(panel.getByRole("button", { name: "Rename" }));
    const taken = await panel.findByText(/A team called Research already exists/);
    await waitFor(() => expect(taken).toBeVisible());
    await expect(field).toHaveAttribute("aria-invalid", "true");
    await expect(field).toHaveFocus();
    // editing the name answers the complaint
    await userEvent.type(field, "2");
    await waitFor(() => expect(field).not.toHaveAttribute("aria-invalid", "true"));
  },
};

export const RenameToAnEmptyNameIsRefusedLocally: Story = {
  render: () => {
    const api = teamsApi();
    return (
      <Harness fetchStub={api.stub} role="admin">
        <Teams />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", { name: "Open Platform" }),
    );
    const panel = within(sheet());
    await userEvent.clear(panel.getByLabelText("Team name"));
    const empty = await panel.findByText("Enter a name for the team.");
    await waitFor(() => expect(empty).toBeVisible());
  },
};

// renaming is the team's admin's; a member opens the sheet and is refused
export const RenameRefusedToAMember: Story = {
  render: () => (
    <Harness fetchStub={teamsApi().stub} role="member">
      <Teams />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", { name: "Open Platform" }),
    );
    const field = within(sheet()).getByLabelText("Team name");
    await userEvent.type(field, "x");
    await expectRefused(document.body, "Rename");
  },
};
