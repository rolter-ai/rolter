import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import App from "./App";
import {
  AppShell,
  EXPERIMENTAL_SUBSYSTEM,
  shellStub,
  shellStubWithStability,
} from "./pages/shell-harness";
import {
  ORG,
  PROJECT,
  TEAM,
  confirmation,
  expectForbidden,
  json,
  pickOption,
  recording,
  withCapabilities,
  type FetchStub,
} from "./pages/story-harness";
import type { InvocationRow, ProviderRow, SubsystemStability, VirtualKeyRow } from "@/lib/api";
import { DEFAULT_LOCALE, LOCALE_NAMES, setLocale } from "@/lib/i18n";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { SHORTCUTS, chordText, shortcutChord } from "@/lib/shortcuts";
import { withPageA11y } from "@/lib/story-a11y";
import {
  atMobile,
  atSplit,
  atTablet,
  expectInFrame,
  expectInViewport,
  expectNoHorizontalOverflow,
  expectNotTruncated,
} from "@/lib/story-viewport";

// The assembled shell (#1239): rail + header + screen, signed in.
//
// Every other story renders one screen or one component. The three pieces #959
// re-shaped only meet here — the drawer's open state is owned by `App`, its
// trigger lives in `ScreenHeader`, and the route change that dismisses it is a
// `useEffect` on `location.pathname` — so nothing asserted they work together
// until these stories did.
//
// Copy is read out of the catalog rather than written out again: a story
// asserting "Dashboard" would keep passing after the entry was reworded, and
// start failing in whatever language the toolbar was left on.

const NAV_LABEL = en.shell.navLabel;
const OPEN_NAV = en.shell.openNav;
const nav = en.nav as Record<string, string>;
const screens = en.screens as Record<string, { title: string }>;
const shortcuts = en.shell.shortcuts;
// the rail footer's entry names itself with the chord it stands in for, so the
// story interpolates the catalog rather than writing "Keyboard shortcuts (?)"
const SHORTCUTS_FOOTER = shortcuts.open.replace("{{chord}}", chordText(shortcutChord("help")));

// this is the one story file that mounts the whole page, so it is where the
// three page-level axe rules the runner defaults off are actually gated —
// at all three widths, since the rail changes shape at each of them (#1353)
const meta = {
  title: "Shell/App",
  component: App,
  parameters: { layout: "fullscreen", ...withPageA11y },
} satisfies Meta<typeof App>;
export default meta;
type Story = StoryObj<typeof meta>;

/** The rail once the session, the capabilities and the scope have all landed. */
async function railOf(canvasElement: HTMLElement, label = NAV_LABEL): Promise<HTMLElement> {
  return within(canvasElement).findByRole("navigation", { name: label });
}

/**
 * At `lg` and up the rail is on screen at full width with its splitter, and
 * the route the shell booted at is the entry marked current.
 */
export const Desktop: Story = {
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const rail = await railOf(canvasElement);

    // labels, not icons: this is the full rail, not the tablet strip
    await expect(within(rail).getByText("rolter")).toBeVisible();
    await expect(within(rail).getByRole("button", { name: nav.playground })).toBeVisible();
    // the splitter belongs to this width and only this width
    await expect(
      within(rail).getByRole("separator", { name: en.shell.resizeSidebar }),
    ).toBeInTheDocument();
    // the shell knows which route it is on, and the screen agrees
    await expect(within(rail).getByRole("button", { name: nav.dashboard })).toHaveAttribute(
      "aria-current",
      "page",
    );
    await expect(
      canvas.getByRole("heading", { level: 1, name: screens.dashboard.title }),
    ).toBeVisible();
    // the header's drawer trigger is `md:hidden`, so above this breakpoint it
    // is out of the accessibility tree entirely — there is nothing to reach a
    // rail that is already on screen
    await expect(canvas.queryByRole("button", { name: OPEN_NAV })).toBeNull();
  },
};

/** `/auth/me` as `user` holding `memberships`, for the shell stub's first route */
const me = (
  user: { is_superadmin: boolean; display_name?: string | null },
  memberships: Record<string, unknown>[] = [],
): [string, () => unknown] => [
  "/api/v1/auth/me",
  () => ({
    user: {
      id: "user-1",
      email: "anya@acme.co",
      display_name: null,
      bio: null,
      created_at: "2026-01-01T00:00:00Z",
      ...user,
    },
    memberships: memberships.map((m) => ({
      id: "membership-1",
      user_id: "user-1",
      source: "manual",
      created_at: "2026-01-01T00:00:00Z",
      ...m,
    })),
    display_name_managed: false,
  }),
];

const menuLabel = en.shell.userMenuLabel;

/** Open the account card's menu from the rail, wherever the rail is. */
async function openAccountMenu(rail: HTMLElement, name: string | RegExp) {
  const card = await within(rail).findByRole("button", { name });
  await userEvent.click(card);
  const menu = await within(rail).findByRole("menu", { name: menuLabel });
  return { card, menu };
}

/**
 * The rail's account block goes by the display name when the account has one and
 * by the email when it has not; the email is never lost, it moves under the name
 * in the account menu (#2434). The menu is about the person alone (#2805): who
 * they are, the role they hold and where, their own two screens, and the way out.
 */
export const AccountMenuShowsTheDisplayName: Story = {
  render: () => (
    <AppShell fetchStub={shellStub([me({ is_superadmin: true, display_name: "Anya Petrova" })])} />
  ),
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    const account = await within(rail).findByText("Anya Petrova");
    await expect(account).toBeVisible();
    await expect(within(rail).queryByText("anya@acme.co")).toBeNull();
    // the card says the role in a word; the level belongs to the menu
    await expect(within(rail).getByText(en.shell.superadmin)).toBeVisible();
    const { menu } = await openAccountMenu(rail, /Anya Petrova/);
    // the menu names the person and keeps the address as the second line
    const panel = menu.parentElement!;
    await expect(within(panel).getByText("Anya Petrova")).toBeVisible();
    await expect(within(panel).getByText("anya@acme.co")).toBeVisible();
    await expect(within(panel).getByText(en.shell.roleLine.superadmin)).toBeVisible();
  },
};

/** With no display name the rail falls back to the email, as it always did. */
export const AccountMenuFallsBackToTheEmail: Story = {
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    await expect(await within(rail).findByText("anya@acme.co")).toBeVisible();
  },
};

/**
 * What the menu holds, and what it no longer does: the entries are the person's
 * own screens and Sign out, in that order, named the way the rail names them —
 * and no scope pickers, no creating or deleting an org beside Sign out.
 */
export const AccountMenuHoldsThePersonOnly: Story = {
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    // the rail's own leaves, found through its search box, which opens a group
    const search = within(rail).getByRole("textbox", { name: en.shell.searchNav });
    await userEvent.type(search, "account");
    await expect(within(rail).getByRole("button", { name: nav["api-keys"] })).toBeVisible();
    await userEvent.clear(search);
    await userEvent.type(search, "preferences");
    await expect(within(rail).getByRole("button", { name: nav.preferences })).toBeVisible();
    await userEvent.clear(search);

    const { menu } = await openAccountMenu(rail, /anya@acme.co/);
    const entries = within(menu)
      .getAllByRole("menuitem")
      .map((el) => el.textContent);
    await expect(entries).toEqual([nav["api-keys"], nav.preferences, en.shell.signOut]);
    await expect(within(menu.parentElement!).queryByRole("combobox")).toBeNull();
    await expect(
      within(rail).queryByRole("menuitem", { name: en.scope.menu.deleteOrg }),
    ).toBeNull();
  },
};

