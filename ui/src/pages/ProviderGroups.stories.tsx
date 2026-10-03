import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import ProviderGroups from "./ProviderGroups";
import {
  Harness,
  adminOfProject,
  cancelConfirmation,
  confirmDestructive,
  expectEmptyState,
  expectLoadError,
  expectAllowed,
  expectNoUxEvent,
  expectRefused,
  expectSheetClosed,
  expectListTable,
  expectNoFalseEmpty,
  expectSkeleton,
  expectUxEvent,
  json,
  pending,
  recordUxEvents,
  recording,
  routes,
  scoped,
  Toasted,
  expectToast,
  uxEvents,
  type Recorder,
} from "./story-harness";
import type { LabelRow, ProviderGroupRow } from "@/lib/api";
import { UxScreenProvider } from "@/lib/ux-react";

const GROUPS: ProviderGroupRow[] = [
  {
    id: "g-1",
    org_id: "org-1",
    name: "frontier",
    slug: "frontier",
    strategy: "least_load",
    created_at: "2026-03-01T00:00:00Z",
    members: [
      { group_id: "g-1", provider_id: "p-1", provider_name: "openai-prod", weight: 3, position: 0 },
      {
        group_id: "g-1",
        provider_id: "p-2",
        provider_name: "anthropic-eu",
        weight: 1,
        position: 1,
      },
    ],
  },
];

const loaded = routes([
  ["/provider-groups", () => GROUPS],
  ["/providers", () => []],
]);

const meta = {
  title: "Screens/ProviderGroups",
  component: ProviderGroups,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof ProviderGroups>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <ProviderGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("frontier")).toBeVisible());
    await expect(canvas.getByText("openai-prod ·3")).toBeVisible();
    await expectListTable(canvasElement, "Provider Groups");
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <ProviderGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No provider groups yet/);
  },
};

/**
 * The bug #1180 names outright: with no search running the screen still said
 * "No provider groups match.", blaming a filter the operator never set. The
 * copy now depends on whether a query is active, and only the search-driven
 * branch offers to clear one.
 */
export const Empty: Story = {
  render: () => (
    <Harness fetchStub={routes([["/provider-groups", () => []]])}>
      <ProviderGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectEmptyState(canvasElement, /No provider groups yet/, /Add group/);
    await expect(canvas.queryByText(/No provider groups match/)).not.toBeInTheDocument();
  },
};

export const NoSearchMatch: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <ProviderGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("frontier")).toBeVisible());
    await userEvent.type(canvas.getByLabelText("Search provider groups"), "zzz");
    await waitFor(() => expect(canvas.getByText(/No provider groups match/)).toBeVisible());
    await expect(canvas.getByRole("button", { name: /Clear filters/i })).toBeInTheDocument();
  },
};

export const Error_: Story = {
  name: "Error",
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "boom" } }, 500))}>
      <ProviderGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return provider groups/i);
    await expectNoFalseEmpty(canvasElement, /No provider groups yet/);
  },
};

export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "forbidden" } }, 403))}>
      <ProviderGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /You do not have access to provider groups/);
    await expectNoFalseEmpty(canvasElement, /No provider groups yet/);
  },
};

// `provider_group` is admin at every action (#1606). The delete is a bare
// button that reads `deleteGate` itself, so it is the one most able to drift
// away from the `GatedButton` above it.
export const RefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="viewer">
      <ProviderGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, /add group/i);
    await expectRefused(canvasElement, "Edit provider group frontier");
    await expectRefused(canvasElement, "Delete provider group frontier");
  },
};

export const RefusedToAMember: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="member">
      <ProviderGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, /add group/i);
    await expectRefused(canvasElement, "Delete provider group frontier");
  },
};

/**
 * The delete is refused (#1607).
 *
 * A group is what routes fan out through, so one that is still referenced
 * cannot go — and the dialog has to stay open saying why rather than closing
 * over a group that is still live. The stub refuses in the same tick, which is
 * the case `ConfirmDialog` used to miss (#1761): the refusal is a row of its
 * own beside the press.
 */
