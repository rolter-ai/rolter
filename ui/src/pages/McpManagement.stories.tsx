import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { McpLibrary, McpSettings, ToolGroups } from "./McpManagement";
import { GROUPS, Harness, LIBRARY, SERVERS, SETTINGS, routed } from "./mcp-story-fixtures";

import {
  cancelConfirmation,
  confirmDestructive,
  expectEmptyState,
  expectRefused,
  expectSheetClosed,
  Harness as GatedHarness,
  json,
  recording,
} from "./story-harness";

const meta = {
  title: "Screens/McpManagement",
  component: McpLibrary,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof McpLibrary>;
export default meta;
type Story = StoryObj<typeof meta>;

export const LibraryLoaded: Story = {
  render: () => (
    <Harness fetchStub={routed()}>
      <McpLibrary />
    </Harness>
  ),
};
// a curated entry now arrives with the manifest the upstream server publishes
// (#1252), which is what an install stores, what the tool tally counts, and
// what a tool group can pick from
export const LibraryShowsCuratedTools: Story = {
  render: () => (
    <Harness fetchStub={routed()}>
      <McpLibrary />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Linear")).toBeVisible());
    await expect(canvas.getByText("search_code")).toBeVisible();
    await expect(canvas.getAllByText("create_issue")).toHaveLength(2);
    await expect(canvas.queryByText("No tools declared")).not.toBeInTheDocument();
    // Notion has tools but no scopes to request — its consent screen grants
    // selected pages — so that line stays honest instead of inventing one
    await expect(canvas.getByText("notion-create-pages")).toBeVisible();
    await expect(canvas.getByText("No OAuth scopes")).toBeVisible();
  },
};
// a control plane older than #1252 still answers with empty lists; every entry
// then claimed "No tools declared" — a statement about the catalog that was
// not true of the servers behind it (#1194)
export const LibraryHidesAnEmptyToolList: Story = {
  render: () => (
    <Harness
      fetchStub={routed({
        library: async () => json(LIBRARY.map((item) => ({ ...item, tools: [] }))),
      })}
    >
      <McpLibrary />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Linear")).toBeVisible());
    await expect(canvas.queryByText("No tools declared")).not.toBeInTheDocument();
  },
};
// notion is filtered out so exactly one entry is uninstalled and "Install"
// names a single button
export const LibraryInstallsServer: Story = {
  render: () => {
    let installed = false;
    const stub = routed({
      library: async () =>
        json(
          LIBRARY.filter((item) => item.slug !== "notion").map((item) =>
            item.slug === "linear" ? { ...item, installed } : item,
          ),
        ),
      servers: async (_input, init) => {
        if (init?.method === "POST") {
          installed = true;
          return json({ ...SERVERS[0], id: "server-linear", name: "Linear", slug: "linear" });
        }
        return json(SERVERS);
      },
    });
    return (
      <Harness fetchStub={stub}>
        <McpLibrary />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Install" }));
    await waitFor(() =>
      expect(canvas.getAllByRole("button", { name: "Installed" })).toHaveLength(2),
    );
  },
};

export const ToolGroupsLoaded: Story = {
  render: () => (
    <Harness fetchStub={routed()}>
      <ToolGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Triage")).toBeVisible());
    await expect(canvas.getByText("GitHub/search_code")).toBeVisible();
  },
};
export const ToolGroupsEmpty: Story = {
  render: () => (
    <Harness fetchStub={routed({ groups: async () => json([]) })}>
      <ToolGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No tool groups/, /Create group/);
  },
};
export const ToolGroupSelectsTools: Story = {
  render: () => (
    <Harness fetchStub={routed({ groups: async () => json([]) })}>
      <ToolGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const create = await canvas.findByRole("button", { name: "Create group" });
    await waitFor(() => expect(create).toBeEnabled());
    await userEvent.click(create);
    const body = within(document.body);
    await userEvent.type(body.getByLabelText("Name"), "Builders");
    await userEvent.click(body.getByRole("button", { name: "create_issue" }));
    await expect(body.getByRole("button", { name: "create_issue" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(body.getByRole("button", { name: "Save group" })).toBeEnabled();
  },
};

// the group delete was the last window.confirm on this screen, and it could not
// even name the group it was about (#1179)
//
// the list shrinks once the DELETE lands, so the story can assert the outcome
// — the dialog closed, the card gone — rather than that the request left. The
// tool-group delete pushes no toast, so the empty state is what it leaves
// behind. A stub that answers the full list forever passes either way, which
// is how a 204 fixture that threw went unnoticed (#1260)
let groupDeleted = false;
const groupDeletes = recording(
  routed({
    groups: async (_input, init) => {
      if (init?.method === "DELETE") {
        groupDeleted = true;
        return json({}, 204);
      }
      return json(groupDeleted ? [] : GROUPS);
    },
  }),
);
export const ToolGroupConfirmsBeforeDeleting: Story = {
  render: () => {
    groupDeleted = false;
    return (
      <Harness fetchStub={groupDeletes.stub}>
        <ToolGroups />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Triage")).toBeVisible());
    // by name, not by position: the row control names the group (#1214)
    const del = () => canvas.getByRole("button", { name: "Delete tool group Triage" });

    await userEvent.click(del());
    await cancelConfirmation();
    groupDeletes.expectNotSent("DELETE", "/mcp/tool-groups/group-1");

    await userEvent.click(del());
    await confirmDestructive(/Triage/, "Delete");
    await groupDeletes.expectSent("DELETE", "/mcp/tool-groups/group-1");

    // the outcome, not just the request: the confirmation closes and the card
    // is gone, leaving the screen on its empty state
    await expectSheetClosed();
    await waitFor(() => expect(canvas.queryByText("Triage")).not.toBeInTheDocument());
    await expectEmptyState(canvasElement, /No tool groups/);
  },
};

export const SettingsLoaded: Story = {
  render: () => (
    <Harness fetchStub={routed()}>
      <McpSettings />
    </Harness>
  ),
};
export const SettingsSavesChanges: Story = {
  render: () => {
    let saved = SETTINGS;
    const stub = routed({
      settings: async (_input, init) => {
        if (init?.method === "PUT")
          saved = {
            ...SETTINGS,
            ...JSON.parse(String(init.body)),
            updated_at: "2026-08-02T00:00:00Z",
          };
        return json(saved);
      },
    });
    return (
      <Harness fetchStub={stub}>
        <McpSettings />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const retries = await canvas.findByLabelText("Maximum retries");
    await userEvent.clear(retries);
    await userEvent.type(retries, "3");
    await userEvent.click(canvas.getByRole("button", { name: "Save MCP settings" }));
    await waitFor(() => expect(retries).toHaveValue(3));
  },
};

// a tool group is its own resource, so the catalog's gates say nothing about it
export const ToolGroupsGatedForMember: Story = {
  render: () => (
    <GatedHarness role="member" fetchStub={routed()}>
      <ToolGroups />
    </GatedHarness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Delete tool group Triage");
    await expectRefused(canvasElement, "Configure tool group Triage");
  },
};
