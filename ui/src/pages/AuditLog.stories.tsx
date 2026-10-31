import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { MemoryRouter } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import AuditLog from "./AuditLog";
import {
  Harness,
  captureFollowedLinks,
  expectEmptyState,
  expectToast,
  expectGateAnswered,
  expectLoadError,
  expectNoFalseEmpty,
  expectSkeleton,
  expectTableStateInFrame,
  json,
  openOptions,
  pending,
  pickOption,
  recording,
  routes,
  savedText,
  scoped,
  STORY_DOWNLOAD_URL,
  stubSavePicker,
  Toasted,
  withCapabilities,
  withExportTickets,
  type FetchStub,
} from "./story-harness";
import type { AuditLogEntry, UserRow } from "@/lib/api";
import { AuthProvider } from "@/lib/auth";
import ru from "@/lib/i18n/locales/ru.json";
import { atMobile } from "@/lib/story-viewport";

const entry = (over: Partial<AuditLogEntry> = {}): AuditLogEntry => ({
  id: "a-1",
  org_id: "org-1",
  actor_user_id: "11111111-1111-1111-1111-111111111111",
  action: "provider.create",
  target_type: "provider",
  target_id: "p-1",
  detail: null,
  at: "2026-08-06T10:00:00Z",
  ...over,
});

const page = (items: AuditLogEntry[]) => ({
  items,
  next_cursor: null,
  previous_cursor: null,
  has_next: false,
  has_previous: false,
  total: items.length,
});

// the screen links a target row through react-router, so the story has to
// supply a router the same way `main.tsx` does
function Screen({ fetchStub }: { fetchStub: FetchStub }) {
  return (
    <MemoryRouter>
      <Harness fetchStub={fetchStub}>
        <AuditLog />
      </Harness>
    </MemoryRouter>
  );
}

const loaded = routes([
  ["/audit-log", () => page([entry(), entry({ id: "a-2", action: "route.delete" })])],
]);

const meta = {
  title: "Screens/AuditLog",
  component: AuditLog,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof AuditLog>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => <Screen fetchStub={loaded} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("provider.create")).toBeVisible());
    await expect(canvas.getByText("route.delete")).toBeVisible();

    const group = canvas.getByRole("radiogroup", { name: "Time range filter" });
    await expect(group).toBeInTheDocument();
    const allRadio = canvas.getByRole("radio", { name: "All" });
    await expect(allRadio).toHaveAttribute("aria-checked", "true");

    const prevButton = canvas.getByRole("button", { name: "Previous page" });
    const nextButton = canvas.getByRole("button", { name: "Next page" });
    await expect(prevButton).toBeDisabled();
    await expect(nextButton).toBeDisabled();
  },
};

export const Paginated: Story = {
  render: () => (
    <Screen
      fetchStub={scoped(async (input) => {
        const url = new URL(String(input), "http://localhost");
        if (url.pathname.includes("/audit-log")) {
          const cursor = url.searchParams.get("cursor");
          if (cursor === "c-next") {
            return json({
              items: [entry({ id: "a-3", action: "virtual_key.create" })],
              next_cursor: null,
              previous_cursor: "c-prev",
              has_next: false,
              has_previous: true,
              total: 3,
            });
          }
          return json({
            items: [entry({ id: "a-1" }), entry({ id: "a-2", action: "route.delete" })],
            next_cursor: "c-next",
            previous_cursor: null,
            has_next: true,
            has_previous: false,
            total: 3,
          });
        }
        return json([]);
      })}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("provider.create")).toBeVisible());

    const prevButton = canvas.getByRole("button", { name: "Previous page" });
    const nextButton = canvas.getByRole("button", { name: "Next page" });
    await expect(prevButton).toBeDisabled();
    await expect(nextButton).toBeEnabled();
  },
};

// the table header is replaced by a shaped skeleton rather than left standing
// over nothing, which is what made a slow page read as an empty one
export const Loading: Story = {
  render: () => <Screen fetchStub={pending} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No audit entries yet/);
  },
};

// the widest window is the default, so an empty page under it is "nothing has
// happened yet" and carries no clear-filters button
export const Empty: Story = {
  render: () => <Screen fetchStub={routes([["/audit-log", () => page([])]])} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectEmptyState(canvasElement, /No audit entries yet/);
    await expect(
      canvas.queryByRole("button", { name: /Clear (search|filters)/i }),
    ).not.toBeInTheDocument();
  },
};

// a narrowed window that matches nothing is a filter answer, and its button
// says what it clears: every filter, not only a search (#2294)
export const NoMatch: Story = {
  render: () => <Screen fetchStub={routes([["/audit-log", () => page([])]])} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("radio", { name: "Last 24h" }));
    await expectEmptyState(canvasElement, /No entries match these filters/);
    await userEvent.click(canvas.getByRole("button", { name: "Clear filters" }));
    await expectEmptyState(canvasElement, /No audit entries yet/);
  },
};

// the placeholder is centred on the part of the table the reader sees, not on
// the whole of a table that scrolls sideways inside its card (#2420)
export const EmptyFitsThePhone: Story = {
  ...atMobile,
  render: () => <Screen fetchStub={routes([["/audit-log", () => page([])]])} />,
  play: async ({ canvasElement }) => {
    await expectTableStateInFrame(canvasElement, {
      says: /No audit entries yet/,
      body: /Every change made through the control plane is recorded here/,
    });
  },
};

export const EmptyFitsThePhoneInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => <Screen fetchStub={routes([["/audit-log", () => page([])]])} />,
  play: async ({ canvasElement }) => {
    const { emptyTitle, emptyBody } = ru.pages.auditLog;
    await expectTableStateInFrame(canvasElement, {
      says: new RegExp(emptyTitle),
      body: new RegExp(emptyBody.slice(0, 24)),
    });
  },
};

export const Error_: Story = {
  name: "Error",
  render: () => (
    <Screen fetchStub={scoped(async () => json({ error: { message: "boom" } }, 500))} />
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return the audit log/i);
    await expectNoFalseEmpty(canvasElement, /No audit entries yet/);
  },
};

export const Forbidden: Story = {
  render: () => (
    <Screen fetchStub={scoped(async () => json({ error: { message: "forbidden" } }, 403))} />
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /You do not have access to the audit log/);
    await expectNoFalseEmpty(canvasElement, /No audit entries yet/);
  },
};

// the deployment-wide read (#2398): a superadmin switches scope and sees the
// rows no per-org view carries, marked rather than left blank
const wideEntries = [
  entry({ id: "w-1", org_id: null, action: "auth.login", target_type: null }),
  entry({
    id: "w-2",
    org_id: null,
    actor_user_id: null,
    action: "auth.login_failed",
    target_type: null,
  }),
];

const wideStub = (calls: { urls: string[] }) =>
  withCapabilities(
    "superadmin",
    scoped(async (input) => {
      const url = String(input);
      if (url.includes("/api/v1/audit-log")) {
        calls.urls.push(url);
        return json(page(wideEntries));
      }
      if (url.includes("/audit-log")) return json(page([entry()]));
      return json([]);
    }),
  );

export const DeploymentWideAsSuperadmin: Story = {
  render: () => {
    const calls = { urls: [] as string[] };
    return (
      <MemoryRouter>
        <Harness fetchStub={wideStub(calls)} role="superadmin">
          <AuditLog />
        </Harness>
      </MemoryRouter>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("provider.create")).toBeVisible());
    const scope = await canvas.findByRole("radiogroup", { name: "Audit log scope" });
    await userEvent.click(within(scope).getByRole("radio", { name: "Whole deployment" }));
    await waitFor(() => expect(canvas.getByText("auth.login_failed")).toBeVisible());
    await expect(canvas.getAllByText("No org")).toHaveLength(2);
    await expect(canvas.getByText("Unknown address")).toBeVisible();
    await expect(canvas.getByRole("columnheader", { name: "Org" })).toBeVisible();
  },
};

export const DeploymentWideIsHiddenFromAnAdmin: Story = {
  render: () => (
    <MemoryRouter>
      <Harness fetchStub={loaded} role="admin">
        <AuditLog />
      </Harness>
    </MemoryRouter>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("provider.create")).toBeVisible());
    await expectGateAnswered();
    await expect(canvas.queryByRole("radiogroup", { name: "Audit log scope" })).toBeNull();
  },
};

// the filters offer every action the control plane audits (#2127), grouped and
// type-to-filter; picking an identity action sends it and links the row to the
// screen that owns the target
export const FilterToASsoProviderChange: Story = {
  render: () => {
    const rec = recording(
      scoped(async (input) => {
        const url = String(input);
        if (url.includes("/audit-log")) {
          return json(
            url.includes("action=sso_provider.update")
              ? page([
                  entry({
                    id: "s-1",
                    action: "sso_provider.update",
                    target_type: "sso_provider",
                    target_id: "5a5a5a5a-0000-0000-0000-000000000000",
                  }),
                ])
              : page([entry()]),
          );
        }
        return json([]);
      }),
    );
    return <Screen fetchStub={rec.stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("provider.create")).toBeVisible());
    const action = canvas.getByRole("combobox", { name: "Filter by action" });
    await userEvent.type(action, "sso_provider.up");
    const option = await within(document.body).findByRole("option", {
      name: "sso_provider.update",
    });
    await expect(
      within(document.body).getByRole("group", { name: "Identity and access" }),
    ).toBeVisible();
    await userEvent.click(option);
    const row = await canvas.findByText("sso_provider/5a5a5a5a");
    await expect(row.closest("a")).toHaveAttribute("href", "/sso");
    await expect(canvas.queryByText("provider.create")).toBeNull();
  },
};

