import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import UserProvisioning from "./UserProvisioning";
import { expectLoadError, expectSkeleton } from "./story-harness";
import type { ScimGroupMappingRow, ScimTokenRow } from "@/lib/api";

const NOW = new Date("2026-07-01T10:00:00Z").toISOString();

const ORG = { id: "org-1", name: "Acme", slug: "acme", created_at: NOW };

const token = (over: Partial<ScimTokenRow> = {}): ScimTokenRow => ({
  id: "tok-1",
  org_id: ORG.id,
  name: "Okta production",
  created_by: "user-1",
  created_at: NOW,
  last_used_at: new Date("2026-07-02T09:30:00Z").toISOString(),
  revoked_at: null,
  ...over,
});

const TOKENS: ScimTokenRow[] = [
  token(),
  token({ id: "tok-2", name: "Entra staging", last_used_at: null }),
  token({
    id: "tok-3",
    name: "Okta legacy",
    revoked_at: new Date("2026-07-03T12:00:00Z").toISOString(),
  }),
];

const mapping = (over: Partial<ScimGroupMappingRow> = {}): ScimGroupMappingRow => ({
  id: "map-1",
  org_id: ORG.id,
  group_name: "platform-engineering",
  team_id: null,
  project_id: null,
  role: "member",
  created_at: NOW,
  ...over,
});

const MAPPINGS: ScimGroupMappingRow[] = [
  mapping(),
  mapping({ id: "map-2", group_name: "sre-oncall", role: "admin", team_id: "team-1" }),
];

type FetchStub = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

// two teams, and a project under each: the scope switcher only ever has one
// team selected, so "payments/checkout" is the project outside it that the
// mapping form still has to be able to name (#1249)
const TEAMS = [
  { id: "team-1", org_id: ORG.id, name: "core", created_at: NOW },
  { id: "team-2", org_id: ORG.id, name: "payments", created_at: NOW },
];

const PROJECTS: Record<string, { id: string; team_id: string; name: string; created_at: string }[]> = {
  "team-1": [{ id: "proj-1", team_id: "team-1", name: "prod", created_at: NOW }],
  "team-2": [{ id: "proj-2", team_id: "team-2", name: "checkout", created_at: NOW }],
};

// the same projects as the org-wide endpoint returns them: one list, every row
// naming its owning team, which is what the mapping form's picker reads (#1357)
const ORG_PROJECTS = TEAMS.flatMap((team) =>
  (PROJECTS[team.id] ?? []).map((project) => ({ ...project, team_name: team.name })),
);

// the screen resolves its org through useScope(), which fetches orgs, teams and
// projects before the token list is even enabled — so every stub has to route
// by url rather than answer one shape
function scoped(
  tokens: (init?: RequestInit) => Promise<Response>,
  mappings: (init?: RequestInit) => Promise<Response> = async () => json([]),
  chain: {
    teams?: () => Promise<Response>;
    projects?: () => Promise<Response>;
    orgProjects?: () => Promise<Response>;
  } = {},
): FetchStub {
  return async (input, init) => {
    const url = String(input);
    const path = new URL(url, "http://localhost").pathname;
    if (url.includes("scim-group-mappings")) return mappings(init);
    if (url.includes("scim-tokens")) return tokens(init);
    if (path === "/api/v1/orgs") return json([ORG]);
    // the projects route also contains "/teams", so it is matched first
    const projects = /^\/api\/v1\/teams\/([^/]+)\/projects$/.exec(path);
    if (projects) return (chain.projects ?? (async () => json(PROJECTS[projects[1]] ?? [])))();
    if (/^\/api\/v1\/orgs\/[^/]+\/projects$/.test(path)) {
      return (chain.orgProjects ?? (async () => json(ORG_PROJECTS)))();
    }
    if (/^\/api\/v1\/orgs\/[^/]+\/teams$/.test(path)) {
      return (chain.teams ?? (async () => json(TEAMS)))();
    }
    return json([]);
  };
}

// the stub is installed during render, not in an effect: child effects run
// before the parent's, so an effect would let the first real fetch through
function Harness({ fetchStub }: { fetchStub: FetchStub }) {
  const original = React.useRef<typeof globalThis.fetch | null>(null);
  const client = React.useMemo(() => {
    original.current ??= globalThis.fetch;
    globalThis.fetch = fetchStub as typeof globalThis.fetch;
    return new QueryClient({ defaultOptions: { queries: { retry: false } } });
  }, [fetchStub]);
  React.useEffect(
    () => () => {
      if (original.current) globalThis.fetch = original.current;
    },
    [],
  );
  return (
    <QueryClientProvider client={client}>
      <UserProvisioning />
    </QueryClientProvider>
  );
}

