import type { Meta, StoryObj } from "@storybook/react-vite";
import { MemoryRouter } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Config from "./Config";
import { ConfigExportDialog } from "@/components/ConfigExportDialog";
import {
  Harness,
  Toasted,
  expectAllowed,
  expectEmptyState,
  expectGateAnswered,
  expectListTable,
  expectLoadError,
  expectNoFalseEmpty,
  expectRefused,
  expectSkeleton,
  expectToast,
  json,
  pending,
  recording,
  scoped,
  withCapabilities,
  type FetchStub,
  type StoryRole,
} from "./story-harness";
import type { ClusterNodeRow } from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";

// the screen links its related settings through react-router, so the story
// supplies a router the same way main.tsx does
// `toasted` is opt-in: the `Toaster` contributes live regions of its own, and
// the stories that query `role="alert"` for the screen's own LoadError would
// stop being able to tell the two apart
function Stage({
  fetchStub,
  role,
  toasted = false,
}: {
  fetchStub: FetchStub;
  role?: StoryRole;
  toasted?: boolean;
}) {
  return (
    <MemoryRouter>
      <Harness fetchStub={fetchStub} role={role}>
        {toasted ? (
          <Toasted>
            <Config />
          </Toasted>
        ) : (
          <Config />
        )}
      </Harness>
    </MemoryRouter>
  );
}

// a slice of what GET /api/v1/config returns: the three tabled sections plus
// a handful of the ~40 generic ones the screen renders collapsed (#1204)
const CONFIG = {
  providers: [
    { name: "openai-prod", kind: "openai", api_base: "https://api.openai.com/v1" },
    { name: "vllm-local", kind: "openai_compatible", api_base: "http://vllm.internal:8000/v1" },
    // a kind a later control plane gained, which this catalog does not name
    { name: "edge-gw", kind: "brand_new_kind", api_base: "https://edge.example.com" },
  ],
  routes: [
    {
      model: "gpt-4o",
      strategy: "round_robin",
      targets: [{ provider: "openai-prod", model: "gpt-4o-2024-08-06", weight: 3 }],
    },
    // the endpoint strips tenancy, so a second org's route can repeat a name
    { model: "gpt-4o", strategy: "failover", targets: [] },
  ],
  virtual_keys: [{ key: "[redacted]", name: "ci-bot", models: ["gpt-4o"] }],
  db_virtual_keys: [{ key_hash: "", id: "k1" }],
  mcp_oauth_sessions: [],
  server: { host: "0.0.0.0", port: 4000, workers: 4 },
  cache: { enabled: true, ttl_secs: 300 },
  budgets: [],
  unpriced_policy: "ignore",
};

// one cluster node as `GET /api/v1/cluster/nodes` lists it
const node = (over: Partial<ClusterNodeRow>): ClusterNodeRow => ({
  id: "gw-1",
  role: "gateway",
  build_version: "0.9.0",
  config_version: 12,
  desired_state: "active",
  state_changed_at: "2026-09-01T08:00:00Z",
  first_seen_at: "2026-09-01T08:00:00Z",
  last_seen_at: "2026-09-01T09:00:00Z",
  live: true,
  converged: true,
  ...over,
});

// two live gateways on the current version, plus what the line must not count:
// one that stopped polling and the control plane itself
const CONVERGED = [
  node({ id: "gw-1" }),
  node({ id: "gw-2" }),
  node({ id: "gw-old", live: false, converged: false, config_version: 9 }),
  node({ id: "cp-1", role: "control" }),
];

// the organizations the export dialog offers
const ORGS = [
  { id: "org-acme", name: "Acme", slug: "acme", created_at: "2026-01-01T00:00:00Z" },
  { id: "org-globex", name: "Globex", slug: "globex", created_at: "2026-01-01T00:00:00Z" },
];

/**
 * The control plane as this screen talks to it: the stored config, the lines
 * the snapshot leaves out, the cluster inventory and the export. Each answer
 * can be swapped for one story; everything else answers like a healthy
 * deployment with nothing left out.
 */
