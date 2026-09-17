import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { useScope } from "@/lib/scope";
import { Harness, type FetchStub, json } from "@/pages/story-harness";

// `useScope` resolves the org/team/project chain every scoped screen loads
// under, and three of its rules are invisible from any single screen: a pick
// reaches hooks that are already mounted, a stored id that no longer exists
// heals itself, and switching org drops the team and project underneath it.
// There is no DOM under `bun test` — the unit suite is pure — so the hook is
// asserted here, where a real browser runs it, the way `lib/auth.stories.tsx`
// asserts the session provider.

const STORAGE_KEY = "rolter.scope";

const org = (id: string, name: string) => ({
  id,
  name,
  slug: id,
  created_at: "2026-01-01T00:00:00Z",
});
const team = (id: string, org_id: string, name: string) => ({
  id,
  org_id,
  name,
  created_at: "2026-01-01T00:00:00Z",
});
const project = (id: string, team_id: string, name: string) => ({
  id,
  team_id,
  name,
  created_at: "2026-01-01T00:00:00Z",
});

const ORGS = [org("org-1", "Acme"), org("org-2", "Globex")];
const TEAMS: Record<string, ReturnType<typeof team>[]> = {
  "org-1": [team("team-1", "org-1", "Platform"), team("team-2", "org-1", "Research")],
  "org-2": [team("team-9", "org-2", "Payments")],
};
const PROJECTS: Record<string, ReturnType<typeof project>[]> = {
  "team-1": [project("project-1", "team-1", "Gateway"), project("project-2", "team-1", "Batch")],
  "team-2": [project("project-3", "team-2", "Evals")],
  "team-9": [project("project-9", "team-9", "Checkout")],
};

/** the whole chain, answered from the tables above */
const chain: FetchStub = async (input) => {
  const path = new URL(String(input), "http://localhost").pathname;
  if (path === "/api/v1/orgs") return json(ORGS);
  const teams = /^\/api\/v1\/orgs\/([^/]+)\/teams$/.exec(path);
  if (teams) return json(TEAMS[teams[1]] ?? []);
  const projects = /^\/api\/v1\/teams\/([^/]+)\/projects$/.exec(path);
  if (projects) return json(PROJECTS[projects[1]] ?? []);
  return json([]);
};

/** an org list that fails, so the hook has to name the reason */
const orgsFail: FetchStub = async (input) => {
  const path = new URL(String(input), "http://localhost").pathname;
  if (path === "/api/v1/orgs") return json({ error: "boom" }, 500);
  return json([]);
};

/**
 * Seed the persisted scope from inside the harness.
 *
 * `Harness` clears the key while it renders, so a story that wants a stored
 * pick has to write it after that and before `useScope` first reads it —
 * during this child's render, which is exactly in between.
 */
function Stored({ scope, children }: { scope: unknown; children: React.ReactNode }) {
  React.useState(() => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(scope));
    return null;
  });
  React.useEffect(() => () => localStorage.removeItem(STORAGE_KEY), []);
  return <>{children}</>;
}

/** what the hook resolved, plus the two buttons a switcher would offer */
function ScopeProbe({ name = "probe" }: { name?: string }) {
  const scope = useScope();
  return (
    <dl className="grid grid-cols-[8rem_1fr] gap-1 font-mono text-sm">
      <dt>org</dt>
      <dd data-testid={`${name}-org`}>{scope.orgId ?? "—"}</dd>
      <dt>team</dt>
      <dd data-testid={`${name}-team`}>{scope.teamId ?? "—"}</dd>
      <dt>project</dt>
      <dd data-testid={`${name}-project`}>{scope.projectId ?? "—"}</dd>
      <dt>error</dt>
      <dd data-testid={`${name}-error`}>{scope.errorKey ?? "—"}</dd>
      <dd className="col-span-2 flex gap-2">
        <button type="button" onClick={() => scope.setOrgId("org-2")}>
          {`${name}: pick Globex`}
        </button>
        <button type="button" onClick={() => scope.setProjectId("project-2")}>
          {`${name}: pick Batch`}
        </button>
      </dd>
    </dl>
  );
}

const meta = {
  title: "Session/Scope",
  component: ScopeProbe,
  parameters: { layout: "padded" },
} satisfies Meta<typeof ScopeProbe>;

export default meta;
type Story = StoryObj<typeof meta>;

