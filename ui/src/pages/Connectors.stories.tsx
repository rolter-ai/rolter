import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Connectors from "./Connectors";
import {
  answerDiscardPrompt,
  cancelConfirmation,
  clickWhenEnabled,
  confirmation,
  confirmDestructive,
  expectAllowed,
  expectClosesWithoutPrompting,
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
import type { ConnectorRow, PublicUrl } from "@/lib/api";
import { formattersFor } from "@/lib/i18n/format";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import {
  atMobile,
  atShort,
  expectInViewport,
  expectNoHorizontalOverflow,
} from "@/lib/story-viewport";

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

// what the control plane reports as its own address. deliberately not the
// Storybook origin: an address built from `window.location` would pass a story
// that only compared against it (#2106)
const PUBLIC_BASE = "https://rolter.acme.example";
const PUBLIC_URL: PublicUrl = { public_url: PUBLIC_BASE, configured: true };
const CONFIG_URL = `${PUBLIC_BASE}/api/v1/connectors/collector-config`;
// `ROLTER_PUBLIC_URL` unset: the control plane falls back to its default
const DEFAULT_BASE = "http://localhost:4001";
const UNSET: PublicUrl = { public_url: DEFAULT_BASE, configured: false };

/**
 * Answer the config endpoint with `config`, the public base with `publicUrl`,
 * everything else with the list.
 */
function withConfig(
  config: () => Response | Promise<Response>,
  connectors: ConnectorRow[] = CONNECTORS,
  publicUrl: () => Response | Promise<Response> = () => json(PUBLIC_URL),
): FetchStub {
  return async (input) => {
    const url = String(input);
    if (url.includes("collector-config")) return config();
    if (new URL(url, "http://localhost").pathname === "/api/v1/public-url") return publicUrl();
    return json(connectors);
  };
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

// a connector that is switched off has to read as off, not as a broken one:
// health is its own axis, so a never-tested connector says `unknown` and
// nothing else on its card said that nothing is being sent (#2349)
export const SwitchedOffConnectorReadsAsOff: Story = {
  render: () => <Harness fetchStub={async () => json(CONNECTORS)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("signoz")).toBeVisible());

    const off = within(canvas.getByRole("group", { name: "honeycomb" }));
    await expect(off.getByText(en.pages.connectors.off)).toBeVisible();
    await expect(off.getByRole("switch", { name: "Enable honeycomb" })).toHaveAttribute(
      "aria-checked",
      "false",
    );
    // off is not unhealthy: nothing has been tested, and nothing is wrong
    await expect(off.getByText("unknown")).toBeVisible();
    await expect(off.queryByText("unhealthy")).toBeNull();
    await expect(off.queryByRole("alert")).toBeNull();

    // the ones that are sending say nothing of the kind
    for (const name of ["signoz", "datadog-staging"]) {
      const on = within(canvas.getByRole("group", { name }));
      await expect(on.queryByText(en.pages.connectors.off)).toBeNull();
      await expect(on.getByRole("switch", { name: `Enable ${name}` })).toHaveAttribute(
        "aria-checked",
        "true",
      );
    }
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

/** Open the collector-config dialog from the toolbar and return it. */
async function openCollectorConfig(canvasElement: HTMLElement) {
  const canvas = within(canvasElement);
  await waitFor(() => expect(canvas.getByText("signoz")).toBeVisible());
  await userEvent.click(canvas.getByRole("button", { name: /Collector config/ }));
  return within(await within(document.body).findByRole("dialog"));
}

// defining a connector delivers nothing on its own — a collector has to be
// running the config rendered from it (#1195, ADR-0026). the screen has to be
// able to show that document, and say where it goes
export const CollectorConfig: Story = {
  render: () => <Harness fetchStub={withConfig(() => yaml(COLLECTOR_CONFIG))} />,
  play: async ({ canvasElement }) => {
    const dialog = await openCollectorConfig(canvasElement);
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

// the endpoint line is the control plane's own address, in full and copyable
// (#2106). it was a bare path, which no script can call, and building it from
// `window.location` would hand out the dashboard's address instead of the one
// the control plane answers on
export const CollectorConfigShowsTheFullEndpointUrl: Story = {
  render: () => <Harness fetchStub={withConfig(() => yaml(COLLECTOR_CONFIG))} />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors.collectorConfig;
    const dialog = await openCollectorConfig(canvasElement);
    const group = await dialog.findByRole("group", { name: copy.endpoint });
    const url = await within(group).findByTestId("collector-config-url");

    await expect(url.textContent).toBe(CONFIG_URL);
    // the base came from the control plane, not from the address this page is
    // served from
    await expect(window.location.origin).not.toBe(PUBLIC_BASE);
    await expect(url.textContent).not.toContain(window.location.origin);
    // a configured base raises no warning
    await expect(within(group).queryByRole("note")).toBeNull();

    // named for what it copies, and copies the address itself
    const button = within(group).getByRole("button", {
      name: en.common.copyValue
        .replace("{{label}}", copy.copyEndpoint)
        .replace("{{value}}", CONFIG_URL),
    });
    const written: string[] = [];
    const original = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: async (value: string) => void written.push(value) },
      configurable: true,
    });
    try {
      await userEvent.click(button);
      await waitFor(() => expect(written).toEqual([CONFIG_URL]));
    } finally {
      if (original) Object.defineProperty(navigator, "clipboard", original);
      else Reflect.deleteProperty(navigator, "clipboard");
    }
  },
};

