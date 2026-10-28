import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { McpLibrary, McpSettings, ToolGroups } from "./McpManagement";
import {
  GROUPS,
  Harness,
  LIBRARY,
  SERVERS,
  SETTINGS,
  Toasts,
  routed,
  urlOf,
} from "./mcp-story-fixtures";

import {
  cancelConfirmation,
  confirmDestructive,
  expectAllowed,
  expectEmptyState,
  expectLoadError,
  expectNoFalseEmpty,
  expectRefused,
  expectSheetClosed,
  expectSkeleton,
  Harness as GatedHarness,
  json,
  recording,
} from "./story-harness";

// no organization to read from: the screen says so instead of sitting on a
// skeleton or an empty list
const noOrg = () => async (input: RequestInfo | URL, init?: RequestInit) =>
  /\/orgs(\?|$)/.test(urlOf(input)) ? json([]) : routed()(input, init);

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
        <Toasts>
          <McpLibrary />
        </Toasts>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Install Linear" }));
    // an installed entry is a status, not a disabled button, and it points on
    // to Configure; the toast says the same where the click happened (#2173)
    await waitFor(() => expect(canvas.getAllByText("Installed")).toHaveLength(2));
    await expect(canvas.queryByRole("button", { name: /^Installed$/ })).not.toBeInTheDocument();
    await expect(canvas.getAllByRole("link", { name: "Configure" })[0]).toHaveAttribute(
      "href",
      "/mcp-catalog",
    );
    await waitFor(() => expect(within(document.body).getByText("Linear installed")).toBeVisible());
    await expect(within(document.body).getByText(/choose Configure/)).toBeVisible();
  },
};
// a refused install used to leave the button as it was and say nothing
export const LibraryInstallFails: Story = {
  render: () => (
    <Harness
      fetchStub={routed({
        servers: async (_input, init) =>
          init?.method === "POST"
            ? json({ error: { message: "slug linear is already registered" } }, 409)
            : json(SERVERS),
      })}
    >
      <Toasts>
        <McpLibrary />
      </Toasts>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const install = await canvas.findByRole("button", { name: "Install Linear" });
    await userEvent.click(install);
    const body = within(document.body);
    await waitFor(() => expect(body.getByText("Could not install Linear")).toBeVisible());
    await expect(body.getByText(/slug linear is already registered/)).toBeVisible();
    await waitFor(() => expect(install).toBeEnabled());
  },
};
export const LibraryLoading: Story = {
  render: () => (
    <Harness fetchStub={routed({ library: () => new Promise<Response>(() => {}) })}>
      <McpLibrary />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /library is empty/);
  },
};
export const LibraryEmpty: Story = {
  render: () => (
    <Harness fetchStub={routed({ library: async () => json([]) })}>
      <McpLibrary />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /The library is empty/);
  },
};
export const LibraryError: Story = {
  render: () => (
    <Harness
      fetchStub={routed({
        library: async () => json({ error: { message: "catalog offline" } }, 503),
      })}
    >
      <McpLibrary />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return the MCP library/i);
    await expectNoFalseEmpty(canvasElement, /library is empty/);
  },
};
export const LibraryNoOrg: Story = {
  render: () => (
    <Harness fetchStub={noOrg()}>
      <McpLibrary />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await waitFor(() =>
      expect(within(canvasElement).getByText("Choose an organization")).toBeVisible(),
    );
    await expect(within(canvasElement).queryByRole("button", { name: /^Install/ })).toBeNull();
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

// #3295: a group can be narrowed to teams or projects. The whole set is sent
// on save, with a virtual-key assignment set through the API carried along
const assignmentsRecorded = recording(
  routed({
    groups: async (_input, init) => json(init?.method === "PUT" ? GROUPS[0] : GROUPS),
    assignments: async (_input, init) =>
      init?.method === "PUT"
        ? json([])
        : json([
            {
              group_id: "group-1",
              subject_kind: "virtual_key",
              subject_id: "key-9",
              created_at: "2026-01-01T00:00:00Z",
            },
          ]),
  }),
);
export const ToolGroupAssignsTeamsAndKeepsKeyAssignments: Story = {
  render: () => (
    <Harness fetchStub={assignmentsRecorded.stub}>
      <ToolGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const narrowed = await canvas.findByText("Assigned callers only");
    await waitFor(() => expect(narrowed).toBeVisible());
    await userEvent.click(
      await canvas.findByRole("button", { name: "Configure tool group Triage" }),
    );
    const body = within(document.body);
    const kept = await body.findByText(/set through the API/);
    await waitFor(() => expect(kept).toBeVisible());
    await userEvent.click(await body.findByRole("button", { name: "Platform" }));
    await userEvent.click(body.getByRole("button", { name: "Save group" }));
    await expect(
      await assignmentsRecorded.expectSentBody("PUT", "/tool-groups/group-1/assignments"),
    ).toEqual({
      assignments: [
        { kind: "team", id: "team-1" },
        { kind: "virtual_key", id: "key-9" },
      ],
    });
  },
};
// an unreadable assignment list must not turn into an editor that saves an
// empty set over the stored one
export const ToolGroupWithoutReadableAssignmentsOffersNoEditor: Story = {
  render: () => (
    <Harness
      fetchStub={routed({
        assignments: async () => json({ error: { message: "boom" } }, 500),
      })}
    >
      <ToolGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Configure tool group Triage" }),
    );
    const body = within(document.body);
    const unread = await body.findByText(/could not be read/);
    await waitFor(() => expect(unread).toBeVisible());
    await expect(body.queryByRole("button", { name: "Platform" })).toBeNull();
  },
};

// the editor must not start from an empty set while the stored one is still on
// its way: saving then would store nothing and hand a narrowed group to the
// whole organization
let releaseAssignments: () => void = () => {};
const slowAssignments = recording(
  routed({
    groups: async (_input, init) => json(init?.method === "PUT" ? GROUPS[0] : GROUPS),
    assignments: async (_input, init) => {
      if (init?.method === "PUT") return json([]);
      await new Promise<void>((resolve) => {
        releaseAssignments = resolve;
      });
      return json([
        {
          group_id: "group-1",
          subject_kind: "team",
          subject_id: "team-1",
          created_at: "2026-01-01T00:00:00Z",
        },
      ]);
    },
  }),
);
export const ToolGroupSaveBeforeAssignmentsLoadLeavesThemAlone: Story = {
  render: () => (
    <Harness fetchStub={slowAssignments.stub}>
      <ToolGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Configure tool group Triage" }),
    );
    const body = within(document.body);
    // still reading: no editor, and the copy says so rather than "could not be read"
    const reading = await body.findByText("Reading the current assignments…");
    await waitFor(() => expect(reading).toBeVisible());
    await expect(body.queryByText(/could not be read/)).toBeNull();
    await expect(body.queryByRole("button", { name: "Platform" })).toBeNull();
    await userEvent.click(body.getByRole("button", { name: "Save group" }));
    await slowAssignments.expectSent("PUT", "/mcp/tool-groups/group-1");
    releaseAssignments();
    // the save is over once the dialog has closed; only then can a follow-up
    // assignments call have been made, so only then does its absence mean anything
    await waitFor(() => expect(within(document.body).queryByRole("dialog")).toBeNull());
    await waitFor(() => slowAssignments.expectNotSent("PUT", "/tool-groups/group-1/assignments"));
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
        <Toasts>
          <McpSettings />
        </Toasts>
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
    // the save closes nothing, so without a toast it confirmed nothing (#2173)
    await waitFor(() =>
      expect(within(document.body).getByText("MCP settings saved")).toBeVisible(),
    );
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
    await expectRefused(canvasElement, "Create group");
    await expectRefused(canvasElement, "Delete tool group Triage");
    await expectRefused(canvasElement, "Configure tool group Triage");
  },
};
// the empty state offers the same create, and a gate that refused one of the
// two and not the other would leak
export const ToolGroupsEmptyGatedForMember: Story = {
  render: () => (
    <GatedHarness role="member" fetchStub={routed({ groups: async () => json([]) })}>
      <ToolGroups />
    </GatedHarness>
  ),
  play: async ({ canvasElement }) => {
    await waitFor(() => expect(within(canvasElement).getByText(/No tool groups/)).toBeVisible());
    await expectRefused(canvasElement, "Create group");
  },
};
export const ToolGroupsAllowedForAdmin: Story = {
  render: () => (
    <GatedHarness role="admin" fetchStub={routed()}>
      <ToolGroups />
    </GatedHarness>
  ),
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, "Create group");
    await expectAllowed(canvasElement, "Delete tool group Triage");
    await expectAllowed(canvasElement, "Configure tool group Triage");
  },
};
// the assignment chips are gated by their own resource, which an admin holds
export const ToolGroupAssignmentsAllowedForAdmin: Story = {
  render: () => (
    <GatedHarness role="admin" fetchStub={routed()}>
      <ToolGroups />
    </GatedHarness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", { name: "Configure tool group Triage" }),
    );
    await within(document.body).findByRole("button", { name: "Platform" });
    await expectAllowed(document.body, "Platform");
  },
};
export const LibraryGatedForMember: Story = {
  render: () => (
    <GatedHarness role="member" fetchStub={routed()}>
      <McpLibrary />
    </GatedHarness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Install Notion");
    await expectRefused(canvasElement, "Install Linear");
  },
};
export const LibraryAllowedForAdmin: Story = {
  render: () => (
    <GatedHarness role="admin" fetchStub={routed()}>
      <McpLibrary />
    </GatedHarness>
  ),
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, "Install Notion");
    await expectAllowed(canvasElement, "Install Linear");
  },
};
// the fields stay readable, as a disabled fieldset, and the Save names the
// role that would allow it
export const SettingsGatedForViewer: Story = {
  render: () => (
    <GatedHarness role="viewer" fetchStub={routed()}>
      <McpSettings />
    </GatedHarness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Save MCP settings");
    const retries = within(canvasElement).getByLabelText("Maximum retries");
    await waitFor(() => expect(retries).toBeDisabled());
    await expect(retries).toHaveValue(SETTINGS.max_retries);
    const transport = within(canvasElement).getByRole("combobox", { name: "Default transport" });
    await waitFor(() => expect(transport).toBeDisabled());
  },
};
export const SettingsAllowedForAdmin: Story = {
  render: () => (
    <GatedHarness role="admin" fetchStub={routed()}>
      <McpSettings />
    </GatedHarness>
  ),
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, "Save MCP settings");
    await expectAllowed(canvasElement, "Maximum retries", "spinbutton");
  },
};