function api({
  config = () => json(CONFIG),
  problems = () => json({ problems: [] }),
  nodes = () => json(CONVERGED),
  exported = () => json({ error: { message: "not stubbed" } }, 500),
  orgs = () => json(ORGS),
}: {
  config?: () => Response;
  problems?: () => Response;
  nodes?: () => Response;
  exported?: () => Response;
  orgs?: () => Response | Promise<Response>;
} = {}): FetchStub {
  const rest = scoped(async (input) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (path === "/api/v1/config") return config();
    if (path === "/api/v1/config/problems") return problems();
    if (path === "/api/v1/config/export") return exported();
    if (path === "/api/v1/cluster/nodes") return nodes();
    return json([]);
  });
  // the export dialog's organization picker; the screen mounts no scope chain
  // of its own, so nothing else reads this list
  return (input, init) =>
    new URL(String(input), "http://localhost").pathname === "/api/v1/orgs"
      ? Promise.resolve(orgs())
      : rest(input, init);
}

const loaded = api();
const FORBIDDEN = () => json({ error: { message: "forbidden" } }, 403);
const forbidden = api({ config: FORBIDDEN, problems: FORBIDDEN, nodes: FORBIDDEN });

/**
 * Assert nothing on screen is coloured as a success.
 *
 * The screen used to carry a green "reload-free" dot and a green check that
 * rendered in every state, a failed load included (#2085). A success colour is
 * a claim about the fleet, so it may appear only where the inventory made it.
 */
async function expectNoSuccessStatus(canvasElement: HTMLElement): Promise<void> {
  await expect(canvasElement.querySelectorAll('[class*="--status-success"]')).toHaveLength(0);
}

const meta = {
  title: "Screens/Config",
  component: Config,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Config>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => <Stage fetchStub={loaded} />,
  // every non-tabled section is listed once, collapsed, with its shape; the
  // gateway-only sections (digests, redacted sessions) are not
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectListTable(canvasElement, "Providers");
    await expectListTable(canvasElement, "Routes");
    await expectListTable(canvasElement, "Virtual keys (from the config file)");
    // the kind column reads the catalog's display name, and a kind it does not
    // name keeps its stored id
    const providers = canvas.getByRole("table", { name: "Providers" });
    await expect(within(providers).getByText("OpenAI")).toBeVisible();
    await expect(within(providers).getByText("OpenAI-compatible")).toBeVisible();
    await expect(within(providers).queryByText("openai_compatible")).toBeNull();
    await expect(within(providers).getByText("brand_new_kind")).toBeVisible();
    // a target names its provider, the model it calls and a labelled weight
    await expect(canvas.getByText("model gpt-4o-2024-08-06", { exact: false })).toBeVisible();
    await expect(canvas.getByText("weight 3", { exact: false })).toBeVisible();
    await expect(canvas.getByText("no targets")).toBeVisible();
    // the key's secret is never drawn
    await expect(canvas.queryByText("[redacted]")).toBeNull();
    await expect(await canvas.findByText("4 sections")).toBeVisible();
    await expect(canvas.getByText("server")).toBeVisible();
    await expect(canvas.getByText("3 fields")).toBeVisible();
    await expect(canvas.queryByText("db_virtual_keys")).toBeNull();
    await expect(canvas.queryByText("mcp_oauth_sessions")).toBeNull();
    // nothing is left out of the snapshot, so nothing is marked
    await expect(canvas.queryByText("not served")).toBeNull();
    await expect(canvas.queryByText(/not being served/)).toBeNull();

    await userEvent.click(canvas.getByText("cache"));
    // the block highlights through a lazy chunk that splits the JSON into token
    // spans, so read the region's text rather than a single text node
    const region = await canvas.findByRole("region", { name: /^cache/ });
    await expect(region).toBeVisible();
    await waitFor(() => expect(region).toHaveTextContent(/"ttl_secs": 300/));
  },
};

// forty sections in one list want a filter: a section is found by its name or
// by anything written inside it, and the count says how many survived (#2087)
export const FilterSections: Story = {
  render: () => <Stage fetchStub={loaded} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const filter = await canvas.findByRole("searchbox", {
      name: "Filter sections by name or content",
    });
    await userEvent.type(filter, "ttl");
    // `ttl_secs` sits inside `cache`, so the section is found by its content
    await expect(await canvas.findByText("1 of 4 sections match")).toBeVisible();
    await expect(canvas.getByText("cache")).toBeVisible();
    await expect(canvas.queryByText("server")).toBeNull();
    await userEvent.clear(filter);
    await userEvent.type(filter, "nothing-like-this");
    await expect(await canvas.findByText(/No section is called or contains/)).toBeVisible();
    await expect(canvas.queryByText("cache")).toBeNull();
    // the tables above are not a section and stay whatever is typed
    await expect(canvas.getByRole("table", { name: "Providers" })).toBeVisible();
  },
};

