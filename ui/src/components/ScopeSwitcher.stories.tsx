import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { ScopeSwitcher } from "./ScopeSwitcher";
import { UxScreenProvider } from "@/lib/ux-react";
import {
  Harness,
  ORG,
  PROJECT,
  TEAM,
  cancelConfirmation,
  confirmDestructive,
  confirmation,
  expectNoUxEvent,
  expectSheetClosed,
  expectUxEvent,
  json,
  pending,
  recordUxEvents,
  recording,
  type FetchStub,
} from "@/pages/story-harness";

/**
 * The scope chain, answered directly rather than through `scoped()`.
 *
 * This component *is* what the shared helper stands in for on every other
 * screen, so a story that used it could never show an org with no teams — the
 * fixture would answer the component's own query.
 */
const chain =
  (
    over: {
      orgs?: () => Response | Promise<Response>;
      teams?: () => Response | Promise<Response>;
      projects?: () => Response | Promise<Response>;
    } = {},
  ): FetchStub =>
  async (input) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (path === "/api/v1/orgs") return (over.orgs ?? (() => json([ORG])))();
    if (/^\/api\/v1\/orgs\/[^/]+\/teams$/.test(path)) {
      return (over.teams ?? (() => json([TEAM])))();
    }
    if (/^\/api\/v1\/teams\/[^/]+\/projects$/.test(path)) {
      return (over.projects ?? (() => json([PROJECT])))();
    }
    return json({});
  };

/** the recorder the story under way installed, read back by its play function */
let calls: ReturnType<typeof recording>;

const meta = {
  title: "Components/ScopeSwitcher",
  component: ScopeSwitcher,
  parameters: { layout: "padded" },
} satisfies Meta<typeof ScopeSwitcher>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={chain()}>
      <ScopeSwitcher />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // each level is a named picker, not three anonymous dropdowns. a combobox
    // reads as the row's label; the id behind it is what goes on the wire
    await waitFor(() => expect(canvas.getByLabelText("Org")).toHaveValue(ORG.name));
    await expect(canvas.getByLabelText("Team")).toHaveValue(TEAM.name);
    await expect(canvas.getByLabelText("Project")).toHaveValue(PROJECT.name);
  },
};

/** Three sequential requests, so the in-flight state is worth its own story. */
export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <ScopeSwitcher />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("Loading scope…")).toBeVisible();
  },
};

/**
 * A fresh control plane with nothing in it. The lower levels are disabled
 * rather than empty-and-clickable, because a team cannot be created before the
 * org it would belong to.
 */
export const NoOrgYet: Story = {
  render: () => (
    <Harness fetchStub={chain({ orgs: () => json([]) })}>
      <ScopeSwitcher />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByLabelText("Org")).toBeDisabled());
    await expect(canvas.getByLabelText("Team")).toBeDisabled();
    await expect(canvas.getByText(/no org configured/)).toBeVisible();
    // the only offer that makes sense at this point
    await expect(canvas.getByRole("button", { name: "Add org" })).toBeVisible();
  },
};

/** An org with no team: the org level is usable, the two below it are not. */
export const NoTeamYet: Story = {
  render: () => (
    <Harness fetchStub={chain({ teams: () => json([]) })}>
      <ScopeSwitcher />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByLabelText("Org")).toBeEnabled());
    await expect(canvas.getByLabelText("Team")).toBeDisabled();
    await expect(canvas.getByText(/no team configured/)).toBeVisible();
  },
};

/**
 * The list failed rather than came back empty. The switcher says which level
 * broke, since "no org" and "orgs did not load" call for different actions.
 */
export const OrgsFailed: Story = {
  render: () => (
    <Harness fetchStub={chain({ orgs: () => json({ error: { message: "boom" } }, 500) })}>
      <ScopeSwitcher />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText(/failed to load orgs/)).toBeVisible());
  },
};

/** Creating a team posts under the org in scope. */
export const CreatesATeam: Story = {
  render: () => {
    const recorder = recording(chain());
    calls = recorder;
    return (
      <Harness fetchStub={recorder.stub}>
        <ScopeSwitcher />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Add team" }));
    const dialog = within(await confirmation());
    await expect(dialog.getByText("New team")).toBeVisible();
    await userEvent.type(dialog.getByLabelText("Name"), "Research");
    await userEvent.click(dialog.getByRole("button", { name: "Create" }));
    const body = await calls.expectSentBody("POST", `/orgs/${ORG.id}/teams`);
    await expect(body).toEqual({ name: "Research" });
  },
};

/**
 * Deleting names the thing first, through the shared `ConfirmDialog` (#1760):
 * the title names the org, and the body says it takes everything under it.
 */
export const DeleteNamesWhatItTakes: Story = {
  render: () => {
    const recorder = recording(chain());
    calls = recorder;
    return (
      <Harness fetchStub={recorder.stub}>
        <ScopeSwitcher />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Delete org" }));
    await expect(
      await within(await confirmation()).findByRole("heading", {
        name: `Delete org ${ORG.name}?`,
      }),
    ).toBeVisible();
    await confirmDestructive(/everything under it/i, "Delete org");
    await calls.expectSent("DELETE", `/orgs/${ORG.id}`);
    await expectSheetClosed();
  },
};

/**
 * Backing out of the confirmation sends nothing, and is filed as an abandon of
 * the level that was asked about.
 *
 * The app shell mounts the switcher in the user menu, outside any screen, and a
 * form rendered there is silent rather than mislabelled
 * (`docs/dev-docs/development/ux-telemetry.md`). The screen key here is only so
 * the story can read *which* form the cancel is filed under: the target row is
 * gone by the closing edge, and a key that followed it would name another level.
 */
export const DeleteCanBeCancelled: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    const recorder = recording(chain());
    calls = recorder;
    return (
      <Harness fetchStub={recorder.stub}>
        <UxScreenProvider screen="providers">
          <ScopeSwitcher />
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Delete team" }));
    await expect(
      await within(await confirmation()).findByRole("heading", {
        name: `Delete team ${TEAM.name}?`,
      }),
    ).toBeVisible();
    await cancelConfirmation();
    calls.expectNotSent("DELETE", `/teams/${TEAM.id}`);
    const abandon = await expectUxEvent("form_abandon", "team-delete");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "team-delete");
    expectNoUxEvent("form_abandon", "project-delete");
  },
};