/** Each entry goes where its name says, and takes the menu with it. */
export const AccountMenuEntriesNavigate: Story = {
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const rail = await railOf(canvasElement);
    const { card, menu } = await openAccountMenu(rail, /anya@acme.co/);
    await userEvent.click(within(menu).getByRole("menuitem", { name: nav["api-keys"] }));
    await canvas.findByRole("heading", { level: 1, name: screens["api-keys"].title });
    await expect(within(rail).getByRole("button", { name: nav["api-keys"] })).toHaveAttribute(
      "aria-current",
      "page",
    );
    await waitFor(() => expect(within(rail).queryByRole("menu")).toBeNull());
    await waitFor(() => expect(card).toHaveFocus());

    await userEvent.click(card);
    await userEvent.click(await within(rail).findByRole("menuitem", { name: nav.preferences }));
    await canvas.findByRole("heading", { level: 1, name: screens.preferences.title });
  },
};

/**
 * The role line names the level the role applies at, read off the membership
 * that reaches the scope in view (#2805): an org grant, a team grant and a
 * project grant each say so, with the name of the thing they were made on.
 */
export const RoleLineNamesAnOrgGrant: Story = {
  render: () => (
    <AppShell
      fetchStub={shellStub([me({ is_superadmin: false }, [{ org_id: ORG.id, role: "admin" }])])}
    />
  ),
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    const { menu } = await openAccountMenu(rail, /anya@acme.co/);
    await expect(within(menu.parentElement!).getByText(`Admin · org ${ORG.name}`)).toBeVisible();
  },
};

export const RoleLineNamesATeamGrant: Story = {
  render: () => (
    <AppShell
      fetchStub={shellStub([
        me({ is_superadmin: false }, [{ org_id: ORG.id, team_id: TEAM.id, role: "member" }]),
      ])}
    />
  ),
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    const { menu } = await openAccountMenu(rail, /anya@acme.co/);
    await expect(within(menu.parentElement!).getByText(`Member · team ${TEAM.name}`)).toBeVisible();
  },
};

export const RoleLineNamesAProjectGrant: Story = {
  render: () => (
    <AppShell
      fetchStub={shellStub([
        me({ is_superadmin: false }, [
          {
            project_id: PROJECT.id,
            scope_org_id: ORG.id,
            scope_team_id: TEAM.id,
            role: "viewer",
          },
        ]),
      ])}
    />
  ),
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    const { menu } = await openAccountMenu(rail, /anya@acme.co/);
    await expect(
      within(menu.parentElement!).getByText(`Viewer · project ${PROJECT.name}`),
    ).toBeVisible();
  },
};

/**
 * A superadmin holds no membership at all and is still a superadmin
 * everywhere: the line says the whole deployment, and the scope path still
 * renders for the rest of the dashboard to read.
 */
export const SuperadminWithNoMembership: Story = {
  render: () => <AppShell fetchStub={shellStub([me({ is_superadmin: true }, [])])} />,
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    const { menu } = await openAccountMenu(rail, /anya@acme.co/);
    await expect(within(menu.parentElement!).getByText(en.shell.roleLine.superadmin)).toBeVisible();
    await expect(
      await within(rail).findByRole("button", {
        name: en.scope.trigger.replace("{{path}}", `${ORG.name} / ${TEAM.name} / ${PROJECT.name}`),
      }),
    ).toBeVisible();
  },
};

/**
 * The scope switcher is the rail's header, under the brand and above the search
 * box (#2805), on its own and not behind the account card: what the whole
 * dashboard shows is not a property of who is signed in. It shows the path in
 * view, and the popover changes it.
 */
export const ScopeSwitcherSitsUnderTheBrand: Story = {
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    const path = `${ORG.name} / ${TEAM.name} / ${PROJECT.name}`;
    const trigger = await within(rail).findByRole("button", {
      name: en.scope.trigger.replace("{{path}}", path),
    });
    const brand = within(rail).getByText("rolter");
    const search = within(rail).getByRole("textbox", { name: en.shell.searchNav });
    const follows = (a: Element, b: Element) =>
      a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING;
    await expect(follows(brand, trigger)).toBeTruthy();
    await expect(follows(trigger, search)).toBeTruthy();

    await userEvent.click(trigger);
    const popover = within(await within(rail).findByRole("dialog", { name: en.shell.scope }));
    await expect(popover.getByLabelText(en.scope.rows.org)).toHaveValue(ORG.name);
    await expect(popover.getByLabelText(en.scope.rows.team)).toHaveValue(TEAM.name);
    await expect(popover.getByLabelText(en.scope.rows.project)).toHaveValue(PROJECT.name);
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(trigger).toHaveFocus());
  },
};

/** A pick in the popover reaches the shell's own copy of the scope, not just its own. */
export const ScopePopoverChangesTheScope: Story = {
  render: () => (
    <AppShell
      fetchStub={async (input, init) => {
        const path = new URL(String(input), "http://localhost").pathname;
        if (/^\/api\/v1\/orgs\/[^/]+\/teams$/.test(path)) {
          return json([TEAM, { ...TEAM, id: "team-2", name: "Research" }]);
        }
        if (path === "/api/v1/teams/team-2/projects") {
          return json([{ ...PROJECT, id: "project-2", team_id: "team-2", name: "Search" }]);
        }
        return shellStub()(input, init);
      }}
    />
  ),
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    await userEvent.click(
      await within(rail).findByRole("button", {
        name: en.scope.trigger.replace("{{path}}", `${ORG.name} / ${TEAM.name} / ${PROJECT.name}`),
      }),
    );
    const popover = within(await within(rail).findByRole("dialog", { name: en.shell.scope }));
    await pickOption(popover.getByLabelText(en.scope.rows.team), "Research");
    await waitFor(() =>
      expect(
        within(rail).getByRole("button", {
          name: en.scope.trigger.replace("{{path}}", `${ORG.name} / Research / Search`),
        }),
      ).toBeVisible(),
    );
    // still open: the project under the new team is usually the next pick
    await expect(popover.getByLabelText(en.scope.rows.project)).toHaveValue("Search");
  },
};

/**
 * The landing screen's reference render is one day of traffic, not figures
 * over empty charts. The shell's stub answered the summary and nothing else, so
 * the tiles said 132 requests beside a spend chart, a donut, bars and a request
 * log that said there was nothing (#1994), and the latency tile read "0 ms"
 * from a summary with no average.
 */
export const TheLandingScreenHoldsOneDayOfTraffic: Story = {
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const figures = await canvas.findByTestId("dashboard-figures");
    await expect(await within(figures).findByText("132")).toBeVisible();
    await expect(figures).toHaveTextContent(/215\s*ms/);
    await expect(
      await canvas.findByRole("img", {
        name: en.pages.dashboard.spendChartAria.replace("{{window}}", en.common.timeWindow.last24h),
      }),
    ).toBeVisible();
    await expect(
      await within(canvas.getByTestId("dashboard-by-model")).findByText("gpt-4o"),
    ).toBeVisible();
    await expect(
      await within(canvas.getByTestId("dashboard-recent")).findByText("gpt-4o", {
        selector: "td span",
      }),
    ).toBeVisible();
    await expect(canvas.queryByText(en.analytics.noRowsYet)).toBeNull();
    await expect(canvas.queryByText(en.pages.dashboard.noTraffic)).toBeNull();
    await expect(canvas.queryByText(en.pages.dashboard.nothingLogged)).toBeNull();
  },
};

