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
  screens = NAV,
}: {
  fetchStub?: FetchStub;
  startOpen?: boolean;
  recent?: string[];
  screens?: NavDef[];
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
        nav={screens}
        recent={recent}
        onNavigate={(screen, search) => setWent(`${screen}${search ?? ""}`)}
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

const input = async () => body().findByRole("combobox", { name: palette.label });

/**
 * The dialog hands focus to the field in an effect, one frame after the field
 * itself is in the document, so a story that read the field and acted on it
 * straight away typed into the body instead. Every story that drives the
 * palette from the keyboard waits here first.
 */
const focused = async () => {
  const field = await input();
  await waitFor(() => expect(field).toHaveFocus());
  return field;
};

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
    // it opens ready to be typed into, which is the whole point of a palette.
    // `focused()` is the assertion — it waits for the field to take focus, which
    // the dialog hands over in an effect a step after the field is mounted
    await focused();
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
    await waitFor(() => expect(options()[0].textContent).toContain(nav["routing-rules"]));
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
    const field = await focused();
    const first = options()[0];
    await expect(field).toHaveAttribute("aria-activedescendant", first.id);

    await userEvent.keyboard("{ArrowDown}");
    const second = options()[1];
    await waitFor(() => expect(field).toHaveAttribute("aria-activedescendant", second.id));
    await expect(second).toHaveAttribute("aria-selected", "true");
    await expect(first).toHaveAttribute("aria-selected", "false");
    // focus never left the field it is being typed into
    await expect(field).toHaveFocus();

    // ArrowUp comes back, and Enter opens the entry that is selected
    await userEvent.keyboard("{ArrowUp}{Enter}");
    await waitFor(() => expect(canvas.getByTestId("went")).toHaveTextContent("playground"));
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
    await expect(await focused()).toHaveFocus();

    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(body().queryByRole("listbox", { name: palette.label })).toBeNull());
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
    await waitFor(() => expect(canvas.getByTestId("went")).toHaveTextContent("virtual-keys"));
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

const REQUEST_ID = "3f2c9a1e-7b4d-4f10-9c2e-0a1b2c3d4e5f";
const TRACE_ID = "0af7651916cd43dd8448eb211c80319c";

/**
 * A request id pasted into the field is offered as a lookup in LLM Logs (#1861):
 * one entry, selected, naming the id and saying what kind it is. Enter opens
 * the screen on it, through the address the screen reads its lookup from, and
 * the palette closes behind it.
 */
export const APastedRequestIdOffersLogs: Story = {
  render: () => <Palette />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await focused();
    await userEvent.paste(`${REQUEST_ID}\n`);

    const group = await body().findByRole("group", { name: palette.sections.lookup });
    const offer = await within(group).findByRole("option", {
      name: new RegExp(palette.openRequest.replace("{{id}}", REQUEST_ID)),
    });
    await expect(offer).toHaveTextContent(palette.kinds.requestId);
    await expect(offer).toHaveAttribute("aria-selected", "true");
    // the live region counts the offer as the one result
    await waitFor(() => expect(body().getByText("1 result")).toBeInTheDocument());

    await userEvent.keyboard("{Enter}");
    await waitFor(() =>
      expect(canvas.getByTestId("went")).toHaveTextContent(`logs?request_id=${REQUEST_ID}`),
    );
    await waitFor(() => expect(document.body.querySelector('[role="listbox"]')).toBeNull());
  },
};

/**
 * A `traceparent` header pasted whole is offered as its trace id, with its own
 * wording, and opens the screen on the trace rather than on a request.
 */
export const APastedTraceparentOffersTheTrace: Story = {
  render: () => <Palette />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await focused();
    await userEvent.paste(`00-${TRACE_ID}-b7ad6b7169203331-01`);

    const offer = await body().findByRole("option", {
      name: new RegExp(palette.openTrace.replace("{{id}}", TRACE_ID)),
    });
    await expect(offer).toHaveTextContent(palette.kinds.traceId);
    await userEvent.click(offer);
    await waitFor(() =>
      expect(canvas.getByTestId("went")).toHaveTextContent(`logs?trace_id=${TRACE_ID}`),
    );
  },
};