// the rows for scoped API tokens (#3289) and LDAP directories (#3261) link to
// the screens that own them
export const ApiTokenAndLdapRowsLinkToTheirScreens: Story = {
  render: () => (
    <Screen
      fetchStub={scoped(async (input) =>
        String(input).includes("/audit-log")
          ? json(
              page([
                entry({
                  id: "t-1",
                  action: "api_token.create",
                  target_type: "api_token",
                  target_id: "7a7a7a7a-0000-0000-0000-000000000000",
                }),
                entry({
                  id: "t-2",
                  action: "ldap_directory.update",
                  target_type: "ldap_directory",
                  target_id: "8b8b8b8b-0000-0000-0000-000000000000",
                }),
              ]),
            )
          : json([]),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const token = await canvas.findByText("api_token/7a7a7a7a");
    await expect(token.closest("a")).toHaveAttribute("href", "/api-tokens");
    const directory = await canvas.findByText("ldap_directory/8b8b8b8b");
    await expect(directory.closest("a")).toHaveAttribute("href", "/ldap");
  },
};

export const TargetFilterOffersIdentityTypes: Story = {
  render: () => <Screen fetchStub={loaded} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const target = await canvas.findByRole("combobox", { name: "Filter by target type" });
    await userEvent.click(target);
    for (const kind of ["sso_provider", "scim_token", "custom_role", "invitation", "team", "org"]) {
      await expect(
        await within(document.body).findByRole("option", { name: kind }),
      ).toBeInTheDocument();
    }
  },
};

// -------------------------- naming the actor (#2858)

const account = (id: string, email: string, is_superadmin = false): UserRow => ({
  id,
  email,
  is_superadmin,
  deactivated_at: null,
  created_at: "2026-01-01T00:00:00Z",
});

/** holds a role in the org, so the org's people list names them */
const MEMBER = account("11111111-1111-1111-1111-111111111111", "ada@example.com");
/**
 * the operator `rolter-seed --admin-email` creates: a superadmin with no
 * membership, so only `include_unassigned=true` lists them
 */
const OPERATOR = account("22222222-2222-2222-2222-222222222222", "root@example.com", true);
/** an actor no list the caller can read carries */
const STRANGER = "33333333-3333-3333-3333-333333333333";
/**
 * holds a role in another org only: neither the org's people nor the
 * unassigned accounts reach her, and only the deployment's account list does
 */
const ELSEWHERE = account("44444444-4444-4444-4444-444444444444", "grace@other.example");
/** every account in the deployment, e-mail ordered, as `GET /api/v1/users` answers */
const EVERYONE = [MEMBER, ELSEWHERE, OPERATOR];

const byActor = [
  entry({ id: "m-1", action: "provider.create", actor_user_id: MEMBER.id }),
  entry({ id: "m-2", action: "route.delete", actor_user_id: OPERATOR.id }),
  entry({ id: "m-3", action: "virtual_key.create", actor_user_id: STRANGER }),
];

/** the one row only the deployment-wide read carries: its actor is in another org */
const byElsewhere = entry({ id: "m-4", action: "budget.update", actor_user_id: ELSEWHERE.id });

/**
 * The control plane as it answers the audit log and the people lists: the
 * org's people carry the unassigned accounts only when `include_unassigned=true`
 * is asked for, the deployment's account list (`/api/v1/users`) is everyone,
 * and each audit read honours the `actor` filter it was sent. The
 * deployment-wide read spans orgs, so it also returns the other org's row.
 * `deploymentUsers` replaces the answer to that account list.
 */
function auditWithOperator(deploymentUsers?: () => Promise<Response> | Response) {
  return recording(
    scoped(async (input) => {
      const url = new URL(String(input), "http://localhost");
      if (url.pathname === "/api/v1/users") {
        return deploymentUsers ? deploymentUsers() : json(EVERYONE);
      }
      if (url.pathname.endsWith("/users")) {
        return json(
          url.searchParams.get("include_unassigned") === "true" ? [MEMBER, OPERATOR] : [MEMBER],
        );
      }
      if (url.pathname.endsWith("/audit-log")) {
        const actor = url.searchParams.get("actor");
        const rows = url.pathname === "/api/v1/audit-log" ? [...byActor, byElsewhere] : byActor;
        return json(page(rows.filter((row) => !actor || row.actor_user_id === actor)));
      }
      return json([]);
    }),
  );
}

/** the calls that asked for every account in the deployment */
const deploymentUserReads = (recorder: { calls: { url: string }[] }) =>
  recorder.calls.filter((c) => c.url === "/api/v1/users");

/**
 * Signed in the way a login leaves the browser, minus the token, so the
 * provider knows the account without asking /auth/me for it. The screen reads
 * it to tell a superadmin from anyone else.
 */
function SignedInAs({ user, children }: { user: UserRow; children: React.ReactNode }) {
  React.useState(() => {
    localStorage.setItem("rolter.session.email", user.email);
    localStorage.setItem("rolter.session.user", JSON.stringify(user));
    localStorage.removeItem("rolter.session.token");
  });
  React.useEffect(
    () => () => {
      localStorage.removeItem("rolter.session.email");
      localStorage.removeItem("rolter.session.user");
    },
    [],
  );
  return <AuthProvider>{children}</AuthProvider>;
}

function SignedInScreen({
  fetchStub,
  user,
  role,
}: {
  fetchStub: FetchStub;
  user: UserRow;
  role: "superadmin" | "admin";
}) {
  return (
    <MemoryRouter>
      <Harness fetchStub={fetchStub} role={role}>
        <SignedInAs user={user}>
          <AuditLog />
        </SignedInAs>
      </Harness>
    </MemoryRouter>
  );
}

const orgScope = auditWithOperator();

/**
 * A superadmin with no membership is on no org's people list, so the actor
 * column named them by the first eight hex digits of their id. The screen asks
 * for the unassigned accounts too, names them, and the actor filter picks them
 * by e-mail. An actor nobody can resolve keeps the short id, with the whole id
 * on hover.
 */
export const NamesASuperadminWithNoMembership: Story = {
  render: () => <SignedInScreen fetchStub={orgScope.stub} user={OPERATOR} role="superadmin" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const operator = await canvas.findByText("root@example.com");
    await expect(operator).toHaveAttribute("title", OPERATOR.id);
    await expect(canvas.getByText("ada@example.com")).toHaveAttribute("title", MEMBER.id);
    await expect(canvas.getByText("Not in this org")).toHaveAttribute("title", STRANGER);
    await orgScope.expectSent("GET", "/orgs/org-1/users?include_unassigned=true");
    // the deployment's account list is for the other scope, not asked here
    await expectGateAnswered();
    await expect(deploymentUserReads(orgScope)).toHaveLength(0);

    await pickOption(canvas.getByRole("combobox", { name: "Filter by actor" }), "root@example.com");
    await orgScope.expectSent("GET", `/orgs/org-1/audit-log?`);
    await waitFor(() => expect(canvas.queryByText("provider.create")).toBeNull());
    await expect(canvas.getByText("route.delete")).toBeVisible();
    await expect(canvas.queryByText("virtual_key.create")).toBeNull();
    await expect(
      orgScope.calls.some(
        (c) => c.url.includes(`/orgs/org-1/audit-log?`) && c.url.includes(`actor=${OPERATOR.id}`),
      ),
    ).toBe(true);
  },
};

