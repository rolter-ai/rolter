import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Connectors from "./Connectors";
import {
  cancelConfirmation,
  clickWhenEnabled,
  confirmation,
  confirmDestructive,
  expectForbidden,
  expectLoadError,
  expectNoFalseEmpty,
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
import { formattersFor } from "@/lib/i18n/format";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { atMobile, expectNoHorizontalOverflow } from "@/lib/story-viewport";

const CHECKED_AT = "2026-08-06T10:00:00Z";

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
  health_checked_at: CHECKED_AT,
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
    // a rate that is not a whole percent: rounding it would read "0% sampled"
    sampling_rate: 0.004,
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

    // 0.4 % is not "0% sampled": a card that rounded it away would say nothing
    // is sent while something is (#2104)
    await expect(canvas.getByText("0.4% sampled")).toBeVisible();
    await expect(canvas.getByText("100% sampled")).toBeVisible();

    // a probe from last week must not read as one from today (#2108): the row
    // says how long ago, as a <time> that keeps the full stamp for the hover
    const [checked] = canvas.getAllByText(/^checked /);
    await expect(checked.tagName).toBe("TIME");
    await expect(checked).toHaveAttribute("datetime", CHECKED_AT);
    await expect(checked).toHaveAttribute("title", formattersFor("en").dateTime(CHECKED_AT));
    await expect(checked).toHaveTextContent(
      en.pages.connectors.checkedAt.replace("{{time}}", formattersFor("en").relative(CHECKED_AT)),
    );
    await expect(checked.textContent).not.toMatch(/\d:\d\d/);
  },
};

export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No connectors yet/);
  },
};

// the default for every deployment: connectors are opt-in, so an untouched
// install has none and no egress path at all
export const Empty: Story = {
  render: () => <Harness fetchStub={async () => json([])} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText(/No connectors yet/)).toBeVisible());
    // the toolbar keeps its create and the empty state offers the same one; the
    // toolbar's carries the Plus icon, not a "+" typed into the label (#2108)
    await expect(canvas.getAllByRole("button", { name: "Add connector" })).toHaveLength(2);
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
    await expectNoFalseEmpty(canvasElement, /No connectors yet/);
    // nor a count for a list that was not read: "0 connectors" states a figure
    // the screen does not have (#2211, #2108)
    await expect(canvas.queryByText(/OTLP\/HTTP sinks for request logs/)).toBeNull();
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

// the add sheet: sampling is typed as a percentage and read as typed (#2104).
// `Number("0") || 100` made a typed 0 a rate of 1, so a connector meant to send
// nothing shipped every request, and a blank field or 150 became 100 too
const created = (body: Partial<ConnectorRow> = {}) =>
  recording(async (_input, init) =>
    init?.method === "POST"
      ? json(connector({ id: "c-4", name: "audit-sink", ...body }))
      : json(CONNECTORS),
  );

interface SentConnector {
  name: string;
  endpoint: string;
  enabled: boolean;
  sampling_rate: number;
}

/** Open the add sheet with the two required rows filled in; sampling is left to the story. */
async function openAddSheet(canvasElement: HTMLElement, copy = en.pages.connectors) {
  const canvas = within(canvasElement);
  await waitFor(() => expect(canvas.getByText("signoz")).toBeVisible());
  await clickWhenEnabled(canvasElement, copy.add);
  const form = within(await within(document.body).findByRole("dialog"));
  await userEvent.type(await form.findByLabelText(copy.form.name), "audit-sink");
  await userEvent.type(
    await form.findByLabelText(copy.form.endpoint),
    "https://otlp.example.com/v1/logs",
  );
  return form;
}

const sentZero = created({ sampling_rate: 0 });

export const SamplingOfZeroIsKept: Story = {
  render: () => <Harness fetchStub={sentZero.stub} toasted />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors;
    const form = await openAddSheet(canvasElement);
    const sampling = await form.findByLabelText(copy.form.sampling);
    await userEvent.clear(sampling);
    await userEvent.type(sampling, "0");

    // what 0 means is on the form, and a valid 0 is not an error
    await expect(form.getByText(copy.form.samplingHint)).toBeVisible();
    await expect(sampling).not.toHaveAttribute("aria-invalid", "true");

    await userEvent.click(form.getByRole("button", { name: en.common.create }));
    const body = await sentZero.expectSentBody<SentConnector>("POST", "/api/v1/connectors");
    await expect(body.sampling_rate).toBe(0);
    await expect(body).toMatchObject({
      name: "audit-sink",
      endpoint: "https://otlp.example.com/v1/logs",
    });
    await expectToast(canvasElement, /audit-sink created/);
  },
};

const sentDefault = created();

export const UntouchedSamplingSendsEveryRequest: Story = {
  render: () => <Harness fetchStub={sentDefault.stub} toasted />,
  play: async ({ canvasElement }) => {
    const form = await openAddSheet(canvasElement);
    await userEvent.click(form.getByRole("button", { name: en.common.create }));
    const body = await sentDefault.expectSentBody<SentConnector>("POST", "/api/v1/connectors");
    await expect(body.sampling_rate).toBe(1);
  },
};