// the advice used to end "or point the collector straight at the URL above".
// that URL answers a superadmin only and a collector has no session, so
// following it meant the admin token in the collector's deployment (#2106). the
// dialog now says what the endpoint takes, and sends the operator to the saved
// file
export const CollectorConfigSaysWhatTheEndpointTakes: Story = {
  render: () => <Harness fetchStub={withConfig(() => yaml(COLLECTOR_CONFIG))} />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors.collectorConfig;
    const dialog = await openCollectorConfig(canvasElement);
    const group = await dialog.findByRole("group", { name: copy.endpoint });
    await expect(within(group).getByText(copy.endpointHint)).toBeVisible();
    await expect(within(group).getByText(/superadmin credential/)).toBeVisible();

    // the recommendation is the saved file, re-saved when the connectors change
    await waitFor(() => expect(dialog.getByText(copy.deploy)).toBeVisible());
    // and nothing in the dialog tells a collector to read the endpoint itself
    const text = document.body.textContent ?? "";
    await expect(text).not.toMatch(/point the collector straight at the URL/);
    await expect(text).not.toContain("--config");
  },
};

// `ROLTER_PUBLIC_URL` unset: the control plane's base is its default, which only
// a caller on its own host can reach. the address is still shown and copyable,
// with that said under it instead of leaving a script's connection error to
export const CollectorConfigWarnsWhenThePublicUrlIsUnset: Story = {
  render: () => (
    <Harness
      fetchStub={withConfig(
        () => yaml(COLLECTOR_CONFIG),
        CONNECTORS,
        () => json(UNSET),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors.collectorConfig;
    const dialog = await openCollectorConfig(canvasElement);
    const group = await dialog.findByRole("group", { name: copy.endpoint });
    const url = await within(group).findByTestId("collector-config-url");
    await expect(url.textContent).toBe(`${DEFAULT_BASE}/api/v1/connectors/collector-config`);
    const notice = within(group).getByRole("note");
    await expect(notice).toHaveTextContent("ROLTER_PUBLIC_URL is not set");
    await expect(notice).toHaveTextContent(/restart the control plane/);
  },
};

// the public base is still in flight: the document is on screen, and the
// address holds its space as a labelled skeleton instead of claiming a value
export const CollectorConfigWaitsForThePublicUrl: Story = {
  render: () => (
    <Harness
      fetchStub={withConfig(
        () => yaml(COLLECTOR_CONFIG),
        CONNECTORS,
        () => new Promise<Response>(() => {}),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openCollectorConfig(canvasElement);
    await waitFor(() =>
      expect(
        dialog.getByRole("region", { name: /OpenTelemetry Collector config/i }),
      ).toHaveTextContent("otlphttp/signoz"),
    );
    await expectSkeleton(document.body);
    await expect(dialog.queryByTestId("collector-config-url")).toBeNull();
    await expect(dialog.queryByRole("note")).toBeNull();
  },
};

// the base could not be read. the address is not guessed from the browser: the
// dialog says what failed and offers a retry, and the address appears once the
// retry lands. the document below is unaffected
export const CollectorConfigPublicUrlUnreadable: Story = {
  render: () => {
    let reads = 0;
    return (
      <Harness
        fetchStub={withConfig(
          () => yaml(COLLECTOR_CONFIG),
          CONNECTORS,
          () =>
            ++reads === 1
              ? json({ error: { message: "upstream unavailable" } }, 502)
              : json(PUBLIC_URL),
        )}
      />
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openCollectorConfig(canvasElement);
    await expectLoadError(document.body, /failed to return the public URL/i);
    await expect(dialog.queryByTestId("collector-config-url")).toBeNull();
    await expect(
      await dialog.findByRole("region", { name: /OpenTelemetry Collector config/i }),
    ).toHaveTextContent("otlphttp/signoz");

    await userEvent.click(dialog.getByRole("button", { name: "Try again" }));
    await expect((await dialog.findByTestId("collector-config-url")).textContent).toBe(CONFIG_URL);
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
    const copy = en.pages.connectors.collectorConfig;
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText(/No connectors yet/)).toBeVisible());
    await userEvent.click(canvas.getByRole("button", { name: /Collector config/ }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await expect(dialog.getByText(/Nothing to deliver yet/)).toBeVisible();
    // the other empty state is for connectors that exist
    await expect(dialog.queryByText(copy.allOffTitle)).toBeNull();
  },
};

// connectors exist but none is switched on, which is where an operator lands
// right after the first create, since the add sheet creates one switched off
// (#2349). the document is rendered from the enabled rows alone, so showing it
// would be a valid file with no exporters in it, presented as deployable
// (#2364). the dialog says which case it is and what to do, and never asks the
// control plane for a document it is not going to show
const ALL_OFF = CONNECTORS.map((row) => ({ ...row, enabled: false }));
const allOff = recording(withConfig(() => yaml(COLLECTOR_CONFIG), ALL_OFF));

export const CollectorConfigAllSwitchedOff: Story = {
  render: () => <Harness fetchStub={allOff.stub} />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors.collectorConfig;
    const dialog = await openCollectorConfig(canvasElement);
    await expect(await dialog.findByText(copy.allOffTitle)).toBeVisible();
    // what a switched-off connector does to the document, and the way out
    await expect(dialog.getByText(copy.allOffBody)).toBeVisible();
    await expect(dialog.getByText(copy.allOffBody)).toHaveTextContent(/switched on/);
    await expect(dialog.getByText(copy.allOffBody)).toHaveTextContent(/from its card/);

    // not the "no connectors" state: there are three, and the copy is for them
    await expect(dialog.queryByText(copy.emptyTitle)).toBeNull();
    // no document to paste, and no address to fetch one from
    await expect(
      dialog.queryByRole("region", { name: /OpenTelemetry Collector config/i }),
    ).toBeNull();
    await expect(dialog.queryByRole("group", { name: copy.endpoint })).toBeNull();
    allOff.expectNotSent("GET", "collector-config");
  },
};

// one connector on is enough for a document worth saving, however many are off
export const CollectorConfigWithOneSwitchedOn: Story = {
  render: () => (
    <Harness
      fetchStub={withConfig(
        () => yaml(COLLECTOR_CONFIG),
        ALL_OFF.map((row) => (row.id === "c-2" ? { ...row, enabled: true } : row)),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors.collectorConfig;
    const dialog = await openCollectorConfig(canvasElement);
    await expect(
      await dialog.findByRole("region", { name: /OpenTelemetry Collector config/i }),
    ).toHaveTextContent("otlphttp/signoz");
    await expect(dialog.queryByText(copy.allOffTitle)).toBeNull();
    await expect(dialog.queryByText(copy.emptyTitle)).toBeNull();
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
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
    // no card, so no edit on it either
    await expect(
      within(canvasElement).queryByRole("button", { name: /Edit connector/ }),
    ).toBeNull();
  },
};

export const RefusedToAViewer: Story = {
  render: () => <Harness fetchStub={withConfig(() => yaml(COLLECTOR_CONFIG))} role="viewer" />,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
    await expect(
      within(canvasElement).queryByRole("button", { name: /Edit connector/ }),
    ).toBeNull();
  },
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

// the add sheet starts a connector switched off unless the operator turns the
// switch on (#2349). it used to send `enabled: true` with no control for it, so
// request logs began leaving for an external endpoint the moment Create landed
// and the first test delivery could only come after the first real record
const startedOff = created({ enabled: false });

export const CreatesSwitchedOffUntilTheOperatorTurnsItOn: Story = {
  render: () => <Harness fetchStub={startedOff.stub} toasted />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors;
    const form = await openAddSheet(canvasElement);

    // the switch is on the sheet, named for what it does, and off
    await expect(form.getByRole("switch", { name: copy.form.start })).toHaveAttribute(
      "aria-checked",
      "false",
    );
    await expect(form.getByText(copy.form.startHint)).toBeVisible();

    await userEvent.click(form.getByRole("button", { name: en.common.create }));
    const body = await startedOff.expectSentBody<SentConnector>("POST", "/api/v1/connectors");
    await expect(body.enabled).toBe(false);
  },
};

const startedOn = created({ enabled: true });

export const StartsSendingWhenTheSwitchIsTurnedOn: Story = {
  render: () => <Harness fetchStub={startedOn.stub} toasted />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors;
    const canvas = within(canvasElement);
    const form = await openAddSheet(canvasElement);

    const start = form.getByRole("switch", { name: copy.form.start });
    await userEvent.click(start);
    await expect(start).toHaveAttribute("aria-checked", "true");
    // the choice that sends request logs out says so, in place of the default's hint
    await expect(form.getByText(copy.form.startOnHint)).toBeVisible();
    await expect(form.queryByText(copy.form.startHint)).toBeNull();

    await userEvent.click(form.getByRole("button", { name: en.common.create }));
    const body = await startedOn.expectSentBody<SentConnector>("POST", "/api/v1/connectors");
    await expect(body.enabled).toBe(true);

    // one that is on has nothing to add to the plain confirmation
    await expectToast(canvasElement, /audit-sink created/);
    await expect(canvas.queryByText(/switched off/)).toBeNull();
  },
};

// the switch is part of the draft, and a reopened sheet starts from the default
// again rather than from the last draft: a switch left on would make the next
// connector live by accident
export const SwitchStartsOffAgainWhenTheSheetReopens: Story = {
  render: () => <Harness fetchStub={created().stub} />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors;
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("signoz")).toBeVisible());
    await clickWhenEnabled(canvasElement, copy.add);
    const form = within(await within(document.body).findByRole("dialog"));
    const start = await form.findByRole("switch", { name: copy.form.start });
    await userEvent.click(start);
    await expect(start).toHaveAttribute("aria-checked", "true");

    // nothing else was typed, and closing still asks before throwing it away
    await userEvent.click(form.getByRole("button", { name: en.common.cancel }));
    await answerDiscardPrompt(true);
    await expectSheetClosed();

    await clickWhenEnabled(canvasElement, copy.add);
    const reopened = within(await within(document.body).findByRole("dialog"));
    await expect(await reopened.findByRole("switch", { name: copy.form.start })).toHaveAttribute(
      "aria-checked",
      "false",
    );
  },
};

// a connector left off has to say so once the sheet has closed, and what to do
// next, or the operator is left wondering why nothing arrives (#2349)
const feedback = created({ enabled: false });

export const SaysANewConnectorIsOffAndWhatToDoNext: Story = {
  render: () => <Harness fetchStub={feedback.stub} toasted />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors;
    const canvas = within(canvasElement);
    const form = await openAddSheet(canvasElement);
    await userEvent.click(form.getByRole("button", { name: en.common.create }));
    await feedback.expectSent("POST", "/api/v1/connectors");

    await expectSheetClosed();
    await expectToast(canvasElement, /audit-sink created, but switched off/);
    // the next step names the two controls on the card that do it
    const next = await canvas.findByText(copy.createdOffNext);
    await expect(next).toBeVisible();
    await expect(next).toHaveTextContent(/Test delivery/);
    await expect(next).toHaveTextContent(/switch/);
  },
};

