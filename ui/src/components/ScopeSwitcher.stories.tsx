import type { Meta, StoryObj } from "@storybook/react-vite";
import type { ReactNode } from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { CreateProjectHost, ScopeSwitcher } from "./ScopeSwitcher";
import en from "@/lib/i18n/locales/en.json";
import { openCreateProject } from "@/lib/scope";
import { UxScreenProvider } from "@/lib/ux-react";
import {
  Harness,
  NEEDS_ADMIN,
  NEEDS_SUPERADMIN,
  ORG,
  PROJECT,
  TEAM,
  cancelConfirmation,
  confirmDestructive,
  confirmation,
  expectAllowed,
  expectNoUxEvent,
  expectRefused,
  expectSheetClosed,
  expectUxEvent,
  json,
  pickOption,
  recordUxEvents,
  recording,
  type FetchStub,
} from "@/pages/story-harness";
import { atTablet } from "@/lib/story-viewport";

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

/** a second team with a project of its own, for a story that picks across levels */
const RESEARCH = { ...TEAM, id: "team-2", name: "Research" };
const SEARCH = { ...PROJECT, id: "project-2", team_id: "team-2", name: "Search" };
const twoTeams = (): FetchStub => async (input, init) => {
  const path = new URL(String(input), "http://localhost").pathname;
  if (/^\/api\/v1\/orgs\/[^/]+\/teams$/.test(path)) return json([TEAM, RESEARCH]);
  if (path === `/api/v1/teams/${RESEARCH.id}/projects`) return json([SEARCH]);
  return chain()(input, init);
};

const PATH = `${ORG.name} / ${TEAM.name} / ${PROJECT.name}`;
const TRIGGER = `Scope: ${PATH}`;
const ROWS = en.scope.rows;
const ACTIONS = en.scope.actions;

/**
 * The rail the switcher sits in: its container is a `nav`, which is the edge a
 * folded popover stands against.
 */
function Rail({ folded = false, children }: { folded?: boolean; children: ReactNode }) {
  return (
    <nav
      aria-label="Rail"
      className={`flex flex-col px-2 py-3 ${folded ? "w-[52px]" : "w-[232px]"}`}
    >
      {children}
    </nav>
  );
}

const meta = {
  title: "Components/ScopeSwitcher",
  component: ScopeSwitcher,
  parameters: { layout: "padded" },
} satisfies Meta<typeof ScopeSwitcher>;

export default meta;
type Story = StoryObj<typeof meta>;

/** Open the popover from the trigger, once the scope has landed. */
async function openScope(canvasElement: HTMLElement, name = TRIGGER) {
  const trigger = await within(canvasElement).findByRole("button", { name });
  await userEvent.click(trigger);
  const popover = await within(canvasElement).findByRole("dialog", { name: en.shell.scope });
  return { trigger, popover };
}

/** Open a row's overflow menu and hand back the menu. */
async function openActions(popover: HTMLElement, row: string) {
  await userEvent.click(within(popover).getByRole("button", { name: row }));
  return within(popover).findByRole("menu", { name: row });
}

/** Choose one entry from a row's overflow menu. */
async function chooseAction(popover: HTMLElement, row: string, entry: string) {
  const menu = await openActions(popover, row);
  await userEvent.click(within(menu).getByRole("menuitem", { name: entry }));
}

/**
 * The rail shows the path it is on in mono, with the full path in its accessible
 * name; opening it gives three rows that are named, not three anonymous
 * dropdowns.
 */