/**
 * The shell's team has no project yet. The shared chain answers the project list
 * before a story's own routes, so this answers it first.
 */
function withoutProjects(): FetchStub {
  const shell = shellStub();
  return async (input, init) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (path === `/api/v1/teams/${TEAM.id}/projects` && init?.method !== "POST") return json([]);
    return shell(input, init);
  };
}

/**
 * Getting started opens the create-project dialog from the Dashboard with the
 * scope popover closed (#2611). The switcher's own **New project** entry is only
 * in the document while its menu is open, so this is the story that fails if the
 * dialog ever moves back into the switcher.
 */
export const GettingStartedOpensCreateProject: Story = {
  // the dismissal is persisted per browser, and a card another story put away
  // would leave nothing here to click
  beforeEach: () => localStorage.removeItem("rolter.getting-started.dismissed"),
  render: () => <AppShell route="/dashboard" fetchStub={withoutProjects()} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const create = await canvas.findByRole("button", {
      name: en.pages.gettingStarted.createProject,
    });
    // the switcher's own entry for it is not on screen: its menu is shut
    await expect(canvas.queryByRole("menuitem", { name: en.scope.newProject })).toBeNull();
    await userEvent.click(create);
    const dialog = within(await confirmation());
    await expect(dialog.getByText(en.scope.newProject)).toBeVisible();
    await expect(dialog.getByText(en.scope.newProjectHint)).toBeVisible();
  },
};

/**
 * Between `md` and `lg` the rail is still in the flow but folded to icons, and
 * the splitter is gone: dragging a 52px strip wider is not that width's
 * affordance.
 */
export const Tablet: Story = {
  ...atTablet,
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    await waitFor(() => expect(rail.getBoundingClientRect().width).toBe(52));
    await expect(within(rail).queryByRole("separator")).toBeNull();
    await expect(within(rail).queryByText("rolter")).toBeNull();
    await expectNoHorizontalOverflow();
  },
};

/**
 * A group on the folded rail used to do nothing when pressed: the click flipped
 * state that only the full-width rail draws (#2803). It opens a flyout of the
 * group's screens beside the icon; picking one goes there, puts the flyout away
 * and leaves the group marked as the section you are in.
 */
export const FoldedRailOpensAGroup: Story = {
  ...atTablet,
  render: () => <AppShell route="/providers" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const rail = await railOf(canvasElement);
    await waitFor(() => expect(rail.getBoundingClientRect().width).toBe(52));
    const group = await within(rail).findByRole("button", { name: nav.models });
    // the screen is out of sight on the folded rail, so its group says so
    await expect(group).toHaveAttribute("aria-current", "true");

    await userEvent.click(group);
    const flyout = within(await within(rail).findByRole("group", { name: nav.models }));
    await expect(flyout.getByRole("button", { name: nav.providers })).toHaveAttribute(
      "aria-current",
      "page",
    );
    await userEvent.click(flyout.getByRole("button", { name: nav["routing-rules"] }));

    await waitFor(() =>
      expect(
        canvas.getByRole("heading", { level: 1, name: screens["routing-rules"].title }),
      ).toBeVisible(),
    );
    await waitFor(() => expect(within(rail).queryByRole("group", { name: nav.models })).toBeNull());
    await waitFor(() => expect(group).toHaveFocus());
    await expect(group).toHaveAttribute("aria-current", "true");
    await expectNoHorizontalOverflow();
  },
};

/**
 * The flyout lists what the rail would: a screen the role may not read stays
 * out of it, as it stays out of the full-width rail (#1183).
 */
export const FoldedRailFlyoutLeavesOutRefusedScreens: Story = {
  ...atTablet,
  render: () => <AppShell route="/dashboard" fetchStub={withCapabilities("viewer", shellStub())} />,
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    await waitFor(() => expect(rail.getBoundingClientRect().width).toBe(52));
    await waitFor(() =>
      expect(within(rail).queryByRole("button", { name: nav.alerting })).toBeNull(),
    );
    await userEvent.click(within(rail).getByRole("button", { name: nav.governance }));
    const flyout = within(await within(rail).findByRole("group", { name: nav.governance }));
    await expect(flyout.getByRole("button", { name: nav["gov-teams"] })).toBeVisible();
    await expect(flyout.queryByRole("button", { name: nav["audit-logs"] })).toBeNull();
    await expect(flyout.queryByRole("button", { name: nav.sso })).toBeNull();
  },
};

/**
 * Folded to the icon strip the card is the initials alone, and its menu opens
 * beside the rail with the same entries and the same keyboard (#2805): Enter
 * opens it onto the first entry, the arrows walk it, Enter takes one.
 */
export const AccountMenuOnTheFoldedRail: Story = {
  ...atTablet,
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const rail = await railOf(canvasElement);
    await waitFor(() => expect(rail.getBoundingClientRect().width).toBe(52));
    const card = await within(rail).findByRole("button", { name: "anya@acme.co" });
    card.focus();
    await userEvent.keyboard("{Enter}");
    const menu = within(await within(rail).findByRole("menu", { name: menuLabel }));
    await waitFor(() =>
      expect(menu.getByRole("menuitem", { name: nav["api-keys"] })).toHaveFocus(),
    );
    const box = within(rail).getByRole("menu", { name: menuLabel }).parentElement!;
    await expect(box.getBoundingClientRect().left).toBeGreaterThanOrEqual(
      rail.getBoundingClientRect().right,
    );
    await expect(within(box).getByText(en.shell.roleLine.superadmin)).toBeVisible();
    await expectNoHorizontalOverflow();

    await userEvent.keyboard("{ArrowDown}{Enter}");
    await canvas.findByRole("heading", { level: 1, name: screens.preferences.title });
    await waitFor(() => expect(card).toHaveFocus());
  },
};

/**
 * The scope switcher folds to a building: no text, the path in its name, and its
 * popover opens beside the rail with the three pickers in it.
 */
export const ScopePopoverOnTheFoldedRail: Story = {
  ...atTablet,
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    await waitFor(() => expect(rail.getBoundingClientRect().width).toBe(52));
    const trigger = await within(rail).findByRole("button", {
      name: en.scope.trigger.replace("{{path}}", `${ORG.name} / ${TEAM.name} / ${PROJECT.name}`),
    });
    await expect(within(rail).queryByText(ORG.name)).toBeNull();
    await userEvent.click(trigger);
    const dialog = await within(rail).findByRole("dialog", { name: en.shell.scope });
    await expect(dialog.getBoundingClientRect().left).toBeGreaterThanOrEqual(
      rail.getBoundingClientRect().right,
    );
    await expect(within(dialog).getByLabelText(en.scope.rows.project)).toHaveValue(PROJECT.name);
    await expectNoHorizontalOverflow();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(trigger).toHaveFocus());
  },
};

/**
 * Inside the drawer the popover and the menu own Escape while they are up: the
 * first press puts them away, and only a press with nothing open closes the
 * drawer.
 */
