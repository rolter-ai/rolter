import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Plugins from "./Plugins";
import {
  cancelConfirmation,
  confirmDestructive,
  expectEmptyState,
  expectForbidden,
  expectRefused,
  expectSheetClosed,
  expectSkeleton,
  expectToast,
  Harness as ScreenHarness,
  json,
  NEEDS_ADMIN,
  pickOption,
  recording,
  Toasted,
  type FetchStub,
  type StoryRole,
} from "./story-harness";
import type { PluginInstanceRow } from "@/lib/api";

const PLUGINS: PluginInstanceRow[] = [
  {
    id: "plugin-redact",
    org_id: "org-1",
    project_id: null,
    name: "PII redaction",
    slug: "pii-redaction",
    description: "Redacts configured entities before requests leave the gateway.",
    kind: "webhook",
    stage: "pre_upstream",
    enabled: true,
    position: 10,
    failure_mode: "fail_closed",
    endpoint: "https://plugins.internal/redact",
    secret_env: "ROLTER_PLUGIN_TOKEN",
    config: { entities: ["email", "phone"] },
    created_at: "2026-08-02T00:00:00Z",
    updated_at: "2026-08-02T00:00:00Z",
  },
  {
    id: "plugin-audit",
    org_id: "org-1",
    project_id: "project-1",
    name: "Response audit",
    slug: "response-audit",
    description: "Forwards response metadata to the internal policy archive.",
    kind: "webhook",
    stage: "post_response",
    enabled: false,
    position: 20,
    failure_mode: "fail_open",
    endpoint: "https://plugins.internal/audit",
    secret_env: null,
    config: {},
    created_at: "2026-08-02T00:00:00Z",
    updated_at: "2026-08-02T00:00:00Z",
  },
  // post-response and fail closed: the one shape that refuses streamed
  // requests, so the card carries the warning (#2178)
  {
    id: "plugin-policy",
    org_id: "org-1",
    project_id: null,
    name: "Response policy",
    slug: "response-policy",
    description: "Blocks responses that fail the internal content policy.",
    kind: "webhook",
    stage: "post_response",
    enabled: true,
    position: 30,
    failure_mode: "fail_closed",
    endpoint: "https://plugins.internal/policy",
    secret_env: null,
    config: {},
    created_at: "2026-08-02T00:00:00Z",
    updated_at: "2026-08-02T00:00:00Z",
  },
];

const scopeResponse = (url: string) => {
  if (url.endsWith("/api/v1/orgs"))
    return json([{ id: "org-1", name: "Rolter", slug: "rolter", created_at: "" }]);
  if (url.includes("/teams"))
    return json([{ id: "team-1", org_id: "org-1", name: "Platform", created_at: "" }]);
  if (url.includes("/projects"))
    return json([{ id: "project-1", team_id: "team-1", name: "Gateway", created_at: "" }]);
  return null;
};

/**
 * The screen under the shared fetch-stub harness, with a role to render as.
 *
 * `role` is what a story needs to mount a `CapabilityProvider` at all: with no
 * provider above it `can()` answers "unknown" and every gated control renders
 * enabled, so a story without one can never see a control refused (#1606).
 *
 * `toasted` is opt-in rather than always on: the Toaster contributes its own
 * role="status" and role="alert" regions, and the stories that query those by
 * role would stop being able to.
 */
function Harness({
  fetchStub,
  role,
  toasted,
}: {
  fetchStub: FetchStub;
  role?: StoryRole;
  toasted?: boolean;
}) {
  return (
    <ScreenHarness fetchStub={fetchStub} role={role}>
      {toasted ? (
        <Toasted>
          <Plugins />
        </Toasted>
      ) : (
        <Plugins />
      )}
    </ScreenHarness>
  );
}

const withPlugins =
  (plugins: PluginInstanceRow[], pluginStatus = 200): FetchStub =>
  async (input) => {
    const url = String(input);
    return (
      scopeResponse(url) ??
      json(pluginStatus === 200 ? plugins : { error: { message: "forbidden" } }, pluginStatus)
    );
  };