// the edit sheet (#2101): a connector could only be deleted and added again,
// which took its delivery history with it and left a window with no export
// while a rotated token was being typed in. an edit is one PUT to the same id.
// the control plane replaces the whole row from the body, so the fields the
// sheet has no control for go back as found, and it reads an absent
// `managed_auth_secret` as "keep the stored one"
interface SentUpdate {
  name: string;
  kind: string;
  endpoint: string;
  enabled: boolean;
  sampling_rate: number;
  auth_secret_ref: string | null;
  managed_auth_secret?: string;
}

const editing = (list: ConnectorRow[] = CONNECTORS) =>
  recording(async (_input, init) => (init?.method === "PUT" ? json(list[0]) : json(list)));

/** Open the edit sheet for `name` and wait for it to be filled in. */
async function openEditSheet(
  canvasElement: HTMLElement,
  name = "signoz",
  label = `Edit connector ${name}`,
  copy = en.pages.connectors,
) {
  const canvas = within(canvasElement);
  await waitFor(() => expect(canvas.getByText(name)).toBeVisible());
  await clickWhenEnabled(canvasElement, label);
  const form = within(await within(document.body).findByRole("dialog", { name: label }));
  // the sheet seeds its rows from an effect, a step after it opens
  await waitFor(() => expect(form.getByLabelText(copy.form.name)).toHaveValue(name));
  return form;
}

