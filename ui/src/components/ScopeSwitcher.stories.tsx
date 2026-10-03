import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { CreateProjectHost, ScopeSwitcher } from "./ScopeSwitcher";
import { openCreateProject } from "@/lib/scope";
import { UxScreenProvider } from "@/lib/ux-react";
import {
  Harness,
  ORG,
  PROJECT,
  TEAM,
  cancelConfirmation,
  confirmDestructive,
  confirmation,
  expectAllowed,
  expectRefused,
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
  async (input, init) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (path === "/api/v1/orgs") return (over.orgs ?? (() => json([ORG])))();
    if (/^\/api\/v1\/orgs\/[^/]+\/teams$/.test(path)) {
      return (over.teams ?? (() => json([TEAM])))();
    }
    if (/^\/api\/v1\/teams\/[^/]+\/projects$/.test(path)) {
      return (over.projects ?? (() => json([PROJECT])))();
    }
    // the project's own settings echo what was saved, the way the server does
    if (/^\/api\/v1\/projects\/[^/]+\/settings$/.test(path)) {
      if (init?.method === "PUT") return json(JSON.parse(String(init.body)));
      return json({ payload_min_role: "member" });
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
 * Another screen opens the project dialog through `openCreateProject()`, with no
 * switcher mounted (#2611). That is the shell's real situation: the switcher
 * lives in the account menu and is unmounted whenever the menu is closed, so the
 * dialog belongs to `CreateProjectHost`, which the shell always mounts.
 */
export const OpensCreateProjectFromElsewhere: Story = {
  render: () => {
    const recorder = recording(chain());
    calls = recorder;
    return (
      <Harness fetchStub={recorder.stub}>
        <button type="button" onClick={openCreateProject}>
          Open from another screen
        </button>
        <CreateProjectHost />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the team's project list is only asked for once a team is in scope, which
    // is what the host needs before it has anything to open
    await calls.expectSent("GET", `/teams/${TEAM.id}/projects`);
    await userEvent.click(canvas.getByRole("button", { name: "Open from another screen" }));
    const dialog = within(await confirmation());
    await expect(dialog.getByText("New project")).toBeVisible();
    await userEvent.type(dialog.getByLabelText("Name"), "Search");
    await userEvent.click(dialog.getByRole("button", { name: "Create" }));
    const body = await calls.expectSentBody("POST", `/teams/${TEAM.id}/projects`);
    await expect(body).toEqual({ name: "Search" });
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

/**
 * #1820: a project admin opens the project's settings from its row and lets
 * the project's viewers read captured request and response bodies. Off is the
 * default, so the switch starts off and the save sends the lowered floor.
 */
export const ProjectSettingsLetViewersReadPayloads: Story = {
  render: () => {
    const recorder = recording(chain());
    calls = recorder;
    return (
      <Harness fetchStub={recorder.stub} role="admin">
        <ScopeSwitcher />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Project settings" }));
    const dialog = await confirmation();
    const toggle = await within(dialog).findByRole("switch", {
      name: "Viewers can read captured payloads",
    });
    await expect(toggle).not.toBeChecked();
    await expectAllowed(dialog, "Viewers can read captured payloads", "switch");
    await userEvent.click(toggle);
    const body = await calls.expectSentBody("PUT", `/projects/${PROJECT.id}/settings`);
    await expect(body).toEqual({ payload_min_role: "viewer" });
  },
};

/**
 * A viewer can open the dialog and see where the setting stands, but the
 * switch is refused up front and says the role it needs.
 */
export const ProjectSettingsAreAnAdminsToChange: Story = {
  render: () => (
    <Harness fetchStub={chain()} role="viewer">
      <ScopeSwitcher />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Project settings" }));
    const dialog = await confirmation();
    await within(dialog).findByRole("switch", { name: "Viewers can read captured payloads" });
    await expectRefused(dialog, "Viewers can read captured payloads", undefined, "switch");
  },
};

// the control plane refuses to delete a project, or a team holding one, while a
// provider or group is scoped to it, and says which (#1919). The refusal is the
// dialog's own error, so the confirmation stays open on it
const SCOPED_ROWS_REFUSAL = (what: string) =>
  json(
    {
      error: {
        message: `this ${what} still owns provider 'gateway-private', provider group 'gateway-fleet'; delete them or make them org-wide first, since deleting the project would otherwise widen their access or destroy them`,
      },
    },
    409,
  );

export const DeleteProjectRefusedWhileItOwnsScopedRows: Story = {
  render: () => {
    const recorder = recording(async (input, init) => {
      if (init?.method === "DELETE") return SCOPED_ROWS_REFUSAL("project");
      return chain()(input, init);
    });
    calls = recorder;
    return (
      <Harness fetchStub={recorder.stub}>
        <ScopeSwitcher />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Delete project" }));
    const dialog = within(await confirmation());
    await userEvent.click(await dialog.findByRole("button", { name: "Delete project" }));
    const alert = await dialog.findByRole("alert");
    await expect(alert).toHaveTextContent(/provider 'gateway-private', provider group/);
    await expect(alert).toHaveTextContent(/make them org-wide first/);
    await calls.expectSent("DELETE", `/projects/${PROJECT.id}`);
    // still there to be cancelled, not closed over the refusal
    await expect(await confirmation()).toBeVisible();
  },
};

export const DeleteTeamRefusedWhileItsProjectsOwnScopedRows: Story = {
  render: () => {
    const recorder = recording(async (input, init) => {
      if (init?.method === "DELETE") return SCOPED_ROWS_REFUSAL("team");
      return chain()(input, init);
    });
    calls = recorder;
    return (
      <Harness fetchStub={recorder.stub}>
        <ScopeSwitcher />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Delete team" }));
    const dialog = within(await confirmation());
    await userEvent.click(await dialog.findByRole("button", { name: "Delete team" }));
    await expect(await dialog.findByRole("alert")).toHaveTextContent(
      /this team still owns provider 'gateway-private'/,
    );
    await calls.expectSent("DELETE", `/teams/${TEAM.id}`);
  },
};