export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={chain()}>
      <Rail>
        <ScopeSwitcher />
      </Rail>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const trigger = await within(canvasElement).findByRole("button", { name: TRIGGER });
    await expect(trigger).toHaveAttribute("title", PATH);
    await expect(trigger).toHaveAttribute("aria-expanded", "false");
    // the path is mono, one run per level, so a long name shortens on its own
    await expect(trigger.querySelector(".font-mono")).not.toBeNull();
    for (const level of [ORG.name, TEAM.name, PROJECT.name]) {
      await expect(within(trigger).getByText(level)).toBeVisible();
    }

    const { popover } = await openScope(canvasElement);
    await expect(trigger).toHaveAttribute("aria-expanded", "true");
    const rows = within(popover);
    // each level is a named picker. a combobox reads as the row's label; the id
    // behind it is what goes on the wire
    await expect(rows.getByLabelText(ROWS.org)).toHaveValue(ORG.name);
    await expect(rows.getByLabelText(ROWS.team)).toHaveValue(TEAM.name);
    await expect(rows.getByLabelText(ROWS.project)).toHaveValue(PROJECT.name);
    // the popover stands under the trigger, not inside it
    await expect(popover.getBoundingClientRect().top).toBeGreaterThanOrEqual(
      trigger.getBoundingClientRect().bottom,
    );
    // opening it puts the keyboard on the first picker
    await waitFor(() => expect(rows.getByLabelText(ROWS.org)).toHaveFocus());
  },
};

/**
 * Folded to the icon strip the trigger is a building: no text, the full path in
 * its name and its tooltip. The popover opens beside the rail rather than under
 * the icon, which would be 52px wide.
 */
export const Folded: Story = {
  ...atTablet,
  render: () => (
    <Harness fetchStub={chain()}>
      <Rail folded>
        <ScopeSwitcher folded />
      </Rail>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const trigger = await within(canvasElement).findByRole("button", { name: TRIGGER });
    await expect(trigger).toHaveAttribute("title", TRIGGER);
    await expect(within(canvasElement).queryByText(PATH)).toBeNull();

    const { popover } = await openScope(canvasElement);
    const rail = canvasElement.querySelector("nav") as HTMLElement;
    await expect(popover.getBoundingClientRect().left).toBeGreaterThanOrEqual(
      rail.getBoundingClientRect().right,
    );
    await expect(within(popover).getByLabelText(ROWS.team)).toHaveValue(TEAM.name);
  },
};

/**
 * Three sequential requests, so the in-flight state is worth its own story: the
 * orgs never answer here, so nothing below them is asked either.
 */
export const Loading: Story = {
  render: () => (
    <Harness fetchStub={chain({ orgs: () => new Promise<Response>(() => {}) })}>
      <Rail>
        <ScopeSwitcher />
      </Rail>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: en.scope.loading }));
    const popover = await canvas.findByRole("dialog", { name: en.shell.scope });
    // the popover holds a skeleton of its three rows rather than pickers with
    // nothing in them
    await expect(await within(popover).findByRole("status")).toBeVisible();
    await expect(within(popover).queryByRole("combobox")).toBeNull();
  },
};

/**
 * A fresh control plane with nothing in it. The lower levels are disabled
 * rather than empty-and-clickable, because a team cannot be created before the
 * org it would belong to, and each says why under its picker.
 */
export const NoOrgYet: Story = {
  render: () => (
    <Harness fetchStub={chain({ orgs: () => json([]) })}>
      <Rail>
        <ScopeSwitcher />
      </Rail>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const { popover } = await openScope(canvasElement, `Scope: ${en.scope.noOrg}`);
    const rows = within(popover);
    await waitFor(() => expect(rows.getByLabelText(ROWS.org)).toBeDisabled());
    await expect(rows.getByLabelText(ROWS.team)).toBeDisabled();
    await expect(rows.getByLabelText(ROWS.project)).toBeDisabled();
    await expect(rows.getByText(/no org configured/)).toBeVisible();
    await expect(rows.getByText(en.scope.needsOrg)).toBeVisible();
    await expect(rows.getByText(en.scope.needsTeam)).toBeVisible();
    // the only offer that makes sense at this point: the first organization
    await expect(rows.getByRole("button", { name: ACTIONS.team })).toBeDisabled();
    const menu = await openActions(popover, ACTIONS.org);
    await expect(within(menu).getByRole("menuitem", { name: en.scope.newOrg })).toBeVisible();
    await expect(
      within(menu).queryByRole("menuitem", { name: en.scope.menu.deleteOrg }),
    ).toBeNull();
  },
};

