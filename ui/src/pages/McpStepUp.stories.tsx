import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import McpLogs from "./McpLogs";
import { McpCatalog } from "./McpManagement";
import {
  AUTHORIZE_STARTED,
  CONNECTABLE,
  Harness,
  Toasts,
  routed,
  type FetchStub,
} from "./mcp-story-fixtures";
import { json } from "./story-harness";
import type { McpLogDetail, McpOAuthGrantRow, McpOAuthSessionRow, McpServerRow } from "@/lib/api";
import { AuthProvider } from "@/lib/auth";

// a step-up (#3296): the gateway answers `mcp_insufficient_scope`, the log row
// says `auth_denied` and, when the upstream named them, the scopes it asked for
// (`error_scopes`, #3375), and the owner reconnects with more access. a row
// without them offers the scopes the server declares that the live session lacks

const ME = { id: "user-ada", email: "ada@acme.dev", is_superadmin: false };

const LINEAR: McpServerRow = {
  ...CONNECTABLE,
  required_scopes: ["read", "write"],
  default_scopes: ["read"],
};

const GRANT: McpOAuthGrantRow = {
  id: "grant-linear",
  server_id: LINEAR.id,
  user_id: ME.id,
  scopes: ["read"],
  granted_at: "2026-09-30T10:00:00Z",
  revoked_at: null,
  revoked_by: null,
  active: true,
};

const session = (over: Partial<McpOAuthSessionRow> = {}): McpOAuthSessionRow => ({
  id: "sess-linear",
  grant_id: GRANT.id,
  scopes: ["read"],
  expires_at: new Date(Date.now() + 3_600_000).toISOString(),
  refresh_expires_at: null,
  revoked_at: null,
  created_at: "2026-09-30T10:00:00Z",
  last_used_at: null,
  has_refresh_token: true,
  ...over,
});

