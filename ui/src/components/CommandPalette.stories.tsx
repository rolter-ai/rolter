import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { CommandPalette } from "./CommandPalette";
import en from "@/lib/i18n/locales/en.json";
import type { NavDef } from "@/lib/nav";
import {
  Harness,
  expectEmptyState,
  expectLoadError,
  expectSkeleton,
  json,
  pending,
  routes,
  scopeResponse,
  type FetchStub,
} from "@/pages/story-harness";

const nav = en.nav as Record<string, string>;
const palette = en.shell.palette;

// a slice of the real IA rather than invented labels: the palette reads its
// copy out of the catalog under `nav.<key>`, so a fixture with labels of its
// own would be testing a screen list that does not exist
const NAV: NavDef[] = [
  { key: "playground", icon: null },
  {
    key: "observability",
    icon: null,
    children: [
      { key: "dashboard", icon: null },
      { key: "logs", icon: null },
    ],
  },
  {
    key: "models",
    icon: null,
    children: [
      { key: "providers", icon: null },
      { key: "routing-rules", icon: null },
    ],
  },
  {
    key: "governance",
    icon: null,
    children: [{ key: "virtual-keys", icon: null }],
  },
];

const KEY = {
  id: "vk-1",
  project_id: "project-1",
  key_hash: "hash",
  key_prefix: "rk_live_ac",
  name: "acme-production",
  models: [],
  providers: [],
  disabled: false,
  created_by: null,
  business_unit_id: null,
  customer_id: null,
  created_at: "2026-01-01T00:00:00Z",
};

const PROVIDER = {
  id: "prov-1",
  org_id: "org-1",
  name: "OpenAI EU",
  slug: "openai-eu",
  kind: "openai",
  api_base: "https://api.openai.com/v1",
  egress_proxies: [],
  created_at: "2026-01-01T00:00:00Z",
};

const ROUTE = {
  id: "route-1",
  project_id: "project-1",
  model: "gpt-4o-mini",
  strategy: "round-robin",
  enabled: true,
  params: {},
  param_policy: {},
  advanced: {},
  created_at: "2026-01-01T00:00:00Z",
};

const records: FetchStub = routes([
  ["/virtual-keys", () => [KEY]],
  ["/providers", () => [PROVIDER]],
  ["/routes", () => [ROUTE]],
]);

/** every record list answers 500, which is the palette's error state */
const brokenRecords: FetchStub = async (input) =>
  scopeResponse(String(input)) ?? json({ error: "boom" }, 500);

/** The palette with somewhere to report the screen it opened. */
function Palette({
  fetchStub = records,
  startOpen = true,
  recent = ["logs", "providers"],
}: {
  fetchStub?: FetchStub;
  startOpen?: boolean;
  recent?: string[];
}) {
  const [open, setOpen] = React.useState(startOpen);
  const [went, setWent] = React.useState("none");
  return (
    <Harness fetchStub={fetchStub}>
      <button type="button" onClick={() => setOpen(true)}>
        open the palette
      </button>
      <p data-testid="went">{went}</p>
      <CommandPalette
        open={open}
        onOpenChange={setOpen}
        nav={NAV}
        recent={recent}
        onNavigate={setWent}
      />
    </Harness>
  );
}

// every story renders through `Palette` below, which owns the open state and
// the harness; the args here only satisfy the component's required props
const meta = {
  title: "Shell/CommandPalette",
  component: CommandPalette,
  args: { open: true, onOpenChange: () => {}, nav: NAV, onNavigate: () => {} },
  parameters: { layout: "centered" },
} satisfies Meta<typeof CommandPalette>;
export default meta;
type Story = StoryObj<typeof meta>;

/** the palette portals to the body, so the canvas is not where it lands */
const body = () => within(document.body);

const input = async () =>
  body().findByRole("combobox", { name: palette.label });

const options = () => body().getAllByRole("option");

const selected = async (name: string) =>
  waitFor(() => {
    const option = body().getByRole("option", { name: new RegExp(name, "i") });
    expect(option).toHaveAttribute("aria-selected", "true");
  });

/**
 * Opened with no query: the screens visited last, then every screen the caller
 * may see. Focus is in the field, which is the whole point of a palette — it
 * opens ready to be typed into.
 */