/** An org with no team: the org level is usable, the two below it are not. */
export const NoTeamYet: Story = {
  render: () => (
    <Harness fetchStub={chain({ teams: () => json([]) })}>
      <Rail>
        <ScopeSwitcher />
      </Rail>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const { popover } = await openScope(canvasElement, `Scope: ${ORG.name}`);
    const rows = within(popover);
    await waitFor(() => expect(rows.getByLabelText(ROWS.org)).toBeEnabled());
    await expect(rows.getByLabelText(ROWS.team)).toBeDisabled();
    await expect(rows.getByText(/no team configured/)).toBeVisible();
    // the team row still offers the way out: a first team
    await expect(rows.getByRole("button", { name: ACTIONS.team })).toBeEnabled();
  },
};

/**
 * The list failed rather than came back empty. The switcher says which level
 * broke, since "no org" and "orgs did not load" call for different actions.
 */
export const OrgsFailed: Story = {
  render: () => (
    <Harness fetchStub={chain({ orgs: () => json({ error: { message: "boom" } }, 500) })}>
      <Rail>
        <ScopeSwitcher />
      </Rail>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const { popover } = await openScope(canvasElement, `Scope: ${en.scope.noOrg}`);
    await waitFor(() => expect(within(popover).getByText(/failed to load orgs/)).toBeVisible());
  },
};

/**
 * Choosing a value keeps the popover open: the next level usually changes too,
 * and the trigger follows the pick.
 */
export const ChoosingAValueKeepsItOpen: Story = {
  render: () => (
    <Harness fetchStub={twoTeams()}>
      <Rail>
        <ScopeSwitcher />
      </Rail>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const { trigger, popover } = await openScope(canvasElement);
    const rows = within(popover);
    await pickOption(rows.getByLabelText(ROWS.team), RESEARCH.name);
    // the project row follows the team it belongs to, and the rail follows both
    await waitFor(() => expect(rows.getByLabelText(ROWS.project)).toHaveValue(SEARCH.name));
    await expect(rows.getByLabelText(ROWS.team)).toHaveValue(RESEARCH.name);
    await expect(popover).toBeVisible();
    await waitFor(() =>
      expect(trigger).toHaveAccessibleName(
        `Scope: ${ORG.name} / ${RESEARCH.name} / ${SEARCH.name}`,
      ),
    );
  },
};

/**
 * Escape closes the popover and hands focus back to the trigger; a press
 * outside closes it without taking focus. With a picker's list open, Escape
 * closes the list first and the popover on the second press.
 */
export const DismissesFromTheKeyboardAndThePointer: Story = {
  render: () => (
    <Harness fetchStub={chain()}>
      <Rail>
        <ScopeSwitcher />
      </Rail>
      <button type="button">Elsewhere</button>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const { trigger, popover } = await openScope(canvasElement);
    const org = within(popover).getByLabelText(ROWS.org);
    await waitFor(() => expect(org).toHaveFocus());

    await userEvent.keyboard("{ArrowDown}");
    await expect(org).toHaveAttribute("aria-expanded", "true");
    await userEvent.keyboard("{Escape}");
    await expect(org).toHaveAttribute("aria-expanded", "false");
    await expect(canvas.getByRole("dialog", { name: en.shell.scope })).toBeVisible();

    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(canvas.queryByRole("dialog", { name: en.shell.scope })).toBeNull());
    await waitFor(() => expect(trigger).toHaveFocus());
    await expect(trigger).toHaveAttribute("aria-expanded", "false");

    await userEvent.click(trigger);
    await canvas.findByRole("dialog", { name: en.shell.scope });
    await userEvent.click(canvas.getByRole("button", { name: "Elsewhere" }));
    await waitFor(() => expect(canvas.queryByRole("dialog", { name: en.shell.scope })).toBeNull());
  },
};

/**
 * The popover is a form, so Tab walks its pickers and buttons in reading order,
 * and leaving past the last one closes it.
 */
