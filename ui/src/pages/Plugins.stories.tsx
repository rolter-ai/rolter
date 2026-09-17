import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Plugins from "./Plugins";
import {
  cancelConfirmation,
  confirmDestructive,
  expectEmptyState,
  expectForbidden,
  expectRefused,
  expectSkeleton,
  expectToast,
  Harness as ScreenHarness,
  json,
  NEEDS_ADMIN,
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
];

const scopeResponse = (url: string) => {
  if (url.endsWith("/api/v1/orgs")) return json([{ id: "org-1", name: "Rolter", slug: "rolter", created_at: "" }]);
  if (url.includes("/teams")) return json([{ id: "team-1", org_id: "org-1", name: "Platform", created_at: "" }]);
  if (url.includes("/projects")) return json([{ id: "project-1", team_id: "team-1", name: "Gateway", created_at: "" }]);
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

const withPlugins = (plugins: PluginInstanceRow[], pluginStatus = 200): FetchStub => async (input) => {
  const url = String(input);
  return scopeResponse(url) ?? json(pluginStatus === 200 ? plugins : { error: { message: "forbidden" } }, pluginStatus);
};

const meta = { title: "Screens/Plugins", component: Plugins, parameters: { layout: "fullscreen" } } satisfies Meta<typeof Plugins>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = { render: () => <Harness fetchStub={withPlugins(PLUGINS)} /> };
export const Loading: Story = {
  render: () => <Harness fetchStub={async (input) => scopeResponse(String(input)) ?? new Promise<Response>(() => {})} />,
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
  render: () => <Harness fetchStub={async (input, init) => {
    const scoped = scopeResponse(String(input));
    if (scoped) return scoped;
    return init?.method === "POST" ? json(PLUGINS[0], 201) : json(PLUGINS);
  }} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /install plugin/i }));
    const dialog = within(document.body).getByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Name"), "Policy webhook");
    await userEvent.click(within(dialog).getByRole("button", { name: "Install plugin" }));
    await waitFor(() => expect(within(document.body).queryByRole("dialog")).not.toBeInTheDocument());
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
    await waitFor(() =>
      expect(within(document.body).getByRole("dialog")).toBeInTheDocument(),
    );
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
    await expect(within(dialog).getByRole("alert")).toHaveTextContent("Configuration must be a JSON object.");
  },
};

// the delete was a bare window.confirm, so the confirm path had never been
// exercised by a story at all (#1179)
const deletes = recording(async (input, init) => {
  const scoped = scopeResponse(String(input));
  if (scoped) return scoped;
  return init?.method === "DELETE" ? json({}, 204) : json(PLUGINS);
});

export const ConfirmsBeforeDeletingAPlugin: Story = {
  render: () => <Harness fetchStub={deletes.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // by name, not by index: each row control names its own plugin (#1214)
    const del = async () =>
      canvas.findByRole("button", { name: "Delete plugin PII redaction" });

    await userEvent.click(await del());
    await cancelConfirmation();
    deletes.expectNotSent("DELETE", "/plugins/plugin-redact");

    await userEvent.click(await del());
    await confirmDestructive(/PII redaction/, "Delete");
    await deletes.expectSent("DELETE", "/plugins/plugin-redact");
  },
};

// a toggle that never settles: the plugin being switched must be the only one
// whose controls go dead, not every row on the screen (#1128)
export const KeepsOtherRowsInteractiveWhileOneToggles: Story = {
  render: () => <Harness fetchStub={async (input, init) => {
    const scoped = scopeResponse(String(input));
    if (scoped) return scoped;
    return init?.method === "PUT" ? new Promise<Response>(() => {}) : json(PLUGINS);
  }} />,
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