const deploymentScope = auditWithOperator();

/**
 * The whole-deployment scope reads every account in the deployment, so the
 * operator is named and picked by e-mail there too.
 */
export const NamesASuperadminWithNoMembershipInTheWholeDeployment: Story = {
  render: () => (
    <SignedInScreen fetchStub={deploymentScope.stub} user={OPERATOR} role="superadmin" />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const scope = await canvas.findByRole("radiogroup", { name: "Audit log scope" });
    await userEvent.click(within(scope).getByRole("radio", { name: "Whole deployment" }));
    await deploymentScope.expectSent("GET", "/api/v1/audit-log?");
    await deploymentScope.expectSent("GET", "/api/v1/users");
    const operator = await canvas.findByText("root@example.com");
    await expect(operator).toHaveAttribute("title", OPERATOR.id);
    await expect(canvas.getByText("Unknown account")).toHaveAttribute("title", STRANGER);

    const filter = canvas.getByRole("combobox", { name: "Filter by actor" });
    const listbox = await openOptions(filter);
    await expect(within(listbox).getByRole("option", { name: "root@example.com" })).toBeVisible();
    await userEvent.keyboard("{Escape}");

    await pickOption(filter, "root@example.com");
    await waitFor(() =>
      expect(
        deploymentScope.calls.some(
          (c) => c.url.startsWith("/api/v1/audit-log?") && c.url.includes(`actor=${OPERATOR.id}`),
        ),
      ).toBe(true),
    );
    await waitFor(() => expect(canvas.queryByText("virtual_key.create")).toBeNull());
    await expect(canvas.getByText("route.delete")).toBeVisible();
  },
};

const customActor = auditWithOperator();

/**
 * An actor whose account is gone is on no list, whatever the scope: it is
 * filtered by typing its id in whole, which the plain org scope never needs.
 */
export const DeploymentFiltersToAnActorOutsideTheList: Story = {
  render: () => <SignedInScreen fetchStub={customActor.stub} user={OPERATOR} role="superadmin" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const scope = await canvas.findByRole("radiogroup", { name: "Audit log scope" });
    await userEvent.click(within(scope).getByRole("radio", { name: "Whole deployment" }));
    await customActor.expectSent("GET", "/api/v1/audit-log?");
    await expect(await canvas.findByText("virtual_key.create")).toBeVisible();

    await userEvent.type(canvas.getByRole("combobox", { name: "Filter by actor" }), STRANGER);
    await userEvent.click(await canvas.findByRole("option", { name: `Use “${STRANGER}”` }));
    await waitFor(() => expect(canvas.queryByText("route.delete")).toBeNull());
    await expect(canvas.getByText("virtual_key.create")).toBeVisible();
    await expect(
      customActor.calls.some(
        (c) => c.url.startsWith("/api/v1/audit-log?") && c.url.includes(`actor=${STRANGER}`),
      ),
    ).toBe(true);
  },
};

const acrossOrgs = auditWithOperator();

/**
 * #2871: an actor who belongs to another org is on neither the org's people nor
 * the unassigned accounts, so the whole-deployment read named them by the first
 * eight digits of their id. A superadmin there reads every account in the
 * deployment instead: the actor column names the other org's member by e-mail,
 * the filter lists and picks them, and an actor nobody can resolve (a deleted
 * account) keeps the short id with the whole id on hover.
 */