// the display name follows the dashboard's language, the id never leaks through
export const KindNamesInRussian: Story = {
  globals: { locale: "ru" },
  render: () => <Stage fetchStub={loaded} />,
  play: async ({ canvasElement }) => {
    const providers = await within(canvasElement).findByRole("table", { name: "Провайдеры" });
    await expect(within(providers).getByText("OpenAI-совместимый")).toBeVisible();
    await expect(within(providers).queryByText("openai_compatible")).toBeNull();
    await expect(within(providers).getByText("brand_new_kind")).toBeVisible();
  },
};

const empty = api({
  config: () => json({ providers: [], routes: [], virtual_keys: [], server: { port: 4000 } }),
});

// a fresh install: each table says it is empty and points at where to fill it
export const Empty: Story = {
  render: () => <Stage fetchStub={withCapabilities("superadmin", empty)} role="superadmin" />,
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No providers configured/, /Go to Providers/);
    await expectEmptyState(canvasElement, /No routes configured/, /Go to Routing Rules/);
    await expectListTable(canvasElement, "Providers");
    await expectListTable(canvasElement, "Routes");
    await expect(within(canvasElement).queryByRole("table", { name: /Virtual keys/ })).toBeNull();
  },
};

export const Loading: Story = {
  render: () => <Stage fetchStub={pending} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No (providers|routes) configured/);
    await expectNoSuccessStatus(canvasElement);
  },
};

// a refused read is one alert under no healthy-looking indicator: the problems
// read is refused as well, and the config's alert is the one that says so
export const Forbidden: Story = {
  render: () => <Stage fetchStub={forbidden} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("alert")).toHaveTextContent(/do not have access/i);
    await expect(canvas.getAllByRole("alert")).toHaveLength(1);
    await expectNoFalseEmpty(canvasElement, /No (providers|routes) configured/);
    await expectNoSuccessStatus(canvasElement);
  },
};

// what a store that cannot read `security_settings.required_headers` answers
// since #2943: the config load fails, and the problems read says why
const UNLOADABLE = [
  "security_settings.required_headers is unreadable: header 'x-tenant' has a value that is not a non-empty string",
  "the config could not be loaded, so no snapshot is being published and gateways keep serving the config they last received; the control plane log has the cause",
];

// the config read fails while the problems read answers: the screen says the
// load failed and still shows what the control plane can say about why, with
// no gateway line, for a superadmin who could otherwise read one
export const ConfigFailsWhileProblemsAnswer: Story = {
  render: () => (
    <Stage
      fetchStub={withCapabilities(
        "superadmin",
        api({
          config: () => json({ error: { message: "internal server error" } }, 500),
          problems: () => json({ problems: UNLOADABLE }),
        }),
      )}
      role="superadmin"
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /failed to return the effective config/);
    await expect(
      await canvas.findByText("2 config entries are not being served as written"),
    ).toBeVisible();
    await expect(canvas.getByText(/required_headers is unreadable/)).toBeVisible();
    await expect(canvas.getByText(/no snapshot is being published/)).toBeVisible();
    await expectGateAnswered();
    await expect(canvas.queryByText(/live gateway/)).toBeNull();
    await expect(canvas.queryByRole("link", { name: "View nodes" })).toBeNull();
    await expectNoFalseEmpty(canvasElement, /No (providers|routes) configured/);
    await expectNoSuccessStatus(canvasElement);
  },
};