export const Default: Story = {
  render: () => <Palette />,
  play: async () => {
    const field = await input();
    await expect(field).toHaveFocus();
    // the listbox the field drives, named the same way
    await expect(body().getByRole("listbox", { name: palette.label })).toBeVisible();
    // recents come first and in the order they were visited
    const recent = body().getByRole("group", { name: palette.sections.recent });
    const names = within(recent)
      .getAllByRole("option")
      .map((o) => o.textContent);
    await expect(names[0]).toContain(nav.logs);
    await expect(names[1]).toContain(nav.providers);
    // and every leaf is offered below them, hinted with the group it sits in
    const screens = body().getByRole("group", { name: palette.sections.screens });
    await expect(
      within(screens).getByRole("option", { name: new RegExp(nav["routing-rules"], "i") }),
    ).toBeVisible();
    // the first entry is selected already, so Enter opens something
    await expect(options()[0]).toHaveAttribute("aria-selected", "true");
  },
};

/**
 * A half-remembered name still lands: "rr" is not a substring of anything, and
 * it reaches Routing Rules ahead of the screens that merely contain both
 * letters.
 */
export const FuzzyQuery: Story = {
  render: () => <Palette />,
  play: async () => {
    await userEvent.type(await input(), "rr");
    await waitFor(() =>
      expect(options()[0].textContent).toContain(nav["routing-rules"]),
    );
    await selected(nav["routing-rules"]);
    // and the entries that do not match are gone, not merely ranked lower
    await expect(
      body().queryByRole("option", { name: new RegExp(nav.playground, "i") }),
    ).toBeNull();
  },
};

/**
 * The arrow keys move the selection while focus stays in the field — the
 * `aria-activedescendant` pattern — and Enter opens what is selected.
 */
export const KeyboardSelection: Story = {
  render: () => <Palette recent={[]} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const field = await input();
    const first = options()[0];
    await expect(field).toHaveAttribute("aria-activedescendant", first.id);

    await userEvent.keyboard("{ArrowDown}");
    const second = options()[1];
    await waitFor(() =>
      expect(field).toHaveAttribute("aria-activedescendant", second.id),
    );
    await expect(second).toHaveAttribute("aria-selected", "true");
    await expect(first).toHaveAttribute("aria-selected", "false");
    // focus never left the field it is being typed into
    await expect(field).toHaveFocus();

    // ArrowUp comes back, and Enter opens the entry that is selected
    await userEvent.keyboard("{ArrowUp}{Enter}");
    await waitFor(() =>
      expect(canvas.getByTestId("went")).toHaveTextContent("playground"),
    );
    // opening a screen closes the palette behind it
    await waitFor(() => expect(document.body.querySelector('[role="listbox"]')).toBeNull());
  },
};

/**
 * Escape closes the palette and hands focus back to whatever opened it, so a
 * keyboard user is never left on an element behind a dismissed dialog.
 */
export const EscapeCloses: Story = {
  render: () => <Palette startOpen={false} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const opener = canvas.getByRole("button", { name: /open the palette/i });
    await userEvent.click(opener);
    await expect(await input()).toHaveFocus();

    await userEvent.keyboard("{Escape}");
    await waitFor(() =>
      expect(body().queryByRole("listbox", { name: palette.label })).toBeNull(),
    );
    await waitFor(() => expect(opener).toHaveFocus());
  },
};

/**
 * Records, not only screens: a virtual key found by its name, labelled with
 * what kind of record it is, opening the screen that lists it.
 */
export const RecordsFound: Story = {
  render: () => <Palette />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await input(), "acme");
    const group = await body().findByRole("group", { name: palette.sections.records });
    const hit = await within(group).findByRole("option", { name: /acme-production/i });
    await expect(hit).toHaveTextContent(palette.kinds.virtualKey);
    await userEvent.click(hit);
    await waitFor(() =>
      expect(canvas.getByTestId("went")).toHaveTextContent("virtual-keys"),
    );
  },
};

/**
 * The record lists are three requests, so the section stands in a skeleton
 * while they are out rather than claiming there is nothing to find. The query
 * matches no screen, so the skeleton is the only thing the section can show.
 */
export const RecordsLoading: Story = {
  render: () => <Palette fetchStub={pending} />,
  play: async () => {
    await userEvent.type(await input(), "acme");
    await body().findByRole("group", { name: palette.sections.records });
    await expectSkeleton(document.body);
  },
};

/** A record list that failed says so, in the palette, with a retry. */
export const RecordsFailed: Story = {
  render: () => <Palette fetchStub={brokenRecords} />,
  play: async () => {
    await userEvent.type(await input(), "acme");
    await expectLoadError(document.body, new RegExp(en.errors.resources.paletteRecords));
  },
};

/** Nothing matches: the palette says so, and quotes what was typed. */
export const NoMatches: Story = {
  render: () => <Palette />,
  play: async () => {
    await userEvent.type(await input(), "zzzz");
    await expectEmptyState(document.body, new RegExp(palette.noMatches));
    await expect(body().getByText(/zzzz/)).toBeVisible();
    await expect(body().queryAllByRole("option")).toHaveLength(0);
  },
};
