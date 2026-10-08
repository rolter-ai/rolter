import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { ProjectScopeField } from "./ProjectScopeField";
import { ProviderGroupSheet } from "./ProviderGroupSheet";
import { ProviderSheet } from "./ProviderSheet";
import type { ProviderGroupRow, ProviderRow } from "@/lib/api";
import {
  Harness,
  NEEDS_ADMIN,
  expectGateAnswered,
  expectRefused,
  json,
  recording,
  scoped,
  type FetchStub,
  type Recorder,
} from "@/pages/story-harness";

// the recorder of the story being rendered, which its play reads back
let recorder: Recorder;

// the two projects of the org the scope picker offers; the harness's own scope
// chain only knows the first
const PROJECTS = [
  {
    id: "project-1",
    team_id: "team-1",
    name: "Gateway",
    team_name: "Platform",
    created_at: "2026-01-01T00:00:00Z",
  },
  {
    id: "project-2",
    team_id: "team-1",
    name: "Search",
    team_name: "Platform",
    created_at: "2026-01-01T00:00:00Z",
  },
];

const provider = (over: Partial<ProviderRow> & Pick<ProviderRow, "id" | "name">): ProviderRow => ({
  org_id: "org-1",
  slug: over.name,
  kind: "openai",
  api_base: "https://api.openai.com",
  api_key_env: null,
  egress_proxy: null,
  egress_proxies: [],
  created_at: "2026-08-01T10:00:00Z",
  ...over,
});

const ORG_WIDE = provider({ id: "p-org", name: "openai-shared" });
const IN_GATEWAY = provider({ id: "p-gw", name: "gateway-private", project_id: "project-1" });
const IN_SEARCH = provider({ id: "p-search", name: "search-private", project_id: "project-2" });
const ALL = [ORG_WIDE, IN_GATEWAY, IN_SEARCH];

const KINDS = [
  {
    kind: "openai",
    base_includes_v1: false,
    request: "chat",
    request_path: "/v1/chat/completions",
    auth_header: "authorization",
  },
];

const GROUP: ProviderGroupRow = {
  id: "g-1",
  org_id: "org-1",
  name: "gateway-fleet",
  slug: "gateway-fleet",
  strategy: "round_robin",
  project_id: "project-1",
  created_at: "2026-08-01T10:00:00Z",
  members: [
    {
      group_id: "g-1",
      provider_id: "p-gw",
      provider_name: "gateway-private",
      upstream_model: null,
      weight: 1,
      position: 0,
    },
  ],
};

/** the endpoints both sheets read, over whatever else the story answers */
function api(handler: FetchStub = async () => json({})): FetchStub {
  return async (input, init) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (/^\/api\/v1\/orgs\/[^/]+\/projects$/.test(path)) return json(PROJECTS);
    if (path.endsWith("/provider-kinds")) return json(KINDS);
    return scoped(handler)(input, init);
  };
}

const refuse = (message: string) => async (_input: RequestInfo | URL, init?: RequestInit) =>
  init?.method === "POST" || init?.method === "PUT" ? json({ error: { message } }, 409) : json({});

const meta = {
  title: "Overlays/ProjectScope",
  component: ProjectScopeField,
  parameters: { layout: "fullscreen" },
  // every story renders a real sheet, which owns the props
  args: {
    resource: "provider",
    mode: "add",
    orgId: "org-1",
    id: "scope",
    value: "",
    onChange: () => {},
    mayWiden: true,
  },
} satisfies Meta<typeof ProjectScopeField>;

export default meta;
type Story = StoryObj<typeof meta>;

// sheets portal onto the body, so everything is queried from there
const screen = () => within(document.body);