const meta = {
  title: "Screens/Plugins",
  component: Plugins,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Plugins>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = { render: () => <Harness fetchStub={withPlugins(PLUGINS)} /> };
export const Loading: Story = {
  render: () => (
    <Harness
      fetchStub={async (input) => scopeResponse(String(input)) ?? new Promise<Response>(() => {})}
    />
  ),
  play: async ({ canvasElement }) => expectSkeleton(canvasElement),
};
export const Empty: Story = {
  render: () => <Harness fetchStub={withPlugins([])} />,
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No plugins installed/, /Install first plugin/);
  },
};
export const Forbidden: Story = {
  render: () => <Harness fetchStub={withPlugins([], 403)} />,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

export const InstallsWebhookConfiguration: Story = {
  render: () => (
    <Harness
      fetchStub={async (input, init) => {
        const scoped = scopeResponse(String(input));
        if (scoped) return scoped;
        return init?.method === "POST" ? json(PLUGINS[0], 201) : json(PLUGINS);
      }}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /install plugin/i }));
    const dialog = within(document.body).getByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Name"), "Policy webhook");
    await userEvent.click(within(dialog).getByRole("button", { name: "Install plugin" }));
    await waitFor(() =>
      expect(within(document.body).queryByRole("dialog")).not.toBeInTheDocument(),
    );
  },
};

/**
 * The install is refused (#1607).
 *
 * `RejectsInvalidConfiguration` is the client-side guard — this is the server's
 * answer, which is a different code path. The dialog stays open with the name
 * and the endpoint typed, and the refusal reaches the toast queue.
 */
export const InstallRejectedByTheServer: Story = {
  render: () => (
    <Harness
      toasted
      fetchStub={async (input, init) => {
        const scoped = scopeResponse(String(input));
        if (scoped) return scoped;
        if (init?.method === "POST") {
          return json({ error: { message: "slug policy-webhook is already installed" } }, 409);
        }
        return json(PLUGINS);
      }}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /install plugin/i }));
    const dialog = within(document.body).getByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Name"), "Policy webhook");
    await userEvent.click(within(dialog).getByRole("button", { name: "Install plugin" }));

    await expectToast(canvasElement, /already installed/, "error");
    await waitFor(() => expect(within(document.body).getByRole("dialog")).toBeInTheDocument());
    await expect(within(dialog).getByLabelText("Name")).toHaveValue("Policy webhook");
  },
};

export const RejectsInvalidConfiguration: Story = {
  render: () => <Harness fetchStub={withPlugins(PLUGINS)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /install plugin/i }));
    const dialog = within(document.body).getByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Name"), "Broken plugin");
    await userEvent.clear(within(dialog).getByLabelText("Plugin configuration"));
    await userEvent.click(within(dialog).getByLabelText("Plugin configuration"));
    await userEvent.paste("[]");
    await userEvent.click(within(dialog).getByRole("button", { name: "Install plugin" }));
    await expect(within(dialog).getByRole("alert")).toHaveTextContent(
      "Configuration must be a JSON object.",
    );
  },
};

const REFUSES_STREAMS = "Refuses streamed requests in its scope while enabled";
// each note also names the other policy's option, as the way out of its outcome
const REFUSED_NOTE =
  /streamed requests in its scope are refused with 400 plugin_streaming_unsupported\..*choose Fail open/;
const SKIPPED_NOTE =
  /Streamed responses skip this plugin and reach the client unchecked.*Under Fail closed/;

const card = async (canvasElement: HTMLElement, name: string) => {
  const heading = await within(canvasElement).findByRole("heading", { name: new RegExp(name) });
  return heading.closest("article") as HTMLElement;
};

/**
 * A post-response plugin that fails closed refuses every streamed request in
 * its scope with a 400 `plugin_streaming_unsupported` (#1776), and nothing on
 * the card used to say so beyond `fail-closed` (#2178).
 *
 * Only that combination carries the line: a fail-open post-response plugin
 * lets streams through, and a fail-closed plugin at an earlier stage sees the
 * request, which a stream does not change. Configuring the plugin opens the
 * dialog with the full note already showing.
 */