export const NamesAnActorFromAnotherOrgInTheWholeDeployment: Story = {
  render: () => <SignedInScreen fetchStub={acrossOrgs.stub} user={OPERATOR} role="superadmin" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const scope = await canvas.findByRole("radiogroup", { name: "Audit log scope" });
    await userEvent.click(within(scope).getByRole("radio", { name: "Whole deployment" }));
    await acrossOrgs.expectSent("GET", "/api/v1/users");

    const grace = await canvas.findByText("grace@other.example");
    await expect(grace).toHaveAttribute("title", ELSEWHERE.id);
    await expect(canvas.getByText("budget.update")).toBeVisible();
    await expect(canvas.getByText("ada@example.com")).toHaveAttribute("title", MEMBER.id);
    await expect(canvas.getByText("root@example.com")).toHaveAttribute("title", OPERATOR.id);
    await expect(canvas.getByText("Unknown account")).toHaveAttribute("title", STRANGER);
    await expect(canvas.queryByText("44444444")).toBeNull();

    const filter = canvas.getByRole("combobox", { name: "Filter by actor" });
    const listbox = await openOptions(filter);
    for (const email of ["ada@example.com", "grace@other.example", "root@example.com"]) {
      await expect(within(listbox).getByRole("option", { name: email })).toBeVisible();
    }
    await userEvent.keyboard("{Escape}");

    await pickOption(filter, "grace@other.example");
    await waitFor(() =>
      expect(
        acrossOrgs.calls.some(
          (c) => c.url.startsWith("/api/v1/audit-log?") && c.url.includes(`actor=${ELSEWHERE.id}`),
        ),
      ).toBe(true),
    );
    await waitFor(() => expect(canvas.queryByText("provider.create")).toBeNull());
    await expect(canvas.getByText("budget.update")).toBeVisible();
    await expect(deploymentUserReads(acrossOrgs)).toHaveLength(1);
  },
};

const listFails = auditWithOperator(() => json({ error: { message: "store down" } }, 500));

/**
 * The deployment's account list is a nicety on top of a log that still reads.
 * When it fails the screen keeps naming the org's people and the unassigned
 * accounts from the list it already had, the actor from another org falls back
 * to the short id with the whole id on hover, and the log itself is not turned
 * into an error.
 */
export const AFailedDeploymentListFallsBackToTheOrgsPeople: Story = {
  render: () => <SignedInScreen fetchStub={listFails.stub} user={OPERATOR} role="superadmin" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const scope = await canvas.findByRole("radiogroup", { name: "Audit log scope" });
    await userEvent.click(within(scope).getByRole("radio", { name: "Whole deployment" }));
    await listFails.expectSent("GET", "/api/v1/users");

    await expect(await canvas.findByText("budget.update")).toBeVisible();
    await expect(canvas.getByText("root@example.com")).toHaveAttribute("title", OPERATOR.id);
    await expect(canvas.getByText("ada@example.com")).toHaveAttribute("title", MEMBER.id);
    await expect(canvas.getByTitle(ELSEWHERE.id)).toHaveTextContent("Unknown account");
    await expect(canvas.queryByText("grace@other.example")).toBeNull();
    await expect(canvas.queryByRole("alert")).toBeNull();
    const listbox = await openOptions(canvas.getByRole("combobox", { name: "Filter by actor" }));
    await expect(within(listbox).getByRole("option", { name: "root@example.com" })).toBeVisible();
    await expect(within(listbox).queryByRole("option", { name: "grace@other.example" })).toBeNull();
  },
};

const listLoading = auditWithOperator(() => new Promise<Response>(() => {}));

/**
 * While the deployment's account list is still on its way the names the screen
 * already had stay on the rows, so entering the scope does not blank the actor
 * column into short ids for the length of one request.
 */
export const TheOrgsPeopleStayNamedWhileTheDeploymentListLoads: Story = {
  render: () => <SignedInScreen fetchStub={listLoading.stub} user={OPERATOR} role="superadmin" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("root@example.com")).toBeVisible();
    const scope = await canvas.findByRole("radiogroup", { name: "Audit log scope" });
    await userEvent.click(within(scope).getByRole("radio", { name: "Whole deployment" }));
    await listLoading.expectSent("GET", "/api/v1/users");

    await expect(await canvas.findByText("budget.update")).toBeVisible();
    await expect(canvas.getByText("root@example.com")).toBeVisible();
    await expect(canvas.getByText("ada@example.com")).toBeVisible();
    await expect(canvas.getByTitle(ELSEWHERE.id)).toHaveTextContent("Unknown account");
    await expect(canvas.getByRole("combobox", { name: "Filter by actor" })).toBeVisible();
  },
};

const asAdmin = auditWithOperator();

/**
 * Anyone who is not a superadmin never asks for the unassigned accounts: the
 * flag would change nothing server-side. The operator stays a short id, as
 * before, and the filter lists the org's people only.
 */
export const AnAdminDoesNotAskForUnassignedAccounts: Story = {
  render: () => <SignedInScreen fetchStub={asAdmin.stub} user={MEMBER} role="admin" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("ada@example.com")).toBeVisible();
    await expect(canvas.getByTitle(OPERATOR.id)).toHaveTextContent("Not in this org");
    await expect(canvas.queryByText("root@example.com")).toBeNull();
    await expectGateAnswered();
    await expect(asAdmin.calls.filter((c) => c.url.includes("include_unassigned"))).toHaveLength(0);
    await asAdmin.expectSent("GET", "/orgs/org-1/users");
    // nor does it ask for every account in the deployment: that read is the
    // superadmin's, and the scope switch that leads to it is not offered
    await expect(deploymentUserReads(asAdmin)).toHaveLength(0);
    await expect(canvas.queryByRole("radiogroup", { name: "Audit log scope" })).toBeNull();

    const listbox = await openOptions(canvas.getByRole("combobox", { name: "Filter by actor" }));
    await expect(within(listbox).getByRole("option", { name: "ada@example.com" })).toBeVisible();
    await expect(within(listbox).queryByRole("option", { name: "root@example.com" })).toBeNull();
  },
};