function ProviderSheetUnder({
  recorder,
  role = "admin",
  mode = "add",
  row = null,
  defaultProjectId = null,
}: {
  recorder: Recorder;
  role?: "admin" | "member";
  mode?: "add" | "edit";
  row?: ProviderRow | null;
  defaultProjectId?: string | null;
}) {
  return (
    <Harness fetchStub={recorder.stub} role={role}>
      <ProviderSheet
        open
        mode={mode}
        onOpenChange={() => {}}
        orgId="org-1"
        provider={row}
        defaultProjectId={defaultProjectId}
        onDone={() => {}}
      />
    </Harness>
  );
}

function GroupSheetUnder({
  recorder,
  role = "admin",
  mode = "add",
  row = null,
  defaultProjectId = null,
}: {
  recorder: Recorder;
  role?: "admin" | "member";
  mode?: "add" | "edit";
  row?: ProviderGroupRow | null;
  defaultProjectId?: string | null;
}) {
  return (
    <Harness fetchStub={recorder.stub} role={role}>
      <ProviderGroupSheet
        open
        mode={mode}
        onOpenChange={() => {}}
        orgId="org-1"
        providers={ALL}
        group={row}
        defaultProjectId={defaultProjectId}
        onDone={() => {}}
      />
    </Harness>
  );
}

/** the picker's own options, read by opening it and closing it again */
async function optionsOf(combobox: HTMLElement): Promise<string[]> {
  await userEvent.click(combobox);
  const options = await screen().findAllByRole("option");
  const names = options.map((o) => o.textContent ?? "");
  await userEvent.keyboard("{Escape}");
  return names;
}

async function pick(combobox: HTMLElement, name: string) {
  await userEvent.click(combobox);
  await userEvent.click(await screen().findByRole("option", { name }));
}

const scopePicker = () => screen().findByRole("combobox", { name: "Scope" });

async function fillProvider(name: string) {
  await userEvent.type(await screen().findByLabelText("Name"), name);
  await userEvent.type(screen().getByLabelText("API base"), "https://api.example.com");
}

// -- the provider sheet ------------------------------------------------------

/** The default is the whole organization, and an org-wide create names no project. */
export const ProviderDefaultsToTheWholeOrganization: Story = {
  render: () => <ProviderSheetUnder recorder={recording(api())} />,
  play: async () => {
    const scope = await scopePicker();
    await expect(scope).toHaveValue("Whole organization");
    await expectGateAnswered();
  },
};

export const ProviderCreateSendsNoProjectWhenOrgWide: Story = {
  render: () => {
    recorder = recording(api(async () => json(ORG_WIDE)));
    return <ProviderSheetUnder recorder={recorder} />;
  },
  play: async () => {
    await scopePicker();
    await fillProvider("openai-shared");
    await userEvent.click(screen().getByRole("button", { name: "Create provider" }));
    const body = await recorder.expectSentBody<Record<string, unknown>>("POST", "/providers");
    await expect(body.project_id).toBeUndefined();
  },
};

/** Picking a project sends its id on create. */
export const ProviderCreateScopedToAProject: Story = {
  render: () => {
    recorder = recording(api(async () => json(IN_GATEWAY)));
    return <ProviderSheetUnder recorder={recorder} />;
  },
  play: async () => {
    await pick(await scopePicker(), "Search");
    await fillProvider("search-private");
    await userEvent.click(screen().getByRole("button", { name: "Create provider" }));
    const body = await recorder.expectSentBody<{ project_id: string }>("POST", "/providers");
    await expect(body.project_id).toBe("project-2");
  },
};

/** An admin can offer the org and every project, grouped under the team that owns it. */
export const ProviderScopeOffersTheOrgAndEveryProject: Story = {
  render: () => <ProviderSheetUnder recorder={recording(api())} />,
  play: async () => {
    await expectGateAnswered();
    await expect(await optionsOf(await scopePicker())).toEqual([
      "Whole organization",
      "Gateway",
      "Search",
    ]);
  },
};