export const ToolGroupsLoading: Story = {
  render: () => (
    <Harness fetchStub={routed({ groups: () => new Promise<Response>(() => {}) })}>
      <ToolGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No tool groups/);
  },
};
export const ToolGroupsError: Story = {
  render: () => (
    <Harness
      fetchStub={routed({
        groups: async () => json({ error: { message: "groups offline" } }, 503),
      })}
    >
      <ToolGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return tool groups/i);
    await expectNoFalseEmpty(canvasElement, /No tool groups/);
  },
};
// a failed servers read is not "no servers": the empty state must not send an
// admin off to register servers that may already exist
export const ToolGroupsServersFailed: Story = {
  render: () => (
    <Harness
      fetchStub={routed({
        groups: async () => json([]),
        servers: async () => json({ error: { message: "registry offline" } }, 503),
      })}
    >
      <ToolGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /failed to return MCP servers/i);
    await expect(canvas.getByText(/server list could not be read/)).toBeVisible();
    await expect(canvas.queryByText(/Register MCP servers and declare/)).not.toBeInTheDocument();
    await expect(canvas.getByRole("button", { name: "Create group" })).toBeDisabled();
    await expect(await canvas.findByRole("button", { name: /try again/i })).toBeVisible();
  },
};
export const ToolGroupsNoOrg: Story = {
  render: () => (
    <Harness fetchStub={noOrg()}>
      <ToolGroups />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Choose an organization")).toBeVisible());
    await expect(canvas.queryByRole("button", { name: "Create group" })).toBeNull();
  },
};
const created = recording(
  routed({
    groups: async (_input, init) =>
      init?.method === "POST" ? json({ ...GROUPS[0], name: "Builders" }, 201) : json([]),
  }),
);
export const ToolGroupCreateIsAnnounced: Story = {
  render: () => (
    <Harness fetchStub={created.stub}>
      <Toasts>
        <ToolGroups />
      </Toasts>
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
    await userEvent.click(body.getByRole("button", { name: "Save group" }));
    await expect(await created.expectSentBody("POST", "/mcp/tool-groups")).toMatchObject({
      name: "Builders",
      tools: [{ server_id: "server-github", tool: "create_issue" }],
    });
    await expectSheetClosed();
    await waitFor(() => expect(within(document.body).getByText("Builders created")).toBeVisible());
  },
};

