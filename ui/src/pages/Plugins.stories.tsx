import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Plugins from "./Plugins";
import {
  cancelConfirmation,
  expectLoadError,
  expectNoFalseEmpty,
  confirmDestructive,
  expectAnalyticsUnavailable,
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
import en from "@/lib/i18n/locales/en.json";
import { expectFieldHints } from "@/lib/story-hints";

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

const isOutcomes = (input: RequestInfo | URL) =>
  new URL(String(input), "http://localhost").pathname === "/api/v1/analytics/plugins";

/** the call counts of #2243, answered on their own path; none recorded unless a story says */
const NO_CALLS = () => json({ recorded: true, data: [] });

const withPlugins =
  (
    plugins: PluginInstanceRow[],
    pluginStatus = 200,
    outcomes: () => Response = NO_CALLS,
  ): FetchStub =>
  async (input) => {
    const url = String(input);
    if (isOutcomes(input)) return outcomes();
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

export const Loaded: Story = {
  render: () => <Harness fetchStub={withPlugins(PLUGINS)} />,
  play: async ({ canvasElement }) => {
    const redact = await card(canvasElement, "PII redaction");
    // enabled reads in the success tone the guardrail screens use, and an
    // enabled plugin says what its failure policy does to traffic
    await expect(within(redact).getByText("enabled").className).toMatch(/status-success/);
    await expect(
      within(redact).getByText(/If the endpoint fails, requests are refused/),
    ).toBeVisible();
    const audit = await card(canvasElement, "Response audit");
    await expect(within(audit).queryByText(/If the endpoint fails/)).toBeNull();
    // the shell's ScreenHeader owns the page's only h1 (#2241)
    await expect(within(canvasElement).queryAllByRole("heading", { level: 1 })).toHaveLength(0);
  },
};
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

// a 5xx is retryable, unlike the 403 above: the alert offers Try again, and
// the list that comes back replaces it
let pluginsDown = true;
export const LoadFailedThenRetried: Story = {
  render: () => {
    pluginsDown = true;
    return (
      <Harness
        fetchStub={async (input) =>
          scopeResponse(String(input)) ??
          (pluginsDown ? json({ error: { message: "boom" } }, 500) : json(PLUGINS))
        }
      />
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /failed to return plugins/i);
    await expectNoFalseEmpty(canvasElement, /No plugins installed/);
    pluginsDown = false;
    await userEvent.click(canvas.getByRole("button", { name: /try again/i }));
    await card(canvasElement, "PII redaction");
  },
};

// no organization to list plugins for: a state of its own, not a stray line
export const NoOrganization: Story = {
  render: () => (
    <Harness
      fetchStub={async (input) =>
        String(input).endsWith("/api/v1/orgs") ? json([]) : json(PLUGINS)
      }
    />
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No organization to show plugins for/);
    await expect(within(canvasElement).queryByRole("button", { name: /install/i })).toBeNull();
  },
};

/**
 * The install form (#2179): the endpoint starts empty, with an example as its
 * placeholder, so an enabled plugin cannot point live traffic at a host nobody
 * chose; the position follows the last plugin at the chosen stage until it is
 * typed over; a broken configuration is flagged as it is typed.
 */
export const InstallFormDefaults: Story = {
  render: () => <Harness fetchStub={withPlugins(PLUGINS)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /install plugin/i }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    const endpoint = dialog.getByLabelText("Webhook endpoint");
    await expect(endpoint).toHaveValue("");
    await expect(endpoint).toHaveAttribute("placeholder", expect.stringMatching(/^https:\/\//));

    const position = dialog.getByLabelText("Position");
    // nothing at pre-route yet; 10 sits at pre-upstream and 30 at post-response
    await expect(position).toHaveValue(0);
    await pickOption(dialog.getByLabelText("Pipeline stage"), "Pre-upstream");
    await waitFor(() => expect(position).toHaveValue(11));
    await pickOption(dialog.getByLabelText("Pipeline stage"), "Post-response");
    await waitFor(() => expect(position).toHaveValue(31));
    await userEvent.clear(position);
    await userEvent.type(position, "5");
    await pickOption(dialog.getByLabelText("Pipeline stage"), "Pre-upstream");
    await expect(position).toHaveValue(5);

    // flagged while typing, with no press on Install
    const config = dialog.getByLabelText("Plugin configuration");
    await expect(config).not.toHaveAttribute("aria-invalid");
    await userEvent.type(config, "x");
    await waitFor(() => expect(config).toHaveAttribute("aria-invalid", "true"));
    await expect(config).toHaveAccessibleDescription("Configuration must be valid JSON.");
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
    await userEvent.type(
      within(dialog).getByLabelText("Webhook endpoint"),
      "https://hooks.example.com/p",
    );
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
    await userEvent.type(
      within(dialog).getByLabelText("Webhook endpoint"),
      "https://hooks.example.com/p",
    );
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
    await userEvent.type(
      within(dialog).getByLabelText("Webhook endpoint"),
      "https://hooks.example.com/p",
    );
    await userEvent.clear(within(dialog).getByLabelText("Plugin configuration"));
    await userEvent.click(within(dialog).getByLabelText("Plugin configuration"));
    await userEvent.paste("[]");
    await userEvent.click(within(dialog).getByRole("button", { name: "Install plugin" }));
    const config = within(dialog).getByLabelText("Plugin configuration");
    await expect(within(dialog).getByRole("alert")).toHaveTextContent(
      "Configuration must be a JSON object.",
    );
    await expect(config).toHaveAttribute("aria-invalid", "true");
    await expect(config).toHaveAccessibleDescription("Configuration must be a JSON object.");
    await waitFor(() => expect(config).toHaveFocus());
  },
};

// nothing is flagged before the first press on Install; then every failing
// field is marked under itself and focus moves to the first one (#2651)
export const MarksEveryInvalidField: Story = {
  render: () => <Harness fetchStub={withPlugins(PLUGINS)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /install plugin/i }));
    const dialog = within(document.body).getByRole("dialog");
    const name = within(dialog).getByLabelText("Name");
    const endpoint = within(dialog).getByLabelText("Webhook endpoint");
    await userEvent.clear(endpoint);
    await userEvent.type(endpoint, "plugins.internal/hook");
    await expect(name).not.toHaveAttribute("aria-invalid");
    await expect(within(dialog).queryByRole("alert")).toBeNull();
    // mandatory fields are announced up front, without flagging the pristine form (#3451)
    for (const field of [name, endpoint]) {
      await expect(field).toHaveAttribute("aria-required", "true");
      await expect(field).not.toHaveAttribute("required");
    }
    await expect(within(dialog).getByLabelText("Description")).not.toBeRequired();
    await expect(within(dialog).getAllByText("*")).toHaveLength(2);
    await userEvent.click(within(dialog).getByRole("button", { name: "Install plugin" }));
    await waitFor(() => expect(name).toHaveAttribute("aria-invalid", "true"));
    await expect(name).toHaveAccessibleDescription("Name is required.");
    await expect(endpoint).toHaveAttribute("aria-invalid", "true");
    await expect(endpoint).toHaveAccessibleDescription(
      "Endpoint must start with http:// or https://.",
    );
    await waitFor(() => expect(name).toHaveFocus());
    // fixing a field clears its message as it is typed
    await userEvent.type(name, "Policy webhook");
    await expect(name).not.toHaveAttribute("aria-invalid");
    await expect(endpoint).toHaveAttribute("aria-invalid", "true");
  },
};

