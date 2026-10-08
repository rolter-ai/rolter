import { Boxes, KeyRound, LogOut, Play, ScrollText, UserCog } from "lucide-react";
import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, fn, userEvent, waitFor, within } from "storybook/test";

import { MenuItem, MenuSeparator } from "./menu";
import {
  NAV_MAX_WIDTH,
  NAV_MIN_WIDTH,
  NavSidebar,
  type NavSidebarProps,
  type NavUserMenu,
} from "./nav-sidebar";
import en from "@/lib/i18n/locales/en.json";
import {
  atMobile,
  atTablet,
  expectInFrame,
  expectNoHorizontalOverflow,
  expectNotTruncated,
} from "@/lib/story-viewport";

const meta = {
  title: "Navigation/NavSidebar",
  component: NavSidebar,
  args: {
    brand: "rolter",
    groups: [
      {
        items: [
          { key: "playground", label: "Playground", icon: <Play /> },
          { key: "models", label: "Models", icon: <Boxes /> },
          { key: "keys", label: "Keys", icon: <KeyRound /> },
          { key: "logs", label: "Logs", icon: <ScrollText /> },
        ],
      },
      {
        label: "Operate",
        items: [
          {
            key: "analytics",
            label: "Analytics",
            icon: <Boxes />,
            children: [
              { key: "usage", label: "Usage" },
              { key: "costs", label: "Costs" },
            ],
          },
        ],
      },
    ],
    activeKey: "models",
    searchable: true,
    collapsible: true,
    version: "v0.0.1",
    user: { name: "admin@rolter.dev", role: "Admin", initials: "A" },
  },
} satisfies Meta<typeof NavSidebar>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const Collapsed: Story = {
  args: { defaultCollapsed: true },
};

// the search box was the only control in the first fourteen tab stops with no
// visible focus indicator at all (#963). the ring is drawn as a box-shadow on
// the wrapping label, so "no indicator" is exactly `box-shadow: none` there
export const SearchFocusRing: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const search = canvas.getByRole("textbox", { name: /search/i });
    const wrapper = search.closest("label");
    await expect(wrapper).not.toBeNull();
    await expect(getComputedStyle(wrapper as HTMLElement).boxShadow).toBe("none");
    await userEvent.click(search);
    await expect(search).toHaveFocus();
    await expect(getComputedStyle(wrapper as HTMLElement).boxShadow).not.toBe("none");
  },
};

// the rail is a primary navigation control, so the splitter has to work for a
// keyboard user too: it is a focusable `separator` carrying its own width, and
// arrows move it in 16px steps within the same bounds a drag obeys (#950)
// a per-story key: the stories share one browser, and a width remembered by
// one of them must not decide where another one starts
const resizable = (name: string) => ({
  resizable: true,
  storageKey: `rolter.nav.width.story.${name}`,
});

// the rail animates its width, so the settled value is what matters
const expectWidth = (nav: HTMLElement, px: number) =>
  waitFor(() => expect(nav.getBoundingClientRect().width).toBe(px));

export const Resizable: Story = {
  args: resizable("default"),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const handle = canvas.getByRole("separator", { name: /resize/i });
    await expect(handle).toHaveAttribute("aria-orientation", "vertical");
    await expect(handle).toHaveAttribute("aria-valuemin", String(NAV_MIN_WIDTH));
    await expect(handle).toHaveAttribute("aria-valuemax", String(NAV_MAX_WIDTH));
    const nav = canvasElement.querySelector("nav") as HTMLElement;
    await expectWidth(nav, 232);
  },
};

/** A touch drag must resize the rail, not scroll the page (#2573). */
export const SplitterOptsOutOfTouchScrolling: Story = {
  args: resizable("default"),
  play: async ({ canvasElement }) => {
    const handle = within(canvasElement).getByRole("separator", { name: /resize/i });
    await expect(getComputedStyle(handle).touchAction).toBe("none");
  },
};

export const DraggedNarrow: Story = {
  args: resizable("narrow"),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const nav = canvasElement.querySelector("nav") as HTMLElement;
    const handle = canvas.getByRole("separator", { name: /resize/i });
    handle.focus();
    // Home is the fastest route to the narrow bound; the rail must stop there
    // rather than continue toward an unusable width
    await userEvent.keyboard("{Home}");
    await expectWidth(nav, NAV_MIN_WIDTH);
    await userEvent.keyboard("{ArrowLeft}{ArrowLeft}");
    await expectWidth(nav, NAV_MIN_WIDTH);
    await expect(handle).toHaveAttribute("aria-valuenow", String(NAV_MIN_WIDTH));
    // a narrow rail still names every item: the labels truncate, they do not
    // fall out of the tree
    await expect(canvas.getByRole("button", { name: "Playground" })).toBeVisible();
  },
};

