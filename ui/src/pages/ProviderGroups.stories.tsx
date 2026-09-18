import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import ProviderGroups from "./ProviderGroups";
import {
  Harness,
  expectEmptyState,
  expectLoadError,
  expectRefused,
  expectSkeleton,
  json,
  pending,
  routes,
  scoped,
  Toasted,
  expectToast,
} from "./story-harness";
import type { LabelRow, ProviderGroupRow } from "@/lib/api";

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
    await expect(canvas.getByRole("button", { name: /Clear search/i })).toBeInTheDocument();
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
 * over a group that is still live.
 */
export const DeleteRejectedByTheServer: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        init?.method === "DELETE"
          ? json({ error: { message: "frontier is still the target of 3 routes" } }, 409)
          : loaded(input, init),
      )}
    >
      <Toasted>
        <ProviderGroups />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Delete provider group frontier" }),
    );
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.click(dialog.getByRole("button", { name: "Delete" }));

    await expectToast(canvasElement, /still the target of 3 routes/, "error");
    await waitFor(() => expect(dialog.getByText(/still the target of 3 routes/)).toBeVisible());
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
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
    await userEvent.click(canvas.getByRole("button", { name: /Clear search/i }));
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