const sentTooMuch = created();

export const SamplingAbove100IsRefused: Story = {
  render: () => <Harness fetchStub={sentTooMuch.stub} />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors;
    const form = await openAddSheet(canvasElement);
    const sampling = await form.findByLabelText(copy.form.sampling);
    await userEvent.clear(sampling);
    await userEvent.type(sampling, "150");

    // said next to the field and tied to it, not clamped to 100 in silence
    await expect(await form.findByText(copy.form.samplingRange)).toBeVisible();
    await expect(sampling).toHaveAttribute("aria-invalid", "true");
    await expect(sampling).toHaveAccessibleDescription(copy.form.samplingRange);
    await expect(form.getByRole("button", { name: en.common.create })).toBeDisabled();
    sentTooMuch.expectNotSent("POST", "/api/v1/connectors");
  },
};

const sentBlank = created();

export const BlankSamplingIsRefused: Story = {
  render: () => <Harness fetchStub={sentBlank.stub} />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors;
    const form = await openAddSheet(canvasElement);
    const sampling = await form.findByLabelText(copy.form.sampling);
    await userEvent.clear(sampling);

    await expect(await form.findByText(copy.form.samplingInvalid)).toBeVisible();
    await expect(sampling).toHaveAttribute("aria-invalid", "true");
    await expect(sampling).toHaveAccessibleDescription(copy.form.samplingInvalid);
    await expect(form.getByRole("button", { name: en.common.create })).toBeDisabled();
    sentBlank.expectNotSent("POST", "/api/v1/connectors");

    // and the same field accepts a value again once one is typed
    await userEvent.type(sampling, "25");
    await waitFor(() => expect(form.getByRole("button", { name: en.common.create })).toBeEnabled());
    await expect(form.queryByText(copy.form.samplingInvalid)).toBeNull();
  },
};

// the probe: `delivered` is the whole verdict, and each outcome has to say so
// where the operator is looking (#2108). a rejected one used to be the only one
// that spoke, and a failed request printed a raw message below the whole grid
const probing = (answer: () => Response | Promise<Response>, list = () => CONNECTORS) =>
  recording(async (input, init) =>
    init?.method === "POST" && String(input).includes("/test") ? answer() : json(list()),
  );

const TESTED_AT = "2026-09-30T10:00:00Z";

const deliveredProbe = probing(() =>
  json({ delivered: true, health_status: "healthy", health_checked_at: TESTED_AT }),
);

export const TestDeliverySaysItWorked: Story = {
  render: () => <Harness fetchStub={deliveredProbe.stub} toasted />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("signoz")).toBeVisible());
    await userEvent.click(canvas.getByLabelText("Test delivery to signoz"));
    await deliveredProbe.expectSent("POST", "/connectors/c-1/test");

    // a connector that was already healthy changes nothing else on its card
    await expectToast(canvasElement, /Test delivery to signoz succeeded/);
    await expect(
      within(canvas.getByRole("group", { name: "signoz" })).queryByRole("alert"),
    ).toBeNull();
  },
};

const rejectedProbe = probing(() =>
  json({
    delivered: false,
    health_status: "unhealthy",
    health_checked_at: TESTED_AT,
    health_error: "sink returned HTTP 503",
  }),
);

export const RejectedTestShowsOnItsCard: Story = {
  render: () => <Harness fetchStub={rejectedProbe.stub} toasted />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("signoz")).toBeVisible());
    await userEvent.click(canvas.getByLabelText("Test delivery to signoz"));

    const card = within(canvas.getByRole("group", { name: "signoz" }));
    await expect(await card.findByRole("alert")).toHaveTextContent(
      "Delivery failed: sink returned HTTP 503",
    );
    // the other cards are not the probe's business
    await expect(
      within(canvas.getByRole("group", { name: "honeycomb" })).queryByRole("alert"),
    ).toBeNull();
  },
};

// once the list is refetched the card carries the reason itself, so the probe's
// own line steps aside instead of saying the same thing twice
let probed = false;
const repeatedProbe = probing(
  () => {
    probed = true;
    return json({
      delivered: false,
      health_status: "unhealthy",
      health_checked_at: TESTED_AT,
      health_error: "sink returned HTTP 503",
    });
  },
  () =>
    probed
      ? [
          connector({
            health_status: "unhealthy",
            health_checked_at: TESTED_AT,
            health_error: "sink returned HTTP 503",
          }),
          ...CONNECTORS.slice(1),
        ]
      : CONNECTORS,
);