export const DeleteRejectedByTheServer: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        init?.method === "DELETE"
          ? json({ error: { message: "frontier is still the target of 3 routes" } }, 409)
          : loaded(input, init),
      )}
    >
      <UxScreenProvider screen="provider-groups">
        <Toasted>
          <ProviderGroups />
        </Toasted>
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Delete provider group frontier" }),
    );
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.click(dialog.getByRole("button", { name: "Delete group" }));
    // read before the toast waits, so the queue's flush timer cannot drain them
    await waitFor(() =>
      expect(
        uxEvents()
          .filter((e) => e.action === "form_submit" && e.target === "provider-group-delete")
          .map((e) => e.outcome),
      ).toEqual(["ok", "error"]),
    );
    expectNoUxEvent("save_confirmed", "provider-group-delete");

    await expectToast(canvasElement, /still the target of 3 routes/, "error");
    await waitFor(() => expect(dialog.getByText(/still the target of 3 routes/)).toBeVisible());
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
  },
};

/**
 * The delete confirms through the shared `ConfirmDialog` (#1760): the title
 * names the group, a cancel sends nothing and is an abandon, and a confirm
 * sends the DELETE and is a submit that lands as `save_confirmed`.
 */
let groupDeletes: Recorder;
export const DeleteIsConfirmedAndReported: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    groupDeletes = recording(
      scoped(async (input, init) =>
        init?.method === "DELETE" ? json(null, 204) : loaded(input, init),
      ),
    );
    return (
      <Harness fetchStub={groupDeletes.stub}>
        <UxScreenProvider screen="provider-groups">
          <ProviderGroups />
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const remove = await canvas.findByRole("button", { name: "Delete provider group frontier" });
    await userEvent.click(remove);
    await expect(
      await within(document.body).findByRole("heading", {
        name: "Delete provider group frontier?",
      }),
    ).toBeInTheDocument();
    await cancelConfirmation();
    groupDeletes.expectNotSent("DELETE", "/provider-groups/");
    const abandon = await expectUxEvent("form_abandon", "provider-group-delete");
    await expect(abandon.screen).toBe("provider-groups");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "provider-group-delete");

    await userEvent.click(remove);
    // the body carries the address the group stops answering on
    await confirmDestructive(/frontier\/model/, "Delete group");
    await groupDeletes.expectSent("DELETE", "/provider-groups/g-1");
    await expectSheetClosed();
    const submit = await expectUxEvent("form_submit", "provider-group-delete");
    await expect(submit.outcome).toBe("ok");
    await expectUxEvent("save_confirmed", "provider-group-delete");
  },
};

// ------------------------------------------------------------- labels (#1329)

const GROUP_LABELS: LabelRow[] = [
  {
    id: "gl-1",
    subject_type: "provider_group",
    subject_id: "g-1",
    key: "tier",
    value: "frontier",
    source: "custom",
    created_at: "2026-03-01T00:00:00Z",
    updated_at: "2026-03-01T00:00:00Z",
  },
  {
    id: "gl-2",
    subject_type: "provider_group",
    subject_id: "g-1",
    key: "tier",
    value: "observed-frontier",
    source: "auto",
    observed_at: "2026-03-02T10:00:00Z",
    observation: "every member priced in the last day",
    created_at: "2026-03-02T10:00:00Z",
    updated_at: "2026-03-02T10:00:00Z",
  },
];

const withLabels = scoped(async (input) => {
  const url = new URL(String(input), "http://localhost");
  if (url.pathname.endsWith("/labels")) {
    const subject = url.searchParams.get("subject_id");
    return json(subject ? GROUP_LABELS.filter((l) => l.subject_id === subject) : GROUP_LABELS);
  }
  if (url.pathname.endsWith("/provider-groups")) return json(GROUPS);
  return json([]);
});