// a 400 that names a field lands on that field, with focus, not in a toast
export const ServerRejectionLandsOnTheField: Story = {
  render: () => (
    <Harness
      fetchStub={async (input, init) => {
        const scoped = scopeResponse(String(input));
        if (scoped) return scoped;
        if (init?.method === "POST") {
          return json(
            { error: { message: "secret_env must be omitted or name an environment variable" } },
            400,
          );
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
    const secret = within(dialog).getByLabelText("Secret environment variable");
    await userEvent.type(secret, " ");
    await userEvent.type(
      within(dialog).getByLabelText("Webhook endpoint"),
      "https://hooks.example.com/p",
    );

    await userEvent.click(within(dialog).getByRole("button", { name: "Install plugin" }));
    await waitFor(() => expect(secret).toHaveAttribute("aria-invalid", "true"));
    await expect(secret).toHaveAccessibleDescription(
      "secret_env must be omitted or name an environment variable",
    );
    await waitFor(() => expect(secret).toHaveFocus());
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
    await waitFor(() =>
      expect(
        within(dialog.getByRole("note")).getByText("400 plugin_streaming_unsupported"),
      ).toBeVisible(),
    );

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

const minutesAgo = (minutes: number) => new Date(Date.now() - minutes * 60_000).toISOString();

/** one row of the rollup; the counts arrive as strings, like ClickHouse sends them */
const outcomeRow = (
  plugin: string,
  counts: { calls: number; blocked?: number; transformed?: number; errors?: number },
  lastError?: { minutes: number; kind: string },
  orgId = "org-1",
) => ({
  org_id: orgId,
  plugin,
  calls: String(counts.calls),
  blocked: String(counts.blocked ?? 0),
  transformed: String(counts.transformed ?? 0),
  annotated: "0",
  errors: String(counts.errors ?? 0),
  last_error_at: lastError ? minutesAgo(lastError.minutes) : null,
  last_error_kind: lastError?.kind ?? "",
});

/**
 * Each card states what its plugin did over the last seven days, and the
 * newest failure with its cause (#2243, #2224). A plugin that failed every call
 * is flagged with what its failure policy is doing to traffic because of it; a
 * paused one is not, as it sees no traffic. Another organization's plugin of
 * the same slug is not counted.
 */
export const ShowsWhatEachPluginDid: Story = {
  render: () => (
    <Harness
      fetchStub={withPlugins(PLUGINS, 200, () =>
        json({
          recorded: true,
          data: [
            outcomeRow(
              "pii-redaction",
              { calls: 120, blocked: 4, transformed: 30, errors: 2 },
              { minutes: 5, kind: "timeout" },
            ),
            outcomeRow(
              "response-policy",
              { calls: 40, errors: 40 },
              { minutes: 1, kind: "connect" },
            ),
            outcomeRow(
              "response-audit",
              { calls: 5, errors: 5 },
              { minutes: 60, kind: "status_503" },
            ),
            outcomeRow("pii-redaction", { calls: 9999, blocked: 9999 }, undefined, "org-2"),
          ],
        }),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const redact = within(await card(canvasElement, "PII redaction"));
    await expect(await redact.findByText("Calls in the last 7 days")).toBeVisible();
    for (const [label, value] of [
      ["Calls", "120"],
      ["Blocked", "4"],
      ["Transformed", "30"],
      ["Errors", "2"],
    ]) {
      await expect(redact.getByText(label, { exact: false, selector: "li" })).toHaveTextContent(
        `${label} ${value}`,
      );
    }
    await expect(redact.getByText(/Last error: the call timed out, .+ ago\./)).toBeVisible();
    // two failures in 120 calls is not "failing every call"
    await expect(redact.queryByText(/Failing every call/)).toBeNull();

    // fail-closed and failing every call: its traffic is being refused
    const policy = within(await card(canvasElement, "Response policy"));
    await expect(
      await policy.findByText(
        /Failing every call: 40 of 40 calls in the last 7 days got no answer\. Requests it covers are being refused\./,
      ),
    ).toBeVisible();

    // paused: counts stay, the flag does not, and the status code is named
    const audit = within(await card(canvasElement, "Response audit"));
    await expect(
      await audit.findByText(/Last error: the endpoint answered with status 503/),
    ).toBeVisible();
    await expect(audit.queryByText(/Failing every call/)).toBeNull();
  },
};

/** A fail-open plugin whose endpoint is down still forwards, and the card says so. */
export const FailingOpenForwardsUnchecked: Story = {
  render: () => (
    <Harness
      fetchStub={withPlugins(
        PLUGINS.map((plugin) =>
          plugin.id === "plugin-audit" ? { ...plugin, enabled: true } : plugin,
        ),
        200,
        () =>
          json({
            recorded: true,
            data: [
              outcomeRow(
                "response-audit",
                { calls: 3, errors: 3 },
                { minutes: 2, kind: "refused" },
              ),
            ],
          }),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const audit = within(await card(canvasElement, "Response audit"));
    await expect(
      await audit.findByText(/Failing every call: 3 of 3 calls.*Requests continue unchecked\./),
    ).toBeVisible();
    await expect(audit.getByText(/the egress policy refused the endpoint/)).toBeVisible();
  },
};

/** A plugin nothing has called in the window says "no calls", not a row of zeros. */
export const NoCallsYet: Story = {
  render: () => <Harness fetchStub={withPlugins(PLUGINS)} />,
  play: async ({ canvasElement }) => {
    const redact = within(await card(canvasElement, "PII redaction"));
    await expect(await redact.findByText("No calls in the last 7 days.")).toBeVisible();
    await expect(redact.queryByText("Calls", { selector: "li" })).toBeNull();
    await expect(redact.queryByText(/Failing every call/)).toBeNull();
  },
};

/**
 * A deployment with no ClickHouse cannot count anything. The screen says so
 * once, calmly, and the cards claim neither calls nor their absence.
 */
export const NoAnalytics: Story = {
  render: () => (
    <Harness
      fetchStub={withPlugins(PLUGINS, 200, () =>
        json({ error: { message: "analytics unavailable: no clickhouse_url configured" } }, 503),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    await expectAnalyticsUnavailable(
      canvasElement,
      "Call counts need an analytics store",
      "analytics unavailable: no clickhouse_url configured",
    );
    await expect(canvasElement).not.toHaveTextContent(/No calls in the last 7 days/);
    await expect(canvasElement).not.toHaveTextContent(/Counting calls/);
  },
};

/** ClickHouse is there but the file that keeps plugin calls was never applied. */
export const CallsNotRecorded: Story = {
  render: () => (
    <Harness fetchStub={withPlugins(PLUGINS, 200, () => json({ recorded: false, data: [] }))} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/Call counts are not being kept/)).toBeVisible();
    await expect(canvasElement).not.toHaveTextContent(/No calls in the last 7 days/);
  },
};

/** The rollup failing leaves the plugin list usable, with the cards saying why they have no counts. */
export const CallCountsFailed: Story = {
  render: () => (
    <Harness
      fetchStub={withPlugins(PLUGINS, 200, () => json({ error: { message: "boom" } }, 500))}
    />
  ),
  play: async ({ canvasElement }) => {
    const redact = within(await card(canvasElement, "PII redaction"));
    await expect(
      await redact.findByText("Could not load call counts.", undefined, { timeout: 8000 }),
    ).toBeVisible();
    await expect(
      redact.getByRole("button", { name: "Configure plugin PII redaction" }),
    ).toBeEnabled();
  },
};

/**
 * Scope, stage, endpoint and failure policy explain themselves in an (i)
 * (#3285): who the plugin covers, when it runs, what it is sent and what a
 * failure does to a request.
 */
export const ExplainsEachFieldInAHint: Story = {
  render: () => <Harness fetchStub={withPlugins(PLUGINS)} />,
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", { name: /install plugin/i }),
    );
    const dialog = await within(document.body).findByRole("dialog");
    const p = en.pages.plugins;
    await expectFieldHints(dialog, en.common.aboutField, [
      [p.fieldScope, p.fieldScopeInfo],
      [p.fieldStage, p.fieldStageInfo],
      [p.fieldEndpoint, p.fieldEndpointInfo],
      [p.fieldFailure, p.fieldFailureInfo],
    ]);
  },
};