/** Editing a scoped provider shows its project, and org-wide again is `null`, not omitted. */
export const ProviderEditMakesItOrgWideAgain: Story = {
  render: () => {
    recorder = recording(api(async () => json(ORG_WIDE)));
    return <ProviderSheetUnder recorder={recorder} mode="edit" row={IN_GATEWAY} />;
  },
  play: async () => {
    const scope = await scopePicker();
    await expect(scope).toHaveValue("Gateway");
    await pick(scope, "Whole organization");
    await userEvent.click(screen().getByRole("button", { name: "Save provider" }));
    const body = await recorder.expectSentBody<Record<string, unknown>>("PUT", "/providers/p-gw");
    await expect(body.project_id).toBeNull();
  },
};

/** An edit that leaves the scope alone does not send it. */
export const ProviderEditLeavesTheScopeOutWhenUnchanged: Story = {
  render: () => {
    recorder = recording(api(async () => json(IN_GATEWAY)));
    return <ProviderSheetUnder recorder={recorder} mode="edit" row={IN_GATEWAY} />;
  },
  play: async () => {
    await userEvent.type(await screen().findByLabelText("Egress proxy (optional)"), "http://p:1");
    await userEvent.click(screen().getByRole("button", { name: "Save provider" }));
    const body = await recorder.expectSentBody<Record<string, unknown>>("PUT", "/providers/p-gw");
    await expect("project_id" in body).toBe(false);
  },
};

/**
 * Someone who may not make a provider org-wide has no org option, starts on the
 * project the dashboard is open on, and may not read an env var into it.
 */
export const ProviderForANonAdminIsOwnProjectOnly: Story = {
  render: () => {
    recorder = recording(api(async () => json(IN_GATEWAY)));
    return <ProviderSheetUnder recorder={recorder} role="member" defaultProjectId="project-1" />;
  },
  play: async () => {
    await expectGateAnswered();
    const scope = await scopePicker();
    await waitFor(async () => expect(await optionsOf(scope)).toEqual(["Gateway", "Search"]));
    await expect(scope).toHaveValue("Gateway");
    const env = screen().getByLabelText("Provider key env var (optional)");
    await waitFor(() => expect(env).toBeDisabled());
    await expect(env).toHaveAccessibleDescription(/Only an organization admin/);
    await fillProvider("gateway-private");
    await userEvent.click(screen().getByRole("button", { name: "Create provider" }));
    const body = await recorder.expectSentBody<{ project_id: string; api_key_env?: string }>(
      "POST",
      "/providers",
    );
    await expect(body.project_id).toBe("project-1");
    await expect(body.api_key_env).toBeUndefined();
  },
};

/** With no project to start on, the create waits until one is named. */
export const ProviderForANonAdminNeedsAProject: Story = {
  render: () => {
    recorder = recording(api(async () => json(IN_GATEWAY)));
    return <ProviderSheetUnder recorder={recorder} role="member" />;
  },
  play: async () => {
    await expectGateAnswered();
    await fillProvider("gateway-private");
    await waitFor(() =>
      expect(screen().getByRole("button", { name: "Create provider" })).toBeDisabled(),
    );
    await pick(await scopePicker(), "Gateway");
    // naming one is what lets the create go out
    await userEvent.click(screen().getByRole("button", { name: "Create provider" }));
    await recorder.expectSentBody("POST", "/providers");
  },
};

/** Changing the scope of an existing row is refused to them, naming the role it takes. */
export const ProviderScopeChangeIsRefusedToANonAdmin: Story = {
  render: () => (
    <ProviderSheetUnder
      recorder={recording(api())}
      role="member"
      mode="edit"
      row={{ ...IN_GATEWAY, api_key_env: "GATEWAY_KEY" }}
    />
  ),
  play: async () => {
    await expectRefused(document.body, "Scope", NEEDS_ADMIN, "combobox");
    await expect(await scopePicker()).toHaveValue("Gateway");
    const env = screen().getByLabelText("Provider key env var (optional)");
    await waitFor(() => expect(env).toBeDisabled());
    await expect(env).toHaveValue("GATEWAY_KEY");
    await expect(screen().getByText(/Moving it to another project/)).toBeVisible();
  },
};