const keepsSecret = editing();

export const EditsAConnectorInPlaceKeepingItsSecret: Story = {
  render: () => <Harness fetchStub={keepsSecret.stub} toasted />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors;
    const form = await openEditSheet(canvasElement);

    // the row as stored, with the rate read as a percentage
    await expect(form.getByLabelText(copy.form.endpoint)).toHaveValue(
      "https://collector.example.com/v1/logs",
    );
    await expect(form.getByLabelText(copy.form.sampling)).toHaveValue(100);
    // the stored secret is never read back, and the field says what a blank does
    const secret = form.getByLabelText(copy.form.secretEdit);
    await expect(secret).toHaveValue("");
    await expect(form.getByText(copy.form.secretKeepHint)).toBeVisible();
    // `enabled` belongs to the card's switch: the sheet has no start switch
    await expect(form.queryByRole("switch")).toBeNull();

    const endpoint = form.getByLabelText(copy.form.endpoint);
    await userEvent.clear(endpoint);
    await userEvent.type(endpoint, "https://collector.example.com/v2/logs");
    await userEvent.click(form.getByRole("button", { name: en.common.save }));

    const body = await keepsSecret.expectSentBody<SentUpdate>("PUT", "/connectors/c-1");
    // exactly these keys: no `managed_auth_secret`, so the stored one stays, and
    // the switch goes back as it was found rather than as a create would send it
    await expect(body).toEqual({
      name: "signoz",
      kind: "otlp_http",
      endpoint: "https://collector.example.com/v2/logs",
      enabled: true,
      sampling_rate: 1,
      auth_secret_ref: null,
    });
    await expect(body).not.toHaveProperty("managed_auth_secret");
    // the same row, not a new one and not the old one gone
    keepsSecret.expectNotSent("POST", "/api/v1/connectors");
    keepsSecret.expectNotSent("DELETE", "/connectors");
    await expectSheetClosed();
    // the card keeps the health the last test recorded, which now describes
    // the old endpoint, so the confirmation says to test again
    await expectToast(canvasElement, /health still describes the old endpoint or secret/);
  },
};