/** both sources on one key, told apart by name rather than by colour */
export const Labelled: Story = {
  render: () => (
    <Harness fetchStub={withLabels}>
      <ProviderGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByLabelText("tier=frontier, your label")).toBeVisible());
    await expect(canvas.getByLabelText("tier=observed-frontier, automatic label")).toBeVisible();
  },
};

// a label on a group that is not in the list — the shape a label left behind by
// a deleted group, or one on a group another filter is hiding, actually has
const ORPHAN_LABEL: LabelRow[] = [
  { ...GROUP_LABELS[0], id: "gl-9", subject_id: "g-missing", value: "legacy" },
];

/** a label filter that matches nothing blames the filter, not an empty org */
export const NoLabelMatch: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = new URL(String(input), "http://localhost");
        if (url.pathname.endsWith("/labels")) return json(ORPHAN_LABEL);
        if (url.pathname.endsWith("/provider-groups")) return json(GROUPS);
        return json([]);
      })}
    >
      <ProviderGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("frontier").length).toBeGreaterThan(0));
    await userEvent.click(canvas.getByRole("combobox", { name: /Filter by label/i }));
    await userEvent.click(
      await within(document.body).findByRole("option", { name: "tier=legacy" }),
    );
    await waitFor(() => expect(canvas.getByText(/No provider groups match/i)).toBeVisible());
    await userEvent.click(canvas.getByRole("button", { name: /Clear filters/i }));
    // both narrowings went, so the group is back
    await waitFor(() => expect(canvas.getAllByText("frontier").length).toBeGreaterThan(0));
  },
};

/** the panel opens on the group it names, and the observation is read-only */
export const LabelPanel: Story = {
  render: () => (
    <Harness fetchStub={withLabels}>
      <ProviderGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("frontier").length).toBeGreaterThan(0));
    await userEvent.click(canvas.getByRole("button", { name: "Labels on frontier" }));
    const panel = within(await within(document.body).findByRole("dialog"));
    await expect(await panel.findByText(/every member priced/)).toBeVisible();
    await expect(panel.getByRole("button", { name: "Remove tier=frontier" })).toBeVisible();
    await expect(panel.queryByRole("button", { name: "Remove tier=observed-frontier" })).toBeNull();
  },
};

/** A group scoped to a project says so, and an org-wide one says it is org-wide (#1919). */
export const ShowsWhichProjectEachGroupIsScopedTo: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        [
          "/provider-groups",
          () => [
            { ...GROUPS[0], project_id: "project-1" },
            { ...GROUPS[0], id: "g-2", name: "shared", slug: "shared", project_id: null },
          ],
        ],
        ["/providers", () => []],
      ])}
    >
      <ProviderGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("Project: Gateway")).toBeVisible();
    await expect(canvas.getByText("Organization-wide")).toBeVisible();
    const row = canvas.getByText("frontier").closest('[role="row"]') as HTMLElement;
    await expect(within(row).getByText("Project: Gateway")).toBeVisible();
    await expectListTable(canvasElement, "Provider Groups");
  },
};

// the same mixed list as the Providers screen (#2522): a project admin may edit
// the group scoped to their project and is refused the org-wide one
export const ProjectAdminOnAMixedList: Story = {
  render: () => (
    <Harness
      role={adminOfProject("project-1")}
      fetchStub={routes([
        [
          "/provider-groups",
          () => [
            { ...GROUPS[0], project_id: "project-1" },
            { ...GROUPS[0], id: "g-2", name: "fallback", slug: "fallback", project_id: null },
          ],
        ],
        ["/providers", () => []],
      ])}
    >
      <ProviderGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, "Edit provider group frontier");
    await expectAllowed(canvasElement, "Delete provider group frontier");
    await expectRefused(canvasElement, "Edit provider group fallback");
    await expectRefused(canvasElement, "Delete provider group fallback");
  },
};
