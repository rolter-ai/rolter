import type { Meta, StoryObj } from "@storybook/react-vite";
import { MemoryRouter } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Config from "./Config";
import {
  Harness,
  Toasted,
  expectRefused,
  expectToast,
  json,
  pending,
  recording,
  scoped,
  withCapabilities,
  type FetchStub,
  type StoryRole,
} from "./story-harness";
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
  providers: [{ name: "openai-prod", kind: "openai", api_base: "https://api.openai.com/v1" }],
  routes: [{ model: "gpt-4o", strategy: "round_robin", targets: [{ provider: "openai-prod", weight: 1 }] }],
  virtual_keys: [],
  db_virtual_keys: [{ key_hash: "", id: "k1" }],
  mcp_oauth_sessions: [],
  server: { host: "0.0.0.0", port: 4000, workers: 4 },
  cache: { enabled: true, ttl_secs: 300 },
  budgets: [],
  unpriced_policy: "ignore",
};

const loaded = scoped(async (input) =>
  String(input).includes("/api/v1/config") ? json(CONFIG) : json([]),
);
const forbidden = scoped(async () => json({ error: { message: "forbidden" } }, 403));

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
    await expect(await canvas.findByText("4 sections")).toBeVisible();
    await expect(canvas.getByText("server")).toBeVisible();
    await expect(canvas.getByText("3 fields")).toBeVisible();
    await expect(canvas.queryByText("db_virtual_keys")).toBeNull();
    await expect(canvas.queryByText("mcp_oauth_sessions")).toBeNull();

    await userEvent.click(canvas.getByText("cache"));
    await expect(canvas.getByText(/"ttl_secs": 300/)).toBeVisible();
  },
};

export const Loading: Story = {
  render: () => <Stage fetchStub={pending} />,
};

export const Forbidden: Story = {
  render: () => <Stage fetchStub={forbidden} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("alert")).toHaveTextContent(/do not have access/i);
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

const exporting = recording(
  scoped(async (input) => (String(input).includes(EXPORT_PATH) ? toml() : json(CONFIG))),
);

// the whole point of the button: one GET to the endpoint the control plane
// already gates, and the document handed to the browser as a dated file
export const ExportsTheConfiguration: Story = {
  render: () => (
    <Stage fetchStub={withCapabilities("superadmin", exporting.stub)} role="superadmin" />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const names: string[] = [];
    await captureDownload(names, async () => {
      await userEvent.click(await canvas.findByRole("button", { name: "Export rolter.toml" }));
      await exporting.expectSent("GET", EXPORT_PATH);
      await waitFor(() => expect(names).toHaveLength(1));
    });
    await expect(names[0]).toMatch(/^rolter-config-\d{4}-\d{2}-\d{2}\.toml$/);
  },
};

const exportFails = recording(
  scoped(async (input) =>
    String(input).includes(EXPORT_PATH)
      ? json({ error: { message: "store unavailable" } }, 500)
      : json(CONFIG),
  ),
);

// a failed export says so in the toast queue rather than silently downloading
// nothing — the one outcome a browser download has no way to report on its own
export const ExportFails: Story = {
  render: () => (
    <Stage fetchStub={withCapabilities("superadmin", exportFails.stub)} role="superadmin" toasted />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const names: string[] = [];
    await captureDownload(names, async () => {
      await userEvent.click(await canvas.findByRole("button", { name: "Export rolter.toml" }));
      await expectToast(canvasElement, /Could not export the configuration export/, "error");
    });
    await expect(names).toHaveLength(0);
  },
};

// the endpoint is whole-deployment and superadmin-only, so every scoped role —
// a viewer included — is refused before the click, and the tooltip says which
// role would allow it rather than leaving "disabled" as the whole answer
export const AsViewer: Story = {
  render: () => <Stage fetchStub={withCapabilities("viewer", loaded)} role="viewer" />,
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Export rolter.toml", NEEDS_SUPERADMIN);
    // reading is untouched: the tables still render for a viewer
    await expect(within(canvasElement).getByText("openai-prod")).toBeVisible();
  },
};