const meta = {
  title: "Screens/UserProvisioning",
  component: UserProvisioning,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof UserProvisioning>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => <Harness fetchStub={scoped(async () => json(TOKENS))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Okta production")).toBeVisible());
    // a token an IdP has never presented is distinguishable from a live one
    await expect(canvas.getByText("never used")).toBeVisible();
    await expect(canvas.getByText("REVOKED")).toBeVisible();
  },
};

export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

export const Empty: Story = {
  render: () => <Harness fetchStub={scoped(async () => json([]))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText("No provisioning tokens yet")).toBeVisible(),
    );
  },
};

// listing, minting and revoking all need Admin on the org; a lesser principal
// gets a calm explanation and no mint button rather than a red error
export const Forbidden: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async () => json({ error: { message: "forbidden" } }, 403))}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(
        canvas.getByText(/visible to org admins only/),
      ).toBeVisible(),
    );
    await expect(canvas.getByRole("button", { name: /Issue token/ })).toBeDisabled();
  },
};

// the whole point of the screen: the plaintext comes back once, from the create
// response, and the UI has to say so unmissably
export const IssueRevealsTheSecretOnce: Story = {
  render: () => {
    const stub = scoped(async (init) => {
      if (init?.method === "POST") {
        return json({ ...token({ id: "tok-new", name: "Okta production" }), secret: "rolter_scim_deadbeef" });
      }
      return json([]);
    });
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the empty placeholder repeats the toolbar action, so both are on screen
    await userEvent.click(
      (await canvas.findAllByRole("button", { name: /Issue token/ }))[0],
    );
    // sheets portal to document.body, so the panel is not under the canvas root
    const sheet = within(await within(document.body).findByRole("dialog"));
    await userEvent.type(
      sheet.getByPlaceholderText("Okta production"),
      "Okta production",
    );
    await userEvent.click(sheet.getByRole("button", { name: /Issue token/ }));
    await waitFor(() =>
      expect(sheet.getByTestId("scim-token-secret")).toHaveTextContent(
        "rolter_scim_deadbeef",
      ),
    );
    await expect(sheet.getByText(/only time this token is shown/)).toBeVisible();
    // and it is copyable, because it can never be read back
    await expect(
      sheet.getByRole("button", { name: /Copy provisioning token/ }),
    ).toBeVisible();
  },
};

// revoking is immediate and does not touch the accounts already provisioned —
// the confirmation has to say that before the operator commits
export const RevokeExplainsWhatItDoesNotDo: Story = {
  render: () => {
    let revoked = false;
    const stub = scoped(async (init) => {
      if (init?.method === "DELETE") {
        revoked = true;
        return json(token({ revoked_at: NOW }));
      }
      return json([revoked ? token({ revoked_at: NOW }) : token()]);
    });
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // by name, not by position: the row control names the token (#1214)
    await userEvent.click(
      await canvas.findByRole("button", {
        name: "Revoke provisioning token Okta production",
      }),
    );
    const modal = within(await within(document.body).findByRole("dialog"));
    await expect(
      modal.getByText(/nobody is deactivated or logged out/),
    ).toBeVisible();
    await userEvent.click(modal.getByRole("button", { name: "Revoke" }));
    await waitFor(() => expect(canvas.getByText("REVOKED")).toBeVisible());
  },
};

// the second half of the screen (#1186): the tokens decide who exists, the
// mappings decide what they may do. a mapping names its scope, so a team-scoped
// grant is distinguishable from an org-wide one at a glance
export const GroupMappingsListed: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(
        async () => json(TOKENS),
        async () => json(MAPPINGS),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText("platform-engineering")).toBeVisible(),
    );
    // asserted per row rather than per string: the add form's own selects carry
    // the same scope and role labels as options
    await expect(canvas.getByText("platform-engineering").closest("li")).toHaveTextContent(
      "Whole organization",
    );
    const scoped = canvas.getByText("sre-oncall").closest("li");
    // team-1 is "core" in the scope stub, so the stored id is shown as its name
    await expect(scoped).toHaveTextContent("core");
    await expect(scoped).toHaveTextContent("Admin");
  },
};

// an org with tokens but no mappings is the trap the screen has to name: the
// IdP syncs happily and everyone it provisions can still do nothing
export const GroupMappingsEmpty: Story = {
  render: () => <Harness fetchStub={scoped(async () => json(TOKENS))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(
        canvas.getByText(/every provisioned account joins as a viewer/),
      ).toBeVisible(),
    );
  },
};

// what the stub recorded, asserted in `play`. a module-level sink rather than a
// second fetch wrapper: the stub is already the only thing the screen talks to
const postedMappings: unknown[] = [];