// -------------------------- entries shown whole (#2129)

const FULL_TARGET = "5a5a5a5a-1111-2222-3333-444444444444";

const wholeEntries = [
  entry({
    id: "w-1",
    action: "provider.update",
    target_id: FULL_TARGET,
    actor_user_id: STRANGER,
    detail: { name: "openai-prod" },
  }),
  entry({ id: "w-2", action: "route.delete", actor_user_id: null, target_id: null, detail: null }),
];

/**
 * The detail opens in a row of its own under the entry it belongs to,
 * labelled with the action, target and time; every id is whole and copyable;
 * an actor the org's list does not carry says so instead of showing a prefix;
 * and a row with no actor reads "system" from the catalog.
 */
export const DetailOpensUnderItsRow: Story = {
  render: () => <Screen fetchStub={routes([["/audit-log", () => page(wholeEntries)]])} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("Not in this org")).toBeVisible();
    await expect(canvas.getByText("system")).toBeVisible();

    await userEvent.click(
      canvas.getByRole("button", {
        name: "Show details for action provider.update on target provider",
      }),
    );
    const entryRegion = await canvas.findByRole("region", {
      name: /^provider\.update · provider · /,
    });
    await expect(entryRegion).toHaveTextContent("2026");
    await expect(within(entryRegion).getByTestId("audit-targetId")).toHaveTextContent(FULL_TARGET);
    await expect(within(entryRegion).getByTestId("audit-actorId")).toHaveTextContent(STRANGER);
    await expect(within(entryRegion).getByText("2026-08-06T10:00:00Z")).toBeVisible();
    await expect(within(entryRegion).getByRole("button", { name: /Copy Target id/ })).toBeVisible();
    // directly under its own row, not after the table
    const rows = canvas.getAllByRole("row");
    const own = rows.findIndex((r) => r.textContent?.includes("provider.update"));
    await expect(rows[own + 1]).toContainElement(entryRegion);
    await expect(within(entryRegion).getByText(/openai-prod/)).toBeVisible();

    await userEvent.click(
      canvas.getByRole("button", {
        name: "Hide details for action provider.update on target provider",
      }),
    );
    await waitFor(() => expect(canvas.queryByRole("region")).toBeNull());
  },
};

// -------------------------- export (#2130, #3051, #3545)

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

/** the screen with its toasts, on `stub` */
function Exporting({ stub }: { stub: FetchStub }) {
  return (
    <MemoryRouter>
      <Harness fetchStub={stub}>
        <Toasted>
          <AuditLog />
        </Toasted>
      </Harness>
    </MemoryRouter>
  );
}

/** pick the action filter and the 7-day window the export stories send */
async function filterTheLog(canvasElement: HTMLElement) {
  const canvas = within(canvasElement);
  await canvas.findByText("provider.create");
  await userEvent.type(
    canvas.getByRole("combobox", { name: "Filter by action" }),
    "sso_provider.up",
  );
  await userEvent.click(
    await within(document.body).findByRole("option", { name: "sso_provider.update" }),
  );
  await userEvent.click(canvas.getByRole("radio", { name: "Last 7d" }));
}

/**
 * a control plane that predates the export endpoint: it answers 404 there, and
 * for the download ticket, so the export has to follow the cursor itself, with
 * the filters on both pages
 */
const notMounted = () =>
  json({ error: { code: "no_such_endpoint", message: "no such endpoint" } }, 404);
const exportPages = () =>
  recording(
    scoped(async (input) => {
      const url = new URL(String(input), "http://localhost");
      if (url.pathname.endsWith("/audit-log/export") || url.pathname.includes("/exports/")) {
        return notMounted();
      }
      if (url.pathname.endsWith("/audit-log")) {
        return json(
          url.searchParams.get("cursor")
            ? { ...page([entry({ id: "x-2", action: "route.delete" })]), has_previous: true }
            : {
                ...page([entry({ id: "x-1" })]),
                next_cursor: "c-2",
                has_next: true,
              },
        );
      }
      return json([]);
    }),
  );

const exporting = exportPages();

// the fallback for an older control plane: the action filter rides on every
// page request, the cursor is followed, and the name carries the org and window
export const ExportFallsBackToPagesOnAnOlderControlPlane: Story = {
  render: () => <Exporting stub={exporting.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await filterTheLog(canvasElement);

    const names: string[] = [];
    await stubSavePicker().within(
      () =>
        captureDownload(names, async () => {
          await userEvent.click(canvas.getByRole("button", { name: "Export CSV" }));
          await waitFor(() => expect(names).toHaveLength(1));
        }),
      false,
    );
    await expect(names[0]).toMatch(
      /^audit-log-rolter-\d{4}-\d{2}-\d{2}_to_\d{4}-\d{2}-\d{2}\.csv$/,
    );
    await expectToast(canvasElement, /Exported 2 entries/, "success");

    const exportCalls = exporting.calls.filter(
      (c) => c.url.includes("/audit-log") && c.url.includes("limit=500"),
    );
    await expect(exportCalls).toHaveLength(2);
    for (const call of exportCalls) {
      await expect(call.url).toContain("action=sso_provider.update");
      await expect(call.url).toContain("from=");
      await expect(call.url).not.toContain("include_total");
    }
    await expect(exportCalls[1].url).toContain("cursor=c-2");
  },
};