export const TabWalksTheRowsAndLeaves: Story = {
  render: () => (
    <Harness fetchStub={chain()}>
      <Rail>
        <ScopeSwitcher />
      </Rail>
      <button type="button">Elsewhere</button>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const trigger = await canvas.findByRole("button", { name: TRIGGER });
    trigger.focus();
    await userEvent.keyboard("{Enter}");
    const popover = within(await canvas.findByRole("dialog", { name: en.shell.scope }));
    await waitFor(() => expect(popover.getByLabelText(ROWS.org)).toHaveFocus());
    await userEvent.tab();
    await expect(popover.getByRole("button", { name: ACTIONS.org })).toHaveFocus();
    await userEvent.tab();
    await expect(popover.getByLabelText(ROWS.team)).toHaveFocus();
    await userEvent.tab();
    await userEvent.tab();
    await expect(popover.getByLabelText(ROWS.project)).toHaveFocus();
    await userEvent.tab();
    await expect(popover.getByRole("button", { name: ACTIONS.project })).toHaveFocus();
    await userEvent.tab();
    await expect(canvas.getByRole("button", { name: "Elsewhere" })).toHaveFocus();
    await waitFor(() => expect(canvas.queryByRole("dialog", { name: en.shell.scope })).toBeNull());
  },
};

/**
 * Each row has one overflow button holding what can be done to its level. It is
 * a real menu: it opens onto its first entry, the arrow keys walk it, Escape
 * closes only the menu and returns to the button, and the popover stays.
 */
export const RowMenuIsAMenu: Story = {
  render: () => (
    <Harness fetchStub={chain()}>
      <Rail>
        <ScopeSwitcher />
      </Rail>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const { popover } = await openScope(canvasElement);
    const button = within(popover).getByRole("button", { name: ACTIONS.project });
    await expect(button).toHaveAttribute("aria-haspopup", "menu");
    await expect(button).toHaveAttribute("aria-expanded", "false");

    await userEvent.click(button);
    const menu = within(await within(popover).findByRole("menu", { name: ACTIONS.project }));
    await expect(button).toHaveAttribute("aria-expanded", "true");
    const entries = menu.getAllByRole("menuitem").map((el) => el.textContent);
    await expect(entries).toEqual([
      en.scope.newProject,
      en.scope.projectSettings,
      en.scope.menu.deleteProject,
    ]);
    const [create, settings, remove] = menu.getAllByRole("menuitem");
    await waitFor(() => expect(create).toHaveFocus());

    await userEvent.keyboard("{ArrowDown}");
    await expect(settings).toHaveFocus();
    await userEvent.keyboard("{End}");
    await expect(remove).toHaveFocus();
    await userEvent.keyboard("{ArrowDown}");
    await expect(create).toHaveFocus();
    await userEvent.keyboard("{ArrowUp}");
    await expect(remove).toHaveFocus();
    await userEvent.keyboard("{Home}");
    await expect(create).toHaveFocus();

    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(within(popover).queryByRole("menu")).toBeNull());
    await waitFor(() => expect(button).toHaveFocus());
    // only the menu went: the popover it was opened from is still there
    await expect(canvas.getByRole("dialog", { name: en.shell.scope })).toBeVisible();

    // Tab leaves the menu for the control after its button, and closes it
    await userEvent.keyboard("{Enter}");
    await within(popover).findByRole("menu", { name: ACTIONS.project });
    await userEvent.tab();
    await waitFor(() => expect(within(popover).queryByRole("menu")).toBeNull());
  },
};

/** The menus differ by what the level can do: only a project has settings. */
export const RowMenusOfferWhatTheLevelCanDo: Story = {
  render: () => (
    <Harness fetchStub={chain()}>
      <Rail>
        <ScopeSwitcher />
      </Rail>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const { popover } = await openScope(canvasElement);
    const names = async (row: string) => {
      const menu = await openActions(popover, row);
      const found = within(menu)
        .getAllByRole("menuitem")
        .map((el) => el.textContent);
      await userEvent.keyboard("{Escape}");
      await waitFor(() => expect(within(popover).queryByRole("menu")).toBeNull());
      return found;
    };
    await expect(await names(ACTIONS.org)).toEqual([en.scope.newOrg, en.scope.menu.deleteOrg]);
    await expect(await names(ACTIONS.team)).toEqual([en.scope.newTeam, en.scope.menu.deleteTeam]);
    await expect(await names(ACTIONS.project)).toEqual([
      en.scope.newProject,
      en.scope.projectSettings,
      en.scope.menu.deleteProject,
    ]);
  },
};