export const DraggedWide: Story = {
  args: resizable("wide"),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const nav = canvasElement.querySelector("nav") as HTMLElement;
    const handle = canvas.getByRole("separator", { name: /resize/i });
    handle.focus();
    await userEvent.keyboard("{End}");
    await expectWidth(nav, NAV_MAX_WIDTH);
    await userEvent.keyboard("{ArrowRight}");
    await expectWidth(nav, NAV_MAX_WIDTH);
    // Enter returns the rail to the shipped default from either bound
    await userEvent.keyboard("{Enter}");
    await expectWidth(nav, 232);
  },
};

// collapsing wins over resizing: a 52px icon rail has no edge worth dragging,
// and leaving the splitter behind would let a keyboard user stretch a rail
// whose labels are hidden
export const CollapsedHasNoSplitter: Story = {
  args: { ...resizable("collapsed"), defaultCollapsed: true },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.queryByRole("separator")).toBeNull();
  },
};

// the footer's update hint (#902): a small link beside the version when the
// control plane knows of a newer stable release, an icon with a dot in the
// folded rail, and nothing at all in every other state — checking, disabled,
// offline, current. the accessible name carries the version and what the link
// opens, and it leaves the dashboard in a new tab without a referrer
const update = { latest: "0.2.0", url: "https://github.com/rolter-ai/rolter/releases/tag/v0.2.0" };
const hintName = /rolter v0\.2\.0 is available/i;

const expectReleaseLink = async (link: HTMLElement) => {
  await expect(link).toHaveAttribute("href", update.url);
  await expect(link).toHaveAttribute("target", "_blank");
  await expect(link).toHaveAttribute("rel", "noreferrer");
};

export const UpdateAvailable: Story = {
  args: { version: "v0.1.0", update },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("v0.1.0")).toBeVisible();
    const link = canvas.getByRole("link", { name: hintName });
    await expect(link).toBeVisible();
    await expect(link).toHaveTextContent("v0.2.0 available");
    await expectReleaseLink(link);
    await expectNoHorizontalOverflow();
  },
};

export const UpdateAvailableCollapsed: Story = {
  args: { version: "v0.1.0", update, defaultCollapsed: true },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the folded rail has no room for the text: the icon carries the name
    const link = canvas.getByRole("link", { name: hintName });
    await expect(link).toBeVisible();
    await expect(link).toHaveAttribute("title", expect.stringMatching(hintName));
    await expect(link).not.toHaveTextContent("available");
    await expectReleaseLink(link);
    await expect(canvas.queryByText("v0.1.0")).toBeNull();
  },
};

export const UpToDate: Story = {
  args: { version: "v0.2.0", update: null },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("v0.2.0")).toBeVisible();
    await expect(canvas.queryByRole("link", { name: hintName })).toBeNull();
    await expect(canvas.queryByText(/available/)).toBeNull();
  },
};

export const UpToDateCollapsed: Story = {
  args: { version: "v0.2.0", update: null, defaultCollapsed: true },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.queryByRole("link", { name: hintName })).toBeNull();
  },
};

// the check is disabled, still running, or the control plane is offline: the
// shell hands the rail no hint at all, and the footer is the plain version
export const UpdateCheckDisabled: Story = {
  args: { version: "v0.1.0", update: undefined },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("v0.1.0")).toBeVisible();
    await expect(canvas.queryByRole("link", { name: hintName })).toBeNull();
    await expect(canvas.queryByText(/available/)).toBeNull();
  },
};

export const UpdateCheckDisabledCollapsed: Story = {
  args: { version: "v0.1.0", update: undefined, defaultCollapsed: true },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.queryByRole("link", { name: hintName })).toBeNull();
    await expect(canvas.queryByText("v0.1.0")).toBeNull();
  },
};

// the shell the two viewport stories below need: the drawer's open state is
// owned above the rail, exactly as `App` owns it, and the trigger stands in for
// the hamburger `ScreenHeader` grows below `md`
function Shell(props: NavSidebarProps) {
  const [open, setOpen] = React.useState(false);
  return (
    <div className="flex h-[560px] w-full">
      <NavSidebar {...props} open={open} onOpenChange={setOpen} />
      <div className="min-w-0 flex-1 p-3">
        <button type="button" onClick={() => setOpen(true)}>
          Open navigation
        </button>
      </div>
    </div>
  );
}

/**
 * #959: at 375px the rail kept its 232px and left the screen 143px. Below `md`
 * it is out of the flow entirely until the header asks for it, and it comes
 * back as a modal drawer over a scrim.
 */
export const MobileDrawer: Story = {
  ...atMobile,
  render: (args) => <Shell {...args} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // nothing of the rail is on screen, and nothing overflows without it
    await expect(canvas.queryByRole("navigation")).toBeNull();
    await expectNoHorizontalOverflow();

    await userEvent.click(canvas.getByRole("button", { name: "Open navigation" }));
    const drawer = await canvas.findByRole("dialog", { name: /navigation/i });
    // labels are readable in the drawer whatever the rail was folded to
    await waitFor(() =>
      expect(within(drawer).getByRole("button", { name: "Playground" })).toBeVisible(),
    );
    await expectNoHorizontalOverflow();

    // Escape puts it away again, like every other modal in the dashboard
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(canvas.queryByRole("dialog")).toBeNull());
  },
};