/** the screen's own reads, with the download tickets answered in front of them */
const ticketed = (reads: Record<string, unknown>[]) =>
  recording(
    withExportTickets(
      scoped(async (input) => {
        const url = new URL(String(input), "http://localhost");
        return url.pathname.endsWith("/audit-log") ? json(page([entry()])) : json([]);
      }),
      reads,
    ),
  );

const throughTicket = ticketed([
  { state: "pending" },
  { state: "streaming", rows: 1000 },
  { state: "complete", rows: 1200 },
]);

/**
 * #3545: in a browser with no save dialog (Firefox, Safari, plain http) the
 * file goes to the browser's download manager through a single-use ticket, so
 * the tab never holds the log. The ticket names the org's export with the
 * filters on screen and no paging, the page follows its link once, and reads
 * its status for the rows written until the file is complete.
 */
export const ExportsTheFilteredLogThroughTheBrowser: Story = {
  render: () => <Exporting stub={throughTicket.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await filterTheLog(canvasElement);

    const links: string[] = [];
    await stubSavePicker().within(
      () =>
        captureFollowedLinks(links, async () => {
          await userEvent.click(canvas.getByRole("button", { name: "Export JSON" }));
          await expect(await canvas.findByText(/^[\d,]+ rows written$/)).toHaveAttribute(
            "role",
            "status",
          );
          await expectToast(canvasElement, /Export downloaded/, "success");
        }),
      false,
    );
    await expect(links).toEqual([STORY_DOWNLOAD_URL]);
    const minted = await throughTicket.expectSentBody<{ path: string; query: string }>(
      "POST",
      "/api/v1/analytics/exports/tickets",
    );
    await expect(minted.path).toBe("/api/v1/orgs/org-1/audit-log/export");
    const query = new URLSearchParams(minted.query);
    await expect(query.get("format")).toBe("json");
    await expect(query.get("action")).toBe("sso_provider.update");
    await expect(query.get("from")).not.toBeNull();
    await expect(query.has("limit")).toBe(false);
    // the page reads neither the export nor the link itself, so nothing of the
    // file passes through the tab
    await expect(throughTicket.calls.some((c) => c.url.includes("/audit-log/export"))).toBe(false);
    await expect(throughTicket.calls.some((c) => c.url.includes("/exports/download/"))).toBe(false);
  },
};

const refusedTicket = ticketed([{ state: "refused", status: 403 }]);

/**
 * The capability is checked again when the browser follows the link: a reader
 * who lost the role meanwhile is refused, which only the status tells the page.
 */
export const ARefusedAuditDownloadNamesItsStatus: Story = {
  render: () => <Exporting stub={refusedTicket.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("provider.create");
    await stubSavePicker().within(
      () =>
        captureFollowedLinks([], async () => {
          await userEvent.click(canvas.getByRole("button", { name: "Export CSV" }));
          await expectToast(canvasElement, /refused the download \(HTTP 403\)/, "error");
        }),
      false,
    );
  },
};

/** the control plane streams the file: one request, the filters on it, no paging */
const streamed = recording(
  scoped(async (input) => {
    const url = new URL(String(input), "http://localhost");
    if (url.pathname.endsWith("/audit-log/export")) {
      return new Response("at,actor_email\r\n", {
        headers: {
          "content-type": "text/csv; charset=utf-8",
          "content-disposition": 'attachment; filename="audit-log-rolter-all_to_2026-10-13.csv"',
        },
      });
    }
    return url.pathname.endsWith("/audit-log") ? json(page([entry()])) : json([]);
  }),
);

// where the browser has a save dialog the body streams into the picked file as
// it arrives (#3434, #3545): the filters ride on one export request, the
// dialog offers the dashboard's name for it, and no ticket is minted
export const ExportsTheFilteredLogToAPickedFile: Story = {
  render: () => <Exporting stub={streamed.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await filterTheLog(canvasElement);

    const picker = stubSavePicker();
    await picker.within(async () => {
      await userEvent.click(canvas.getByRole("button", { name: "Export CSV" }));
      await expectToast(canvasElement, /Audit log saved to the file you chose/, "success");
    });
    await expect(picker.files).toHaveLength(1);
    await expect(picker.files[0].suggestedName).toMatch(
      /^audit-log-rolter-\d{4}-\d{2}-\d{2}_to_\d{4}-\d{2}-\d{2}\.csv$/,
    );
    await expect(savedText(picker.files[0])).toBe("at,actor_email\r\n");
    await expect(picker.files[0].closed).toBe(true);

    const exportCalls = streamed.calls.filter((c) => c.url.includes("/audit-log/export"));
    await expect(exportCalls).toHaveLength(1);
    await expect(exportCalls[0].url).toContain("format=csv");
    await expect(exportCalls[0].url).toContain("action=sso_provider.update");
    await expect(exportCalls[0].url).toContain("from=");
    await expect(exportCalls[0].url).not.toContain("limit=");
    streamed.expectNotSent("POST", "/exports/tickets");
  },
};

