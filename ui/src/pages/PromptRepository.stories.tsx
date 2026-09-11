import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import PromptRepository from "./PromptRepository";
import { Toasted, expectRefused, expectToast, withCapabilities, type StoryRole } from "./story-harness";
import type {
  PromptTemplateRow,
  PromptTemplateScopeRow,
  PromptTemplateVersionRow,
} from "@/lib/api";
import { CapabilityProvider } from "@/lib/can";

const ORG = "00000000-0000-4000-8000-000000000001";
const TEAM = "00000000-0000-4000-8000-000000000002";
const PROJECT = "00000000-0000-4000-8000-000000000003";
const TEMPLATE = "00000000-0000-4000-8000-000000000004";
const ROUTE = "00000000-0000-4000-8000-000000000005";
const KEY = "00000000-0000-4000-8000-000000000006";

const template: PromptTemplateRow = {
  id: TEMPLATE,
  org_id: ORG,
  name: "Support concierge",
  slug: "support-concierge",
  description: "Sets a consistent support voice and adds escalation context.",
  published_version: 2,
  created_at: "2026-07-28T09:00:00Z",
};

const versions: PromptTemplateVersionRow[] = [
  {
    template_id: TEMPLATE,
    version: 2,
    variables: [
      { name: "customer_name", required: true },
      { name: "tone", required: false, default: "calm and direct" },
    ],
    decorators: [
      {
        role: "system",
        position: "prepend",
        content: "You support {{ customer_name }}. Keep the tone {{ tone }}.",
      },
      {
        role: "assistant",
        position: "append",
        content: "If the issue remains unresolved, summarize the next action.",
      },
    ],
    created_at: "2026-07-30T12:15:00Z",
  },
  {
    template_id: TEMPLATE,
    version: 1,
    variables: [{ name: "customer_name", required: true }],
    decorators: [
      {
        role: "system",
        position: "prepend",
        content: "You are the support concierge for {{ customer_name }}.",
      },
    ],
    created_at: "2026-07-28T09:05:00Z",
  },
];

const scopes: PromptTemplateScopeRow[] = [
  {
    template_id: TEMPLATE,
    version: 2,
    scope_type: "project",
    scope_id: PROJECT,
    created_at: "2026-07-30T12:15:00Z",
  },
];

type FetchStub = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function loadedStub(): FetchStub {
  let currentVersions = [...versions];
  let deleted = false;
  return async (input, init) => {
    const url = String(input);
    if (url === "/api/v1/orgs") return json([{ id: ORG, name: "Northstar", slug: "northstar", created_at: "2026-01-01T00:00:00Z" }]);
    if (url.endsWith(`/orgs/${ORG}/teams`)) return json([{ id: TEAM, org_id: ORG, name: "Platform", created_at: "2026-01-01T00:00:00Z" }]);
    if (url.endsWith(`/teams/${TEAM}/projects`)) return json([{ id: PROJECT, team_id: TEAM, name: "Production", created_at: "2026-01-01T00:00:00Z" }]);
    if (url.endsWith(`/orgs/${ORG}/prompt-templates`)) return json(deleted ? [] : [template]);
    if (url.endsWith(`/projects/${PROJECT}/routes`)) return json([{ id: ROUTE, project_id: PROJECT, model: "support", strategy: "round_robin", enabled: true, params: {}, param_policy: {}, created_at: "2026-01-01T00:00:00Z" }]);
    if (url.endsWith(`/projects/${PROJECT}/virtual-keys`)) return json([{ id: KEY, project_id: PROJECT, key_hash: "hash", key_prefix: "rlt_prod", name: "Support app", models: ["support"], disabled: false, created_at: "2026-01-01T00:00:00Z" }]);
    if (url.endsWith(`/prompt-templates/${TEMPLATE}/versions`) && init?.method === "POST") {
      const body = JSON.parse(String(init.body)) as Pick<PromptTemplateVersionRow, "variables" | "decorators">;
      const created = { template_id: TEMPLATE, version: 3, ...body, created_at: "2026-08-02T08:00:00Z" };
      currentVersions = [created, ...currentVersions];
      return json(created);
    }
    if (url.endsWith(`/prompt-templates/${TEMPLATE}/versions`)) return json(currentVersions);
    if (url.includes(`/prompt-templates/${TEMPLATE}/versions/`) && url.endsWith("/scopes")) {
      if (init?.method === "PUT") return json(JSON.parse(String(init.body)).scopes);
      return json(url.includes("/2/") ? scopes : []);
    }
    if (url.endsWith(`/prompt-templates/${TEMPLATE}`) && init?.method === "PUT") {
      const body = JSON.parse(String(init.body)) as { name?: string; description?: string };
      return json({ ...template, ...body });
    }
    if (url.endsWith(`/prompt-templates/${TEMPLATE}`) && init?.method === "DELETE") {
      deleted = true;
      return new Response(null, { status: 204 });
    }
    if (url.endsWith(`/prompt-templates/${TEMPLATE}/publish`)) return json({ ...template, published_version: JSON.parse(String(init?.body)).version });
    if (url.endsWith(`/prompt-templates/${TEMPLATE}/rollback`)) return json({ ...template, published_version: JSON.parse(String(init?.body)).version });
    return json({ error: { message: `unhandled story request: ${url}` } }, 500);
  };
}