export const ScopePopoverInsideTheDrawerOwnsEscape: Story = {
  ...atMobile,
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: OPEN_NAV }));
    const drawer = within(await canvas.findByRole("dialog", { name: NAV_LABEL }));
    await userEvent.click(
      await drawer.findByRole("button", {
        name: en.scope.trigger.replace("{{path}}", `${ORG.name} / ${TEAM.name} / ${PROJECT.name}`),
      }),
    );
    await drawer.findByRole("dialog", { name: en.shell.scope });
    await expectNoHorizontalOverflow();

    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(drawer.queryByRole("dialog", { name: en.shell.scope })).toBeNull());
    await expect(canvas.getByRole("dialog", { name: NAV_LABEL })).toBeVisible();

    await userEvent.click(await drawer.findByRole("button", { name: /anya@acme.co/ }));
    await drawer.findByRole("menu", { name: menuLabel });
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(drawer.queryByRole("menu")).toBeNull());
    await expect(canvas.getByRole("dialog", { name: NAV_LABEL })).toBeVisible();

    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(canvas.queryByRole("dialog", { name: NAV_LABEL })).toBeNull());
  },
};

// the longest copy the new rail pieces carry is Russian: the role line names a
// level and a name, the trigger a whole path. at the narrowest full rail, 1024px,
// and folded, the card, the trigger and both panels have to stay inside the page
const RU_1024 = {
  ...atSplit,
  globals: { ...atSplit.globals, locale: "ru" },
} as const;

export const AccountMenuAndScopeFitInRussian: Story = {
  ...RU_1024,
  render: () => (
    <AppShell
      fetchStub={shellStub([
        me({ is_superadmin: false }, [{ org_id: ORG.id, team_id: TEAM.id, role: "member" }]),
      ])}
    />
  ),
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement, ru.shell.navLabel);
    const path = `${ORG.name} / ${TEAM.name} / ${PROJECT.name}`;
    await userEvent.click(
      await within(rail).findByRole("button", { name: ru.scope.trigger.replace("{{path}}", path) }),
    );
    const popover = await within(rail).findByRole("dialog", { name: ru.shell.scope });
    await expect(within(popover).getByLabelText(ru.scope.rows.org)).toHaveValue(ORG.name);
    await expect(within(popover).getAllByText(/./).length).toBeGreaterThan(0);
    await userEvent.keyboard("{Escape}");

    await userEvent.click(await within(rail).findByRole("button", { name: /anya@acme.co/ }));
    const menu = await within(rail).findByRole("menu", { name: ru.shell.userMenuLabel });
    await expect(
      within(menu.parentElement!).getByText(`Участник · команда ${TEAM.name}`),
    ).toBeVisible();
    // each entry is whole: no label is cut by the panel's own edge
    for (const entry of within(menu).getAllByRole("menuitem")) {
      await expect(entry.scrollWidth).toBeLessThanOrEqual(entry.clientWidth + 1);
    }
    const box = menu.parentElement!.getBoundingClientRect();
    await expect(box.right).toBeLessThanOrEqual(window.innerWidth);
    await expectNoHorizontalOverflow();
  },
};

/** The same two panels beside the folded rail, in Russian, stay inside the window. */
export const AccountMenuAndScopeFitInRussianFolded: Story = {
  parameters: atTablet.parameters,
  globals: { ...atTablet.globals, locale: "ru" },
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement, ru.shell.navLabel);
    await waitFor(() => expect(rail.getBoundingClientRect().width).toBe(52));
    const path = `${ORG.name} / ${TEAM.name} / ${PROJECT.name}`;
    await userEvent.click(
      await within(rail).findByRole("button", { name: ru.scope.trigger.replace("{{path}}", path) }),
    );
    await expectInViewport(await within(rail).findByRole("dialog", { name: ru.shell.scope }));
    await userEvent.keyboard("{Escape}");

    await userEvent.click(await within(rail).findByRole("button", { name: "anya@acme.co" }));
    const menu = await within(rail).findByRole("menu", { name: ru.shell.userMenuLabel });
    await expectInViewport(menu.parentElement!);
    for (const entry of within(menu).getAllByRole("menuitem")) {
      await expect(entry.scrollWidth).toBeLessThanOrEqual(entry.clientWidth + 1);
    }
    await expectNoHorizontalOverflow();
  },
};

/**
 * Below `md` the rail is out of the flow entirely until the header asks for
 * it. The half neither #1238 story could cover on its own is the last one
 * here: picking an entry has to navigate *and* put the drawer away, or the
 * screen it just opened is behind a scrim.
 */
export const Mobile: Story = {
  ...atMobile,
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the landing screen renders first, and it has the whole width to do it in
    await expect(
      await canvas.findByRole("heading", { level: 1, name: screens.dashboard.title }),
    ).toBeVisible();
    await expect(canvas.queryByRole("navigation")).toBeNull();
    await expectNoHorizontalOverflow();

    await userEvent.click(canvas.getByRole("button", { name: OPEN_NAV }));
    const drawer = await canvas.findByRole("dialog", { name: NAV_LABEL });
    await expectNoHorizontalOverflow();

    // a top-level leaf, so the assertion is about the drawer and not about
    // expanding a parent on the way to a child
    await userEvent.click(await within(drawer).findByRole("button", { name: nav.playground }));

    // navigated…
    await waitFor(() =>
      expect(
        canvas.getByRole("heading", { level: 1, name: screens.playground.title }),
      ).toBeVisible(),
    );
    // …and closed behind itself
    await waitFor(() => expect(canvas.queryByRole("dialog")).toBeNull());
    await expectNoHorizontalOverflow();
  },
};

/**
 * The experimental marker, end to end (#1386): `/api/v1/stability` names a
 * subsystem and the nav leaves it maps to, and the rail marks exactly those
 * entries in place — no new section, no regrouping, every other entry
 * untouched.
 *
 * The word is read out of the catalog for the same reason the labels are, and
 * it lands *inside* the button, so a screen reader hears "Plugins,
 * Experimental" rather than having to find a badge sitting next to it.
 */
export const ExperimentalMarker: Story = {
  render: () => (
    <AppShell route="/dashboard" fetchStub={shellStubWithStability([EXPERIMENTAL_SUBSYSTEM])} />
  ),
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    const marked = await within(rail).findByRole("button", {
      name: `${nav.plugins} ${en.shell.experimental}`,
    });
    await expect(marked).toBeVisible();
    // the badge is the entry's own, not a row of its own
    const badge = within(marked).getByText(en.shell.experimental);
    await expect(badge).toBeVisible();
    // its note is the catalog's copy for the subsystem id, not the English
    // prose the stub sent on the wire (#1401)
    await expect(badge).toHaveAttribute("title", en.stability.notes.plugins);
    // and the marker is the exception it claims to be: a sibling the answer
    // did not name carries nothing
    const plain = within(rail).getByRole("button", { name: nav.playground });
    await expect(within(plain).queryByText(en.shell.experimental)).toBeNull();
  },
};

/**
 * The note follows the dashboard's locale (#1401). The control plane only ever
 * sends English prose, so a Russian rail used to read "Экспериментально" with a
 * sentence of English behind it; the rail now looks the note up by subsystem
 * id, and the wire's `note` is never what a user reads.
 */
export const ExperimentalMarkerTranslated: Story = {
  globals: { locale: "ru" },
  render: () => (
    <AppShell route="/dashboard" fetchStub={shellStubWithStability([EXPERIMENTAL_SUBSYSTEM])} />
  ),
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement, ru.shell.navLabel);
    const marked = await within(rail).findByRole("button", {
      name: `${ru.nav.plugins} ${ru.shell.experimental}`,
    });
    const badge = within(marked).getByText(ru.shell.experimental);
    await expect(badge).toHaveAttribute("title", ru.stability.notes.plugins);
    await expect(badge).not.toHaveAttribute("title", EXPERIMENTAL_SUBSYSTEM.note);
  },
};