/**
 * The drawer is a modal like any sheet (#1998): the screen beside it is inert
 * while it is up, yet its scrim still takes a real click, and closing hands
 * focus back to the hamburger that opened it.
 */
export const MobileDrawerLeavesTheScreenInert: Story = {
  ...atMobile,
  render: (args) => <Shell {...args} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const trigger = canvas.getByRole("button", { name: "Open navigation" });
    await userEvent.click(trigger);
    const drawer = await canvas.findByRole("dialog", { name: /navigation/i });
    await expect(trigger.closest("[inert]")).not.toBeNull();
    await expect(drawer.closest("[inert]")).toBeNull();

    // inert hit-tests as `pointer-events: none`; the scrim shares the drawer's
    // fixed layer, so what a real pointer lands on beside the drawer is still it
    const scrim = drawer.previousElementSibling as HTMLElement;
    const view = canvasElement.ownerDocument.defaultView as Window;
    await expect(
      canvasElement.ownerDocument.elementFromPoint(view.innerWidth - 10, view.innerHeight / 2),
    ).toBe(scrim);
    await userEvent.click(scrim);
    await waitFor(() => expect(canvas.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(trigger).toHaveFocus());
    await expect(trigger.closest("[inert]")).toBeNull();
  },
};

/**
 * Between `md` and `lg` the rail is on screen but folded to icons, and the
 * splitter is gone: dragging a 52px strip wider is not the affordance that
 * width needs.
 */
export const TabletIconRail: Story = {
  ...atTablet,
  args: resizable("tablet"),
  render: (args) => <Shell {...args} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const nav = canvasElement.querySelector("nav") as HTMLElement;
    await waitFor(() => expect(nav.getBoundingClientRect().width).toBe(52));
    await expect(canvas.queryByRole("separator")).toBeNull();
    await expectNoHorizontalOverflow();
  },
};

// the experimental marker (#1386): the rail marks an individual entry in place
// — the grouping is untouched and there is no "experimental" section — so the
// story that matters is a group holding both kinds of item at once.
const EXPERIMENTAL_NOTE =
  "tool-group manifests are stored, but the proxy does not enforce group membership";

const mixedGroups = [
  {
    items: [
      { key: "playground", label: "Playground", icon: <Play /> },
      {
        key: "tool-groups",
        label: "Tool groups",
        icon: <Boxes />,
        experimental: true,
        experimentalNote: EXPERIMENTAL_NOTE,
      },
      { key: "keys", label: "Keys", icon: <KeyRound /> },
    ],
  },
];

export const ExperimentalItems: Story = {
  args: { groups: mixedGroups, activeKey: "playground" },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the badge is inside the button, so the entry names itself and its
    // stability in one accessible name
    const marked = canvas.getByRole("button", {
      name: `Tool groups ${en.shell.experimental}`,
    });
    await expect(marked).toBeVisible();
    // the note explains what is unfinished, without spending a line of a
    // 232px rail on it. the sidebar renders whatever it is handed; the shell
    // resolves it from the catalog (#1401)
    const badge = within(marked).getByText(en.shell.experimental);
    await expect(badge).toHaveAttribute("title", EXPERIMENTAL_NOTE);
    // an unmarked sibling is exactly as it was
    await expect(canvas.getByRole("button", { name: "Keys" })).toBeVisible();
    await expect(canvas.getAllByText(en.shell.experimental)).toHaveLength(1);
    await expectNoHorizontalOverflow();
  },
};

// folded there is no room for a word. The dot is decorative and the tooltip
// carries the meaning — on a button with no text content, that tooltip is also
// what a screen reader announces.
export const ExperimentalItemsCollapsed: Story = {
  args: { groups: mixedGroups, activeKey: "playground", defaultCollapsed: true },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.queryByText(en.shell.experimental)).toBeNull();
    const marked = canvas.getByRole("button", {
      name: en.shell.experimentalItem.replace("{{label}}", "Tool groups"),
    });
    await expect(marked).toBeVisible();
    // an unmarked entry still names itself and nothing more
    await expect(canvas.getByRole("button", { name: "Keys" })).toBeVisible();
    await expectNoHorizontalOverflow();
  },
};

// the narrowest the rail can be dragged is where a badge crowds a label. The
// label does not give way (#2812): the marker drops under the name, and the
// name is the whole of itself — which is the opposite of the bargain every
// plain long entry makes, and the one an entry's own name is owed
export const ExperimentalItemsNarrow: Story = {
  args: { ...resizable("experimental"), groups: mixedGroups, activeKey: "playground" },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const nav = canvasElement.querySelector("nav") as HTMLElement;
    const handle = canvas.getByRole("separator", { name: /resize/i });
    handle.focus();
    await userEvent.keyboard("{Home}");
    await expectWidth(nav, NAV_MIN_WIDTH);
    const marked = canvas.getByRole("button", { name: `Tool groups ${en.shell.experimental}` });
    await expect(marked).toBeVisible();
    await expectNotTruncated(within(marked).getByText("Tool groups"));
    await expectInFrame(within(marked).getByText(en.shell.experimental), marked);
    await expectNoHorizontalOverflow();
  },
};