/** read back what a probe resolved, once the chain has settled */
const settled = async (
  canvasElement: HTMLElement,
  name: string,
  expected: [string, string, string],
) => {
  const canvas = within(canvasElement);
  await waitFor(async () => {
    await expect(canvas.getByTestId(`${name}-org`)).toHaveTextContent(expected[0]);
    await expect(canvas.getByTestId(`${name}-team`)).toHaveTextContent(expected[1]);
    await expect(canvas.getByTestId(`${name}-project`)).toHaveTextContent(expected[2]);
  });
};

/** nothing stored: the first of each list, so a fresh browser lands somewhere valid */
export const DefaultsToTheFirstOfEachList: Story = {
  render: () => (
    <Harness fetchStub={chain}>
      <ScopeProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await settled(canvasElement, "probe", ["org-1", "team-1", "project-1"]);
    await expect(within(canvasElement).getByTestId("probe-error")).toHaveTextContent("—");
  },
};

/** a stored pick that still exists wins over the list order */
export const PrefersTheStoredPick: Story = {
  render: () => (
    <Harness fetchStub={chain}>
      <Stored scope={{ orgId: "org-1", teamId: "team-2", projectId: "project-3" }}>
        <ScopeProbe />
      </Stored>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await settled(canvasElement, "probe", ["org-1", "team-2", "project-3"]);
  },
};

/**
 * A stored id the control plane no longer returns — the org was deleted from
 * another session — falls back instead of pinning the dashboard to a scope
 * that resolves nothing.
 */
export const SelfHealsAStaleStoredPick: Story = {
  render: () => (
    <Harness fetchStub={chain}>
      <Stored scope={{ orgId: "org-gone", teamId: "team-gone", projectId: "project-gone" }}>
        <ScopeProbe />
      </Stored>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await settled(canvasElement, "probe", ["org-1", "team-1", "project-1"]);
  },
};

/**
 * A pick made in the switcher has to reach the hooks that are already mounted —
 * the shell, and the capability query that re-asks when the org changes
 * (#1183). Before the broadcast they kept the previous scope until they
 * remounted.
 */
export const BroadcastsAPickToEveryMountedHook: Story = {
  render: () => (
    <Harness fetchStub={chain}>
      <div className="flex gap-8">
        <ScopeProbe name="switcher" />
        <ScopeProbe name="shell" />
      </div>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await settled(canvasElement, "shell", ["org-1", "team-1", "project-1"]);
    await userEvent.click(canvas.getByRole("button", { name: "switcher: pick Globex" }));
    await settled(canvasElement, "shell", ["org-2", "team-9", "project-9"]);
    await settled(canvasElement, "switcher", ["org-2", "team-9", "project-9"]);
  },
};

/** switching org drops the team and project under it rather than carrying a mismatched pair */
export const SwitchingOrgResetsWhatIsUnderIt: Story = {
  render: () => (
    <Harness fetchStub={chain}>
      <Stored scope={{ orgId: "org-1", teamId: "team-2", projectId: "project-3" }}>
        <ScopeProbe />
      </Stored>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await settled(canvasElement, "probe", ["org-1", "team-2", "project-3"]);
    await userEvent.click(canvas.getByRole("button", { name: "probe: pick Globex" }));
    await settled(canvasElement, "probe", ["org-2", "team-9", "project-9"]);
    await expect(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}")).toEqual({ orgId: "org-2" });
  },
};

/** a project pick persists the whole chain, so a reload lands back on it */
export const PersistsThePickForTheNextLoad: Story = {
  render: () => (
    <Harness fetchStub={chain}>
      <ScopeProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await settled(canvasElement, "probe", ["org-1", "team-1", "project-1"]);
    await userEvent.click(canvas.getByRole("button", { name: "probe: pick Batch" }));
    await settled(canvasElement, "probe", ["org-1", "team-1", "project-2"]);
    await expect(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}")).toEqual({
      orgId: "org-1",
      teamId: "team-1",
      projectId: "project-2",
    });
  },
};

/**
 * The hook is called from places that have no `t` of its own, so it names the
 * catalog key and leaves the wording to the caller.
 */
export const NamesTheCatalogKeyWhenTheChainCannotResolve: Story = {
  render: () => (
    <Harness fetchStub={orgsFail}>
      <ScopeProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(async () =>
      expect(canvas.getByTestId("probe-error")).toHaveTextContent("scope.errors.orgsFailed"),
    );
    await expect(canvas.getByTestId("probe-org")).toHaveTextContent("—");
  },
};

/** an account with no org at all is a different message from a failed request */
export const SaysWhenThereIsNoOrgAtAll: Story = {
  render: () => (
    <Harness fetchStub={async () => json([])}>
      <ScopeProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(async () =>
      expect(canvas.getByTestId("probe-error")).toHaveTextContent("scope.errors.noOrg"),
    );
  },
};