export const SettingsLoading: Story = {
  render: () => (
    <Harness fetchStub={routed({ settings: () => new Promise<Response>(() => {}) })}>
      <McpSettings />
    </Harness>
  ),
  play: async ({ canvasElement }) => expectSkeleton(canvasElement),
};
export const SettingsError: Story = {
  render: () => (
    <Harness
      fetchStub={routed({
        settings: async () => json({ error: { message: "settings offline" } }, 503),
      })}
    >
      <McpSettings />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return MCP settings/i);
    await expect(within(canvasElement).queryByLabelText("Maximum retries")).toBeNull();
  },
};
export const SettingsNoOrg: Story = {
  render: () => (
    <Harness fetchStub={noOrg()}>
      <McpSettings />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Choose an organization")).toBeVisible());
    await expect(canvas.queryByRole("button", { name: "Save MCP settings" })).toBeNull();
  },
};
export const SettingsSaveFails: Story = {
  render: () => (
    <Harness
      fetchStub={routed({
        settings: async (_input, init) =>
          init?.method === "PUT"
            ? json({ error: { message: "timeout out of range" } }, 400)
            : json(SETTINGS),
      })}
    >
      <Toasts>
        <McpSettings />
      </Toasts>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Save MCP settings" }));
    await waitFor(() =>
      expect(within(document.body).getByText("Could not save the MCP settings")).toBeVisible(),
    );
    // the reason sits beside the Save and again in the toast, which outlives it
    await expectLoadError(canvasElement, /timeout out of range/);
    await expect(canvas.getAllByText(/timeout out of range/).length).toBeGreaterThanOrEqual(2);
  },
};