// the name is the entry, so a name longer than the rail wraps rather than
// being clipped behind its marker, and the marker stays whole beneath it
export const ExperimentalItemWithALongNameWraps: Story = {
  args: {
    ...resizable("experimental-long"),
    activeKey: "playground",
    groups: [
      {
        items: [
          { key: "playground", label: "Playground", icon: <Play /> },
          {
            key: "tool-groups",
            label: "Organisation-wide tool group manifests and their access boundaries",
            icon: <Boxes />,
            experimental: true,
            experimentalNote: EXPERIMENTAL_NOTE,
          },
        ],
      },
    ],
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const marked = await canvas.findByRole("button", {
      name: /Organisation-wide tool group manifests/,
    });
    const name = within(marked).getByText(/^Organisation-wide/);
    await expectNotTruncated(name);
    const line = parseFloat(getComputedStyle(name).lineHeight);
    await expect(name.getBoundingClientRect().height).toBeGreaterThan(line * 1.5);
    await expectInFrame(within(marked).getByText(en.shell.experimental), marked);
    await expectNoHorizontalOverflow();
  },
};

// the marker sits beside the name while the two fit one line, as it always did,
// and under it when they do not; either way the entry keeps one accessible name
export const ExperimentalMarkerBesideTheNameWhenItFits: Story = {
  args: { groups: mixedGroups, activeKey: "playground" },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const marked = canvas.getByRole("button", { name: `Tool groups ${en.shell.experimental}` });
    const name = within(marked).getByText("Tool groups").getBoundingClientRect();
    const badge = within(marked).getByText(en.shell.experimental).getBoundingClientRect();
    await expect(badge.left).toBeGreaterThanOrEqual(name.right);
    await expect(
      Math.abs(badge.top + badge.height / 2 - (name.top + name.height / 2)),
    ).toBeLessThan(name.height);
  },
};

// searching for a group's own name used to expand the group and then filter
// every one of its children out of it, leaving a heading over nothing: the
// match was on the parent, and the children were re-tested against a query
// none of them contains (#1198)
export const SearchMatchesGroupLabel: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(canvas.getByRole("textbox", { name: /search/i }), "analytics");
    const parent = await canvas.findByRole("button", { name: "Analytics" });
    await expect(parent).toHaveAttribute("aria-expanded", "true");
    // the subtree the match is about, whole
    await expect(canvas.getByRole("button", { name: "Usage" })).toBeVisible();
    await expect(canvas.getByRole("button", { name: "Costs" })).toBeVisible();
    // and nothing else: this is still a filter
    await expect(canvas.queryByRole("button", { name: "Playground" })).toBeNull();
  },
};

// a query nothing matches used to empty the rail, which reads as "navigation
// broke" rather than "try another word" (#1198)
export const SearchMatchesNothing: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(canvas.getByRole("textbox", { name: /search/i }), "zzzz");
    await expect(await canvas.findByText(en.shell.noNavMatches)).toBeVisible();
    await expect(canvas.queryByRole("button", { name: "Playground" })).toBeNull();
    // clearing the box brings the whole rail back
    await userEvent.click(canvas.getByRole("button", { name: en.common.clearSearch }));
    await expect(await canvas.findByRole("button", { name: "Playground" })).toBeVisible();
    await expect(canvas.queryByText(en.shell.noNavMatches)).toBeNull();
  },
};

// the folded rail has no room to unfold a group in place, so a click on a
// group's icon used to flip state nothing drew (#2803). it opens a flyout
// beside the icon instead, listing the group's screens
const openGroup = async (canvasElement: HTMLElement, name = "Analytics") => {
  const trigger = within(canvasElement).getByRole("button", { name });
  await expect(trigger).toHaveAttribute("aria-expanded", "false");
  await userEvent.click(trigger);
  const flyout = await within(canvasElement).findByRole("group", { name });
  await waitFor(() => expect(flyout).toBeVisible());
  return { trigger, flyout };
};

