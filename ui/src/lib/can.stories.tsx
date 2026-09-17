import type { Meta, StoryObj } from "@storybook/react";
import { expect, waitFor, within } from "storybook/test";

import { GatedButton } from "@/components/GatedButton";
import { Harness, NEEDS_ADMIN, json, type FetchStub } from "@/pages/story-harness";

// `CapabilityProvider` decides whether every gated control in the dashboard is
// disabled, and it only asks once the org/team/project chain has settled. What
// "settled" means is the whole of #1623: a deployment whose org has no team
// yet, or whose team has no project, has a chain that is *finished* rather than
// in flight, and a provider that waits for it anyway never asks at all — so
// every gated control renders enabled for every role, which is the click and
// the 403 afterwards that #1183 set out to remove.
//
// Asserted here rather than on a screen because it is the same bug on all of
// them, and on the screen where it was found (Teams) the empty list under test
// *is* the scope's own.

const ORG = { id: "org-1", name: "Rolter", slug: "rolter", created_at: "2026-01-01T00:00:00Z" };
const TEAM = { id: "team-1", org_id: "org-1", name: "Platform", created_at: "2026-01-01T00:00:00Z" };

/** a chain that stops at `depth`: the org exists, the rest of it does not */
function chain(depth: "org" | "team"): FetchStub {
  return async (input) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (path === "/api/v1/orgs") return json([ORG]);
    if (/\/teams$/.test(path)) return json(depth === "org" ? [] : [TEAM]);
    if (/\/projects$/.test(path)) return json([]);
    return json([]);
  };
}

function Gated() {
  return <GatedButton gate="team:create">New team</GatedButton>;
}

const meta = {
  title: "Behaviour/CapabilityGate",
  component: Gated,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Gated>;

export default meta;
type Story = StoryObj<typeof meta>;

const expectRefusal = async (canvasElement: HTMLElement) => {
  const canvas = within(canvasElement);
  await waitFor(() => expect(canvas.getByRole("button", { name: "New team" })).toBeDisabled());
  await expect(canvas.getByRole("button", { name: "New team" })).toHaveAttribute(
    "title",
    NEEDS_ADMIN,
  );
};

/**
 * An org with no teams — a fresh deployment, or any org created through the
 * dashboard before its first team. The projects query never runs, which is an
 * answer, and the gate has to resolve on it.
 */
export const ResolvesInAnOrgWithNoTeams: Story = {
  render: () => (
    <Harness fetchStub={chain("org")} role="member">
      <Gated />
    </Harness>
  ),
  play: async ({ canvasElement }) => expectRefusal(canvasElement),
};

/** and the same one level down: a team whose first project has not been made */
export const ResolvesInATeamWithNoProjects: Story = {
  render: () => (
    <Harness fetchStub={chain("team")} role="member">
      <Gated />
    </Harness>
  ),
  play: async ({ canvasElement }) => expectRefusal(canvasElement),
};

/** the ordinary case, which was never broken */
export const ResolvesInAFullyPopulatedChain: Story = {
  render: () => (
    <Harness
      fetchStub={async (input) => {
        const path = new URL(String(input), "http://localhost").pathname;
        if (path === "/api/v1/orgs") return json([ORG]);
        if (/\/teams$/.test(path)) return json([TEAM]);
        if (/\/projects$/.test(path))
          return json([
            { id: "project-1", team_id: "team-1", name: "Gateway", created_at: "2026-01-01T00:00:00Z" },
          ]);
        return json([]);
      }}
      role="member"
    >
      <Gated />
    </Harness>
  ),
  play: async ({ canvasElement }) => expectRefusal(canvasElement),
};