/**
 * A subsystem this build's catalogs have no note for — a newer control plane
 * behind an older bundle — still marks its entry. Only the explanation is
 * missing, and nothing falls back to the English on the wire.
 */
export const ExperimentalMarkerWithoutNote: Story = {
  render: () => (
    <AppShell
      route="/dashboard"
      fetchStub={shellStubWithStability([{ ...EXPERIMENTAL_SUBSYSTEM, id: "not_in_this_build" }])}
    />
  ),
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    const marked = await within(rail).findByRole("button", {
      name: `${nav.plugins} ${en.shell.experimental}`,
    });
    const badge = within(marked).getByText(en.shell.experimental);
    await expect(badge).toBeVisible();
    await expect(badge).not.toHaveAttribute("title");
  },
};

/**
 * Folded to icons there is no room for a word, so the marker becomes a dot on
 * the corner of the entry's icon and the name it lost moves into the tooltip —
 * which, on a button with no text, is also its accessible name.
 */
export const ExperimentalMarkerOnIconRail: Story = {
  ...atTablet,
  render: () => (
    <AppShell route="/dashboard" fetchStub={shellStubWithStability([EXPERIMENTAL_SUBSYSTEM])} />
  ),
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    await waitFor(() => expect(rail.getBoundingClientRect().width).toBe(52));
    const marked = await within(rail).findByRole("button", {
      name: en.shell.experimentalItem.replace("{{label}}", nav.plugins),
    });
    await expect(marked).toBeVisible();
    // the word itself is not painted at this width
    await expect(within(rail).queryByText(en.shell.experimental)).toBeNull();
    await expectNoHorizontalOverflow();
  },
};

// A 1024px window (#2812). The rail is the full 232px one and the screen beside
// it has what is left, which is where the livetest pass met a sheet subtitle
// cut mid-sentence, "Репоз…" in place of "Репозиторий навыков" and a key named
// "Pl…". These stories mount the whole shell at that width, in both locales,
// since the Russian copy runs a third longer than the English.

/** the three entries that sit in the rail's tightest places: top level, nested, a short one */
const SPLIT_SUBSYSTEMS: SubsystemStability[] = [
  {
    id: "skills_repository",
    stability: "experimental",
    note: "a skill resolves only through the control-plane API",
    nav_keys: ["skills-repo"],
  },
  {
    id: "mcp_settings",
    stability: "experimental",
    note: "organization MCP defaults are stored but not yet read by the proxy",
    nav_keys: ["mcp-settings"],
  },
  EXPERIMENTAL_SUBSYSTEM,
];

/**
 * An experimental entry shows its whole name and its marker, in `locale`.
 *
 * The marker used to hold its size and the name gave way: at 232px the label of
 * "Репозиторий навыков" was left with room for four letters. Now the name is
 * measured, not just found, and so is the badge, which has to stay inside the
 * rail it is drawn in.
 */
function entriesStayWhole(locale: "en" | "ru") {
  const cat = locale === "ru" ? ru : en;
  const labels = cat.nav as Record<string, string>;
  const word = cat.shell.experimental;
  return {
    ...atSplit,
    globals: { ...atSplit.globals, locale },
    render: () => (
      <AppShell route="/dashboard" fetchStub={shellStubWithStability(SPLIT_SUBSYSTEMS)} />
    ),
    play: async ({ canvasElement }: { canvasElement: HTMLElement }) => {
      const rail = await railOf(canvasElement, cat.shell.navLabel);
      const whole = async (entry: HTMLElement, name: string) => {
        await expectNotTruncated(within(entry).getByText(name));
        // and nothing inside the entry is wider than the entry
        await expect(entry.scrollWidth).toBeLessThanOrEqual(entry.clientWidth);
        const badge = within(entry).getByText(word);
        await expectInFrame(badge, entry);
        await expectInFrame(badge, rail);
        await expect(badge).toBeVisible();
      };

      // a top-level entry
      const skills = await within(rail).findByRole("button", {
        name: `${labels["skills-repo"]} ${word}`,
      });
      await whole(skills, labels["skills-repo"]);
      // one nested under its group, which has the least room of all
      await userEvent.click(within(rail).getByRole("button", { name: labels.mcp }));
      const settings = await within(rail).findByRole("button", {
        name: `${labels["mcp-settings"]} ${word}`,
      });
      await whole(settings, labels["mcp-settings"]);
      await expectNoHorizontalOverflow();
    },
  };
}

export const ExperimentalEntriesStayWholeAt1024: Story = entriesStayWhole("en");
export const ExperimentalEntriesStayWholeAt1024InRussian: Story = entriesStayWhole("ru");

/**
 * The folded rail's flyout draws the same entry, so it keeps the same promise:
 * the name whole, the marker beside it or under it.
 */
export const ExperimentalEntriesStayWholeInTheFlyoutInRussian: Story = {
  ...atSplit,
  globals: { ...atSplit.globals, locale: "ru" },
  render: () => (
    <AppShell route="/dashboard" fetchStub={shellStubWithStability(SPLIT_SUBSYSTEMS)} />
  ),
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement, ru.shell.navLabel);
    await userEvent.click(
      await within(rail).findByRole("button", { name: ru.shell.collapseSidebar }),
    );
    await waitFor(() => expect(rail.getBoundingClientRect().width).toBe(52));
    await userEvent.click(within(rail).getByRole("button", { name: ru.nav.mcp }));
    const flyout = within(await within(rail).findByRole("group", { name: ru.nav.mcp }));
    const entry = flyout.getByRole("button", {
      name: `${ru.nav["mcp-settings"]} ${ru.shell.experimental}`,
    });
    await expectNotTruncated(within(entry).getByText(ru.nav["mcp-settings"]));
    await expectInViewport(within(entry).getByText(ru.shell.experimental));
  },
};

const LONG_PROVIDER = "vllm-a10g-eu-west-prod-cluster-b";

const SPLIT_PROVIDERS: ProviderRow[] = [
  {
    id: "p-1",
    org_id: "org-1",
    name: "vllm-a10g-prod",
    slug: "vllm-a10g",
    kind: "openai_compatible",
    api_base: "https://vllm.internal.example.com/v1",
    api_key_env: "VLLM_API_KEY",
    egress_proxies: [],
    created_at: "2026-01-02T00:00:00Z",
  },
  {
    id: "p-2",
    org_id: "org-1",
    name: LONG_PROVIDER,
    slug: "vllm-a10g-eu-west",
    kind: "openai_compatible",
    api_base: "https://vllm-b.internal.example.com/v1",
    api_key_env: "VLLM_B_API_KEY",
    egress_proxies: [],
    created_at: "2026-01-03T00:00:00Z",
  },
];

/** what the screen's first column must at least hold, in px (11rem at 16px) */
const IDENTIFYING_FLOOR = 176;

/**
 * The provider's name is the column a row is found by, and at 1024px it was
 * given what the other six columns left over: "vllm-a10…" for a 14-character
 * name. It keeps a floor now, and a name longer than the column still says
 * itself in full on hover.
 */