/**
 * Creating from a row's menu closes the popover, raises the dialog, and posts
 * under the level above — a team under the org in scope.
 */
export const CreatesATeam: Story = {
  render: () => {
    const recorder = recording(chain());
    calls = recorder;
    return (
      <Harness fetchStub={recorder.stub}>
        <Rail>
          <ScopeSwitcher />
        </Rail>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const { popover } = await openScope(canvasElement);
    await chooseAction(popover, ACTIONS.team, en.scope.newTeam);
    const dialog = within(await confirmation());
    await expect(dialog.getByText(en.scope.newTeam)).toBeVisible();
    // the picker went with the pick: the dialog is the only thing open
    await expect(canvas.queryByRole("dialog", { name: en.shell.scope })).toBeNull();
    await userEvent.type(dialog.getByLabelText("Name"), "Research");
    await userEvent.click(dialog.getByRole("button", { name: "Create" }));
    const body = await calls.expectSentBody("POST", `/orgs/${ORG.id}/teams`);
    await expect(body).toEqual({ name: "Research" });
  },
};

/**
 * Another screen opens the project dialog through `openCreateProject()`, with no
 * switcher mounted (#2611). That is the shell's real situation below `md`, where
 * the rail is a drawer that is out of the document until it is opened, so the
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
        <Rail>
          <ScopeSwitcher />
        </Rail>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const { popover } = await openScope(canvasElement);
    await chooseAction(popover, ACTIONS.org, en.scope.menu.deleteOrg);
    await expect(
      await within(await confirmation()).findByRole("heading", {
        name: `Delete organization ${ORG.name}?`,
      }),
    ).toBeVisible();
    await confirmDestructive(/everything under it/i, en.scope.deleteOrg);
    await calls.expectSent("DELETE", `/orgs/${ORG.id}`);
    await expectSheetClosed();
  },
};

/**
 * Backing out of the confirmation sends nothing, is filed as an abandon of the
 * level that was asked about, and gives focus back to the trigger the dialog
 * was raised from.
 *
 * The screen key here is only so the story can read *which* form the cancel is
 * filed under: the target row is gone by the closing edge, and a key that
 * followed it would name another level.
 */
export const DeleteCanBeCancelled: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    const recorder = recording(chain());
    calls = recorder;
    return (
      <Harness fetchStub={recorder.stub}>
        <UxScreenProvider screen="providers">
          <Rail>
            <ScopeSwitcher />
          </Rail>
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const { trigger, popover } = await openScope(canvasElement);
    await chooseAction(popover, ACTIONS.team, en.scope.menu.deleteTeam);
    await expect(
      await within(await confirmation()).findByRole("heading", {
        name: `Delete team ${TEAM.name}?`,
      }),
    ).toBeVisible();
    await cancelConfirmation();
    calls.expectNotSent("DELETE", `/teams/${TEAM.id}`);
    await waitFor(() => expect(trigger).toHaveFocus());
    const abandon = await expectUxEvent("form_abandon", "team-delete");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "team-delete");
    expectNoUxEvent("form_abandon", "project-delete");
  },
};

/**
 * #1820: a project admin opens the project's settings from its row's menu and
 * lets the project's viewers read captured request and response bodies. Off is
 * the default, so the switch starts off and the save sends the lowered floor.
 */
