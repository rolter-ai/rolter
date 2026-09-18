import type { Meta, StoryObj } from "@storybook/react";
import { expect, waitFor, within } from "storybook/test";

import { useAuth } from "@/lib/auth";
import { Harness, StaleSession, json } from "@/pages/story-harness";

// The boot check has three outcomes and only one of them may sign the operator
// out (#1196). There is no DOM test environment under `bun test` — the unit
// suite is pure — so the provider's own behaviour is asserted here, where a
// real browser runs it: a probe renders what `useAuth()` reports, and each
// story stubs a different answer from `/api/v1/auth/me`.
function SessionProbe() {
  const { email, status, user, memberships } = useAuth();
  return (
    <dl className="grid grid-cols-[10rem_1fr] gap-1 font-mono text-sm">
      <dt>status</dt>
      <dd data-testid="status">{status}</dd>
      <dt>signed in as</dt>
      <dd data-testid="email">{email ?? "—"}</dd>
      <dt>superadmin</dt>
      <dd data-testid="superadmin">{String(user?.is_superadmin ?? "unknown")}</dd>
      <dt>member of</dt>
      <dd data-testid="orgs">{memberships.map((m) => m.org_id).join(", ") || "—"}</dd>
    </dl>
  );
}

const meta = {
  title: "Session/Revalidation",
  component: SessionProbe,
  parameters: { layout: "padded" },
} satisfies Meta<typeof SessionProbe>;

export default meta;
type Story = StoryObj<typeof meta>;

const ME = {
  user: {
    id: "user-1",
    email: "anya@acme.co",
    is_superadmin: true,
    deactivated_at: null,
    created_at: "2026-01-01T00:00:00Z",
  },
  memberships: [
    {
      id: "m-1",
      user_id: "user-1",
      org_id: "org-7",
      team_id: null,
      project_id: null,
      role: "admin",
      source: "manual",
      created_at: "2026-01-01T00:00:00Z",
    },
  ],
};

/** `/auth/me` answers: the token is live, and the account comes from the server. */
export const TokenAccepted: Story = {
  render: () => (
    <Harness fetchStub={async () => json(ME)}>
      <StaleSession>
        <SessionProbe />
      </StaleSession>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the check is one request: waited out rather than sampled, since the probe
    // renders "checking" first and `findBy*` only waits for the element
    await waitFor(() => expect(canvas.getByTestId("status")).toHaveTextContent("ready"));
    await expect(canvas.getByTestId("email")).toHaveTextContent("anya@acme.co");
    // superadmin is the server's answer, not the cached login blob
    await expect(canvas.getByTestId("superadmin")).toHaveTextContent("true");
    // and the memberships are what seeds the scope switcher
    await expect(canvas.getByTestId("orgs")).toHaveTextContent("org-7");
  },
};

/** The token expired while the tab was closed: sign out, do not keep pretending. */
export const TokenRejected: Story = {
  render: () => (
    <Harness
      fetchStub={async () =>
        json({ error: { message: "missing or invalid session", code: "unauthenticated" } }, 401)
      }
    >
      <StaleSession>
        <SessionProbe />
      </StaleSession>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByTestId("email")).toHaveTextContent("—"));
    await expect(canvas.getByTestId("status")).toHaveTextContent("ready");
  },
};

/**
 * The control plane blinked. A 5xx says nothing about the token, so signing
 * the operator out over it would be a self-inflicted outage.
 */
export const ControlPlaneBlinked: Story = {
  render: () => (
    <Harness fetchStub={async () => json({ error: { message: "boom" } }, 503)}>
      <StaleSession>
        <SessionProbe />
      </StaleSession>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByTestId("status")).toHaveTextContent("ready"));
    await expect(canvas.getByTestId("email")).toHaveTextContent("anya@acme.co");
    // kept, but not verified: nothing was learned about the account
    await expect(canvas.getByTestId("superadmin")).toHaveTextContent("unknown");
  },
};

/**
 * Open mode with no store never mounts `/api/v1/auth/*` at all, so the check
 * 404s. That is a deployment shape, not a rejected session.
 */
export const AuthEndpointsNotMounted: Story = {
  render: () => (
    <Harness fetchStub={async () => new Response("Not Found", { status: 404 })}>
      <StaleSession>
        <SessionProbe />
      </StaleSession>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByTestId("status")).toHaveTextContent("ready"));
    await expect(canvas.getByTestId("email")).toHaveTextContent("anya@acme.co");
  },
};
