import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Connectors from "./Connectors";
import {
  cancelConfirmation,
  confirmDestructive,
  expectForbidden,
  expectLoadError,
  expectSheetClosed,
  expectSkeleton,
  expectToast,
  Harness as ScreenHarness,
  json,
  recording,
  Toasted,
  type FetchStub,
  type StoryRole,
} from "./story-harness";
import type { ConnectorRow } from "@/lib/api";

const connector = (over: Partial<ConnectorRow> = {}): ConnectorRow => ({
  id: "c-1",
  name: "signoz",
  kind: "otlp_http",
  endpoint: "https://collector.example.com/v1/logs",
  enabled: true,
  sampling_rate: 1,
  auth_secret_ref: null,
  auth_secret_configured: true,
  health_status: "healthy",
  // fixed rather than relative: nothing here should depend on wall-clock time
  health_checked_at: "2026-08-06T10:00:00Z",
  health_error: null,
  created_at: "2026-08-01T10:00:00Z",
  updated_at: "2026-08-06T10:00:00Z",
  ...over,
});

const CONNECTORS: ConnectorRow[] = [
  connector(),
  // configured but never turned on — the default state, since connectors are
  // strictly opt-in
  connector({
    id: "c-2",
    name: "honeycomb",
    enabled: false,
    sampling_rate: 0.1,
    health_status: "unknown",
    health_checked_at: null,
    auth_secret_configured: false,
  }),
  // an endpoint that answered, badly
  connector({
    id: "c-3",
    name: "datadog-staging",
    health_status: "unhealthy",
    health_error: "sink returned HTTP 401",
  }),
];

// the collector config comes back as a yaml *document*, not json — the shape
// `render_yaml` in crates/rolter-control/src/collector_config.rs produces
const COLLECTOR_CONFIG = `# rendered by rolter (GET /api/v1/connectors/collector-config); do not edit by hand
receivers:
  otlp:
    protocols:
      grpc:
        endpoint: 0.0.0.0:4317

exporters:
  otlphttp/signoz:
    endpoint: "https://collector.example.com/v1/logs"

service:
  pipelines:
    logs/signoz:
      receivers: [otlp]
      exporters: [otlphttp/signoz]
`;

const yaml = (body: string, status = 200) =>
  new Response(body, { status, headers: { "Content-Type": "application/yaml" } });

/** Answer the config endpoint with `config`, everything else with the list. */
function withConfig(
  config: () => Response | Promise<Response>,
  connectors: ConnectorRow[] = CONNECTORS,
): FetchStub {
  return async (input) =>
    String(input).includes("collector-config") ? config() : json(connectors);
}

/**
 * The screen under the shared fetch-stub harness, with a role to render as.
 *
 * `role` is what a story needs to mount a `CapabilityProvider` at all: with no
 * provider above it `can()` answers "unknown", the `superadminOnly` wrapper
 * never blocks, and a story can only reach the 403 by stubbing one — which
 * tests the screen's own error path rather than the gate (#1606).
 */
function Harness({
  fetchStub,
  role,
  toasted,
}: {
  fetchStub: FetchStub;
  role?: StoryRole;
  /** mount the shell's toast queue, for a story that asserts the outcome */
  toasted?: boolean;
}) {
  return (
    <ScreenHarness fetchStub={fetchStub} role={role}>
      {toasted ? (
        <Toasted>
          <Connectors />
        </Toasted>
      ) : (
        <Connectors />
      )}
    </ScreenHarness>
  );
}

const meta = {
  title: "Screens/Connectors",
  component: Connectors,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Connectors>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => <Harness fetchStub={async () => json(CONNECTORS)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("signoz")).toBeVisible());

    // health is its own axis, independent of enabled: a connector that has
    // never been tested reports `unknown` rather than claiming to be healthy
    await expect(canvas.getByText("healthy")).toBeVisible();
    await expect(canvas.getByText("unknown")).toBeVisible();
    await expect(canvas.getByText("unhealthy")).toBeVisible();

    // the sampling rate is shown as a percentage, so 0.1 must read as 10%
    await expect(canvas.getByText("10% sampled")).toBeVisible();

    // a failure names the status, never the sink's response body
    await expect(canvas.getByText(/HTTP 401/)).toBeVisible();
  },
};

export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

// the default for every deployment: connectors are opt-in, so an untouched
// install has none and no egress path at all
export const Empty: Story = {
  render: () => <Harness fetchStub={async () => json([])} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText(/No connectors yet/)).toBeVisible());
  },
};

// connectors are a deployment-wide egress decision, so a non-superadmin gets 403
export const Error_: Story = {
  name: "Error",
  render: () => <Harness fetchStub={async () => json({ error: { message: "forbidden" } }, 403)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText(/You do not have access to connectors/)).toBeVisible(),
    );
  },
};

// shipping request logs somewhere is an egress decision; unmaking it takes the
// delivery history with it, so the connector is named before anything goes
// (#1179)
//
// the list shrinks once the DELETE lands, so the story can assert the outcome
// — the toast, the row gone — rather than that the request left. A stub that
// answers the full list forever passes either way, which is how a 204 fixture
// that threw went unnoticed (#1260)
let connectorDeleted = false;
const deletes = recording(async (_input, init) => {
  if (init?.method === "DELETE") {
    connectorDeleted = true;
    return json({}, 204);
  }
  return json(connectorDeleted ? CONNECTORS.filter((row) => row.id !== "c-1") : CONNECTORS);
});