export const WarnsThatAFailClosedPostResponsePluginRefusesStreams: Story = {
  render: () => <Harness fetchStub={withPlugins(PLUGINS)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const policy = await card(canvasElement, "Response policy");
    await expect(within(policy).getByText(REFUSES_STREAMS)).toBeVisible();
    for (const name of ["Response audit", "PII redaction"]) {
      await expect(
        within(await card(canvasElement, name)).queryByText(REFUSES_STREAMS),
      ).not.toBeInTheDocument();
    }

    await userEvent.click(canvas.getByRole("button", { name: "Configure plugin Response policy" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await expect(await dialog.findByRole("note")).toHaveTextContent(REFUSED_NOTE);
  },
};

/**
 * The dialog states what streamed requests get before the plugin is saved
 * (#2178). The note appears once Post-response is picked, follows the failure
 * policy (refused when closed, unchecked when open), goes when another stage
 * is picked, and is the description of both pickers while it shows, so a
 * screen reader hears it from either one.
 */
export const DialogNoteTracksStageAndFailurePolicy: Story = {
  render: () => <Harness fetchStub={withPlugins(PLUGINS)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /install plugin/i }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    const stage = dialog.getByLabelText("Pipeline stage");
    const failure = dialog.getByLabelText("Failure policy");
    await expect(dialog.queryByRole("note")).not.toBeInTheDocument();

    // a new plugin starts fail open
    await pickOption(stage, "Post-response");
    const note = await dialog.findByRole("note");
    await expect(note).toHaveTextContent(SKIPPED_NOTE);
    await expect(stage).toHaveAttribute("aria-describedby", note.id);
    await expect(failure).toHaveAttribute("aria-describedby", note.id);

    await pickOption(failure, "Fail closed");
    await waitFor(() => expect(dialog.getByRole("note")).toHaveTextContent(REFUSED_NOTE));
    await expect(
      within(dialog.getByRole("note")).getByText("400 plugin_streaming_unsupported"),
    ).toBeVisible();

    await pickOption(stage, "Pre-upstream");
    await waitFor(() => expect(dialog.queryByRole("note")).not.toBeInTheDocument());
    await expect(stage).not.toHaveAttribute("aria-describedby");
    await expect(failure).not.toHaveAttribute("aria-describedby");
  },
};

// the delete was a bare window.confirm, so the confirm path had never been
// exercised by a story at all (#1179)
//
// the list shrinks once the DELETE lands, so the story can assert the outcome
// — the toast, the row gone — rather than that the request left. A stub that
// answers the full list forever passes either way, which is how a 204 fixture
// that threw went unnoticed (#1260)
let pluginDeleted = false;
const deletes = recording(async (input, init) => {
  const scoped = scopeResponse(String(input));
  if (scoped) return scoped;
  if (init?.method === "DELETE") {
    pluginDeleted = true;
    return json({}, 204);
  }
  return json(pluginDeleted ? PLUGINS.filter((row) => row.id !== "plugin-redact") : PLUGINS);
});

export const ConfirmsBeforeDeletingAPlugin: Story = {
  render: () => {
    pluginDeleted = false;
    return <Harness fetchStub={deletes.stub} toasted />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // by name, not by index: each row control names its own plugin (#1214)
    const del = async () => canvas.findByRole("button", { name: "Delete plugin PII redaction" });

    await userEvent.click(await del());
    await cancelConfirmation();
    deletes.expectNotSent("DELETE", "/plugins/plugin-redact");

    await userEvent.click(await del());
    await confirmDestructive(/PII redaction/, "Delete");
    await deletes.expectSent("DELETE", "/plugins/plugin-redact");

    // the outcome, not just the request: the confirmation closes, the queue
    // announces it, and the row is gone from the list
    await expectSheetClosed();
    await expectToast(canvasElement, /PII redaction deleted/);
    await waitFor(() => expect(canvas.queryByText("PII redaction")).not.toBeInTheDocument());
  },
};

// a toggle that never settles: the plugin being switched must be the only one
// whose controls go dead, not every row on the screen (#1128)
export const KeepsOtherRowsInteractiveWhileOneToggles: Story = {
  render: () => (
    <Harness
      fetchStub={async (input, init) => {
        const scoped = scopeResponse(String(input));
        if (scoped) return scoped;
        return init?.method === "PUT" ? new Promise<Response>(() => {}) : json(PLUGINS);
      }}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const toggled = await canvas.findByRole("switch", { name: "Enable PII redaction" });
    const untouched = await canvas.findByRole("switch", { name: "Enable Response audit" });
    await userEvent.click(toggled);
    await waitFor(() => expect(toggled).toBeDisabled());
    await expect(untouched).toBeEnabled();
    await expect(
      canvas.getByRole("button", { name: "Delete plugin Response audit" }),
    ).toBeEnabled();
  },
};

// `plugin` is admin at every action (#1606). The card carries three controls
// on three separate gates — the enable switch is a `GatedSwitch`, not a
// `GatedButton` — so the toolbar's create says nothing about any of them.
export const RefusedToAViewer: Story = {
  render: () => <Harness fetchStub={withPlugins(PLUGINS)} role="viewer" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectRefused(canvasElement, "Install plugin");
    await expectRefused(canvasElement, "Delete plugin PII redaction");
    await expectRefused(canvasElement, "Configure plugin PII redaction");
    const toggle = await canvas.findByRole("switch", { name: "Enable PII redaction" });
    await waitFor(() => expect(toggle).toBeDisabled());
    await expect(toggle).toHaveAttribute("title", NEEDS_ADMIN);
  },
};

export const RefusedToAMember: Story = {
  render: () => <Harness fetchStub={withPlugins(PLUGINS)} role="member" />,
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Install plugin");
    await expectRefused(canvasElement, "Delete plugin PII redaction");
  },
};
