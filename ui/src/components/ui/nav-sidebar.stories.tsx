import { Boxes, KeyRound, Play, ScrollText } from "lucide-react";
import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { NAV_MAX_WIDTH, NAV_MIN_WIDTH, NavSidebar, type NavSidebarProps } from "./nav-sidebar";
import en from "@/lib/i18n/locales/en.json";
import { atMobile, atTablet, expectNoHorizontalOverflow } from "@/lib/story-viewport";

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
    await expect(within(drawer).getByRole("button", { name: "Playground" })).toBeVisible();
    await expectNoHorizontalOverflow();

    // Escape puts it away again, like every other modal in the dashboard
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(canvas.queryByRole("dialog")).toBeNull());
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
    // the note the build sent explains what is unfinished, without spending a
    // line of a 232px rail on it
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

// the narrowest the rail can be dragged is where a badge would crowd a label
// if it were allowed to: the marker holds its size and the label truncates,
// which is the same bargain every long entry already makes at this width
export const ExperimentalItemsNarrow: Story = {
  args: { ...resizable("experimental"), groups: mixedGroups, activeKey: "playground" },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const nav = canvasElement.querySelector("nav") as HTMLElement;
    const handle = canvas.getByRole("separator", { name: /resize/i });
    handle.focus();
    await userEvent.keyboard("{Home}");
    await expectWidth(nav, NAV_MIN_WIDTH);
    await expect(
      canvas.getByRole("button", { name: `Tool groups ${en.shell.experimental}` }),
    ).toBeVisible();
    await expectNoHorizontalOverflow();
  },
};