/** Scoping a provider that another project's route still uses is a 409 that names them. */
export const ProviderScopeChangeStillUsedElsewhere: Story = {
  render: () => {
    recorder = recording(
      api(
        refuse(
          "provider 'openai-shared' is used by route 'search/gpt-4o', provider group 'search-fleet', which belong to other projects; remove it from them before scoping it to one project",
        ),
      ),
    );
    return <ProviderSheetUnder recorder={recorder} mode="edit" row={ORG_WIDE} />;
  },
  play: async () => {
    await pick(await scopePicker(), "Gateway");
    await userEvent.click(screen().getByRole("button", { name: "Save provider" }));
    const alert = await screen().findByRole("alert");
    await expect(alert).toHaveTextContent(/route 'search\/gpt-4o', provider group 'search-fleet'/);
    await expect(alert).toHaveTextContent(/before scoping it to one project/);
    const body = await recorder.expectSentBody<{ project_id: string }>("PUT", "/providers/p-org");
    await expect(body.project_id).toBe("project-1");
    // the sheet stays open on the draft rather than discarding it
    await expect(screen().getByRole("dialog")).toBeVisible();
  },
};

/** The picker's own states: loading holds the label, and a failed read says so. */
export const ScopeListFailsToLoad: Story = {
  render: () => (
    <ProviderSheetUnder
      recorder={recording(async (input, init) => {
        const path = new URL(String(input), "http://localhost").pathname;
        if (/^\/api\/v1\/orgs\/[^/]+\/projects$/.test(path)) {
          return json({ error: { message: "boom" } }, 500);
        }
        return api()(input, init);
      })}
    />
  ),
  play: async () => {
    await scopePicker();
    // the org-wide option is still there, so the sheet is usable without the list
    await expect(await screen().findByRole("alert")).toBeVisible();
  },
};

// -- the group sheet ---------------------------------------------------------

/** A scoped group offers its project's providers and the org-wide ones, never another project's. */
export const GroupOffersItsProjectsProvidersAndOrgWideOnes: Story = {
  render: () => <GroupSheetUnder recorder={recording(api())} />,
  play: async () => {
    await pick(await scopePicker(), "Gateway");
    await userEvent.click(await screen().findByRole("button", { name: "Add member" }));
    const member = await screen().findByRole("combobox", { name: "Provider" });
    await expect(await optionsOf(member)).toEqual(["openai-shared", "gateway-private"]);
  },
};

/** An org-wide group offers org-wide providers only. */
export const OrgWideGroupOffersOrgWideProvidersOnly: Story = {
  render: () => <GroupSheetUnder recorder={recording(api())} />,
  play: async () => {
    await expect(await scopePicker()).toHaveValue("Whole organization");
    await userEvent.click(await screen().findByRole("button", { name: "Add member" }));
    const member = await screen().findByRole("combobox", { name: "Provider" });
    await expect(await optionsOf(member)).toEqual(["openai-shared"]);
  },
};

/** Moving a group to the whole organization flags a member that only fits its project. */
export const GroupMemberOutsideTheNewScopeIsFlagged: Story = {
  render: () => <GroupSheetUnder recorder={recording(api())} mode="edit" row={GROUP} />,
  play: async () => {
    const scope = await scopePicker();
    await expect(scope).toHaveValue("Gateway");
    await expect(screen().queryByText(/exposes it to every project|would expose it/)).toBeNull();
    await pick(scope, "Whole organization");
    await expect(await screen().findByText(/gateway-private is scoped to a project/)).toBeVisible();
    const member = screen().getByRole("combobox", { name: "Provider" });
    await expect(member).toHaveAccessibleDescription(/an organization-wide group would expose it/);
  },
};

