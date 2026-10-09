import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { MemoryRouter } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import AuditLog from "./AuditLog";
import {
  Harness,
  expectEmptyState,
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
  scoped,
  withCapabilities,
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

const byActor = [
  entry({ id: "m-1", action: "provider.create", actor_user_id: MEMBER.id }),
  entry({ id: "m-2", action: "route.delete", actor_user_id: OPERATOR.id }),
  entry({ id: "m-3", action: "virtual_key.create", actor_user_id: STRANGER }),
];

/**
 * The control plane as it answers the audit log and the org's people: the
 * unassigned accounts ride along only when `include_unassigned=true` is asked
 * for, and the audit read honours the `actor` filter it was sent.
 */
function auditWithOperator() {
  return recording(
    scoped(async (input) => {
      const url = new URL(String(input), "http://localhost");
      if (url.pathname.endsWith("/users")) {
        return json(
          url.searchParams.get("include_unassigned") === "true" ? [MEMBER, OPERATOR] : [MEMBER],
        );
      }
      if (url.pathname.endsWith("/audit-log")) {
        const actor = url.searchParams.get("actor");
        return json(page(byActor.filter((row) => !actor || row.actor_user_id === actor)));
      }
      return json([]);
    }),
  );
}

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
    await expect(canvas.getByText("33333333")).toHaveAttribute("title", STRANGER);
    await orgScope.expectSent("GET", "/orgs/org-1/users?include_unassigned=true");

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
 * The whole-deployment read has no people list of its own; it reads the scope's
 * org, whose unassigned accounts are the same everywhere. The operator is named
 * and picked by e-mail there too.
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
    const operator = await canvas.findByText("root@example.com");
    await expect(operator).toHaveAttribute("title", OPERATOR.id);
    await expect(canvas.getByText("33333333")).toHaveAttribute("title", STRANGER);

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
 * The list is one org's people, the read is every org's: an actor the list does
 * not carry is filtered by typing its id in whole, which the plain org scope
 * never needs.
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
    await expect(canvas.getByText("22222222")).toHaveAttribute("title", OPERATOR.id);
    await expect(canvas.queryByText("root@example.com")).toBeNull();
    await expectGateAnswered();
    await expect(asAdmin.calls.filter((c) => c.url.includes("include_unassigned"))).toHaveLength(0);
    await asAdmin.expectSent("GET", "/orgs/org-1/users");

    const listbox = await openOptions(canvas.getByRole("combobox", { name: "Filter by actor" }));
    await expect(within(listbox).getByRole("option", { name: "ada@example.com" })).toBeVisible();
    await expect(within(listbox).queryByRole("option", { name: "root@example.com" })).toBeNull();
  },
};