export const ConfirmsBeforeDeletingAConnector: Story = {
  render: () => {
    connectorDeleted = false;
    return <Harness fetchStub={deletes.stub} toasted />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("signoz")).toBeVisible());

    await userEvent.click(canvas.getByLabelText("Delete connector signoz"));
    await cancelConfirmation();
    deletes.expectNotSent("DELETE", "/connectors/c-1");

    await userEvent.click(canvas.getByLabelText("Delete connector signoz"));
    await confirmDestructive(/signoz/, /delete connector/i);
    await deletes.expectSent("DELETE", "/connectors/c-1");

    // the outcome, not just the request: the confirmation closes, the queue
    // announces it, and the row is gone from the list
    await expectSheetClosed();
    await expectToast(canvasElement, /signoz deleted/);
    await waitFor(() => expect(canvas.queryByText("signoz")).not.toBeInTheDocument());
  },
};

// the delete is on the wire: the confirm button spins and neither button is
// clickable again
export const DeletingAConnector: Story = {
  render: () => (
    <Harness
      fetchStub={async (_input, init) =>
        init?.method === "DELETE" ? new Promise<Response>(() => {}) : json(CONNECTORS)
      }
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("signoz")).toBeVisible());
    await userEvent.click(canvas.getByLabelText("Delete connector signoz"));
    await confirmDestructive(/signoz/, /delete connector/i);

    const dialog = within(document.body).getByRole("dialog");
    await waitFor(() =>
      expect(within(dialog).getByRole("button", { name: /delete connector/i })).toBeDisabled(),
    );
  },
};

// defining a connector delivers nothing on its own — a collector has to be
// running the config rendered from it (#1195, ADR-0026). the screen has to be
// able to show that document, and say where it goes
export const CollectorConfig: Story = {
  render: () => <Harness fetchStub={withConfig(() => yaml(COLLECTOR_CONFIG))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("signoz")).toBeVisible());
    await userEvent.click(canvas.getByRole("button", { name: /Collector config/ }));

    const dialog = within(await within(document.body).findByRole("dialog"));
    // the document itself, verbatim — one exporter and one pipeline per
    // enabled connector. asserted on the region rather than on a text node:
    // the yaml is highlighted now, so a name is split across token spans (#949)
    const document_ = dialog.getByRole("region", {
      name: /OpenTelemetry Collector config/i,
    });
    await waitFor(() => expect(document_).toHaveTextContent("otlphttp/signoz"));
    // and where it goes, which is the part a connector row never said
    await expect(dialog.getByText(/collector\.compose\.yaml/)).toBeVisible();
    // copyable, because pasting it into a collector is the whole point
    await expect(
      dialog.getByRole("button", { name: /^Copy OpenTelemetry Collector config/ }),
    ).toBeVisible();
  },
};

// the document is rendered on request from the connector rows, so it can be
// slow; the dialog stands in a skeleton rather than an empty frame
export const CollectorConfigLoading: Story = {
  render: () => <Harness fetchStub={withConfig(() => new Promise<Response>(() => {}))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("signoz")).toBeVisible());
    await userEvent.click(canvas.getByRole("button", { name: /Collector config/ }));
    await expectSkeleton(document.body);
  },
};

// with no connectors the config renders no exporters at all: a valid document
// that delivers nothing, which is worth saying rather than showing
export const CollectorConfigEmpty: Story = {
  render: () => <Harness fetchStub={withConfig(() => yaml(COLLECTOR_CONFIG), [])} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText(/No connectors yet/)).toBeVisible());
    await userEvent.click(canvas.getByRole("button", { name: /Collector config/ }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await expect(dialog.getByText(/Nothing to deliver yet/)).toBeVisible();
  },
};

// the list can load while the render fails — a KEK the control plane cannot
// open, say. the failure belongs in the dialog, not on the screen behind it
export const CollectorConfigError: Story = {
  render: () => (
    <Harness fetchStub={withConfig(() => json({ error: { message: "kek unavailable" } }, 500))} />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("signoz")).toBeVisible());
    await userEvent.click(canvas.getByRole("button", { name: /Collector config/ }));
    await expectLoadError(document.body, /collector config/i);
  },
};

// What a non-superadmin gets: the screen refused before it asks (#1606).
//
// A connector ships the deployment's own telemetry, so `connector` is
// superadmin at every action and `superadminOnly` never mounts the screen for
// an org role. The stub answers with a good payload on purpose: if the wrapper
// is dropped the screen renders that payload and this story fails, which the
// `Forbidden` story cannot do, since it stubs the 403 itself.
export const RefusedToAnAdmin: Story = {
  render: () => <Harness fetchStub={withConfig(() => yaml(COLLECTOR_CONFIG))} role="admin" />,
  play: async ({ canvasElement }) => expectForbidden(canvasElement),
};

export const RefusedToAViewer: Story = {
  render: () => <Harness fetchStub={withConfig(() => yaml(COLLECTOR_CONFIG))} role="viewer" />,
  play: async ({ canvasElement }) => expectForbidden(canvasElement),
};