function providersKeepTheirNames(locale: "en" | "ru") {
  const cat = locale === "ru" ? ru : en;
  return {
    ...atSplit,
    globals: { ...atSplit.globals, locale },
    render: () => (
      <AppShell
        route="/providers"
        fetchStub={shellStub([
          ["/api/v1/stability", () => []],
          ["/providers", () => SPLIT_PROVIDERS],
          ["/config/problems", () => ({ problems: [] })],
        ])}
      />
    ),
    play: async ({ canvasElement }: { canvasElement: HTMLElement }) => {
      const canvas = within(canvasElement);
      const table = await canvas.findByRole("table", { name: cat.screens.providers.title });
      const name = await within(table).findByText("vllm-a10g-prod");
      await expectNotTruncated(name);
      const cell = name.closest('[role="cell"]') as HTMLElement;
      await expect(cell.getBoundingClientRect().width).toBeGreaterThanOrEqual(
        IDENTIFYING_FLOOR - 1,
      );
      // a name wider than its column is cut by the column, and says itself
      // whole once the pointer is on it; one that fits has nothing to add
      const long = within(table).getByText(LONG_PROVIDER);
      await userEvent.hover(long);
      await expect(long).toHaveAttribute("title", LONG_PROVIDER);
      await userEvent.hover(name);
      await expect(name).not.toHaveAttribute("title");
      await expectNoHorizontalOverflow();
    },
  };
}

export const ProviderNamesKeepTheirColumnAt1024: Story = providersKeepTheirNames("en");
export const ProviderNamesKeepTheirColumnAt1024InRussian: Story = providersKeepTheirNames("ru");

const SPLIT_KEYS: VirtualKeyRow[] = [
  {
    id: "vk-1",
    project_id: "project-1",
    key_hash: "hash-1",
    key_prefix: "sk-rolter-play",
    name: "Playground",
    models: ["gpt-4o"],
    providers: [],
    created_by: "user-1",
    business_unit_id: null,
    customer_id: null,
    disabled: false,
    expires_at: "2026-07-01T00:30:00Z",
    cache_enabled: null,
    purpose: "playground",
    created_at: "2026-07-01T00:00:00Z",
  },
  {
    id: "vk-2",
    project_id: "project-1",
    key_hash: "hash-2",
    key_prefix: "sk-rolter-ci",
    name: "ci-nightly-regression-runner-eu-west",
    models: [],
    providers: [],
    created_by: null,
    business_unit_id: null,
    customer_id: null,
    disabled: false,
    expires_at: null,
    cache_enabled: null,
    created_at: "2026-07-01T00:00:00Z",
  },
];

/**
 * "Playground" beside its badge was cut to "Pl…" at this width: the badge held
 * its size and the name took the rest of a column that was 104px wide. Name and
 * badge share the line while they fit and the badge drops under the name when
 * they do not; the name is never what gives way.
 */
function keysKeepTheirNames(locale: "en" | "ru") {
  const cat = locale === "ru" ? ru : en;
  return {
    ...atSplit,
    globals: { ...atSplit.globals, locale },
    render: () => (
      <AppShell
        route="/virtual-keys"
        fetchStub={shellStub([
          ["/api/v1/stability", () => []],
          ["/virtual-keys", () => SPLIT_KEYS],
        ])}
      />
    ),
    play: async ({ canvasElement }: { canvasElement: HTMLElement }) => {
      const canvas = within(canvasElement);
      const table = await canvas.findByRole("table", { name: cat.screens["virtual-keys"].title });
      const row = (
        await within(table).findByText("Playground", { selector: "span.font-semibold" })
      ).closest('[role="row"]') as HTMLElement;
      const name = within(row).getByText("Playground", { selector: "span.font-semibold" });
      await expectNotTruncated(name);
      const cell = name.closest('[role="cell"]') as HTMLElement;
      await expect(cell.getBoundingClientRect().width).toBeGreaterThanOrEqual(
        IDENTIFYING_FLOOR - 1,
      );
      // the badge is beside the name or under it, and inside the column either way
      await expectInFrame(within(row).getByTitle(cat.pages.virtualKeys.playgroundHint), cell);
      const longName = "ci-nightly-regression-runner-eu-west";
      const long = within(table).getByText(longName);
      await userEvent.hover(long);
      await expect(long).toHaveAttribute("title", longName);
      await userEvent.hover(name);
      await expect(name).not.toHaveAttribute("title");
      await expectNoHorizontalOverflow();
    },
  };
}

export const KeyNamesKeepTheirColumnAt1024: Story = keysKeepTheirNames("en");
export const KeyNamesKeepTheirColumnAt1024InRussian: Story = keysKeepTheirNames("ru");

/**
 * The cache policy of a key is a combobox in the row, and "наследовать" was cut
 * to "насле…" in it: the table was already at its floor and the column took
 * what the others left. The control now holds its longest value in full, in
 * either language, so the word the row means is the word on screen.
 */
function keysShowTheirCachePolicy(locale: "en" | "ru") {
  const cat = locale === "ru" ? ru : en;
  return {
    ...atSplit,
    globals: { ...atSplit.globals, locale },
    render: () => (
      <AppShell
        route="/virtual-keys"
        fetchStub={shellStub([
          ["/api/v1/stability", () => []],
          ["/virtual-keys", () => SPLIT_KEYS],
        ])}
      />
    ),
    play: async ({ canvasElement }: { canvasElement: HTMLElement }) => {
      const canvas = within(canvasElement);
      const table = await canvas.findByRole("table", { name: cat.screens["virtual-keys"].title });
      const policy = (name: string) =>
        within(table).findByRole("combobox", {
          name: cat.pages.virtualKeys.cacheAria.replace("{{name}}", name),
        });
      for (const name of ["Playground", "ci-nightly-regression-runner-eu-west"]) {
        const cache = await policy(name);
        const value = cat.pages.virtualKeys.cacheModes.inherit;
        await expect(cache).toHaveValue(value);
        // the text is whole inside the field: nothing runs past its padding box
        await expect(cache.scrollWidth).toBeLessThanOrEqual(cache.clientWidth + 1);
        // and the control names its value to the pointer as well
        await expect(cache).toHaveAttribute("title", value);
      }
      await expectNoHorizontalOverflow();
    },
  };
}

export const KeyCachePolicyIsReadableAt1024: Story = keysShowTheirCachePolicy("en");
export const KeyCachePolicyIsReadableAt1024InRussian: Story = keysShowTheirCachePolicy("ru");

/**
 * A rail entry too long for the rail ends in an ellipsis, and the rest of its
 * name was nowhere on screen: "Адаптивная маршрут…" at the rail's 232px. The
 * folded rail names its icons through `title`; the full rail now does the same
 * for the label it has cut, and only for that one.
 *
 * Every group is opened first, so the nested entries are measured too, and the
 * story insists that something is cut: a catalog reworded to fit would
 * otherwise leave it asserting nothing.
 */