// a provider defined wrong is left out of the snapshot, the route that called
// only it goes with it, and of two scopes' routes sharing `gpt-4o` only the
// one with nothing to serve is pruned. the lines are the exact sentences
// `sanitize_for_snapshot` writes
const OMITTED_CONFIG = {
  ...CONFIG,
  providers: [
    ...CONFIG.providers,
    { name: "openrouter-edge", kind: "openrouter", api_base: "https://edge.example.com/v1" },
  ],
  virtual_keys: [
    ...CONFIG.virtual_keys,
    { key: "[redacted]", name: "edge-bot", models: [] },
    { key: "[redacted]", name: null, models: [] },
  ],
  routes: [
    ...CONFIG.routes,
    {
      model: "claude-sonnet-4",
      strategy: "round_robin",
      targets: [{ provider: "openrouter-edge", model: null, weight: 1 }],
    },
  ],
};
const PROVIDER_REASON =
  "openrouter provider 'openrouter-edge' api_base must be https://openrouter.ai/api/v1 unless the provider sets allow_custom_api_base = true";
const ROUTE_REASON = "it has no target that references a known provider with a positive weight";
const KEY_REASON =
  "it allows only omitted providers ('openrouter-edge'), and an empty allow-list would let it reach every provider";
const EXAMPLE_KEY_REASON =
  "it is the public example key from the bundled rolter.example.toml and allows every model, so a deployment with auth enforced must not hand it to gateways";
// the lines the snapshot writes, and the same findings as records: each
// points at its row by position in the config read above (#1876)
const OMITTED_LINES = [
  `virtual key 'edge-bot' omitted from the snapshot: ${KEY_REASON}`,
  `virtual key 'sk-rolter-dev' omitted from the snapshot: ${EXAMPLE_KEY_REASON}`,
  `provider 'openrouter-edge' omitted from the snapshot: ${PROVIDER_REASON}`,
  "route 'claude-sonnet-4' target 'openrouter-edge' omitted from the snapshot: the provider is itself omitted from the snapshot",
  `route 'claude-sonnet-4' omitted from the snapshot: ${ROUTE_REASON}`,
  `route 'gpt-4o' omitted from the snapshot: ${ROUTE_REASON}`,
];
const OMITTED = [
  { kind: "virtual_key", name: "edge-bot", reason: KEY_REASON, index: 1 },
  { kind: "public_example_key", name: "<unnamed>", reason: EXAMPLE_KEY_REASON, index: 2 },
  { kind: "provider", name: "openrouter-edge", reason: PROVIDER_REASON, index: 3 },
  { kind: "route", name: "claude-sonnet-4", reason: ROUTE_REASON, index: 2 },
  { kind: "route", name: "gpt-4o", reason: ROUTE_REASON, index: 1 },
];

/** The row of `table` whose first cell reads `name`, the `index`th of them. */
function rowNamed(table: HTMLElement, name: string, index = 0): HTMLElement {
  const rows = within(table)
    .getAllByRole("row")
    .filter((row) => within(row).queryAllByRole("cell")[0]?.textContent?.startsWith(name));
  return rows[index];
}

// every row the snapshot leaves out says so, with the control plane's reason,
// and the notice above the tables counts every line (#2085)
export const OmittedEntriesAreMarked: Story = {
  render: () => (
    <Stage
      fetchStub={api({
        config: () => json(OMITTED_CONFIG),
        problems: () => json({ problems: OMITTED_LINES, omitted: OMITTED }),
      })}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByText("6 config entries are not being served as written"),
    ).toBeVisible();

    const providers = canvas.getByRole("table", { name: "Providers" });
    const edge = rowNamed(providers, "openrouter-edge");
    await waitFor(() => expect(within(edge).getByText("not served")).toBeVisible());
    const reason = within(edge).getByText(PROVIDER_REASON);
    await expect(reason).toBeVisible();
    // the sentence is the control plane's, in English whatever the locale
    await expect(reason).toHaveAttribute("lang", "en");
    await expect(within(rowNamed(providers, "openai-prod")).queryByText("not served")).toBeNull();

    const routes = canvas.getByRole("table", { name: "Routes" });
    const claude = rowNamed(routes, "claude-sonnet-4");
    await expect(within(claude).getByText("not served")).toBeVisible();
    await expect(within(claude).getByText(ROUTE_REASON)).toBeVisible();
    // the served `gpt-4o` keeps no mark, its targetless twin is the one pruned
    await expect(within(rowNamed(routes, "gpt-4o", 0)).queryByText("not served")).toBeNull();
    await expect(within(rowNamed(routes, "gpt-4o", 1)).getByText("not served")).toBeVisible();
    await expectListTable(canvasElement, "Providers");
    await expectListTable(canvasElement, "Routes");

    // a key is marked by the row the record points at, named or not; the
    // served key keeps no mark
    const keys = canvas.getByRole("table", { name: "Virtual keys (from the config file)" });
    const bot = rowNamed(keys, "edge-bot");
    await expect(within(bot).getByText("not served")).toBeVisible();
    await expect(within(bot).getByText(KEY_REASON)).toBeVisible();
    const example = rowNamed(keys, "—");
    await expect(within(example).getByText("not served")).toBeVisible();
    await expect(within(example).getByText(EXAMPLE_KEY_REASON)).toBeVisible();
    await expect(within(rowNamed(keys, "ci-bot")).queryByText("not served")).toBeNull();
  },
};