export const ProjectSettingsLetViewersReadPayloads: Story = {
  render: () => {
    const recorder = recording(chain());
    calls = recorder;
    return (
      <Harness fetchStub={recorder.stub} role="admin">
        <Rail>
          <ScopeSwitcher />
        </Rail>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const { popover } = await openScope(canvasElement);
    await chooseAction(popover, ACTIONS.project, en.scope.projectSettings);
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
      <Rail>
        <ScopeSwitcher />
      </Rail>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const { popover } = await openScope(canvasElement);
    await openActions(popover, ACTIONS.project);
    // looking at the setting is not an admin's alone, so its entry is not refused
    await expectAllowed(popover, en.scope.projectSettings, "menuitem");
    await userEvent.click(
      within(popover).getByRole("menuitem", { name: en.scope.projectSettings }),
    );
    const dialog = await confirmation();
    await within(dialog).findByRole("switch", { name: "Viewers can read captured payloads" });
    await expectRefused(dialog, "Viewers can read captured payloads", undefined, "switch");
  },
};

/**
 * Creating and deleting are refused up front for a role that cannot do them,
 * with the role named, instead of after the click as the server's 403. An org
 * admin may delete the org but not create another (that is a superadmin's), and
 * a viewer may do neither at any level.
 */
export const RowMenuEntriesAreGated: Story = {
  render: () => (
    <Harness fetchStub={chain()} role="admin">
      <Rail>
        <ScopeSwitcher />
      </Rail>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const { popover } = await openScope(canvasElement);
    await openActions(popover, ACTIONS.org);
    await expectRefused(popover, en.scope.newOrg, NEEDS_SUPERADMIN, "menuitem");
    await expectAllowed(popover, en.scope.menu.deleteOrg, "menuitem");
    // the reason is on screen, not only in a tooltip a keyboard cannot reach
    await expect(
      within(popover).getByRole("menuitem", { name: en.scope.newOrg }).textContent,
    ).toContain(NEEDS_SUPERADMIN);
    await userEvent.keyboard("{Escape}");
    await openActions(popover, ACTIONS.team);
    await expectAllowed(popover, en.scope.newTeam, "menuitem");
    await expectAllowed(popover, en.scope.menu.deleteTeam, "menuitem");
  },
};

export const RowMenuEntriesAreRefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={chain()} role="viewer">
      <Rail>
        <ScopeSwitcher />
      </Rail>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const { popover } = await openScope(canvasElement);
    await openActions(popover, ACTIONS.project);
    await expectRefused(popover, en.scope.newProject, NEEDS_ADMIN, "menuitem");
    await expectRefused(popover, en.scope.menu.deleteProject, NEEDS_ADMIN, "menuitem");
    await userEvent.keyboard("{Escape}");
    await openActions(popover, ACTIONS.team);
    await expectRefused(popover, en.scope.newTeam, NEEDS_ADMIN, "menuitem");
    await expectRefused(popover, en.scope.menu.deleteTeam, NEEDS_ADMIN, "menuitem");
  },
};

export const RowMenuEntriesAreAllowedToASuperadmin: Story = {
  render: () => (
    <Harness fetchStub={chain()} role="superadmin">
      <Rail>
        <ScopeSwitcher />
      </Rail>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const { popover } = await openScope(canvasElement);
    await openActions(popover, ACTIONS.org);
    await expectAllowed(popover, en.scope.newOrg, "menuitem");
    await expectAllowed(popover, en.scope.menu.deleteOrg, "menuitem");
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
        <Rail>
          <ScopeSwitcher />
        </Rail>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const { popover } = await openScope(canvasElement);
    await chooseAction(popover, ACTIONS.project, en.scope.menu.deleteProject);
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
        <Rail>
          <ScopeSwitcher />
        </Rail>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const { popover } = await openScope(canvasElement);
    await chooseAction(popover, ACTIONS.team, en.scope.menu.deleteTeam);
    const dialog = within(await confirmation());
    await userEvent.click(await dialog.findByRole("button", { name: "Delete team" }));
    await expect(await dialog.findByRole("alert")).toHaveTextContent(
      /this team still owns provider 'gateway-private'/,
    );
    await calls.expectSent("DELETE", `/teams/${TEAM.id}`);
  },
};
