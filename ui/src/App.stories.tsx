import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import App from "./App";
import {
  AppShell,
  EXPERIMENTAL_SUBSYSTEM,
  shellStubWithStability,
} from "./pages/shell-harness";
import en from "@/lib/i18n/locales/en.json";
import { withPageA11y } from "@/lib/story-a11y";
import { atMobile, atTablet, expectNoHorizontalOverflow } from "@/lib/story-viewport";

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
async function railOf(canvasElement: HTMLElement): Promise<HTMLElement> {
  return within(canvasElement).findByRole("navigation", { name: NAV_LABEL });
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
    await expect(
      within(rail).getByRole("button", { name: nav.playground }),
    ).toBeVisible();
    // the splitter belongs to this width and only this width
    await expect(
      within(rail).getByRole("separator", { name: en.shell.resizeSidebar }),
    ).toBeInTheDocument();
    // the shell knows which route it is on, and the screen agrees
    await expect(
      within(rail).getByRole("button", { name: nav.dashboard }),
    ).toHaveAttribute("aria-current", "page");
    await expect(
      canvas.getByRole("heading", { level: 1, name: screens.dashboard.title }),
    ).toBeVisible();
    // the header's drawer trigger is `md:hidden`, so above this breakpoint it
    // is out of the accessibility tree entirely — there is nothing to reach a
    // rail that is already on screen
    await expect(canvas.queryByRole("button", { name: OPEN_NAV })).toBeNull();
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
    await userEvent.click(
      await within(drawer).findByRole("button", { name: nav.playground }),
    );

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
    <AppShell
      route="/dashboard"
      fetchStub={shellStubWithStability([EXPERIMENTAL_SUBSYSTEM])}
    />
  ),
  play: async ({ canvasElement }) => {
    const rail = await railOf(canvasElement);
    const marked = await within(rail).findByRole("button", {
      name: `${nav.plugins} ${en.shell.experimental}`,
    });
    await expect(marked).toBeVisible();
    // the badge is the entry's own, not a row of its own
    await expect(
      within(marked).getByText(en.shell.experimental),
    ).toBeVisible();
    // and the marker is the exception it claims to be: a sibling the answer
    // did not name carries nothing
    const plain = within(rail).getByRole("button", { name: nav.playground });
    await expect(
      within(plain).queryByText(en.shell.experimental),
    ).toBeNull();
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
    <AppShell
      route="/dashboard"
      fetchStub={shellStubWithStability([EXPERIMENTAL_SUBSYSTEM])}
    />
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