const renames = editing();

// nothing about where the records go changed, so there is nothing to test again
export const EditingOnlyTheNameDoesNotAskForANewTest: Story = {
  render: () => <Harness fetchStub={renames.stub} toasted />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors;
    const canvas = within(canvasElement);
    const form = await openEditSheet(canvasElement);
    const name = form.getByLabelText(copy.form.name);
    await userEvent.clear(name);
    await userEvent.type(name, "signoz-eu");
    await userEvent.click(form.getByRole("button", { name: en.common.save }));

    const body = await renames.expectSentBody<SentUpdate>("PUT", "/connectors/c-1");
    await expect(body).toMatchObject({ name: "signoz-eu", sampling_rate: 1, enabled: true });
    await expectToast(canvasElement, /signoz-eu updated\./);
    await expect(canvas.queryByText(/health still describes/)).toBeNull();
  },
};

const replaces = editing();

export const ReplacesTheSecretWhenOneIsTyped: Story = {
  render: () => <Harness fetchStub={replaces.stub} toasted />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors;
    const form = await openEditSheet(canvasElement);
    const secret = form.getByLabelText(copy.form.secretEdit);
    await userEvent.type(secret, "rotated-token");
    // said before the save, not found out after it
    await expect(await form.findByText(copy.form.secretReplaceHint)).toBeVisible();
    await expect(form.queryByText(copy.form.secretKeepHint)).toBeNull();

    await userEvent.click(form.getByRole("button", { name: en.common.save }));
    const body = await replaces.expectSentBody<SentUpdate>("PUT", "/connectors/c-1");
    await expect(body).toEqual({
      name: "signoz",
      kind: "otlp_http",
      endpoint: "https://collector.example.com/v1/logs",
      enabled: true,
      sampling_rate: 1,
      auth_secret_ref: null,
      managed_auth_secret: "rotated-token",
    });
    replaces.expectNotSent("DELETE", "/connectors");
    await expectSheetClosed();
    // the new credential has not been tried, whatever the card said before
    await expectToast(canvasElement, /health still describes the old endpoint or secret/);
  },
};

const blankSecret = editing();

// the control plane refuses an empty `managed_auth_secret`, and a field of
// spaces is empty to a person, so it is left out rather than sent to be refused
export const ASecretOfSpacesIsNotSent: Story = {
  render: () => <Harness fetchStub={blankSecret.stub} />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors;
    const form = await openEditSheet(canvasElement);
    await userEvent.type(form.getByLabelText(copy.form.secretEdit), "   ");
    await expect(form.getByText(copy.form.secretKeepHint)).toBeVisible();
    await userEvent.click(form.getByRole("button", { name: en.common.save }));
    const body = await blankSecret.expectSentBody<SentUpdate>("PUT", "/connectors/c-1");
    await expect(body).not.toHaveProperty("managed_auth_secret");
  },
};

const moves = editing();