// the scope select is why this is more than a group/role pair — a team-scoped
// grant has to send the team id, and only the team id
export const MapGroupPostsTheScopedRole: Story = {
  render: () => {
    postedMappings.length = 0;
    const stub = scoped(
      async () => json(TOKENS),
      async (init) => {
        if (init?.method === "POST") {
          postedMappings.push(JSON.parse(String(init.body)));
          return json(
            mapping({ id: "map-new", group_name: "sre-oncall", role: "admin" }),
          );
        }
        return json(
          postedMappings.length
            ? [mapping({ id: "map-new", group_name: "sre-oncall", role: "admin" })]
            : [],
        );
      },
    );
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await canvas.findByLabelText("IdP group"), "sre-oncall");
    await userEvent.selectOptions(
      canvas.getByLabelText("Where the role applies"),
      "team:team-1",
    );
    await userEvent.selectOptions(canvas.getByLabelText("Role to grant"), "admin");
    await userEvent.click(canvas.getByRole("button", { name: "Map group" }));
    await waitFor(() => expect(postedMappings).toHaveLength(1));
    await expect(postedMappings[0]).toEqual({
      group_name: "sre-oncall",
      role: "admin",
      team_id: "team-1",
    });
    await waitFor(() => expect(canvas.getByText("sre-oncall")).toBeVisible());
  },
};

const deletedMappings: string[] = [];

// removing a mapping withdraws a role from everyone in the group, so it goes
// through ConfirmDialog and says so before the DELETE goes out (#1179)
export const RemoveMappingConfirmsFirst: Story = {
  render: () => {
    deletedMappings.length = 0;
    const stub = scoped(
      async () => json([]),
      async (init) => {
        if (init?.method === "DELETE") {
          deletedMappings.push("deleted");
          return new Response(null, { status: 204 });
        }
        return json(deletedMappings.length ? [] : [mapping()]);
      },
    );
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", {
        name: "Remove the mapping for platform-engineering",
      }),
    );
    const modal = within(await within(document.body).findByRole("dialog"));
    await expect(modal.getByText(/loses Member straight away/)).toBeVisible();
    await userEvent.click(modal.getByRole("button", { name: "Remove mapping" }));
    await waitFor(() => expect(deletedMappings).toHaveLength(1));
    await waitFor(() =>
      expect(canvas.queryByText("platform-engineering")).toBeNull(),
    );
  },
};

// the bug #1249 was filed for: the scope select used to list only the projects
// of the team the switcher had selected, so a mapping onto a project in another
// team could not be written without moving the switcher first. the picker now
// groups every project under its own team, and posts that project's id
export const MapGroupToAProjectInAnotherTeam: Story = {
  render: () => {
    postedMappings.length = 0;
    const stub = scoped(
      async () => json(TOKENS),
      async (init) => {
        if (init?.method === "POST") {
          postedMappings.push(JSON.parse(String(init.body)));
          return json(
            mapping({
              id: "map-new",
              group_name: "checkout-oncall",
              role: "member",
              project_id: "proj-2",
            }),
          );
        }
        return json(
          postedMappings.length
            ? [
                mapping({
                  id: "map-new",
                  group_name: "checkout-oncall",
                  role: "member",
                  project_id: "proj-2",
                }),
              ]
            : [],
        );
      },
    );
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const scope = await canvas.findByLabelText("Where the role applies");
    // grouped by team, so two teams may each have a "prod" without the reader
    // having to guess which one an option means
    await waitFor(() =>
      expect(within(scope).getByRole("group", { name: "Projects in payments" })).toBeInTheDocument(),
    );
    await userEvent.type(await canvas.findByLabelText("IdP group"), "checkout-oncall");
    await userEvent.selectOptions(scope, "project:proj-2");
    await userEvent.selectOptions(canvas.getByLabelText("Role to grant"), "member");
    await userEvent.click(canvas.getByRole("button", { name: "Map group" }));
    await waitFor(() => expect(postedMappings).toHaveLength(1));
    await expect(postedMappings[0]).toEqual({
      group_name: "checkout-oncall",
      role: "member",
      project_id: "proj-2",
    });
    // and the listed mapping names that project, not its raw id
    await waitFor(() =>
      expect(canvas.getByText("checkout-oncall").closest("li")).toHaveTextContent(
        "checkout",
      ),
    );
  },
};

// an org with no teams can only be mapped org-wide, and the form says so rather
// than offering a select with one option and no explanation
export const ScopePickerHasNoTeams: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(
        async () => json(TOKENS),
        async () => json([]),
        { teams: async () => json([]) },
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText(/no teams yet/)).toBeVisible(),
    );
  },
};

// the team list the picker reads can fail on its own: the narrower scopes go
// away, the org-wide mapping the operator was probably writing does not
export const ScopePickerCannotListTeams: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(
        async () => json(TOKENS),
        async () => json([]),
        { teams: async () => json({ error: { message: "boom" } }, 500) },
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /teams and projects/);
    await expect(canvas.getByLabelText("Where the role applies")).toBeVisible();
  },
};