// signed in the way a login leaves the browser, minus the token
function SignedIn({ children }: { children: React.ReactNode }) {
  React.useState(() => {
    localStorage.setItem("rolter.session.email", ME.email);
    localStorage.setItem("rolter.session.user", JSON.stringify(ME));
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

function record(handler: FetchStub) {
  const calls: { method: string; url: string; body?: string }[] = [];
  const stub: FetchStub = async (input, init) => {
    calls.push({
      method: (init?.method ?? "GET").toUpperCase(),
      url: typeof input === "string" ? input : String(input),
      body: typeof init?.body === "string" ? init.body : undefined,
    });
    return handler(input, init);
  };
  const authorize = () => calls.find((c) => c.url.includes("/oauth/authorize"));
  return { stub, calls, authorize };
}

/** stand a pop-up in, because a real new tab would take the test runner with it */
async function withPopup(run: (opened: string[]) => Promise<void>): Promise<void> {
  const opened: string[] = [];
  const original = window.open;
  window.open = ((url?: string | URL) => {
    opened.push(String(url));
    return {} as Window;
  }) as typeof window.open;
  try {
    await run(opened);
  } finally {
    window.open = original;
  }
}

const catalog = (sessions: McpOAuthSessionRow[]) =>
  routed({
    servers: async () => json([LINEAR]),
    grants: async () => json([GRANT]),
    sessions: async () => json(sessions),
  });

const meta = {
  title: "Screens/McpStepUp",
  component: McpCatalog,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof McpCatalog>;

export default meta;
type Story = StoryObj<typeof meta>;

const card = async (canvas: ReturnType<typeof within>, name: string) => {
  const article = (await canvas.findByRole("heading", { name })).closest("article");
  if (!article) throw new Error(`no card for ${name}`);
  return within(article);
};

const steps = record(catalog([session()]));

/** The card names the scope the session lacks and asks for exactly that one. */
export const CardOffersAStepUpForTheMissingScope: Story = {
  render: () => (
    <SignedIn>
      <Harness fetchStub={steps.stub}>
        <Toasts>
          <McpCatalog />
        </Toasts>
      </Harness>
    </SignedIn>
  ),
  play: async ({ canvasElement }) => {
    const linear = await card(within(canvasElement), "Linear");
    await waitFor(() =>
      expect(
        linear.getByText("Your connection lacks the scope write that this server requires."),
      ).toBeVisible(),
    );
    await withPopup(async (opened) => {
      await userEvent.click(
        linear.getByRole("button", { name: "Reconnect Linear with more access" }),
      );
      await waitFor(() => expect(steps.authorize()).toBeDefined());
      await expect(JSON.parse(steps.authorize()?.body ?? "{}")).toEqual({
        step_up_scopes: ["write"],
      });
      await waitFor(() => expect(opened).toEqual([AUTHORIZE_STARTED.authorization_url]));
    });
  },
};

/** Several missing scopes are all asked for in one consent, in the server's order. */
export const CardListsEveryMissingScope: Story = {
  render: () => (
    <SignedIn>
      <Harness
        fetchStub={routed({
          servers: async () => json([{ ...LINEAR, required_scopes: ["read", "write", "admin"] }]),
          grants: async () => json([GRANT]),
          sessions: async () => json([session()]),
        })}
      >
        <McpCatalog />
      </Harness>
    </SignedIn>
  ),
  play: async ({ canvasElement }) => {
    const linear = await card(within(canvasElement), "Linear");
    await waitFor(() =>
      expect(
        linear.getByText(
          "Your connection lacks the scopes write, admin that this server requires.",
        ),
      ).toBeVisible(),
    );
  },
};

/** A session that covers what the server requires has nothing to step up to. */
export const CoveredSessionOffersNoStepUp: Story = {
  render: () => (
    <SignedIn>
      <Harness fetchStub={catalog([session({ scopes: ["read", "write"] })])}>
        <McpCatalog />
      </Harness>
    </SignedIn>
  ),
  play: async ({ canvasElement }) => {
    const linear = await card(within(canvasElement), "Linear");
    await waitFor(() => expect(linear.getByText(/Connected until/)).toBeVisible());
    await expect(linear.queryByRole("button", { name: /with more access/ })).toBeNull();
  },
};

/** An expired session is Reconnect's business, not a step-up's. */
export const ExpiredSessionOffersNoStepUp: Story = {
  render: () => (
    <SignedIn>
      <Harness
        fetchStub={catalog([
          session({ expires_at: new Date(Date.now() - 3_600_000).toISOString() }),
        ])}
      >
        <McpCatalog />
      </Harness>
    </SignedIn>
  ),
  play: async ({ canvasElement }) => {
    const linear = await card(within(canvasElement), "Linear");
    await waitFor(() => expect(linear.getByText(/expired/i)).toBeVisible());
    await expect(linear.queryByRole("button", { name: /with more access/ })).toBeNull();
  },
};

/** The control plane refuses the authorize call: the card says so in a toast, not silently. */
export const StepUpRefusedByTheControlPlane: Story = {
  render: () => (
    <SignedIn>
      <Harness
        fetchStub={routed({
          servers: async () => json([LINEAR]),
          grants: async () => json([GRANT]),
          sessions: async () => json([session()]),
          authorize: async () =>
            json(
              { error: { message: "step_up_scopes must be scope tokens", code: "invalid_field" } },
              400,
            ),
        })}
      >
        <Toasts>
          <McpCatalog />
        </Toasts>
      </Harness>
    </SignedIn>
  ),
  play: async ({ canvasElement }) => {
    const linear = await card(within(canvasElement), "Linear");
    await userEvent.click(
      await linear.findByRole("button", { name: "Reconnect Linear with more access" }),
    );
    await waitFor(() =>
      expect(within(canvasElement).getByText(/Could not start consent for Linear/)).toBeVisible(),
    );
  },
};

// ---------------------------------------------------------------- MCP Logs

const denied = (over: Partial<McpLogDetail> = {}): McpLogDetail => ({
  ts: "2026-08-06T10:00:00Z",
  event_id: "evt-denied",
  server: LINEAR.slug,
  tool: "create_issue",
  transport: "streamable_http",
  status: "auth_denied",
  latency_ms: 12,
  org_id: "org-1",
  team_id: "team-1",
  project_id: "project-1",
  virtual_key_id: "vk-1",
  user_id: ME.id,
  request_id: "req-1",
  trace_id: "trace-1",
  error: null,
  arguments: null,
  result: null,
  ...over,
});

function logs(detail: McpLogDetail, sessions: McpOAuthSessionRow[] = [session()]): FetchStub {
  const catalogStub = catalog(sessions);
  return async (input, init) => {
    const url = typeof input === "string" ? input : String(input);
    if (url.includes("/mcp/logs/summary")) {
      return json({ data: [{ calls: 1, failures: 1, avg_latency_ms: 12, p95_latency_ms: 12 }] });
    }
    if (url.includes(`/mcp/logs/${detail.event_id}`)) return json(detail);
    if (url.includes("/mcp/logs")) return json({ data: [detail], next_cursor: null });
    return catalogStub(input, init);
  };
}

const openDetail = async (canvasElement: HTMLElement) => {
  await userEvent.click(
    await within(canvasElement).findByRole("button", { name: /open details|details/i }),
  );
  return within(document.body);
};

const logSteps = record(logs(denied()));

export const LogDetailOffersTheMissingScope: Story = {
  render: () => (
    <SignedIn>
      <Harness fetchStub={logSteps.stub}>
        <Toasts>
          <McpLogs />
        </Toasts>
      </Harness>
    </SignedIn>
  ),
  play: async ({ canvasElement }) => {
    const body = await openDetail(canvasElement);
    await waitFor(() =>
      expect(body.getByRole("heading", { name: "Authorization refused" })).toBeVisible(),
    );
    await waitFor(() =>
      expect(
        body.getByText("Your connection lacks the scope write that this server requires."),
      ).toBeVisible(),
    );
    await withPopup(async () => {
      await userEvent.click(
        body.getByRole("button", { name: "Reconnect Linear with more access" }),
      );
      await waitFor(() => expect(logSteps.authorize()).toBeDefined());
      await expect(JSON.parse(logSteps.authorize()?.body ?? "{}")).toEqual({
        step_up_scopes: ["write"],
      });
    });
  },
};

const typedSteps = record(logs(denied(), [session({ scopes: ["read", "write"] })]));

/** The session holds everything declared: the owner names the scope the upstream wanted. */
export const LogDetailAsksTheOwnerToNameTheScope: Story = {
  render: () => (
    <SignedIn>
      <Harness fetchStub={typedSteps.stub}>
        <Toasts>
          <McpLogs />
        </Toasts>
      </Harness>
    </SignedIn>
  ),
  play: async ({ canvasElement }) => {
    const body = await openDetail(canvasElement);
    const field = await body.findByLabelText("Scopes to add");
    const action = body.getByRole("button", { name: "Reconnect Linear with more access" });
    // nothing to ask for until a scope is named
    await expect(action).toBeDisabled();
    await userEvent.type(field, "repo, read:org");
    await waitFor(() => expect(action).toBeEnabled());
    await withPopup(async () => {
      await userEvent.click(action);
      await waitFor(() => expect(typedSteps.authorize()).toBeDefined());
      await expect(JSON.parse(typedSteps.authorize()?.body ?? "{}")).toEqual({
        step_up_scopes: ["repo", "read:org"],
      });
    });
  },
};

const askedSteps = record(
  logs(denied({ error: "insufficient scope", error_scopes: ["write", "repo", "read:org"] }), [
    session({ scopes: ["read", "write"] }),
  ]),
);

/** #3375: the refusal recorded what the upstream asked for, so nothing is typed. */
export const LogDetailOffersTheScopesTheUpstreamAskedFor: Story = {
  render: () => (
    <SignedIn>
      <Harness fetchStub={askedSteps.stub}>
        <Toasts>
          <McpLogs />
        </Toasts>
      </Harness>
    </SignedIn>
  ),
  play: async ({ canvasElement }) => {
    const body = await openDetail(canvasElement);
    await waitFor(() =>
      expect(
        body.getByText("write, repo, read:org are the scopes this server asked for."),
      ).toBeVisible(),
    );
    await expect(body.queryByLabelText("Scopes to add")).toBeNull();
    const action = body.getByRole("button", { name: "Reconnect Linear with more access" });
    await waitFor(() => expect(action).toBeEnabled());
    await withPopup(async () => {
      await userEvent.click(action);
      await waitFor(() => expect(askedSteps.authorize()).toBeDefined());
      // `write` is already held, so only what the session lacks is asked for
      await expect(JSON.parse(askedSteps.authorize()?.body ?? "{}")).toEqual({
        step_up_scopes: ["repo", "read:org"],
      });
    });
  },
};

/** Everything the upstream asked for is already held: the button says why it is off. */
export const LogDetailWithEveryAskedScopeHeldExplainsWhy: Story = {
  render: () => (
    <SignedIn>
      <Harness
        fetchStub={logs(denied({ error_scopes: ["read"] }), [
          session({ scopes: ["read", "write"] }),
        ])}
      >
        <McpLogs />
      </Harness>
    </SignedIn>
  ),
  play: async ({ canvasElement }) => {
    const body = await openDetail(canvasElement);
    await waitFor(() => expect(body.getByText(/already holds these scopes/)).toBeVisible());
    const action = body.getByRole("button", { name: "Reconnect Linear with more access" });
    await expect(action).toBeDisabled();
    await expect(action).toHaveAccessibleDescription(/already holds these scopes/);
  },
};

export const LogDetailOfAnotherUsersCallOffersNoAction: Story = {
  render: () => (
    <SignedIn>
      <Harness fetchStub={logs(denied({ user_id: "user-bo" }))}>
        <McpLogs />
      </Harness>
    </SignedIn>
  ),
  play: async ({ canvasElement }) => {
    const body = await openDetail(canvasElement);
    await waitFor(() =>
      expect(body.getByText("Only the key's owner can reconnect with more access.")).toBeVisible(),
    );
    await expect(body.queryByRole("button", { name: /with more access/ })).toBeNull();
  },
};

export const LogDetailOfASuccessfulCallHasNoStepUp: Story = {
  render: () => (
    <SignedIn>
      <Harness fetchStub={logs(denied({ status: "success" }))}>
        <McpLogs />
      </Harness>
    </SignedIn>
  ),
  play: async ({ canvasElement }) => {
    const body = await openDetail(canvasElement);
    await waitFor(() =>
      expect(body.getByText("create_issue", { selector: "span,p,dd,div" })).toBeVisible(),
    );
    await expect(body.queryByRole("heading", { name: "Authorization refused" })).toBeNull();
  },
};
