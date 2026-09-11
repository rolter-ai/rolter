import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import SkillsRepository from "./SkillsRepository";
import { Toasted, expectRefused, expectToast, withCapabilities, type StoryRole } from "./story-harness";
import type { SkillRow, SkillVersionRow } from "@/lib/api";
import { CapabilityProvider } from "@/lib/can";

const ORG = "00000000-0000-4000-8000-000000000011";
const TEAM = "00000000-0000-4000-8000-000000000012";
const PROJECT = "00000000-0000-4000-8000-000000000013";
const SKILL = "00000000-0000-4000-8000-000000000014";

const skill: SkillRow = {
  id: SKILL,
  org_id: ORG,
  name: "Incident coordinator",
  slug: "incident-coordinator",
  description: "Guides agents through evidence gathering, severity, and handoff.",
  retired_at: null,
  published_version: 2,
  allowed_team_ids: [TEAM],
  minimum_role: "member",
  created_at: "2026-07-26T09:00:00Z",
};

const versions: SkillVersionRow[] = [
  {
    skill_id: SKILL,
    version: 2,
    content: "---\nname: incident-coordinator\ndescription: Coordinate incidents\n---\n\n# Workflow\n\n1. Collect evidence.\n2. Assign severity.\n3. Record the handoff.\n",
    content_ref: null,
    metadata: { runtime: "agent", capabilities: ["logs", "handoff"] },
    created_at: "2026-07-30T12:15:00Z",
  },
  {
    skill_id: SKILL,
    version: 1,
    content: null,
    content_ref: "oci://registry.example/skills/incident-coordinator@sha256:abc123",
    metadata: { source: "platform-runbooks" },
    created_at: "2026-07-26T09:05:00Z",
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
  let currentSkill = { ...skill };
  let currentVersions = [...versions];
  let deleted = false;
  return async (input, init) => {
    const url = String(input);
    if (url === "/api/v1/orgs") return json([{ id: ORG, name: "Northstar", slug: "northstar", created_at: "2026-01-01T00:00:00Z" }]);
    if (url.endsWith(`/orgs/${ORG}/teams`)) return json([{ id: TEAM, org_id: ORG, name: "Platform", created_at: "2026-01-01T00:00:00Z" }]);
    if (url.endsWith(`/teams/${TEAM}/projects`)) return json([{ id: PROJECT, team_id: TEAM, name: "Production", created_at: "2026-01-01T00:00:00Z" }]);
    if (url.endsWith(`/orgs/${ORG}/skills`)) return json(deleted ? [] : [currentSkill]);
    if (url.endsWith(`/skills/${SKILL}/versions`) && init?.method === "POST") {
      const body = JSON.parse(String(init.body)) as Partial<SkillVersionRow>;
      const created: SkillVersionRow = { skill_id: SKILL, version: 3, content: body.content ?? null, content_ref: body.content_ref ?? null, metadata: body.metadata ?? {}, created_at: "2026-08-02T08:00:00Z" };
      currentVersions = [created, ...currentVersions];
      return json(created);
    }
    if (url.endsWith(`/skills/${SKILL}/versions`)) return json(currentVersions);
    if (url.endsWith(`/skills/${SKILL}/publish`)) {
      currentSkill = { ...currentSkill, published_version: JSON.parse(String(init?.body)).version };
      return json(currentSkill);
    }
    if (url.endsWith(`/skills/${SKILL}/rollback`)) {
      currentSkill = { ...currentSkill, published_version: JSON.parse(String(init?.body)).version };
      return json(currentSkill);
    }
    if (url.endsWith(`/skills/${SKILL}`) && init?.method === "DELETE") {
      deleted = true;
      return new Response(null, { status: 204 });
    }
    if (url.endsWith(`/skills/${SKILL}`) && init?.method === "PUT") {
      const body = JSON.parse(String(init.body));
      currentSkill = { ...currentSkill, ...body, retired_at: body.retired ? "2026-08-02T08:30:00Z" : null };
      return json(currentSkill);
    }
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
  const screen = <div className="h-screen bg-[color:var(--surface-app)]"><SkillsRepository /></div>;
  return <QueryClientProvider client={client}><Toasted>{role ? <CapabilityProvider>{screen}</CapabilityProvider> : screen}</Toasted></QueryClientProvider>;
}

const meta = {
  title: "Screens/SkillsRepository",
  component: SkillsRepository,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof SkillsRepository>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = { render: () => <Harness fetchStub={loadedStub()} /> };

export const Empty: Story = {
  render: () => {
    const stub = loadedStub();
    return <Harness fetchStub={async (input, init) => String(input).endsWith(`/orgs/${ORG}/skills`) ? json([]) : stub(input, init)} />;
  },
};

export const Loading: Story = { render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} /> };

export const Error: Story = {
  render: () => {
    const stub = loadedStub();
    return <Harness fetchStub={async (input, init) => String(input).endsWith(`/orgs/${ORG}/skills`) ? json({ error: { message: "database is unavailable" } }, 503) : stub(input, init)} />;
  },
};

export const SavesImmutableVersion: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    const content = await canvas.findByRole("textbox", { name: "SKILL.md content" });
    await userEvent.type(content, "\n4. Schedule the follow-up.");
    await userEvent.click(canvas.getByRole("button", { name: /Save new version/ }));
    await expectToast(canvasElement, /Version v3 saved/);
  },
};