export const FoldedGroupOpensAFlyout: Story = {
  args: { defaultCollapsed: true, activeKey: "costs" },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const nav = canvasElement.querySelector("nav") as HTMLElement;
    await expectWidth(nav, 52);
    await expect(canvas.queryByRole("group", { name: "Analytics" })).toBeNull();

    const { trigger, flyout } = await openGroup(canvasElement);
    await expect(trigger).toHaveAttribute("aria-expanded", "true");

    // the group's screens, with the one you are on marked as the page
    const inside = within(flyout);
    await expect(inside.getByRole("button", { name: "Usage" })).toBeVisible();
    await expect(inside.getByRole("button", { name: "Costs" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    await expect(inside.getByRole("button", { name: "Usage" })).not.toHaveAttribute("aria-current");
    // keyboard focus goes to where the reader already is
    await waitFor(() => expect(inside.getByRole("button", { name: "Costs" })).toHaveFocus());

    // the flyout sits beside the icon, over the screen, not inside the 52px rail
    const rail = nav.getBoundingClientRect();
    const box = flyout.getBoundingClientRect();
    await expect(box.left).toBeGreaterThanOrEqual(rail.right);
    await expect(Math.abs(box.top - trigger.getBoundingClientRect().top)).toBeLessThan(2);
    await expectNoHorizontalOverflow();

    // a second press on the icon puts it away again
    await userEvent.click(trigger);
    await waitFor(() => expect(canvas.queryByRole("group", { name: "Analytics" })).toBeNull());
    await expect(trigger).toHaveAttribute("aria-expanded", "false");
  },
};

// the screen itself is out of sight on the folded rail, so the group that
// holds it says "this is the section you are in" — the same thread the active
// leaf carries, and `aria-current="true"` since the group is not the page
export const FoldedGroupMarksTheCurrentSection: Story = {
  args: { defaultCollapsed: true, activeKey: "usage" },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const parent = canvas.getByRole("button", { name: "Analytics" });
    await expect(parent).toHaveAttribute("aria-current", "true");
    await expect(getComputedStyle(parent, "::before").content).not.toBe("none");
    // a group that holds nothing current, and a leaf that is not, carry neither
    const other = canvas.getByRole("button", { name: "Models" });
    await expect(other).not.toHaveAttribute("aria-current");
    await expect(getComputedStyle(other, "::before").content).toBe("none");
  },
};

export const FoldedGroupHoldingNothingCurrentIsUnmarked: Story = {
  args: { defaultCollapsed: true, activeKey: "models" },
  play: async ({ canvasElement }) => {
    const parent = within(canvasElement).getByRole("button", { name: "Analytics" });
    await expect(parent).not.toHaveAttribute("aria-current");
    await expect(getComputedStyle(parent, "::before").content).toBe("none");
    // a leaf that is current is still `page`
    await expect(within(canvasElement).getByRole("button", { name: "Models" })).toHaveAttribute(
      "aria-current",
      "page",
    );
  },
};

// a flyout a mouse can open and a keyboard cannot would be the same gap again:
// Enter, Space or an arrow on the icon opens it, arrows walk its screens and
// wrap, and Escape closes it with focus back on the icon it came from
export const FoldedGroupFlyoutFromTheKeyboard: Story = {
  args: { defaultCollapsed: true, activeKey: "models" },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const trigger = canvas.getByRole("button", { name: "Analytics" });
    trigger.focus();
    await expect(trigger).toHaveFocus();

    await userEvent.keyboard("{Enter}");
    const flyout = await canvas.findByRole("group", { name: "Analytics" });
    const inside = within(flyout);
    // nothing in this group is current, so focus starts on its first screen
    await waitFor(() => expect(inside.getByRole("button", { name: "Usage" })).toHaveFocus());

    await userEvent.keyboard("{ArrowDown}");
    await expect(inside.getByRole("button", { name: "Costs" })).toHaveFocus();
    await userEvent.keyboard("{ArrowDown}");
    await expect(inside.getByRole("button", { name: "Usage" })).toHaveFocus();
    await userEvent.keyboard("{ArrowUp}");
    await expect(inside.getByRole("button", { name: "Costs" })).toHaveFocus();
    await userEvent.keyboard("{Home}");
    await expect(inside.getByRole("button", { name: "Usage" })).toHaveFocus();
    await userEvent.keyboard("{End}");
    await expect(inside.getByRole("button", { name: "Costs" })).toHaveFocus();

    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(canvas.queryByRole("group", { name: "Analytics" })).toBeNull());
    await waitFor(() => expect(trigger).toHaveFocus());
    await expect(trigger).toHaveAttribute("aria-expanded", "false");

    // an arrow opens it too, and the left arrow takes it back
    await userEvent.keyboard("{ArrowRight}");
    await canvas.findByRole("group", { name: "Analytics" });
    await userEvent.keyboard("{ArrowLeft}");
    await waitFor(() => expect(canvas.queryByRole("group", { name: "Analytics" })).toBeNull());
    await waitFor(() => expect(trigger).toHaveFocus());
    await userEvent.keyboard("{ArrowDown}");
    await canvas.findByRole("group", { name: "Analytics" });
  },
};

// picking a screen from the flyout is a navigation like any other leaf: it
// reports the key, puts the flyout away and leaves focus on the group's icon
export const FoldedGroupFlyoutNavigates: Story = {
  args: { defaultCollapsed: true, activeKey: "models", onNavigate: fn() },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);
    const { trigger, flyout } = await openGroup(canvasElement);
    await userEvent.click(within(flyout).getByRole("button", { name: "Costs" }));
    await expect(args.onNavigate).toHaveBeenCalledTimes(1);
    await expect(args.onNavigate).toHaveBeenCalledWith("costs");
    await waitFor(() => expect(canvas.queryByRole("group", { name: "Analytics" })).toBeNull());
    await waitFor(() => expect(trigger).toHaveFocus());
  },
};