export const RailLabelsNameThemselvesWhenCutInRussian: Story = {
  ...atSplit,
  globals: { ...atSplit.globals, locale: "ru" },
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement, ru.shell.navLabel);
    await within(rail).findByRole("button", { name: ru.nav.playground });
    // a group opens in place, so the next closed one is looked up afresh
    const closedGroup = () => rail.querySelector('button[aria-expanded="false"]');
    for (let group = closedGroup(), opened = 0; group && opened < 30; opened++) {
      await userEvent.click(group);
      group = closedGroup();
    }
    const labels = Array.from(rail.querySelectorAll<HTMLElement>("button > span")).filter(
      (el) => getComputedStyle(el).textOverflow === "ellipsis",
    );
    const cut = labels.filter((el) => el.scrollWidth > el.clientWidth + 1);
    const whole = labels.filter((el) => el.scrollWidth <= el.clientWidth + 1);
    await expect(cut.length).toBeGreaterThan(0);
    await expect(whole.length).toBeGreaterThan(0);

    for (const label of cut) {
      await userEvent.hover(label);
      await expect(label).toHaveAttribute("title", label.textContent ?? "");
    }
    // a label that fits has nothing to add
    for (const label of whole.slice(0, 3)) {
      await userEvent.hover(label);
      await expect(label).not.toHaveAttribute("title");
    }
    // the entry the livetest could not read
    const adaptive = labels.find((el) => el.textContent === ru.nav["adaptive-routing"]);
    await expect(adaptive).toBeDefined();
    await expect(adaptive).toHaveAttribute("title", ru.nav["adaptive-routing"]);
    await expectNoHorizontalOverflow();
  },
};

/**
 * ⌘K from anywhere in the shell (#1198): the palette opens with focus already
 * in its field, a screen picked there navigates, and the palette closes behind
 * it. The shortcut is the whole feature — a palette only a mouse can open is
 * the gap the issue was filed over.
 */
export const CommandPaletteShortcut: Story = {
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await railOf(canvasElement);
    const body = within(document.body);
    await expect(body.queryByRole("combobox", { name: en.shell.palette.label })).toBeNull();

    await userEvent.keyboard("{Meta>}k{/Meta}");
    const field = await body.findByRole("combobox", { name: en.shell.palette.label });
    await waitFor(() => expect(field).toHaveFocus());

    await userEvent.keyboard("playg");
    const hit = await body.findByRole("option", { name: new RegExp(nav.playground, "i") });
    await userEvent.click(hit);

    await waitFor(() =>
      expect(
        canvas.getByRole("heading", { level: 1, name: screens.playground.title }),
      ).toBeVisible(),
    );
    await waitFor(() =>
      expect(body.queryByRole("combobox", { name: en.shell.palette.label })).toBeNull(),
    );
  },
};

// the one request the palette story looks up: a failed call whose id a client
// quoted, from outside the log's default window
const LOGGED: InvocationRow = {
  ts: "2025-01-15T09:30:00.000Z",
  request_id: "3f2c9a1e-7b4d-4f10-9c2e-0a1b2c3d4e5f",
  trace_id: "",
  org_id: "org-1",
  team_id: "team-1",
  project_id: "project-1",
  virtual_key_id: "vk-1",
  business_unit_id: "",
  customer_id: "",
  model: "gpt-4o",
  provider: "openai",
  target: "openai/gpt-4o",
  variant: "",
  status: 502,
  // the connection reset before the upstream sent a status line
  upstream_status: 0,
  attempts: 1,
  stream: 0,
  cache_hit: 0,
  cache_read_tokens: 0,
  cache_write_tokens: 0,
  prompt_tokens: 800,
  completion_tokens: 0,
  total_tokens: 800,
  cost_usd: 0,
  unpriced: 0,
  latency_ms: 842,
  ttft_ms: 0,
  error: "upstream reset the connection",
};
const logged = recording(
  shellStub([["/api/v1/analytics/invocations", () => ({ data: [LOGGED] })]]),
);

/**
 * A request id pasted into the palette (#1861) goes through the whole shell: the
 * palette offers the lookup, Enter navigates to `/logs?request_id=…`, the screen
 * reads its lookup from that address, asks the control plane for the id with no
 * window, and opens the row it finds. The palette and the screen are two
 * components that only meet here.
 */
export const APastedIdInThePaletteOpensLlmLogs: Story = {
  render: () => <AppShell route="/dashboard" fetchStub={logged.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const body = within(document.body);
    await railOf(canvasElement);

    await userEvent.keyboard("{Meta>}k{/Meta}");
    const field = await body.findByRole("combobox", { name: en.shell.palette.label });
    await waitFor(() => expect(field).toHaveFocus());
    await userEvent.paste(LOGGED.request_id);
    await body.findByRole("option", {
      name: new RegExp(en.shell.palette.openRequest.replace("{{id}}", LOGGED.request_id)),
    });
    await userEvent.keyboard("{Enter}");

    await waitFor(() =>
      expect(canvas.getByRole("heading", { level: 1, name: screens.logs.title })).toBeVisible(),
    );
    const panel = await canvas.findByRole("complementary", { name: "Details" });
    await waitFor(() => expect(within(panel).getByText(LOGGED.request_id)).toBeVisible());
    await expect(canvas.getByRole("textbox", { name: en.pages.logs.lookup.label })).toHaveValue(
      LOGGED.request_id,
    );
    await waitFor(() =>
      expect(body.queryByRole("combobox", { name: en.shell.palette.label })).toBeNull(),
    );

    // the dashboard the shell booted on asked for its own recent rows before the
    // palette was opened; the reads that count are the log screen's, which name
    // a status class
    const reads = logged.calls
      .filter((c) => c.url.includes("/analytics/invocations"))
      .map((c) => new URL(c.url, "http://localhost").searchParams)
      .filter((q) => q.has("status"));
    await expect(reads.length).toBeGreaterThan(0);
    for (const sent of reads) {
      await expect(sent.get("request_id")).toBe(LOGGED.request_id);
      await expect(sent.has("since")).toBe(false);
    }
  },
};

/**
 * `/` puts the caret in the rail's search box — but only when the caller is
 * not already typing somewhere, or the character would be unwritable in every
 * field in the dashboard (#1198).
 */
export const NavSearchShortcut: Story = {
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    const search = within(rail).getByRole("textbox", { name: en.shell.searchNav });
    await expect(search).not.toHaveFocus();

    await userEvent.keyboard("/");
    await waitFor(() => expect(search).toHaveFocus());
    // the keystroke moved focus, it did not also land in the box
    await expect(search).toHaveValue("");

    // and inside a field a slash is just a slash
    await userEvent.type(search, "a/b");
    await expect(search).toHaveValue("a/b");
  },
};

/**
 * `?` from anywhere in the shell opens the reference (#1676), and the sheet
 * lists the shortcuts the shell actually binds — the list is `SHORTCUTS`
 * mapped on both sides, so this walks the table rather than naming rows.
 */
export const ShortcutReference: Story = {
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    await railOf(canvasElement);
    const body = within(document.body);
    await expect(body.queryByRole("dialog", { name: shortcuts.title })).toBeNull();

    await userEvent.keyboard("?");
    const dialog = await body.findByRole("dialog", { name: shortcuts.title });
    // it animates in, so the first visibility read is polled (#2287)
    await waitFor(() => expect(dialog).toBeVisible());
    const items = shortcuts.items as Record<string, string>;
    for (const shortcut of SHORTCUTS) {
      await expect(within(dialog).getByText(items[shortcut.id]!)).toBeVisible();
    }

    // and it gives Escape back rather than trapping the screen behind it
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(body.queryByRole("dialog", { name: shortcuts.title })).toBeNull());
  },
};

/**
 * The bug every `?` handler ships with: `?` is a character, and a sheet that
 * opened over the field someone was typing in would make it unwritable (#1676).
 * The rail's own search box is the field to prove it in — it is on screen at
 * this width and `/` reaches it.
 */
