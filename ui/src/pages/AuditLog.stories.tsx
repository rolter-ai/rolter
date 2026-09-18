import type { Meta, StoryObj } from "@storybook/react";
import { MemoryRouter } from "react-router";
import { expect, waitFor, within } from "storybook/test";

import AuditLog from "./AuditLog";
import {
  Harness,
  expectEmptyState,
  expectLoadError,
  expectSkeleton,
  json,
  pending,
  routes,
  scoped,
  type FetchStub,
} from "./story-harness";
import type { AuditLogEntry } from "@/lib/api";

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
  },
};

// the table header is replaced by a shaped skeleton rather than left standing
// over nothing, which is what made a slow page read as an empty one
export const Loading: Story = {
  render: () => <Screen fetchStub={pending} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
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

export const Error_: Story = {
  name: "Error",
  render: () => (
    <Screen fetchStub={scoped(async () => json({ error: { message: "boom" } }, 500))} />
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return the audit log/i);
  },
};

export const Forbidden: Story = {
  render: () => (
    <Screen fetchStub={scoped(async () => json({ error: { message: "forbidden" } }, 403))} />
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /You do not have access to the audit log/);
  },
};