// the omissions could not be read: the tables still render, and the screen
// says it cannot tell which of their rows are served rather than marking none
export const ProblemsReadFails: Story = {
  render: () => (
    <Stage
      fetchStub={api({
        problems: () => json({ error: { message: "internal server error" } }, 500),
      })}
    />
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return the list of config problems/);
    const providers = await within(canvasElement).findByRole("table", { name: "Providers" });
    await expect(within(providers).getByText("openai-prod")).toBeVisible();
  },
};

const asSuperadmin = (stub: FetchStub) => withCapabilities("superadmin", stub);

// a caller who may read the inventory is told how many live gateways run the
// current config, and where to look; stale nodes and the control plane do not count
export const GatewaysConverged: Story = {
  render: () => <Stage fetchStub={asSuperadmin(loaded)} role="superadmin" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const line = await canvas.findByText("All 2 live gateways run the current config");
    await expect(line.className).toMatch(/--status-success-text/);
    await expect(canvas.getByRole("link", { name: "View nodes" })).toHaveAttribute(
      "href",
      "/cluster",
    );
  },
};

// a gateway behind the current version turns the line into a warning
export const GatewaysLagging: Story = {
  render: () => (
    <Stage
      fetchStub={asSuperadmin(
        api({
          nodes: () =>
            json([
              node({ id: "gw-1" }),
              node({ id: "gw-2", converged: false, config_version: 11 }),
            ]),
        }),
      )}
      role="superadmin"
    />
  ),
  play: async ({ canvasElement }) => {
    const line = await within(canvasElement).findByText(
      "1 of 2 live gateways run the current config",
    );
    await expect(line.className).toMatch(/--status-warning-text/);
    await expectNoSuccessStatus(canvasElement);
  },
};

// an inventory that could not be read knows nothing about the fleet, so the
// line says that in plain text rather than in a status colour
export const GatewaysUnreadable: Story = {
  render: () => (
    <Stage
      fetchStub={asSuperadmin(
        api({ nodes: () => json({ error: { message: "internal server error" } }, 500) }),
      )}
      role="superadmin"
    />
  ),
  play: async ({ canvasElement }) => {
    const line = await within(canvasElement).findByText("Gateway versions could not be read");
    await expect(line.className).toMatch(/--text-subtle/);
    await expectNoSuccessStatus(canvasElement);
  },
};

// the export endpoint answers with a toml document, not a record, so the stub
// does too — the screen saves what it was handed rather than parsing it
const EXPORT_PATH = "/api/v1/config/export";
const TOML = '[[providers]]\nname = "openai-prod"\n';
const toml = () =>
  new Response(TOML, { headers: { "Content-Type": "application/toml; charset=utf-8" } });

/**
 * The download itself, asserted where it actually happens.
 *
 * A story cannot let the anchor click through — the test browser would start a
 * real download and there would be nothing left to assert — so the click is
 * captured and the filename read off the element the screen built. That name is
 * the contract: dated, so two exports of the same deployment do not overwrite
 * each other.
 */
async function captureDownload(names: string[], run: () => Promise<void>): Promise<void> {
  const original = HTMLAnchorElement.prototype.click;
  HTMLAnchorElement.prototype.click = function capture(this: HTMLAnchorElement) {
    names.push(this.download);
  };
  try {
    await run();
  } finally {
    HTMLAnchorElement.prototype.click = original;
  }
}

// read out of the catalog rather than written out again, so rewording the
// refusal cannot leave this story asserting a sentence nothing renders
const NEEDS_SUPERADMIN = en.rbac.needsSuperadmin;