// when the endpoint moves to another origin the control plane drops the stored
// secret on save unless a new one is typed, and the sheet says so. the body is
// the proof: a save with the field blank carries no secret
export const SaysWhatBecomesOfTheSecretWhenTheEndpointMoves: Story = {
  render: () => <Harness fetchStub={moves.stub} />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors.form;
    const form = await openEditSheet(canvasElement);
    const endpoint = form.getByLabelText(en.pages.connectors.form.endpoint);

    // another path on the same origin changes nothing about the secret
    await userEvent.clear(endpoint);
    await userEvent.type(endpoint, "https://collector.example.com/other");
    await expect(form.getByText(copy.secretKeepHint)).toBeVisible();
    await expect(form.queryByText(copy.secretMovesHint)).toBeNull();

    // another host does
    await userEvent.clear(endpoint);
    await userEvent.type(endpoint, "https://collector.example.net/v1/logs");
    await expect(form.getByText(copy.secretMovesHint)).toBeVisible();
    await expect(form.queryByText(copy.secretKeepHint)).toBeNull();

    // typing the new endpoint's own secret settles it
    const secret = form.getByLabelText(copy.secretEdit);
    await userEvent.type(secret, "other-token");
    await expect(form.getByText(copy.secretReplaceHint)).toBeVisible();
    await expect(form.queryByText(copy.secretMovesHint)).toBeNull();
    await userEvent.clear(secret);
    await expect(form.getByText(copy.secretMovesHint)).toBeVisible();

    await userEvent.click(form.getByRole("button", { name: en.common.save }));
    const body = await moves.expectSentBody<SentUpdate>("PUT", "/connectors/c-1");
    await expect(body.endpoint).toBe("https://collector.example.net/v1/logs");
    await expect(body).not.toHaveProperty("managed_auth_secret");
  },
};

// a connector with no stored secret has nothing to keep, drop or send
const PRECISE = connector({
  id: "c-5",
  name: "precise",
  enabled: false,
  // more digits than the percentage field shows
  sampling_rate: 0.123456789012345,
  auth_secret_ref: "vault:kv/otlp/token",
  auth_secret_configured: false,
  health_status: "unknown",
  health_checked_at: null,
});
const untouched = editing([PRECISE, ...CONNECTORS]);

export const AnEditSendsBackWhatItDidNotChange: Story = {
  render: () => <Harness fetchStub={untouched.stub} toasted />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors;
    const form = await openEditSheet(canvasElement, "precise");
    await expect(form.getByText(copy.form.secretNoneHint)).toBeVisible();

    const name = form.getByLabelText(copy.form.name);
    await userEvent.clear(name);
    await userEvent.type(name, "precise-2");
    await userEvent.click(form.getByRole("button", { name: en.common.save }));

    const body = await untouched.expectSentBody<SentUpdate>("PUT", "/connectors/c-5");
    // the rate the field rounded for display goes back exactly as stored
    await expect(body.sampling_rate).toBe(0.123456789012345);
    // a switched-off connector stays off, and the external reference is not
    // dropped by a PUT that would otherwise replace it with nothing
    await expect(body.enabled).toBe(false);
    await expect(body.auth_secret_ref).toBe("vault:kv/otlp/token");
    await expect(body).not.toHaveProperty("managed_auth_secret");
    // never tested, so its `unknown` health does not describe anything old
    await expectToast(canvasElement, /precise-2 updated\./);
  },
};

const resamples = editing();

export const EditsTheSamplingRate: Story = {
  render: () => <Harness fetchStub={resamples.stub} />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors.form;
    const form = await openEditSheet(canvasElement);
    const sampling = form.getByLabelText(copy.sampling);
    await userEvent.clear(sampling);
    await userEvent.type(sampling, "25");
    await userEvent.click(form.getByRole("button", { name: en.common.save }));
    const body = await resamples.expectSentBody<SentUpdate>("PUT", "/connectors/c-1");
    await expect(body.sampling_rate).toBe(0.25);
  },
};

const parks = editing();

// 0 is a rate, on an edit as on a create: it parks the connector without
// deleting it
export const EditingTheSamplingRateToZeroIsKept: Story = {
  render: () => <Harness fetchStub={parks.stub} />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors.form;
    const form = await openEditSheet(canvasElement);
    const sampling = form.getByLabelText(copy.sampling);
    await userEvent.clear(sampling);
    await userEvent.type(sampling, "0");
    await expect(sampling).not.toHaveAttribute("aria-invalid", "true");
    await userEvent.click(form.getByRole("button", { name: en.common.save }));
    const body = await parks.expectSentBody<SentUpdate>("PUT", "/connectors/c-1");
    await expect(body.sampling_rate).toBe(0);
  },
};

const refusedSampling = editing();

export const EditRefusesASamplingRateOutsideZeroToOneHundred: Story = {
  render: () => <Harness fetchStub={refusedSampling.stub} />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors.form;
    const form = await openEditSheet(canvasElement);
    const sampling = form.getByLabelText(copy.sampling);
    const save = form.getByRole("button", { name: en.common.save });

    await userEvent.clear(sampling);
    await userEvent.type(sampling, "150");
    await expect(await form.findByText(copy.samplingRange)).toBeVisible();
    await expect(sampling).toHaveAttribute("aria-invalid", "true");
    await expect(sampling).toHaveAccessibleDescription(copy.samplingRange);
    await expect(save).toBeDisabled();

    await userEvent.clear(sampling);
    await expect(await form.findByText(copy.samplingInvalid)).toBeVisible();
    await expect(save).toBeDisabled();
    refusedSampling.expectNotSent("PUT", "/connectors/c-1");

    await userEvent.type(sampling, "25");
    await waitFor(() => expect(save).toBeEnabled());
  },
};