export const ShortcutReferenceIgnoresTyping: Story = {
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    const body = within(document.body);
    const search = within(rail).getByRole("textbox", { name: en.shell.searchNav });

    // inside a field a question mark is just a question mark
    await userEvent.type(search, "what?");
    await expect(search).toHaveValue("what?");
    await expect(body.queryByRole("dialog", { name: shortcuts.title })).toBeNull();

    // nor with a modifier held, where the chord belongs to the browser or the os
    await userEvent.click(canvasElement);
    await userEvent.keyboard("{Meta>}?{/Meta}");
    await expect(body.queryByRole("dialog", { name: shortcuts.title })).toBeNull();
    await userEvent.keyboard("{Alt>}?{/Alt}");
    await expect(body.queryByRole("dialog", { name: shortcuts.title })).toBeNull();

    // outside one it opens, which is what makes the three above a guard rather
    // than a handler that never fires
    await userEvent.keyboard("?");
    await expect(await body.findByRole("dialog", { name: shortcuts.title })).toBeVisible();
  },
};

/**
 * The hints (#1676): the palette prints the chord that opens it beside its
 * field, and the rail's search box prints `/`. Both read their chord off the
 * shortcut table, so neither can name a keystroke the shell does not bind.
 */
export const ShortcutHints: Story = {
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    const body = within(document.body);

    // the rail's hint gives way to the clear button once there is a query
    const slash = chordText(shortcutChord("navSearch"));
    await expect(within(rail).getByRole("img", { name: slash })).toBeVisible();
    const search = within(rail).getByRole("textbox", { name: en.shell.searchNav });
    await userEvent.type(search, "log");
    await waitFor(() => expect(within(rail).queryByRole("img", { name: slash })).toBeNull());
    await userEvent.clear(search);

    // and the palette carries its own, whichever glyph this platform prints
    await userEvent.keyboard("{Meta>}k{/Meta}");
    await body.findByRole("combobox", { name: en.shell.palette.label });
    await expect(
      body.getByRole("img", { name: chordText(shortcutChord("palette")) }),
    ).toBeVisible();
  },
};

/**
 * The skip link (#1198): the first stop in the document, ahead of the rail's
 * forty-odd entries, and it actually moves focus into the screen rather than
 * only changing the url.
 */
export const SkipLink: Story = {
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const rail = await railOf(canvasElement);
    const link = canvas.getByRole("link", { name: en.shell.skipToContent });

    // it precedes the navigation it skips…
    await expect(
      link.compareDocumentPosition(rail) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    // …is clipped out of the way until it is reached…
    await expect(link.getBoundingClientRect().width).toBeLessThanOrEqual(1);
    await expect(getComputedStyle(link).overflow).toBe("hidden");

    // …and is the first thing the Tab key finds, revealed rather than merely
    // focused: a skip link still clipped to a pixel is one nobody can read
    await userEvent.tab();
    await expect(link).toHaveFocus();
    await expect(link.getBoundingClientRect().width).toBeGreaterThan(1);
    await expect(getComputedStyle(link).overflow).toBe("visible");

    const main = canvasElement.querySelector("main") as HTMLElement;
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(main).toHaveFocus());
    // focus moved without the fragment landing: letting the default action
    // through would rewrite the url out from under the router
    await expect(window.location.hash).toBe("");
  },
};

/**
 * The mouse path to the same sheet (#1697): `?` only helps the reader who
 * already knows there is something to press, so the rail footer carries an
 * entry beside the palette's magnifier, named with the chord it stands in for.
 */
export const ShortcutReferenceFromRail: Story = {
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    const body = within(document.body);
    const trigger = within(rail).getByRole("button", { name: SHORTCUTS_FOOTER });

    // clicking it opens the reference `?` opens, listing the same table
    await userEvent.click(trigger);
    const dialog = await body.findByRole("dialog", { name: shortcuts.title });
    // it animates in, so the first visibility read is polled (#2287)
    await waitFor(() => expect(dialog).toBeVisible());
    const items = shortcuts.items as Record<string, string>;
    for (const shortcut of SHORTCUTS) {
      await expect(within(dialog).getByText(items[shortcut.id]!)).toBeVisible();
    }

    // and closing it hands focus back to the entry that opened it, rather than
    // dropping the mouse user's caret at the top of the document
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(body.queryByRole("dialog", { name: shortcuts.title })).toBeNull());
    await waitFor(() => expect(trigger).toHaveFocus());
  },
};

/**
 * Folded to the icon strip the footer keeps both entries, so the width where
 * the labels are gone is the one where an icon-only affordance has to carry
 * its own name (#1697).
 */
export const ShortcutReferenceFromIconRail: Story = {
  ...atTablet,
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    const body = within(document.body);

    await userEvent.click(within(rail).getByRole("button", { name: SHORTCUTS_FOOTER }));
    await expect(await body.findByRole("dialog", { name: shortcuts.title })).toBeVisible();
    await expectNoHorizontalOverflow();
  },
};

/**
 * And below `md`, where the rail is a drawer, the entry rides along with it —
 * the width with no keyboard at all is the one that needs the mouse path most.
 */
export const ShortcutReferenceFromDrawer: Story = {
  ...atMobile,
  render: () => <AppShell route="/dashboard" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const body = within(document.body);

    await userEvent.click(await canvas.findByRole("button", { name: OPEN_NAV }));
    const drawer = await canvas.findByRole("dialog", { name: NAV_LABEL });

    await userEvent.click(await within(drawer).findByRole("button", { name: SHORTCUTS_FOOTER }));
    await expect(await body.findByRole("dialog", { name: shortcuts.title })).toBeVisible();
    await expectNoHorizontalOverflow();
  },
};

// the tab title is document state, which outlives a story: blank it first, so
// a title the previous story left behind can never pass for this one's
const blankTitle = () => {
  document.title = "";
};

/**
 * The tab names the screen on display (#2002): the header's own title from
 * `screens.<key>.title`, then the name, lowercase. Picking another language
 * from the rail renames the tab in place, the same way it relabels the screen.
 */
export const DocumentTitle: Story = {
  render: () => <AppShell route="/playground" />,
  beforeEach: () => {
    blankTitle();
    // the picker persists its choice, so hand english back to the next story
    return async () => {
      await setLocale(DEFAULT_LOCALE);
    };
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole("heading", { level: 1, name: screens.playground.title });
    await waitFor(() => expect(document.title).toBe(`${screens.playground.title} · rolter`));

    await userEvent.click(canvas.getByRole("button", { name: en.locale.change }));
    await userEvent.click(await canvas.findByRole("menuitemradio", { name: LOCALE_NAMES.ru }));
    await waitFor(() => expect(document.title).toBe(`${ru.screens.playground.title} · rolter`));
  },
};

/**
 * A screen the caller may not read keeps its name in the tab. The header still
 * names it above the refusal, and the tab says the same thing.
 */
export const DocumentTitleOnARefusedScreen: Story = {
  render: () => (
    <AppShell route="/audit-logs" fetchStub={withCapabilities("viewer", shellStub())} />
  ),
  beforeEach: blankTitle,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
    await expect(document.title).toBe(`${screens["audit-logs"].title} · rolter`);
  },
};

/** A path no screen answers to lands on the dashboard, and the tab follows it there. */
export const DocumentTitleOnAnUnknownPath: Story = {
  render: () => <AppShell route="/no-such-screen" />,
  beforeEach: blankTitle,
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByRole("heading", {
      level: 1,
      name: screens.dashboard.title,
    });
    await waitFor(() => expect(document.title).toBe(`${screens.dashboard.title} · rolter`));
  },
};