export const RejectedTestIsNotRepeatedOnceTheListCatchesUp: Story = {
  render: () => {
    probed = false;
    return <Harness fetchStub={repeatedProbe.stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("signoz")).toBeVisible());
    await userEvent.click(canvas.getByLabelText("Test delivery to signoz"));

    const card = within(canvas.getByRole("group", { name: "signoz" }));
    // only the settled state has the reason in a plain line: the probe's own
    // line is an alert, so this cannot pass on the moment before the refetch
    await waitFor(() => {
      const lines = card.getAllByText(/sink returned HTTP 503/);
      expect(lines).toHaveLength(1);
      expect(lines[0]).not.toHaveAttribute("role", "alert");
    });
  },
};

const failedProbe = probing(() => json({ error: { message: "probe worker unavailable" } }, 500));

export const TestThatCouldNotRunShowsOnItsCard: Story = {
  render: () => <Harness fetchStub={failedProbe.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("signoz")).toBeVisible());
    await userEvent.click(canvas.getByLabelText("Test delivery to signoz"));

    const card = within(canvas.getByRole("group", { name: "signoz" }));
    await expect(await card.findByRole("alert")).toHaveTextContent(
      "Test did not run: probe worker unavailable",
    );
    // the only message on the screen, and it is on the card it belongs to
    await expect(canvas.getAllByRole("alert")).toHaveLength(1);
  },
};

// a delete that failed reports in the confirmation while it is open. closing it
// must not orphan the failure below the grid, so the card takes it over
const refusedDelete = recording(async (_input, init) =>
  init?.method === "DELETE"
    ? json({ error: { message: "connector is referenced by a pipeline" } }, 409)
    : json(CONNECTORS),
);

export const FailedDeleteShowsOnItsCard: Story = {
  render: () => <Harness fetchStub={refusedDelete.stub} toasted />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("signoz")).toBeVisible());

    await userEvent.click(canvas.getByLabelText("Delete connector signoz"));
    await confirmDestructive(/signoz/, /delete connector/i);
    await refusedDelete.expectSent("DELETE", "/connectors/c-1");
    await expect(
      await within(await confirmation()).findByText(/referenced by a pipeline/),
    ).toBeVisible();

    await cancelConfirmation();
    const card = within(canvas.getByRole("group", { name: "signoz" }));
    await expect(await card.findByRole("alert")).toHaveTextContent(
      "Could not delete this connector: connector is referenced by a pipeline",
    );
    await expect(
      within(canvas.getByRole("group", { name: "honeycomb" })).queryByRole("alert"),
    ).toBeNull();
  },
};

// 375 px in Russian, the longest strings the card carries. a long endpoint
// wraps instead of ending in an ellipsis nobody can read past, and the footer
// row's "checked" stamp wraps under the test button rather than off the card
const LONG_ENDPOINT = "https://otlp-collector.observability.internal.example.com/v1/logs";

export const MobileInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => (
    <Harness
      fetchStub={async () =>
        json([
          connector({
            id: "c-long",
            name: "signoz-eu-central",
            endpoint: LONG_ENDPOINT,
            sampling_rate: 0.25,
          }),
          ...CONNECTORS.slice(1),
        ])
      }
    />
  ),
  play: async ({ canvasElement }) => {
    const copy = ru.pages.connectors;
    const canvas = within(canvasElement);
    // the locale decorator switches language from an effect, after first paint
    await waitFor(() => expect(canvas.getAllByText(copy.testDelivery)[0]).toBeVisible());

    const endpoint = canvas.getByText(LONG_ENDPOINT);
    await expect(endpoint).toBeVisible();
    await expect(endpoint.scrollWidth).toBeLessThanOrEqual(endpoint.clientWidth);
    await expect(getComputedStyle(endpoint).textOverflow).not.toBe("ellipsis");

    const card = canvas.getByRole("group", { name: "signoz-eu-central" });
    await expect(card.scrollWidth).toBeLessThanOrEqual(card.clientWidth);
    await expect(within(card).getByText(/^проверено /)).toHaveTextContent(
      copy.checkedAt.replace("{{time}}", formattersFor("ru").relative(CHECKED_AT)),
    );
    await expect(within(card).getByText(/^выборка /)).toHaveTextContent("выборка 25%");
    await expectNoHorizontalOverflow();
  },
};

const sentRussian = created();

export const SamplingErrorInRussianAtMobile: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => <Harness fetchStub={sentRussian.stub} />,
  play: async ({ canvasElement }) => {
    const copy = ru.pages.connectors;
    await waitFor(() => expect(within(canvasElement).getByText("signoz")).toBeVisible());
    await waitFor(() =>
      expect(within(canvasElement).getAllByText(copy.testDelivery)[0]).toBeVisible(),
    );
    const form = await openAddSheet(canvasElement, copy);
    await expect(await form.findByText(copy.form.samplingHint)).toBeVisible();

    const sampling = await form.findByLabelText(copy.form.sampling);
    await userEvent.clear(sampling);
    await userEvent.type(sampling, "150");
    await expect(await form.findByText(copy.form.samplingRange)).toBeVisible();
    await expect(form.getByRole("button", { name: ru.common.create })).toBeDisabled();
    await expectNoHorizontalOverflow();
    sentRussian.expectNotSent("POST", "/api/v1/connectors");
  },
};