const rejected = recording(async (_input, init) =>
  init?.method === "PUT"
    ? json({ error: { message: "endpoint must be an http(s) URL" } }, 400)
    : json(CONNECTORS),
);

// the control plane can still refuse what the form let through. the sheet stays
// open on the draft with the reason in it, and the toast names the connector
export const ARefusedEditKeepsTheSheetOpen: Story = {
  render: () => <Harness fetchStub={rejected.stub} toasted />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors;
    const form = await openEditSheet(canvasElement);
    const endpoint = form.getByLabelText(copy.form.endpoint);
    await userEvent.clear(endpoint);
    await userEvent.type(endpoint, "collector.example.com");
    await userEvent.click(form.getByRole("button", { name: en.common.save }));

    await rejected.expectSent("PUT", "/connectors/c-1");
    await expect(await form.findByText(/endpoint must be an http\(s\) URL/)).toBeVisible();
    await expectToast(canvasElement, /Could not save signoz/, "error");
    // still the draft the operator typed, in the sheet they typed it in
    await expect(form.getByLabelText(copy.form.endpoint)).toHaveValue("collector.example.com");
    await expect(form.getByRole("button", { name: en.common.save })).toBeEnabled();
  },
};

// the save is on the wire: the button locks, so a second press cannot send a
// second PUT
export const SavingAnEditLocksTheSheet: Story = {
  render: () => (
    <Harness
      fetchStub={async (_input, init) =>
        init?.method === "PUT" ? new Promise<Response>(() => {}) : json(CONNECTORS)
      }
    />
  ),
  play: async ({ canvasElement }) => {
    const form = await openEditSheet(canvasElement);
    await userEvent.type(form.getByLabelText(en.pages.connectors.form.name), "-2");
    await userEvent.click(form.getByRole("button", { name: en.common.save }));
    await waitFor(() => expect(form.getByRole("button", { name: en.common.save })).toBeDisabled());
  },
};

// an edit sheet nobody touched closes without asking, one with a draft asks,
// and the next connector opened starts from its own row rather than from the
// draft that was thrown away
export const AnEditedSheetPromptsBeforeDiscarding: Story = {
  render: () => <Harness fetchStub={async () => json(CONNECTORS)} />,
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors;
    await openEditSheet(canvasElement);
    await expectClosesWithoutPrompting();

    const draft = await openEditSheet(canvasElement);
    await userEvent.type(draft.getByLabelText(copy.form.name), "-draft");
    await userEvent.click(draft.getByRole("button", { name: en.common.cancel }));
    await answerDiscardPrompt(true);
    await expectSheetClosed();

    const form = await openEditSheet(canvasElement, "honeycomb");
    await expect(form.getByLabelText(copy.form.endpoint)).toHaveValue(
      "https://collector.example.com/v1/logs",
    );
    await expect(form.getByLabelText(copy.form.sampling)).toHaveValue(10);
    // honeycomb has no secret, so its hint is the one for that
    await expect(form.getByText(copy.form.secretNoneHint)).toBeVisible();
    await expect(form.queryByText(copy.form.secretKeepHint)).toBeNull();
  },
};

// the edit takes the authority the switch beside it does. connectors are
// superadmin-only at every action, so a lesser caller is refused the whole
// screen before a card renders, and the superadmin is the one role that
// reaches the control
export const EditIsOfferedToASuperadmin: Story = {
  render: () => <Harness fetchStub={async () => json(CONNECTORS)} role="superadmin" />,
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, "Edit connector signoz");
    await expectAllowed(canvasElement, "Edit connector honeycomb");
    await expectAllowed(canvasElement, "Edit connector datadog-staging");
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
    // the start switch is the longest row the sheet carries in Russian
    await expect(form.getByRole("switch", { name: copy.form.start })).toBeVisible();
    await expect(form.getByText(copy.form.startHint)).toBeVisible();

    const sampling = await form.findByLabelText(copy.form.sampling);
    await userEvent.clear(sampling);
    await userEvent.type(sampling, "150");
    await expect(await form.findByText(copy.form.samplingRange)).toBeVisible();
    await expect(form.getByRole("button", { name: ru.common.create })).toBeDisabled();
    await expectNoHorizontalOverflow();
    sentRussian.expectNotSent("POST", "/api/v1/connectors");
  },
};