// a press anywhere else is the reader moving on: the flyout closes and focus
// stays where they put it rather than jumping back to the icon
export const FoldedGroupFlyoutClosesOnAnOutsidePress: Story = {
  args: { defaultCollapsed: true, activeKey: "models" },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await openGroup(canvasElement);
    const elsewhere = canvas.getByRole("button", { name: "Keys" });
    await userEvent.click(elsewhere);
    await waitFor(() => expect(canvas.queryByRole("group", { name: "Analytics" })).toBeNull());
    await waitFor(() => expect(elsewhere).toHaveFocus());
    await expect(canvas.getByRole("button", { name: "Analytics" })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
  },
};

// Tab runs on from the last screen into the rest of the rail, and the flyout
// does not stay open behind a focus that has left it
export const FoldedGroupFlyoutClosesWhenFocusLeaves: Story = {
  args: { defaultCollapsed: true, activeKey: "models" },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const { flyout } = await openGroup(canvasElement);
    await waitFor(() =>
      expect(within(flyout).getByRole("button", { name: "Usage" })).toHaveFocus(),
    );
    await userEvent.tab();
    await expect(within(flyout).getByRole("button", { name: "Costs" })).toHaveFocus();
    await userEvent.tab();
    await waitFor(() => expect(canvas.queryByRole("group", { name: "Analytics" })).toBeNull());
  },
};

// only one group's flyout is showing at a time, and unfolding the rail puts it
// away rather than leaving it over a rail that now shows the children itself
export const FoldedGroupFlyoutIsOneAtATime: Story = {
  args: {
    defaultCollapsed: true,
    activeKey: "playground",
    groups: [
      {
        items: [
          { key: "playground", label: "Playground", icon: <Play /> },
          {
            key: "analytics",
            label: "Analytics",
            icon: <Boxes />,
            children: [
              { key: "usage", label: "Usage" },
              { key: "costs", label: "Costs" },
            ],
          },
          {
            key: "governance",
            label: "Governance",
            icon: <KeyRound />,
            children: [{ key: "teams", label: "Teams" }],
          },
        ],
      },
    ],
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await openGroup(canvasElement, "Analytics");
    await userEvent.click(canvas.getByRole("button", { name: "Governance" }));
    await canvas.findByRole("group", { name: "Governance" });
    await waitFor(() => expect(canvas.queryByRole("group", { name: "Analytics" })).toBeNull());
    await expect(canvas.getAllByRole("group")).toHaveLength(1);

    await userEvent.click(canvas.getByRole("button", { name: en.shell.expandSidebar }));
    await waitFor(() => expect(canvas.queryByRole("group", { name: "Governance" })).toBeNull());
    // unfolded, the group's own toggle is back
    await expect(canvas.getByRole("button", { name: "Governance" })).toBeVisible();
  },
};

// experimental entries keep their marker inside the flyout, which has the room
// for the word the folded rail itself does not
export const FoldedGroupFlyoutKeepsTheExperimentalMarker: Story = {
  args: {
    defaultCollapsed: true,
    activeKey: "playground",
    groups: [
      {
        items: [
          { key: "playground", label: "Playground", icon: <Play /> },
          {
            key: "mcp",
            label: "MCP",
            icon: <Boxes />,
            children: [
              { key: "mcp-catalog", label: "Catalog" },
              {
                key: "tool-groups",
                label: "Tool groups",
                experimental: true,
                experimentalNote: EXPERIMENTAL_NOTE,
              },
            ],
          },
        ],
      },
    ],
  },
  play: async ({ canvasElement }) => {
    const { flyout } = await openGroup(canvasElement, "MCP");
    const marked = within(flyout).getByRole("button", {
      name: `Tool groups ${en.shell.experimental}`,
    });
    await expect(within(marked).getByText(en.shell.experimental)).toHaveAttribute(
      "title",
      EXPERIMENTAL_NOTE,
    );
    await expect(within(flyout).getAllByText(en.shell.experimental)).toHaveLength(1);
  },
};

// the rail's list scrolls on a short screen. the flyout is drawn over the page,
// not inside that list, so it has to follow its icon as the list moves and go
// once the icon has scrolled out of sight
const manyLeaves = Array.from({ length: 20 }, (_, i) => ({
  key: `leaf-${i}`,
  label: `Leaf ${i}`,
  icon: <Boxes />,
}));

