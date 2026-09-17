import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Teams from "./Teams";
import {
  Harness,
  ORG,
  Toasted,
  clickWhenEnabled,
  expectEmptyState,
  expectLoadError,
  expectSkeleton,
  expectToast,
  json,
  sheet,
  type FetchStub,
} from "./story-harness";
import type { TeamRow } from "@/lib/api";

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
    await waitFor(() =>
      expect(within(document.body).getByRole("dialog")).toBeInTheDocument(),
    );
    await expect(within(form).getByLabelText("Team name")).toHaveValue("Platform");
  },
};
