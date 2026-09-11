import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { OrgScopePicker, type ScopeTarget } from "./OrgScopePicker";
import { Harness, ORG, json, type FetchStub } from "@/pages/story-harness";

const NOW = "2026-01-01T00:00:00Z";

const TEAMS = [
  { id: "team-1", org_id: ORG.id, name: "Platform", created_at: NOW },
  { id: "team-2", org_id: ORG.id, name: "Payments", created_at: NOW },
];

// each team has a project called "prod": the reason the options are grouped by
// team rather than listed flat. one org-wide list, as the endpoint returns it —
// every row naming the team that owns it
const PROJECTS = [
  { id: "project-1", team_id: "team-1", team_name: "Platform", name: "Gateway", created_at: NOW },
  { id: "project-2", team_id: "team-1", team_name: "Platform", name: "prod", created_at: NOW },
  { id: "project-3", team_id: "team-2", team_name: "Payments", name: "prod", created_at: NOW },
];

/**
 * The org chain, answered directly rather than through `scoped()`.
 *
 * The picker's own queries *are* the chain the shared helper stands in for, so
 * a story that used the helper could never show an org with no teams, nor one
 * whose team list failed.
 */
const chain =
  (
    over: {
      teams?: () => Response | Promise<Response>;
      projects?: () => Response | Promise<Response>;
    } = {},
  ): FetchStub =>
  async (input) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (path === "/api/v1/orgs") return json([ORG]);
    if (/^\/api\/v1\/orgs\/[^/]+\/projects$/.test(path)) {
      return (over.projects ?? (() => json(PROJECTS)))();
    }
    if (/^\/api\/v1\/orgs\/[^/]+\/teams$/.test(path)) {
      return (over.teams ?? (() => json(TEAMS)))();
    }
    return json([]);
  };

// the picker is controlled, and what a story asserts is usually the value it
// pushed back — so the wrapper keeps it and prints it
function Picker({ fetchStub }: { fetchStub: FetchStub }) {
  const [value, setValue] = React.useState<ScopeTarget>("");
  return (
    <Harness fetchStub={fetchStub}>
      <OrgScopePicker
        orgId={ORG.id}
        value={value}
        onChange={setValue}
        label="Where the role applies"
      />
      <p data-testid="picked">{value}</p>
    </Harness>
  );
}

// the wrapper is what the stories mount: `OrgScopePicker` is controlled, and a
// meta pointed at it would make every story restate the same four args
const meta = {
  title: "Components/OrgScopePicker",
  component: Picker,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Picker>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * Every team in the org, and every project in any of those teams — including
 * the ones under a team the scope switcher does not have selected (#1249).
 */
export const EveryScopeInTheOrg: Story = {
  args: { fetchStub: chain() },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const select = await canvas.findByLabelText("Where the role applies");
    await waitFor(() =>
      expect(within(select).getByRole("group", { name: "Teams" })).toBeInTheDocument(),
    );
    // the two "prod" projects are told apart by the team they hang under
    await expect(
      within(select).getByRole("group", { name: "Projects in Platform" }),
    ).toBeInTheDocument();
    await expect(
      within(select).getByRole("group", { name: "Projects in Payments" }),
    ).toBeInTheDocument();
    await expect(within(select).getAllByRole("option", { name: "prod" })).toHaveLength(2);
  },
};

/** A project in the team that is *not* selected is still selectable by id. */
export const PicksAProjectInAnotherTeam: Story = {
  args: { fetchStub: chain() },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const select = await canvas.findByLabelText("Where the role applies");
    await waitFor(() =>
      expect(within(select).getByRole("group", { name: "Projects in Payments" })).toBeInTheDocument(),
    );
    await userEvent.selectOptions(select, "project:project-3");
    await expect(canvas.getByTestId("picked")).toHaveTextContent("project:project-3");
  },
};

/** Two requests, and a control-sized placeholder until both have answered. */
export const Loading: Story = {
  args: { fetchStub: () => new Promise<Response>(() => {}) },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByLabelText("Loading…")).toBeVisible());
  },
};

/** An org with no teams can only be scoped to itself, and the picker says so. */
export const NoTeams: Story = {
  args: { fetchStub: chain({ teams: () => json([]) }) },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText(/no teams yet/)).toBeVisible());
    // the org itself is still offered: the picker degrades, it does not vanish
    await expect(canvas.getByLabelText("Where the role applies")).toBeVisible();
  },
};

/** The team list failing takes the narrower scopes away, not the org-wide one. */
export const TeamsFailed: Story = {
  args: { fetchStub: chain({ teams: () => json({ error: { message: "boom" } }, 500) }) },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByRole("alert")).toHaveTextContent(/teams and projects/),
    );
    await expect(canvas.getByLabelText("Where the role applies")).toBeVisible();
  },
};

/** The project list failing is the same story, one level down. */
export const ProjectsFailed: Story = {
  args: {
    fetchStub: chain({ projects: () => json({ error: { message: "boom" } }, 500) }),
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByRole("alert")).toHaveTextContent(/teams and projects/),
    );
    // the teams still loaded, so the picker keeps offering them
    await expect(
      within(canvas.getByLabelText("Where the role applies")).getByRole("group", {
        name: "Teams",
      }),
    ).toBeInTheDocument();
  },
};
