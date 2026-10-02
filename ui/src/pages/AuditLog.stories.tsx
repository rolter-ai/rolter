import type { Meta, StoryObj } from "@storybook/react";
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
  pending,
  recording,
  routes,
  scoped,
  withCapabilities,
  type FetchStub,
} from "./story-harness";
import type { AuditLogEntry } from "@/lib/api";
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
    await expect(canvas.queryByRole("button", { name: /Clear search/i })).not.toBeInTheDocument();
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