/** an export streamed as JSON, for the save dialog's offered type */
const streamedJson = recording(
  scoped(async (input) => {
    const url = new URL(String(input), "http://localhost");
    if (url.pathname.endsWith("/audit-log/export")) {
      return new Response("[\n]\n", { headers: { "content-type": "application/json" } });
    }
    return url.pathname.endsWith("/audit-log") ? json(page([entry()])) : json([]);
  }),
);

// Export JSON asks the dialog for a JSON file, not the CSV every other export
// offers, and suggests the `.json` name
export const ExportJsonOffersAJsonFile: Story = {
  render: () => <Exporting stub={streamedJson.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("provider.create");
    const picker = stubSavePicker();
    await picker.within(async () => {
      await userEvent.click(canvas.getByRole("button", { name: "Export JSON" }));
      await expectToast(canvasElement, /Audit log saved to the file you chose/, "success");
    });
    await expect(picker.files[0].types).toEqual([
      { description: "JSON", accept: { "application/json": [".json"] } },
    ]);
    await expect(picker.files[0].suggestedName).toMatch(/\.json$/);
    await expect(savedText(picker.files[0])).toBe("[\n]\n");
  },
};

const wideTicket = recording(
  withExportTickets(wideStub({ urls: [] }), [{ state: "complete", rows: 2 }]),
);

/**
 * A superadmin on the whole deployment exports the deployment's log: the
 * ticket names `/api/v1/audit-log/export`, not any org's.
 */
export const ExportsTheDeploymentLogThroughTheBrowser: Story = {
  render: () => (
    <MemoryRouter>
      <Harness fetchStub={wideTicket.stub} role="superadmin">
        <Toasted>
          <AuditLog />
        </Toasted>
      </Harness>
    </MemoryRouter>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("provider.create")).toBeVisible());
    const scope = await canvas.findByRole("radiogroup", { name: "Audit log scope" });
    await userEvent.click(within(scope).getByRole("radio", { name: "Whole deployment" }));
    await waitFor(() => expect(canvas.getByText("auth.login_failed")).toBeVisible());
    const links: string[] = [];
    await stubSavePicker().within(
      () =>
        captureFollowedLinks(links, async () => {
          await userEvent.click(canvas.getByRole("button", { name: "Export CSV" }));
          await expectToast(canvasElement, /Export downloaded/, "success");
        }),
      false,
    );
    await expect(links).toEqual([STORY_DOWNLOAD_URL]);
    const minted = await wideTicket.expectSentBody<{ path: string; query: string }>(
      "POST",
      "/api/v1/analytics/exports/tickets",
    );
    await expect(minted.path).toBe("/api/v1/audit-log/export");
    await expect(new URLSearchParams(minted.query).get("format")).toBe("csv");
  },
};

/**
 * A rolling upgrade: the control plane has download tickets but not yet the
 * audit log's, so its mint refuses the path with a `400` naming the ones it
 * takes. The page reads the log a page at a time instead of failing.
 */
const halfUpgraded = recording(
  scoped(async (input) => {
    const url = new URL(String(input), "http://localhost");
    if (url.pathname === "/api/v1/analytics/exports/tickets") {
      return json(
        {
          error: {
            message:
              "path must be one of /api/v1/analytics/by-attribution/export, /api/v1/analytics/timeseries/export or /api/v1/analytics/invocations/export",
          },
        },
        400,
      );
    }
    return url.pathname.endsWith("/audit-log") ? json(page([entry()])) : json([]);
  }),
);

export const ExportFallsBackToPagesWhenTheMintDoesNotKnowTheAuditLog: Story = {
  render: () => <Exporting stub={halfUpgraded.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("provider.create");
    const names: string[] = [];
    await stubSavePicker().within(
      () =>
        captureDownload(names, async () => {
          await userEvent.click(canvas.getByRole("button", { name: "Export CSV" }));
          await waitFor(() => expect(names).toHaveLength(1));
        }),
      false,
    );
    await expect(names[0]).toMatch(/^audit-log-rolter-.*\.csv$/);
    await expectToast(canvasElement, /Exported 1 entry/, "success");
    await halfUpgraded.expectSent("GET", "limit=500");
  },
};

const exportFails = recording(
  scoped(async (input) => {
    const url = new URL(String(input), "http://localhost");
    if (url.pathname.endsWith("/audit-log/export")) {
      return json({ error: { message: "store unavailable" } }, 500);
    }
    return url.pathname.endsWith("/audit-log") ? json(page([entry()])) : json([]);
  }),
);

export const ExportFails: Story = {
  render: () => <Exporting stub={exportFails.stub} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("provider.create");
    const picker = stubSavePicker();
    await picker.within(async () => {
      await userEvent.click(canvas.getByRole("button", { name: "Export JSON" }));
      await expectToast(canvasElement, /Could not export the audit log export/, "error");
    });
    // the file the dialog made is not left behind half-written
    await expect(picker.files[0].aborted).toBe(true);
  },
};