export const ValidatesArtifactReference: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas }) => {
    await userEvent.click(await canvas.findByRole("radio", { name: /Artifact reference/ }));
    const reference = await canvas.findByRole("textbox", { name: "Content reference" });
    await userEvent.type(reference, "ftp://user:secret@example.com/skill");
    await expect(canvas.getByText("Use an https, git+https, oci, or s3 reference.")).toBeVisible();
    await expect(canvas.getByRole("button", { name: /Save new version/ })).toBeDisabled();
  },
};

export const RetiresSkillWithPolicyIntact: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    await userEvent.click(await canvas.findByRole("button", { name: "Settings" }));
    const page = within(canvasElement.ownerDocument.body);
    await userEvent.click(page.getByRole("checkbox", { name: /Retire this skill/ }));
    await userEvent.click(page.getByRole("button", { name: "Save settings" }));
    await waitFor(() => expect(canvas.getAllByText("Retired")).toHaveLength(2));
    await expectToast(canvasElement, /no longer resolves/);
  },
};

export const ConfirmsRollback: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    await userEvent.click(await canvas.findByRole("button", { name: "Roll back to v1" }));
    const page = within(canvasElement.ownerDocument.body);
    await expect(page.getByRole("heading", { name: "Roll back to v1" })).toBeVisible();
    await expect(page.getByText(/changes the resolved version from v2/)).toBeVisible();
  },
};

export const RequiresSlugToDeleteSkill: Story = {
  render: () => <Harness fetchStub={loadedStub()} />,
  play: async ({ canvas, canvasElement }) => {
    await userEvent.click(await canvas.findByRole("button", { name: "Delete Incident coordinator" }));
    const page = within(canvasElement.ownerDocument.body);
    const dialog = within(await page.findByRole("dialog"));
    await expect(dialog.getByRole("heading", { name: "Delete Incident coordinator?" })).toBeVisible();
    // retiring is offered as the reversible alternative
    await expect(dialog.getByText(/Retire it instead/)).toBeVisible();
    const confirm = dialog.getByRole("button", { name: "Delete skill" });
    await expect(confirm).toBeDisabled();
    await userEvent.type(dialog.getByRole("textbox"), "incident-coordinator");
    await waitFor(() => expect(confirm).toBeEnabled());
    await userEvent.click(confirm);
    await waitFor(() => expect(canvas.getByText("Share your first skill")).toBeVisible());
  },
};

// The same for skills: a viewer reads the library and writes none of it.
// Settings is the skill's access policy, which is a `skill:update` like the
// rest — only the delete asks for `skill:delete` — so Admin is the role every
// refusal here names.
export const AsViewer: Story = {
  render: () => <Harness fetchStub={loadedStub()} role="viewer" />,
  play: async ({ canvas, canvasElement }) => {
    await expectRefused(canvasElement, "Save new version");
    await expectRefused(canvasElement, "Settings");
    await expectRefused(canvasElement, "Delete Incident coordinator");
    await expectRefused(canvasElement, "Roll back to v1");
    await userEvent.click(await canvas.findByRole("button", { name: /^v1\b/ }));
    await expectRefused(canvasElement, "Publish v1");
    // reading is untouched: the slug is on screen in the index and the header
    await expect(canvas.getAllByText("incident-coordinator").length).toBeGreaterThan(0);
  },
};