export const GroupCreateScopedToAProject: Story = {
  render: () => {
    recorder = recording(api(async () => json({ ...GROUP, members: [] })));
    return <GroupSheetUnder recorder={recorder} />;
  },
  play: async () => {
    await pick(await scopePicker(), "Gateway");
    await userEvent.type(screen().getByLabelText("Name"), "gateway-fleet");
    await userEvent.click(screen().getByRole("button", { name: "Create group" }));
    const body = await recorder.expectSentBody<{ project_id: string }>("POST", "/provider-groups");
    await expect(body.project_id).toBe("project-1");
  },
};

export const GroupEditMakesItOrgWideAgain: Story = {
  render: () => {
    recorder = recording(api(async () => json({ ...GROUP, project_id: null })));
    return <GroupSheetUnder recorder={recorder} mode="edit" row={{ ...GROUP, members: [] }} />;
  },
  play: async () => {
    await pick(await scopePicker(), "Whole organization");
    await userEvent.click(screen().getByRole("button", { name: "Save group" }));
    const body = await recorder.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/provider-groups/g-1",
    );
    await expect(body.project_id).toBeNull();
  },
};

export const GroupForANonAdminIsOwnProjectOnly: Story = {
  render: () => {
    recorder = recording(api(async () => json({ ...GROUP, members: [] })));
    return <GroupSheetUnder recorder={recorder} role="member" defaultProjectId="project-1" />;
  },
  play: async () => {
    await expectGateAnswered();
    const scope = await scopePicker();
    await waitFor(async () => expect(await optionsOf(scope)).toEqual(["Gateway", "Search"]));
    await expect(scope).toHaveValue("Gateway");
    await userEvent.type(screen().getByLabelText("Name"), "gateway-fleet");
    await userEvent.click(screen().getByRole("button", { name: "Create group" }));
    const body = await recorder.expectSentBody<{ project_id: string }>("POST", "/provider-groups");
    await expect(body.project_id).toBe("project-1");
  },
};

export const GroupScopeChangeIsRefusedToANonAdmin: Story = {
  render: () => (
    <GroupSheetUnder recorder={recording(api())} role="member" mode="edit" row={GROUP} />
  ),
  play: async () => {
    await expectRefused(document.body, "Scope", NEEDS_ADMIN, "combobox");
    await expect(await scopePicker()).toHaveValue("Gateway");
  },
};

/** A member the group's scope does not allow is a 409 that names the provider. */
export const GroupMemberTheScopeDoesNotAllow: Story = {
  render: () => {
    recorder = recording(
      api(
        refuse(
          "this group cannot use provider 'search-private': scoped to a different project; use org-wide providers or ones scoped to the same project",
        ),
      ),
    );
    return <GroupSheetUnder recorder={recorder} mode="edit" row={GROUP} />;
  },
  play: async () => {
    await userEvent.click(await screen().findByRole("button", { name: "Save group" }));
    const alert = await screen().findByRole("alert");
    await expect(alert).toHaveTextContent(/cannot use provider 'search-private'/);
    await expect(alert).toHaveTextContent(/scoped to the same project/);
  },
};

/** An org-wide group refused a project-scoped member says why. */
export const OrgWideGroupRefusedAScopedMember: Story = {
  render: () => {
    recorder = recording(
      api(
        refuse(
          "this group cannot use provider 'gateway-private': scoped to a project, and an org-wide group would expose it to every project; use org-wide providers or ones scoped to the same project",
        ),
      ),
    );
    return <GroupSheetUnder recorder={recorder} mode="edit" row={{ ...GROUP, project_id: null }} />;
  },
  play: async () => {
    await userEvent.click(await screen().findByRole("button", { name: "Save group" }));
    await expect(await screen().findByRole("alert")).toHaveTextContent(
      /would expose it to every project/,
    );
  },
};