function Harness({ fetchStub, role }: { fetchStub: FetchStub; role?: StoryRole }) {
  const original = React.useRef<typeof globalThis.fetch | null>(null);
  const client = React.useMemo(() => {
    original.current ??= globalThis.fetch;
    globalThis.fetch = (role ? withCapabilities(role, fetchStub) : fetchStub) as typeof globalThis.fetch;
    localStorage.removeItem("rolter.scope");
    return new QueryClient({ defaultOptions: { queries: { retry: false } } });
  }, [fetchStub, role]);
  React.useEffect(() => () => {
    if (original.current) globalThis.fetch = original.current;
  }, []);
  const screen = <div className="h-screen bg-[color:var(--surface-app)]"><PromptRepository /></div>;
  return <QueryClientProvider client={client}><Toasted>{role ? <CapabilityProvider>{screen}</CapabilityProvider> : screen}</Toasted></QueryClientProvider>;
}

const meta = {
  title: "Screens/PromptRepository",
  component: PromptRepository,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof PromptRepository>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
};

export const Empty: Story = {
  render: () => {
    const stub = loadedStub();
    return <Harness fetchStub={async (input, init) => String(input).endsWith(`/orgs/${ORG}/prompt-templates`) ? json([]) : stub(input, init)} />;
  },
};

export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
};

export const Error: Story = {
  render: () => {
    const stub = loadedStub();
    return <Harness fetchStub={async (input, init) => String(input).endsWith(`/orgs/${ORG}/prompt-templates`) ? json({ error: { message: "database is unavailable" } }, 503) : stub(input, init)} />;
  },
};

export const RendersSamplesAndSavesDraft: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    const sample = await canvas.findByRole("textbox", { name: "Sample value for customer_name" });
    await userEvent.type(sample, "Aster Labs");
    await expect(canvas.getByText(/You support Aster Labs/)).toBeVisible();
    await userEvent.click(canvas.getByRole("button", { name: /Save as new draft/ }));
    await expectToast(canvasElement, /Draft v3 saved/);
  },
};

export const ConfirmsRollback: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    await userEvent.click(await canvas.findByRole("button", { name: "Roll back to v1" }));
    const page = within(canvasElement.ownerDocument.body);
    await expect(page.getByRole("heading", { name: "Roll back to v1" })).toBeVisible();
    await expect(page.getByText(/changes the live version from v2/)).toBeVisible();
  },
};

export const RenamesTemplateKeepingSlug: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    await userEvent.click(await canvas.findByRole("button", { name: "Rename Support concierge" }));
    const page = within(canvasElement.ownerDocument.body);
    const dialog = within(await page.findByRole("dialog"));
    await expect(dialog.getByRole("heading", { name: "Rename prompt template" })).toBeVisible();
    // the slug is the stable identity: it is stated, never offered as a field
    await expect(dialog.getByText(/support-concierge/)).toBeVisible();
    const [name] = dialog.getAllByRole("textbox");
    await userEvent.clear(name);
    await userEvent.type(name, "Support desk");
    await userEvent.click(dialog.getByRole("button", { name: "Save details" }));
    await expectToast(canvasElement, /Template details updated/);
  },
};

export const RequiresSlugToDeleteTemplate: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    await userEvent.click(await canvas.findByRole("button", { name: "Delete Support concierge" }));
    const page = within(canvasElement.ownerDocument.body);
    const dialog = within(await page.findByRole("dialog"));
    await expect(dialog.getByRole("heading", { name: "Delete Support concierge?" })).toBeVisible();
    await expect(dialog.getByText(/v2 is live/)).toBeVisible();
    const confirm = dialog.getByRole("button", { name: "Delete template" });
    // the destructive action stays locked until the slug is typed back
    await expect(confirm).toBeDisabled();
    await userEvent.type(dialog.getByRole("textbox"), "support-concierge");
    await waitFor(() => expect(confirm).toBeEnabled());
    await userEvent.click(confirm);
    await waitFor(() => expect(canvas.getByText("Start with a prompt template")).toBeVisible());
  },
};

// A viewer reaches the same workbench — `prompt_template:read` is a viewer's —
// and every control on it that writes is refused before the click rather than
// after the 403. Publishing, rolling back and saving a version are all one
// `prompt_template:update` guard in crates/rolter-control/src/crud.rs, so the
// role that would allow them is Admin.
export const AsViewer: Story = {
  render: () => <Harness fetchStub={loadedStub()} role="viewer" />,
  play: async ({ canvas, canvasElement }) => {
    await expectRefused(canvasElement, "Save as new draft");
    await expectRefused(canvasElement, "Rename Support concierge");
    await expectRefused(canvasElement, "Delete Support concierge");
    await expectRefused(canvasElement, "Roll back to v1");
    // publishing only offers itself on a version that is not the live one, so
    // the story selects v1 before it can assert the control at all
    await userEvent.click(await canvas.findByRole("button", { name: /^v1\b/ }));
    await expectRefused(canvasElement, "Publish v1");
    // reading is untouched: the slug is on screen in the index and the header
    await expect(canvas.getAllByText("support-concierge").length).toBeGreaterThan(0);
  },
};