const exporting = recording(api({ exported: toml }));

/** Open the export dialog from the screen's button and hand back the dialog. */
async function openExport(canvasElement: HTMLElement): Promise<HTMLElement> {
  await userEvent.click(
    await within(canvasElement).findByRole("button", { name: "Export rolter.toml" }),
  );
  return within(document.body).findByRole("dialog", { name: "Export the configuration" });
}

/** The organization picker, once the list behind it has loaded. */
async function orgPicker(dialog: HTMLElement): Promise<HTMLElement> {
  return within(dialog).findByRole("combobox", { name: "Organization" });
}

// the button opens a dialog, and its defaults are the old button's behaviour:
// every organization, person caps in. One GET to the endpoint the control plane
// already gates, with no query, and the document handed to the browser as a
// dated file
export const ExportsTheConfiguration: Story = {
  render: () => (
    <Stage fetchStub={withCapabilities("superadmin", exporting.stub)} role="superadmin" />
  ),
  play: async ({ canvasElement }) => {
    const names: string[] = [];
    const dialog = await openExport(canvasElement);
    await expect(await orgPicker(dialog)).toHaveValue("All organizations");
    await expect(within(dialog).getByRole("switch", { name: /per-person caps/ })).not.toBeChecked();
    await captureDownload(names, async () => {
      await userEvent.click(within(dialog).getByRole("button", { name: "Export" }));
      await exporting.expectSent("GET", EXPORT_PATH);
      await waitFor(() => expect(names).toHaveLength(1));
    });
    await expect(names[0]).toMatch(/^rolter-config-\d{4}-\d{2}-\d{2}\.toml$/);
    await expect(exporting.calls.find((c) => c.url.includes(EXPORT_PATH))?.url).toBe(EXPORT_PATH);
    // saved, so the dialog is gone
    await waitFor(() => expect(within(document.body).queryByRole("dialog")).toBeNull());
  },
};

const scopedExport = recording(api({ exported: toml }));

// one organization and no addresses: both choices reach the endpoint as the
// query it reads, and nothing else is asked for
export const ExportsOneOrganizationWithoutPersonCaps: Story = {
  render: () => (
    <Stage fetchStub={withCapabilities("superadmin", scopedExport.stub)} role="superadmin" />
  ),
  play: async ({ canvasElement }) => {
    const names: string[] = [];
    const dialog = await openExport(canvasElement);
    // by keyboard alone: arrow down to open the list, on to Globex, Enter to take it
    const picker = await orgPicker(dialog);
    picker.focus();
    await userEvent.keyboard("{ArrowDown}");
    await expect(picker).toHaveAttribute("aria-expanded", "true");
    await userEvent.keyboard("{ArrowDown}{ArrowDown}");
    await waitFor(() =>
      expect(
        document.getElementById(picker.getAttribute("aria-activedescendant") ?? ""),
      ).toHaveTextContent("Globex"),
    );
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(picker).toHaveValue("Globex"));
    await userEvent.tab();
    await userEvent.keyboard(" ");
    await expect(within(dialog).getByRole("switch", { name: /per-person caps/ })).toBeChecked();
    await captureDownload(names, async () => {
      await userEvent.click(within(dialog).getByRole("button", { name: "Export" }));
      await scopedExport.expectSent("GET", `${EXPORT_PATH}?org=globex&without_person_caps=true`);
      await waitFor(() => expect(names).toHaveLength(1));
    });
  },
};

// the unscoped export is refused while two organizations share a name. The
// dialog says to pick an organization in its own words, rather than the
// control plane's, takes "All organizations" off the list, and exports the one
// picked
const NEEDS_ORG = {
  error: { code: "config_export_needs_org", message: "this deployment cannot be exported" },
};
const whole = api({ exported: toml });
const refusing = recording(async (input, init) => {
  const url = new URL(String(input), "http://localhost");
  if (url.pathname === "/api/v1/config/export") {
    return url.searchParams.has("org") ? toml() : json(NEEDS_ORG, 400);
  }
  return whole(input, init);
});