/**
 * The offer is for an id, never for a name. A route called `gpt-4o-mini` has a
 * digit and eleven characters, like an id does, and Enter on it still opens
 * Routing Rules. A word with no digit, and a word too short to be an id, are
 * searched as names and offered nothing.
 */
export const ANameIsNotOfferedAsAnId: Story = {
  render: () => <Palette />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const field = await focused();
    await userEvent.type(field, "gpt-4o-mini");
    await body().findByRole("option", { name: /gpt-4o-mini/ });
    await expect(body().queryByRole("group", { name: palette.sections.lookup })).toBeNull();
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(canvas.getByTestId("went")).toHaveTextContent("routing-rules"));

    await userEvent.click(canvas.getByRole("button", { name: /open the palette/i }));
    const again = await focused();
    await userEvent.type(again, "zzzzzzzzzz");
    await expectEmptyState(document.body, new RegExp(palette.noMatches));
    await expect(body().queryByRole("group", { name: palette.sections.lookup })).toBeNull();
  },
};

/**
 * A caller whose nav has no LLM Logs is not offered the lookup: it would open a
 * screen they cannot see, and the palette does not list a dead end.
 */
export const NoLookupWithoutTheLogsScreen: Story = {
  render: () => <Palette screens={NAV.filter((entry) => entry.key !== "observability")} />,
  play: async () => {
    await focused();
    await userEvent.paste(REQUEST_ID);
    await expectEmptyState(document.body, new RegExp(palette.noMatches));
    await expect(body().queryByRole("option")).toBeNull();
  },
};

/**
 * The record lists may still hold the name an id-shaped word is, so a request id
 * waits for them: while they load the section shows its skeleton and nothing is
 * offered ahead of a record that may be about to match. A trace id cannot be a
 * name, so it is offered at once.
 */
export const ALoadingRecordListHoldsTheRequestOfferBack: Story = {
  render: () => <Palette fetchStub={pending} />,
  play: async () => {
    const field = await focused();
    await userEvent.paste(REQUEST_ID);
    await body().findByRole("group", { name: palette.sections.records });
    await expectSkeleton(document.body);
    await expect(body().queryByRole("group", { name: palette.sections.lookup })).toBeNull();

    await userEvent.clear(field);
    await userEvent.paste(TRACE_ID);
    await body().findByRole("option", {
      name: new RegExp(palette.openTrace.replace("{{id}}", TRACE_ID)),
    });
  },
};

/** every answer arrives a quarter of a second late, so the scope resolves after the paste */
const slowRecords: FetchStub = async (input, init) => {
  await new Promise((resolve) => setTimeout(resolve, 250));
  return records(input, init);
};

/**
 * The scope that enables the record lists resolves after the palette is already
 * open, so for a moment nothing is fetching. A pasted request id must not be
 * offered in that gap only to vanish when the lists start loading, nor read as
 * "no screens match": the palette says it is waiting, then offers the id once
 * and keeps it.
 */
export const TheRequestOfferDoesNotFlickerWhileTheScopeResolves: Story = {
  render: () => <Palette fetchStub={slowRecords} />,
  play: async () => {
    await focused();
    await userEvent.paste(REQUEST_ID);
    await expect(body().queryByRole("option")).toBeNull();
    await expect(body().queryByText(palette.noMatches)).toBeNull();

    const offer = await body().findByRole(
      "option",
      { name: new RegExp(palette.openRequest.replace("{{id}}", REQUEST_ID)) },
      { timeout: 6000 },
    );
    // it stays: nothing starts fetching behind it and takes it away
    await new Promise((resolve) => setTimeout(resolve, 700));
    await expect(offer).toBeInTheDocument();
    await expect(body().queryByText(palette.noMatches)).toBeNull();
    await expect(body().queryByRole("status", { busy: true })).toBeNull();
  },
};