export const FoldedGroupFlyoutFollowsTheRailScroll: Story = {
  args: {
    defaultCollapsed: true,
    activeKey: "leaf-0",
    groups: [
      {
        items: [
          ...manyLeaves,
          {
            key: "analytics",
            label: "Analytics",
            icon: <Boxes />,
            children: [
              { key: "usage", label: "Usage" },
              { key: "costs", label: "Costs" },
            ],
          },
        ],
      },
    ],
  },
  render: (args) => (
    <div className="h-[300px]">
      <NavSidebar {...args} />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const trigger = canvas.getByRole("button", { name: "Analytics" });
    const list = trigger.closest(".overflow-y-auto") as HTMLElement;
    list.scrollTop = list.scrollHeight;
    await waitFor(() => expect(list.scrollTop).toBeGreaterThan(0));
    await userEvent.click(trigger);
    const flyout = await canvas.findByRole("group", { name: "Analytics" });
    const levelWithIcon = () =>
      Math.abs(flyout.getBoundingClientRect().top - trigger.getBoundingClientRect().top);
    await waitFor(() => expect(levelWithIcon()).toBeLessThan(2));

    // a little scroll moves the icon, and the flyout stays beside it
    list.scrollTop -= 24;
    await waitFor(() => expect(levelWithIcon()).toBeLessThan(2));
    await expect(canvas.getByRole("group", { name: "Analytics" })).toBeVisible();

    // all the way up, the icon is out of the list's view and so is the flyout
    list.scrollTop = 0;
    await waitFor(() => expect(canvas.queryByRole("group", { name: "Analytics" })).toBeNull());
    await expect(trigger).toHaveAttribute("aria-expanded", "false");
  },
};

// the account card's menu (#2805): a real `menu` about the person, opening above
// the card on the full rail and beside it on the folded one. the entries are the
// shell's own; what the story pins is the menu's shape and keyboard
const accountMenu = (over: { keys?: () => void; signOut?: () => void } = {}): NavUserMenu => ({
  header: (
    <div className="border-b border-[color:var(--border-subtle)] px-3 py-2.5">
      <p className="text-sm font-medium">Anya Petrova</p>
      <p className="text-xs">anya@acme.co</p>
      <p className="text-xs">Admin · org acme</p>
    </div>
  ),
  items: (close) => (
    <>
      <MenuItem
        icon={<KeyRound />}
        onSelect={() => {
          close();
          over.keys?.();
        }}
      >
        Account &amp; keys
      </MenuItem>
      <MenuItem icon={<UserCog />} onSelect={close}>
        Preferences
      </MenuItem>
      <MenuSeparator />
      <MenuItem
        icon={<LogOut />}
        tone="danger"
        onSelect={() => {
          close();
          over.signOut?.();
        }}
      >
        Sign out
      </MenuItem>
    </>
  ),
});

const CARD = /admin@rolter\.dev/;
const USER_MENU = en.shell.userMenuLabel;

const openAccountMenu = async (canvasElement: HTMLElement, name: string | RegExp = CARD) => {
  const card = within(canvasElement).getByRole("button", { name });
  await userEvent.click(card);
  const menu = await within(canvasElement).findByRole("menu", { name: USER_MENU });
  return { card, menu };
};

export const UserMenuOpens: Story = {
  args: { userMenu: accountMenu() },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const card = canvas.getByRole("button", { name: CARD });
    await expect(card).toHaveAttribute("aria-haspopup", "menu");
    await expect(card).toHaveAttribute("aria-expanded", "false");
    await expect(canvas.queryByRole("menu")).toBeNull();

    const { menu } = await openAccountMenu(canvasElement);
    await expect(card).toHaveAttribute("aria-expanded", "true");
    // the entries, in order, and nothing else: no scope pickers, no creating
    // or deleting an org here
    const entries = within(menu).getAllByRole("menuitem");
    await expect(entries.map((el) => el.textContent)).toEqual([
      "Account & keys",
      "Preferences",
      "Sign out",
    ]);
    await expect(within(menu).getByRole("separator")).toBeInTheDocument();
    await expect(canvas.queryByRole("combobox")).toBeNull();
    // the identity block is read with the menu but is not one of its entries:
    // a `menu` may own entries, groups and separators, and nothing else
    await expect(canvas.getByText("anya@acme.co")).toBeVisible();
    await expect(within(menu).queryByText("anya@acme.co")).toBeNull();

    // above the card, as wide as the card
    const box = menu.parentElement!.getBoundingClientRect();
    const cardBox = card.getBoundingClientRect();
    await expect(box.bottom).toBeLessThanOrEqual(cardBox.top);
    await expect(Math.abs(box.width - cardBox.width)).toBeLessThan(1.5);
    await expect(Math.abs(box.left - cardBox.left)).toBeLessThan(1.5);
  },
};

export const UserMenuFromTheKeyboard: Story = {
  args: { userMenu: accountMenu() },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const card = canvas.getByRole("button", { name: CARD });
    card.focus();
    await userEvent.keyboard("{Enter}");
    const menu = within(await canvas.findByRole("menu", { name: USER_MENU }));
    const [keys, prefs, out] = menu.getAllByRole("menuitem");
    // opening puts the keyboard on the first entry
    await waitFor(() => expect(keys).toHaveFocus());
    await userEvent.keyboard("{ArrowDown}");
    await expect(prefs).toHaveFocus();
    await userEvent.keyboard("{ArrowDown}");
    await expect(out).toHaveFocus();
    // the list wraps at both ends, and Home and End are its two edges
    await userEvent.keyboard("{ArrowDown}");
    await expect(keys).toHaveFocus();
    await userEvent.keyboard("{ArrowUp}");
    await expect(out).toHaveFocus();
    await userEvent.keyboard("{Home}");
    await expect(keys).toHaveFocus();
    await userEvent.keyboard("{End}");
    await expect(out).toHaveFocus();

    // Escape closes it and gives focus back to the card
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(canvas.queryByRole("menu")).toBeNull());
    await waitFor(() => expect(card).toHaveFocus());
    await expect(card).toHaveAttribute("aria-expanded", "false");

    // Tab closes it too, rather than walking into the page behind an open menu
    await userEvent.keyboard("{Enter}");
    await canvas.findByRole("menu", { name: USER_MENU });
    await userEvent.tab();
    await waitFor(() => expect(canvas.queryByRole("menu")).toBeNull());
  },
};