export const ExportRefusedWithoutAnOrganization: Story = {
  render: () => (
    <Stage fetchStub={withCapabilities("superadmin", refusing.stub)} role="superadmin" />
  ),
  play: async ({ canvasElement }) => {
    const names: string[] = [];
    const dialog = await openExport(canvasElement);
    await userEvent.click(within(dialog).getByRole("button", { name: "Export" }));
    const picker = await orgPicker(dialog);
    // the dashboard's sentence, not the control plane's, tied to the picker
    await waitFor(() => expect(picker).toBeInvalid());
    // the sentence says what to do, so the cursor is already there
    await waitFor(() => expect(picker).toHaveFocus());
    await expect(within(dialog).getByText(en.pages.config.export.needsOrg)).toBeVisible();
    await expect(within(dialog).queryByText(NEEDS_ORG.error.message)).toBeNull();
    await expect(picker).toHaveValue("");
    // said once, as the picker's own error
    await expect(within(dialog).getAllByRole("alert")).toHaveLength(1);
    await userEvent.click(picker);
    const listbox = await within(document.body).findByRole("listbox");
    await expect(within(listbox).queryByRole("option", { name: "All organizations" })).toBeNull();
    await userEvent.click(within(listbox).getByRole("option", { name: /Acme/ }));
    await waitFor(() => expect(picker).toBeValid());
    await captureDownload(names, async () => {
      await userEvent.click(within(dialog).getByRole("button", { name: "Export" }));
      await refusing.expectSent("GET", `${EXPORT_PATH}?org=acme`);
      await waitFor(() => expect(names).toHaveLength(1));
    });
  },
};

// the refusal is remembered while the screen lives: opening the dialog again
// does not offer the export the control plane just refused
export const RefusalIsRememberedOnReopen: Story = {
  render: () => (
    <Stage fetchStub={withCapabilities("superadmin", refusing.stub)} role="superadmin" />
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openExport(canvasElement);
    await userEvent.click(within(dialog).getByRole("button", { name: "Export" }));
    await within(dialog).findByText(en.pages.config.export.needsOrg);
    await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(within(document.body).queryByRole("dialog")).toBeNull());
    const again = await openExport(canvasElement);
    await expect(await orgPicker(again)).toHaveValue("");
    await expect(within(again).getByText(en.pages.config.export.needsOrg)).toBeVisible();
  },
};

// the list behind the picker is still on its way: a skeleton stands in for it,
// and the export cannot be pressed on a choice nobody has seen
export const OrganizationsLoading: Story = {
  render: () => (
    <Stage
      fetchStub={withCapabilities("superadmin", api({ orgs: () => new Promise(() => {}) }))}
      role="superadmin"
    />
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openExport(canvasElement);
    await expectSkeleton(dialog);
    await expect(within(dialog).queryByRole("combobox")).toBeNull();
    // a list still on its way is not an empty one
    await expectNoFalseEmpty(dialog, /No organizations/);
  },
};

const orgsFail = recording(
  api({ exported: toml, orgs: () => json({ error: { message: "store unavailable" } }, 500) }),
);

// the list failing is not the export failing: the dialog says so, offers a
// retry, and the whole-deployment export stays one press away
export const OrganizationsFailToLoad: Story = {
  render: () => (
    <Stage fetchStub={withCapabilities("superadmin", orgsFail.stub)} role="superadmin" />
  ),
  play: async ({ canvasElement }) => {
    const names: string[] = [];
    const dialog = await openExport(canvasElement);
    await expectLoadError(dialog, /organizations/i);
    await expect(within(dialog).getByRole("button", { name: "Try again" })).toBeVisible();
    // and a failed one is not an empty one
    await expectNoFalseEmpty(dialog, /No organizations/);
    await captureDownload(names, async () => {
      await userEvent.click(within(dialog).getByRole("button", { name: "Export" }));
      await orgsFail.expectSent("GET", EXPORT_PATH);
      await waitFor(() => expect(names).toHaveLength(1));
    });
  },
};

// the picker lists the organizations the caller holds a role in, so it can be
// empty for a caller who may still export (a security auditor holds none): the
// dialog says none are listed for them, and the field still takes a slug
export const NoOrganizationsListed: Story = {
  render: () => (
    <Stage
      fetchStub={withCapabilities("superadmin", api({ orgs: () => json([]) }))}
      role="superadmin"
    />
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openExport(canvasElement);
    await expectEmptyState(dialog, /No organizations are listed for you/);
    await expect(await orgPicker(dialog)).toHaveValue("All organizations");
    await expectAllowed(dialog, "Export");
  },
};