// the address, its note and the document together are taller than a 640x360
// window (1280x720 at 200 % zoom), so the body scrolls between a title and a
// Close that stay on screen (#2003)
export const CollectorConfigInAShortWindow: Story = {
  ...atShort,
  render: () => (
    <Harness
      fetchStub={withConfig(
        () => yaml(COLLECTOR_CONFIG),
        CONNECTORS,
        () => json(UNSET),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const copy = en.pages.connectors.collectorConfig;
    const dialog = await openCollectorConfig(canvasElement);
    const panel = within(document.body).getByRole("dialog");
    await expectInViewport(panel);
    await expectInViewport(dialog.getByRole("heading", { name: copy.title }));
    // the corner X and the footer's Close, both of them
    for (const close of dialog.getAllByRole("button", { name: en.common.close })) {
      await expectInViewport(close);
    }

    // the body is what gave way, and the document is a scroll away
    const document_ = await dialog.findByRole("region", {
      name: /OpenTelemetry Collector config/i,
    });
    const body = document_.closest<HTMLElement>("[data-slot=dialog-body]");
    await expect(body).not.toBeNull();
    await expect(body!.scrollHeight).toBeGreaterThan(body!.clientHeight);
  },
};

// the dialog in Russian at 375 px: the address is one unbroken token that has to
// wrap inside the dialog, and the hint under it is the longest line it carries
export const CollectorConfigInRussianAtMobile: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => (
    <Harness
      fetchStub={withConfig(
        () => yaml(COLLECTOR_CONFIG),
        CONNECTORS,
        () =>
          json({
            public_url: "https://rolter-control.observability.internal.example.com",
            configured: true,
          }),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const copy = ru.pages.connectors.collectorConfig;
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByRole("button", { name: copy.open })).toBeVisible());
    await userEvent.click(canvas.getByRole("button", { name: copy.open }));
    const dialog = within(await within(document.body).findByRole("dialog"));

    const group = await dialog.findByRole("group", { name: copy.endpoint });
    const url = await within(group).findByTestId("collector-config-url");
    await expect(url.textContent).toBe(
      "https://rolter-control.observability.internal.example.com/api/v1/connectors/collector-config",
    );
    await expect(url.scrollWidth).toBeLessThanOrEqual(url.clientWidth);
    await expect(within(group).getByText(copy.endpointHint)).toBeVisible();
    await expect(
      within(group).getByRole("button", { name: new RegExp(`^${copy.copyEndpoint}`) }),
    ).toBeVisible();
    await waitFor(() => expect(dialog.getByText(copy.deploy)).toBeVisible());
    await expectNoHorizontalOverflow();
  },
};

// the all-off notice in Russian at 375 px: a title and a body that say what a
// switched-off connector does to the document have to wrap inside the dialog
export const CollectorConfigAllSwitchedOffInRussianAtMobile: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => <Harness fetchStub={withConfig(() => yaml(COLLECTOR_CONFIG), ALL_OFF)} />,
  play: async ({ canvasElement }) => {
    const copy = ru.pages.connectors.collectorConfig;
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByRole("button", { name: copy.open })).toBeEnabled());
    await userEvent.click(canvas.getByRole("button", { name: copy.open }));
    const dialog = within(await within(document.body).findByRole("dialog"));

    await expect(await dialog.findByText(copy.allOffTitle)).toBeVisible();
    const body = dialog.getByText(copy.allOffBody);
    await expect(body).toBeVisible();
    await expect(body.scrollWidth).toBeLessThanOrEqual(body.clientWidth);
    await expectNoHorizontalOverflow();
  },
};

const editsInRussian = editing();

// the edit sheet in Russian at 375 px: the hint that says where a stored secret
// would go is the longest line the sheet carries, and the title names the
// connector
export const EditSheetInRussianAtMobile: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => <Harness fetchStub={editsInRussian.stub} />,
  play: async ({ canvasElement }) => {
    const copy = ru.pages.connectors;
    await waitFor(() =>
      expect(within(canvasElement).getAllByText(copy.testDelivery)[0]).toBeVisible(),
    );
    const form = await openEditSheet(
      canvasElement,
      "signoz",
      copy.editAria.replace("{{name}}", "signoz"),
      copy,
    );
    await expect(form.getByText(copy.form.secretKeepHint)).toBeVisible();

    const endpoint = form.getByLabelText(copy.form.endpoint);
    await userEvent.clear(endpoint);
    await userEvent.type(endpoint, "https://collector.example.net/v1/logs");
    const hint = await form.findByText(copy.form.secretMovesHint);
    await expect(hint).toBeVisible();
    await expect(hint.scrollWidth).toBeLessThanOrEqual(hint.clientWidth);
    await expect(form.getByRole("button", { name: ru.common.save })).toBeEnabled();
    await expectNoHorizontalOverflow();
    // nothing in the sheet is a start switch, and nothing left for Create
    await expect(form.queryByRole("switch")).toBeNull();
    editsInRussian.expectNotSent("PUT", "/connectors/c-1");
  },
};