const onKeys = fn();

export const UserMenuEntryClosesAndRuns: Story = {
  args: { userMenu: accountMenu({ keys: onKeys }) },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    onKeys.mockClear();
    const { card, menu } = await openAccountMenu(canvasElement);
    await userEvent.click(within(menu).getByRole("menuitem", { name: "Account & keys" }));
    await expect(onKeys).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(canvas.queryByRole("menu")).toBeNull());
    // the entry took the menu with it, and focus went to where the menu opened
    await waitFor(() => expect(card).toHaveFocus());
  },
};

export const UserMenuClosesOnAnOutsidePress: Story = {
  args: { userMenu: accountMenu() },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await openAccountMenu(canvasElement);
    await userEvent.click(canvasElement);
    await waitFor(() => expect(canvas.queryByRole("menu")).toBeNull());
    // a second press on the card puts it away too
    const card = canvas.getByRole("button", { name: CARD });
    await userEvent.click(card);
    await canvas.findByRole("menu", { name: USER_MENU });
    await userEvent.click(card);
    await waitFor(() => expect(canvas.queryByRole("menu")).toBeNull());
  },
};

/**
 * Folded, the card is the initials alone: the menu opens beside the rail with
 * its bottom edge on the card's, so it grows upward from the foot of the rail.
 */
export const UserMenuFolded: Story = {
  args: { userMenu: accountMenu(), defaultCollapsed: true },
  play: async ({ canvasElement }) => {
    const nav = canvasElement.querySelector("nav") as HTMLElement;
    await expectWidth(nav, 52);
    const { card, menu } = await openAccountMenu(canvasElement, "admin@rolter.dev");
    const box = menu.parentElement!.getBoundingClientRect();
    await expect(box.left).toBeGreaterThanOrEqual(nav.getBoundingClientRect().right);
    await expect(Math.abs(box.bottom - card.getBoundingClientRect().bottom)).toBeLessThan(2);
    await expect(within(canvasElement).getByText("anya@acme.co")).toBeVisible();
    await expect(within(menu).getAllByRole("menuitem")).toHaveLength(3);
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(card).toHaveFocus());
  },
};

/**
 * Inside the drawer the menu owns Escape while it is up: the first press puts
 * the menu away, and only the second closes the drawer.
 */
export const UserMenuInsideTheDrawerOwnsEscape: Story = {
  ...atMobile,
  args: { userMenu: accountMenu() },
  render: (args) => <Shell {...args} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "Open navigation" }));
    const drawer = await canvas.findByRole("dialog", { name: /navigation/i });
    await userEvent.click(within(drawer).getByRole("button", { name: CARD }));
    await within(drawer).findByRole("menu", { name: USER_MENU });

    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(canvas.queryByRole("menu")).toBeNull());
    await expect(canvas.getByRole("dialog", { name: /navigation/i })).toBeVisible();

    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(canvas.queryByRole("dialog")).toBeNull());
  },
};

// a card with no menu behind it is still a button that does one thing
export const UserCardWithoutAMenuRunsItsHandler: Story = {
  args: { user: { name: "admin@rolter.dev", role: "Admin", initials: "A", onClick: fn() } },
  play: async ({ canvasElement, args }) => {
    const card = within(canvasElement).getByRole("button", { name: CARD });
    await expect(card).not.toHaveAttribute("aria-haspopup");
    await userEvent.click(card);
    await expect(args.user?.onClick).toHaveBeenCalledTimes(1);
  },
};

// the slot under the brand and above the search box (#2805): the scope switcher
// sits here, on the full rail and the folded one alike, and is told which
const scopeSlot = (folded: boolean) => (
  <button type="button">{folded ? "scope icon" : "scope path"}</button>
);

const expectBefore = (first: Element, second: Element) =>
  expect(first.compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

export const HeaderSlotSitsUnderTheBrand: Story = {
  args: { headerExtra: scopeSlot },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const slot = canvas.getByRole("button", { name: "scope path" });
    await expectBefore(canvas.getByText("rolter"), slot);
    await expectBefore(slot, canvas.getByRole("textbox", { name: /search/i }));
    await expectBefore(slot, canvas.getByRole("button", { name: "Playground" }));
  },
};

export const HeaderSlotFollowsTheFold: Story = {
  args: { headerExtra: scopeSlot, defaultCollapsed: true },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("button", { name: "scope icon" })).toBeVisible();
    await expect(canvas.queryByRole("button", { name: "scope path" })).toBeNull();
  },
};