// the security auditor of a deployment whose organizations share a name: no
// organization is listed, the control plane refuses the whole deployment, and
// the dialog must say so and take the slug typed in, not swallow the click
const auditing = recording(async (input, init) => {
  const url = new URL(String(input), "http://localhost");
  if (url.pathname === "/api/v1/orgs") return json([]);
  if (url.pathname === "/api/v1/config/export") {
    return url.searchParams.has("org") ? toml() : json(NEEDS_ORG, 400);
  }
  return whole(input, init);
});

export const AuditorWithNoListedOrganizationsIsRefused: Story = {
  render: () => (
    <Stage
      fetchStub={withCapabilities("security_auditor", auditing.stub)}
      role="security_auditor"
    />
  ),
  play: async ({ canvasElement }) => {
    const names: string[] = [];
    const dialog = await openExport(canvasElement);
    await expectEmptyState(dialog, /No organizations are listed for you/);
    await userEvent.click(within(dialog).getByRole("button", { name: "Export" }));
    // the refusal is not suppressed because the list is empty
    await within(dialog).findByText(en.pages.config.export.needsOrg);
    const picker = await orgPicker(dialog);
    await waitFor(() => expect(picker).toHaveFocus());
    await userEvent.keyboard("acme");
    await userEvent.click(await within(document.body).findByRole("option", { name: /acme/ }));
    await captureDownload(names, async () => {
      await userEvent.click(within(dialog).getByRole("button", { name: "Export" }));
      await auditing.expectSent("GET", `${EXPORT_PATH}?org=acme`);
      await waitFor(() => expect(names).toHaveLength(1));
    });
    await expect(names[0]).toMatch(/^rolter-config-acme-\d{4}-\d{2}-\d{2}\.toml$/);
  },
};

// the gate on the dialog's own button, which the screen's button also holds: a
// viewer cannot reach it through the screen, so the dialog is rendered open
export const ExportRunIsGated: Story = {
  render: () => (
    <Harness fetchStub={withCapabilities("viewer", api())} role="viewer">
      <ConfigExportDialog open onOpenChange={() => {}} onExported={() => {}} onFailed={() => {}} />
    </Harness>
  ),
  play: async () => {
    await expectRefused(document.body, "Export", NEEDS_SUPERADMIN);
  },
};

const exportFails = recording(
  api({ exported: () => json({ error: { message: "store unavailable" } }, 500) }),
);

// a failed export says so in the toast queue rather than silently downloading
// nothing — the one outcome a browser download has no way to report on its own
export const ExportFails: Story = {
  render: () => (
    <Stage fetchStub={withCapabilities("superadmin", exportFails.stub)} role="superadmin" toasted />
  ),
  play: async ({ canvasElement }) => {
    const names: string[] = [];
    const dialog = await openExport(canvasElement);
    await captureDownload(names, async () => {
      await userEvent.click(within(dialog).getByRole("button", { name: "Export" }));
      await expectToast(canvasElement, /Could not export the configuration(?! export)/, "error");
    });
    await expect(names).toHaveLength(0);
  },
};

// the endpoint is whole-deployment and superadmin-only, so every scoped role —
// a viewer included — is refused before the click, and the tooltip says which
// role would allow it rather than leaving "disabled" as the whole answer
const viewing = recording(api());

// any signed-in role reads the config, so a viewer gets the tables; the
// cluster inventory is superadmin-only, so a viewer gets no gateway line and
// the screen never asks for one
export const AsViewer: Story = {
  render: () => <Stage fetchStub={withCapabilities("viewer", viewing.stub)} role="viewer" />,
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Export rolter.toml", NEEDS_SUPERADMIN);
    // reading is untouched: the tables still render for a viewer
    const providers = await within(canvasElement).findByRole("table", { name: "Providers" });
    await expect(within(providers).getByText("openai-prod")).toBeVisible();
    await expectGateAnswered();
    await expect(within(canvasElement).queryByText(/live gateway/)).toBeNull();
    await expect(within(canvasElement).queryByRole("link", { name: "View nodes" })).toBeNull();
    viewing.expectNotSent("GET", "/api/v1/cluster/nodes");
  },
};
