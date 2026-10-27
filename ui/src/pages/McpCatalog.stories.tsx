import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { McpCatalog } from "./McpManagement";
import {
  AUTHORIZE_STARTED,
  OAUTH,
  UNDISCOVERED,
  BEARER,
  BY_KIND,
  CONNECTABLE,
  DISCOVERED_OK,
  HEADER,
  NOT_PROBED,
  Harness,
  KEK_REFUSAL,
  SERVERS,
  Toasts,
  routed,
  urlOf,
  type FetchStub,
} from "./mcp-story-fixtures";

import {
  cancelConfirmation,
  confirmDestructive,
  expectEmptyState,
  expectLoadError,
  expectNoUxEvent,
  expectRefused,
  expectSheetClosed,
  expectSkeleton,
  expectUxEvent,
  Harness as GatedHarness,
  json,
  NEEDS_ADMIN,
  recordUxEvents,
  recording,
  uxEvents,
  type Recorder,
} from "./story-harness";
import { Toaster } from "@/components/ui/toaster";
import type {
  McpConnectionTest,
  McpOAuthGrantRow,
  McpOAuthSessionRow,
  McpServerRow,
  McpServerToolsResponse,
  McpToolCallResult,
} from "@/lib/api";
import { AuthProvider } from "@/lib/auth";
import { CONSENT_CHANNEL, type ConsentAnnouncement } from "@/lib/mcp-consent";
import { ToastProvider } from "@/lib/toast";
import { UxScreenProvider } from "@/lib/ux-react";
import {
  animationsSettled,
  atMobile,
  expectNoHorizontalOverflow,
  phoneFits,
} from "@/lib/story-viewport";

const meta = {
  title: "Screens/McpCatalog",
  component: McpCatalog,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof McpCatalog>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={routed()}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("GitHub")).toBeVisible());
    await expect(canvas.getByText("3 declared tools")).toBeVisible();
    await expect(canvas.getByRole("switch", { name: "Enable Sentry" })).not.toBeChecked();
    // the shell's ScreenHeader owns the page's only h1 (#2241)
    await expect(canvas.queryAllByRole("heading", { level: 1 })).toHaveLength(0);
  },
};
export const Loading: Story = {
  render: () => (
    <Harness fetchStub={routed({ servers: () => new Promise<Response>(() => {}) })}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => expectSkeleton(canvasElement),
};
export const Empty: Story = {
  render: () => (
    <Harness fetchStub={routed({ servers: async () => json([]) })}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) =>
    expectEmptyState(canvasElement, /No MCP servers registered/, /Register server/),
};
// a 5xx is worth retrying, and the screen offers it
export const Error: Story = {
  render: () => (
    <Harness
      fetchStub={routed({
        servers: async () => json({ error: { message: "registry offline" } }, 503),
      })}
    >
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return MCP servers/i);
    await expect(
      await within(canvasElement).findByRole("button", { name: /try again/i }),
    ).toBeVisible();
  },
};

// fetch never connected: there is no status to read, and the retry stays
export const Unreachable: Story = {
  render: () => (
    <Harness
      fetchStub={routed({
        servers: () => Promise.reject(new TypeError("Failed to fetch")),
      })}
    >
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /cannot reach the control plane/i);
    await expect(
      await within(canvasElement).findByRole("button", { name: /try again/i }),
    ).toBeVisible();
  },
};

// no organization to read a registry from: the screen says so instead of
// sitting on a skeleton or an empty list
export const NoOrg: Story = {
  render: () => (
    <Harness
      fetchStub={async (input, init) =>
        /\/orgs(\?|$)/.test(urlOf(input)) ? json([]) : routed()(input, init)
      }
    >
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Choose an organization")).toBeVisible());
    await expect(canvas.queryByRole("button", { name: "Register server" })).toBeNull();
  },
};
export const Forbidden: Story = {
  render: () => (
    <Harness
      fetchStub={routed({ servers: async () => json({ error: { message: "forbidden" } }, 403) })}
    >
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => expectLoadError(canvasElement, /You do not have access to/),
};
export const ValidatesEndpoint: Story = {
  render: () => (
    <Harness fetchStub={routed()}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Register server" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.type(dialog.getByLabelText("Name"), "Local tools");
    await userEvent.type(dialog.getByLabelText("Endpoint URL"), "ftp://invalid");
    await expect(dialog.getByRole("button", { name: "Register server" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  },
};
/**
 * Deleting a server confirms through the shared `ConfirmDialog` (#1760): the
 * title names the server and the body says what goes with it, a cancel sends
 * nothing and is an abandon, and a confirm sends the DELETE and lands as
 * `save_confirmed`.
 */
let serverDeletes: Recorder;
export const ExplainsDeleteCascade: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    serverDeletes = recording(
      routed({
        servers: async (_input, init) =>
          init?.method === "DELETE" ? json(null, 204) : json(SERVERS),
      }),
    );
    return (
      <Harness fetchStub={serverDeletes.stub}>
        <Toasts>
          <UxScreenProvider screen="mcp-catalog">
            <McpCatalog />
          </UxScreenProvider>
        </Toasts>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const remove = await canvas.findByRole("button", { name: "Delete server GitHub" });
    await userEvent.click(remove);
    await expect(
      await within(document.body).findByRole("heading", { name: "Delete server GitHub?" }),
    ).toBeVisible();
    await cancelConfirmation();
    serverDeletes.expectNotSent("DELETE", "/mcp-servers/");
    const abandon = await expectUxEvent("form_abandon", "mcp-server-delete");
    await expect(abandon.screen).toBe("mcp-catalog");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "mcp-server-delete");

    await userEvent.click(remove);
    await confirmDestructive(/removes every OAuth grant and token session/, "Delete server");
    await serverDeletes.expectSent("DELETE", "/mcp-servers/server-github");
    await expectSheetClosed();
    const submit = await expectUxEvent("form_submit", "mcp-server-delete");
    await expect(submit.outcome).toBe("ok");
    await expectUxEvent("save_confirmed", "mcp-server-delete");
    await waitFor(() => expect(canvas.getByText("GitHub deleted")).toBeVisible());
  },
};

/**
 * A refused delete keeps the dialog open over the control plane's reason, and
 * the refusal is a row of its own beside the press even though the stub
 * answers in the same tick (#1761).
 */
export const DeleteRefused: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness
      fetchStub={routed({
        servers: async (_input, init) =>
          init?.method === "DELETE"
            ? json({ error: { message: "GitHub is still granted to 2 tool groups" } }, 409)
            : json(SERVERS),
      })}
    >
      <Toasts>
        <UxScreenProvider screen="mcp-catalog">
          <McpCatalog />
        </UxScreenProvider>
      </Toasts>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Delete server GitHub" }));
    await confirmDestructive(/GitHub/, "Delete server");
    await waitFor(() => expect(canvas.getByText("Could not delete GitHub")).toBeVisible());
    await waitFor(() =>
      expect(
        uxEvents()
          .filter((e) => e.action === "form_submit" && e.target === "mcp-server-delete")
          .map((e) => e.outcome),
      ).toEqual(["ok", "error"]),
    );
    expectNoUxEvent("save_confirmed", "mcp-server-delete");
    const dialog = within(await within(document.body).findByRole("dialog"));
    await expect(await dialog.findByRole("alert")).toHaveTextContent("still granted to 2 tool");
  },
};

// the shared `recording` helper keeps method and url. the client save has to be
// asserted on its *body* too: a PUT that silently dropped the scopes, or that
// sent an empty `client_secret` and so cleared a secret nobody was rotating,
// passes a url-only check (#1194)
function bodyRecording(handler: FetchStub) {
  const calls: { method: string; url: string; body: unknown }[] = [];
  return {
    calls,
    stub: (async (input, init) => {
      calls.push({
        method: (init?.method ?? "GET").toUpperCase(),
        url: urlOf(input),
        body: init?.body ? JSON.parse(String(init.body)) : undefined,
      });
      return handler(input, init);
    }) as FetchStub,
    sent: async (method: string, fragment: string) => {
      let found: { method: string; url: string; body: unknown } | undefined;
      await waitFor(() => {
        found = calls.find((call) => call.method === method && call.url.includes(fragment));
        expect(found).toBeDefined();
      });
      return found?.body;
    },
  };
}

const clientSaves = bodyRecording(
  routed({
    servers: async (_input, init) => (init?.method === "PATCH" ? json(SERVERS[0]) : json(SERVERS)),
  }),
);
export const RegistersOAuthClient: Story = {
  render: () => (
    <Harness fetchStub={clientSaves.stub}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Configure server GitHub" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    // the client fields are the `oauth` branch of the auth picker now, not a
    // section every server shows (#1447)
    await expect(dialog.queryByLabelText("Client ID")).not.toBeInTheDocument();
    await userEvent.click(await dialog.findByRole("radio", { name: /^OAuth consent/ }));
    // the redirect uri is deployment-derived, so it is read back rather than
    // guessed from the browser's origin
    await expect(dialog.getByLabelText("Redirect URI")).toHaveValue(
      "https://control.example.com/auth/mcp/callback",
    );
    await userEvent.type(
      dialog.getByLabelText("Authorization URL"),
      "https://github.com/login/oauth/authorize",
    );
    await userEvent.type(
      dialog.getByLabelText("Token URL"),
      "https://github.com/login/oauth/access_token",
    );
    await userEvent.type(dialog.getByLabelText("Client ID"), "Iv1.abc123");
    await userEvent.type(dialog.getByLabelText("Client secret"), "s3cr3t");
    await userEvent.type(dialog.getByLabelText("Default scopes"), "repo, read:org");
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    await expect(await clientSaves.sent("PUT", "/mcp-servers/server-github/oauth-client")).toEqual({
      authorize_url: "https://github.com/login/oauth/authorize",
      token_url: "https://github.com/login/oauth/access_token",
      client_id: "Iv1.abc123",
      default_scopes: ["repo", "read:org"],
      client_secret: "s3cr3t",
      // the mode and issuer always travel, since an omitted mode reads as auto
      discovery: "auto",
      issuer: null,
    });
    // a client alone does not make the gateway use it: the kind has to say oauth
    await expect(await clientSaves.sent("PUT", "/mcp-servers/server-github/auth")).toEqual({
      auth_kind: "oauth",
    });
  },
};

// since #1347 a client id alone is a client — discovery supplies the rest — but
// the typed endpoints still travel as a pair, and must be https before the
// control plane will take them (#1415)
export const RefusesAHalfClient: Story = {
  render: () => (
    <Harness fetchStub={routed()}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Configure server Sentry" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.click(await dialog.findByRole("radio", { name: /^OAuth consent/ }));
    await userEvent.type(dialog.getByLabelText("Client ID"), "Iv1.abc123");
    await expect(dialog.getByRole("button", { name: "Save server" })).toBeEnabled();
    await userEvent.type(
      dialog.getByLabelText("Authorization URL"),
      "http://sentry.example.com/authorize",
    );
    await expect(
      dialog.getAllByText("Must be an https URL (http is accepted only on loopback).").length,
    ).toBeGreaterThan(0);
    await expect(dialog.getByRole("button", { name: "Save server" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await userEvent.clear(dialog.getByLabelText("Authorization URL"));
    await userEvent.type(
      dialog.getByLabelText("Authorization URL"),
      "https://sentry.io/oauth/authorize",
    );
    await expect(dialog.getByRole("alert")).toHaveTextContent(
      /Enter both the authorization URL and the token URL/,
    );
    await expect(dialog.getByRole("button", { name: "Save server" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  },
};

const connects = bodyRecording(routed({ servers: async () => json([CONNECTABLE]) }));
export const ConnectStartsConsent: Story = {
  render: () => (
    <Harness fetchStub={connects.stub}>
      <ToastProvider>
        <McpCatalog />
        <Toaster />
      </ToastProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // stubbed because a real new tab would take the test runner with it, and
    // the url the dashboard opens is the assertion that matters
    const opened: string[] = [];
    const original = window.open;
    window.open = ((url?: string | URL) => {
      opened.push(String(url));
      return {} as Window;
    }) as typeof window.open;
    try {
      await userEvent.click(
        await canvas.findByRole("button", { name: "Start the consent flow for Linear" }),
      );
      await connects.sent("POST", "/mcp-servers/server-linear/oauth/authorize");
      await waitFor(() => expect(opened).toEqual([AUTHORIZE_STARTED.authorization_url]));
      // the toast fades in, so it is momentarily transparent: waitFor rather
      // than a bare assertion, which would read opacity 0 on the first frame
      await waitFor(() => expect(canvas.getByText("Consent started for Linear")).toBeVisible());
      await expect(canvas.getByText(/this card shows the new session/)).toBeVisible();
    } finally {
      window.open = original;
    }
  },
};

// the consent finishes in another tab: the one the authorization server sends
// the user back to, which lands on Auth Sessions and announces the new session
// (#2166). the card that started it has to say so without a reload, and only
// for the account that consented
const ME = { id: "user-ada", email: "ada@acme.dev", is_superadmin: false };
const LINEAR_GRANT: McpOAuthGrantRow = {
  id: "grant-linear",
  server_id: CONNECTABLE.id,
  user_id: ME.id,
  scopes: ["read"],
  granted_at: "2026-09-30T10:00:00Z",
  revoked_at: null,
  revoked_by: null,
  active: true,
};
const LINEAR_SESSION: McpOAuthSessionRow = {
  id: "sess-linear",
  grant_id: LINEAR_GRANT.id,
  scopes: ["read"],
  expires_at: new Date(Date.now() + 3_600_000).toISOString(),
  refresh_expires_at: null,
  revoked_at: null,
  created_at: "2026-09-30T10:00:00Z",
  last_used_at: null,
  has_refresh_token: true,
};
// signed in the way a login leaves the browser, minus the token, so the
// provider knows the account without asking /auth/me for it
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
let consented = false;
const consents = routed({
  servers: async () => json([CONNECTABLE, SERVERS[0]]),
  // a colleague's consent to GitHub, which an admin's listing carries too:
  // it must not mark the card for this account
  grants: async () =>
    json([
      { ...LINEAR_GRANT, id: "grant-bo", server_id: SERVERS[0].id, user_id: "user-bo" },
      ...(consented ? [LINEAR_GRANT] : []),
    ]),
  sessions: async () =>
    json([
      { ...LINEAR_SESSION, id: "sess-bo", grant_id: "grant-bo" },
      ...(consented ? [LINEAR_SESSION] : []),
    ]),
});
export const ReflectsANewSession: Story = {
  render: () => {
    consented = false;
    return (
      <SignedIn>
        <Harness fetchStub={consents}>
          <McpCatalog />
        </Harness>
      </SignedIn>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const card = (name: string) => {
      const article = canvas.getByRole("heading", { name }).closest("article");
      if (!article) throw new globalThis.Error(`no card for ${name}`);
      return within(article);
    };
    await canvas.findByRole("heading", { name: "Linear" });
    await waitFor(() => expect(card("Linear").getByText(/^Not connected/)).toBeVisible());

    // what Auth Sessions posts in the other tab once the consent lands
    consented = true;
    const other = new BroadcastChannel(CONSENT_CHANNEL);
    const message: ConsentAnnouncement = {
      kind: "mcp-consent-completed",
      session: LINEAR_SESSION.id,
      server: CONNECTABLE.id,
    };
    other.postMessage(message);
    other.close();

    await waitFor(() => expect(card("Linear").getByText(/^Connected until/)).toBeVisible());
    // GitHub takes no OAuth consent, so there is no session to speak of; a
    // colleague's grant on it changes nothing
    await expect(card("GitHub").queryByText(/Connected|Not connected/)).toBeNull();
    await expect(
      card("GitHub").queryByRole("button", { name: /consent flow|Reconnect/ }),
    ).toBeNull();
  },
};

// Connect belongs to oauth servers only: on none, bearer and header it would
// advise registering a client the server never uses (#2167)
export const ConnectOnlyOnOAuthServers: Story = {
  render: () => (
    <Harness fetchStub={byKind()}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole("heading", { name: "Linear" });
    await expect(canvas.getAllByRole("button", { name: /consent flow|Reconnect/ })).toHaveLength(1);
    await expect(
      canvas.getByRole("button", { name: "Start the consent flow for Linear" }),
    ).toBeEnabled();
    await expect(canvas.queryByText(/Register an OAuth client/)).toBeNull();
  },
};

// an oauth server with no client registered: the badge is not green, and the
// reason Connect is disabled is text on the card that the button points at
export const ConnectNeedsAClient: Story = {
  render: () => (
    <Harness
      fetchStub={routed({ servers: async () => json([{ ...SERVERS[0], auth_kind: "oauth" }]) })}
    >
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const connect = await canvas.findByRole("button", {
      name: "Start the consent flow for GitHub",
    });
    await expect(connect).toBeDisabled();
    await expect(connect).not.toHaveAttribute("title");
    const reason = canvas.getByText("Register an OAuth client on this server before connecting.");
    await expect(reason).toBeVisible();
    await expect(connect).toHaveAccessibleDescription(reason.textContent ?? "");
    await expect(canvas.getByText("oauth, no client")).toBeVisible();
    await expect(canvas.queryByText(/^oauth$/)).toBeNull();
  },
};

// the account's own session decides the card: an expired one offers Reconnect
// and says since when, rather than Connect beside nothing
export const ConnectOffersReconnectWhenExpired: Story = {
  render: () => (
    <SignedIn>
      <Harness
        fetchStub={routed({
          servers: async () => json([CONNECTABLE]),
          grants: async () => json([LINEAR_GRANT]),
          sessions: async () =>
            json([
              { ...LINEAR_SESSION, expires_at: new Date(Date.now() - 86_400_000).toISOString() },
            ]),
        })}
      >
        <McpCatalog />
      </Harness>
    </SignedIn>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/^Session expired/)).toBeVisible();
    await expect(canvas.getByRole("button", { name: "Reconnect to Linear" })).toBeEnabled();
    await expect(canvas.getByText("oauth")).toBeVisible();
  },
};

// the listings are best-effort: when the caller may not read sessions the card
// says nothing about them instead of claiming "not connected"
export const ConnectStatusHiddenWhenSessionsUnreadable: Story = {
  render: () => (
    <SignedIn>
      <Harness
        fetchStub={routed({
          servers: async () => json([CONNECTABLE]),
          sessions: async () => json({ error: { message: "forbidden" } }, 403),
        })}
      >
        <McpCatalog />
      </Harness>
    </SignedIn>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByRole("button", { name: "Start the consent flow for Linear" }),
    ).toBeEnabled();
    await expect(canvas.queryByText(/Not connected|Connected until|Session expired/)).toBeNull();
  },
};

// ---------------------------------------------------------------------------
// the enable switch and the save outcomes (#2168)

// a refused disable is announced, the switch goes back where the server left
// it, and nothing else on the screen locks
export const ToggleRefusedIsAnnounced: Story = {
  render: () => (
    <Harness
      fetchStub={routed({
        servers: async (_input, init) =>
          init?.method === "PATCH"
            ? json({ error: { message: "snapshot rejected the change" } }, 409)
            : json([{ ...SERVERS[0] }, { ...SERVERS[1], enabled: true }]),
      })}
    >
      <Toasts>
        <McpCatalog />
      </Toasts>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const github = await canvas.findByRole("switch", { name: "Enable GitHub" });
    await userEvent.click(github);
    await waitFor(() => expect(canvas.getByText("Could not save GitHub")).toBeVisible());
    await expect(canvas.getByText(/snapshot rejected the change/)).toBeVisible();
    await waitFor(() => expect(github).toBeChecked());
    await expect(github).toBeEnabled();
  },
};

const slowToggle = recording(
  routed({
    servers: async (_input, init) =>
      init?.method === "PATCH" ? new Promise<Response>(() => {}) : json(SERVERS),
  }),
);
export const ToggleLocksOnlyItsOwnCard: Story = {
  render: () => (
    <Harness fetchStub={slowToggle.stub}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const github = await canvas.findByRole("switch", { name: "Enable GitHub" });
    await userEvent.click(github);
    await waitFor(() => expect(github).toBeDisabled());
    await expect(canvas.getByRole("switch", { name: "Enable Sentry" })).toBeEnabled();
    // a second click on the locked switch sends nothing
    await userEvent.click(github);
    await expect(
      slowToggle.calls.filter(
        (call) => call.method === "PATCH" && call.url.includes("server-github"),
      ),
    ).toHaveLength(1);
  },
};

export const ToggleIsAnnounced: Story = {
  render: () => (
    <Harness
      fetchStub={routed({
        servers: async (_input, init) =>
          init?.method === "PATCH" ? json({ ...SERVERS[1], enabled: true }) : json(SERVERS),
      })}
    >
      <Toasts>
        <McpCatalog />
      </Toasts>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("switch", { name: "Enable Sentry" }));
    await waitFor(() => expect(canvas.getByText("Sentry enabled")).toBeVisible());
  },
};

// the dialog closes on a save, so the outcome has to live somewhere that does
// not: a toast naming the server. a refusal keeps the dialog and also toasts
export const SaveIsAnnounced: Story = {
  render: () => (
    <Harness
      fetchStub={routed({
        servers: async (_input, init) =>
          init?.method === "PATCH" ? json(SERVERS[0]) : json(SERVERS),
      })}
    >
      <Toasts>
        <McpCatalog />
      </Toasts>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Configure server GitHub" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.click(await dialog.findByRole("button", { name: "Save server" }));
    await expectSheetClosed();
    await waitFor(() => expect(canvas.getByText("GitHub updated.")).toBeVisible());
  },
};

export const SaveRefusedIsAnnounced: Story = {
  render: () => (
    <Harness
      fetchStub={routed({
        servers: async (_input, init) =>
          init?.method === "PATCH"
            ? json({ error: { message: "slug already taken" } }, 409)
            : json(SERVERS),
      })}
    >
      <Toasts>
        <McpCatalog />
      </Toasts>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Configure server GitHub" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.click(await dialog.findByRole("button", { name: "Save server" }));
    await waitFor(() => expect(canvas.getByText("Could not save GitHub")).toBeVisible());
    // the dialog stays so the operator can correct it
    await waitFor(() => expect(within(document.body).getByRole("dialog")).toBeVisible());
  },
};

export const RegisterIsAnnounced: Story = {
  render: () => (
    <Harness
      fetchStub={routed({
        servers: async (_input, init) =>
          init?.method === "POST"
            ? json({ ...SERVERS[0], id: "server-new", name: "Docs", slug: "docs" }, 201)
            : json([]),
      })}
    >
      <Toasts>
        <McpCatalog />
      </Toasts>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Register server" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.type(dialog.getByLabelText("Name"), "Docs");
    await userEvent.type(dialog.getByLabelText("Endpoint URL"), "https://mcp.example.com/mcp");
    await userEvent.click(dialog.getByRole("button", { name: "Register server" }));
    await expectSheetClosed();
    await waitFor(() => expect(canvas.getByText("Docs created")).toBeVisible());
  },
};

// ---------------------------------------------------------------------------
// authentication and transport overrides (#1447)

const byKind = () =>
  routed({
    servers: async (_input, init) => (init?.method === "PATCH" ? json(SERVERS[1]) : json(BY_KIND)),
  });
const openConfigure = async (canvasElement: HTMLElement, name: string) => {
  await userEvent.click(
    await within(canvasElement).findByRole("button", { name: `Configure server ${name}` }),
  );
  return within(await within(document.body).findByRole("dialog"));
};

// the card says how each server authenticates, so an open server and an armed
// one no longer look the same from the catalog
export const AuthKindOnEveryCard: Story = {
  render: () => (
    <Harness fetchStub={byKind()}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Context7")).toBeVisible());
    for (const badge of ["no auth", "bearer", "api key", "oauth"])
      await expect(canvas.getByText(badge)).toBeVisible();
  },
};

// `none` shows no credential field at all, and says it means unauthenticated
export const AuthNoneAsksForNothing: Story = {
  render: () => (
    <Harness fetchStub={byKind()}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Sentry");
    await expect(dialog.getByRole("radio", { name: /^None — unauthenticated/ })).toBeChecked();
    await expect(dialog.queryByLabelText("Bearer token")).not.toBeInTheDocument();
    await expect(dialog.queryByLabelText("Header name")).not.toBeInTheDocument();
    await expect(dialog.queryByLabelText("Client ID")).not.toBeInTheDocument();
  },
};

const bearerArms = bodyRecording(byKind());
export const AuthBearerNeedsACredential: Story = {
  render: () => (
    <Harness fetchStub={bearerArms.stub}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Sentry");
    await userEvent.click(dialog.getByRole("radio", { name: /^Bearer token/ }));
    await expect(dialog.getByText("No credential stored")).toBeVisible();
    // nothing stored and nothing typed is what the API refuses, so the form does first
    await expect(dialog.getByRole("button", { name: "Save server" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await userEvent.type(dialog.getByLabelText("Bearer token"), "ctx7_live_abc");
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    await expect(await bearerArms.sent("PUT", "/mcp-servers/server-sentry/auth")).toEqual({
      auth_kind: "bearer",
      credential: "ctx7_live_abc",
    });
    // untouched overrides are omitted, not echoed back as null
    const patch = (await bearerArms.sent("PATCH", "/mcp-servers/server-sentry")) as Record<
      string,
      unknown
    >;
    for (const key of ["connect_timeout_ms", "request_timeout_ms", "max_retries"])
      await expect(patch).not.toHaveProperty(key);
  },
};

// renaming the header must not resend — or clear — a secret nobody can read back
const headerRename = bodyRecording(
  routed({
    servers: async (_input, init) => (init?.method === "PATCH" ? json(HEADER) : json(BY_KIND)),
  }),
);
export const AuthHeaderRenameKeepsTheSecret: Story = {
  render: () => (
    <Harness fetchStub={headerRename.stub}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Exa");
    await expect(dialog.getByRole("radio", { name: /^API key header/ })).toBeChecked();
    await expect(dialog.getByText("Credential stored")).toBeVisible();
    await expect(dialog.getByLabelText("API key")).toHaveValue("");
    const header = dialog.getByLabelText("Header name");
    await userEvent.clear(header);
    await userEvent.type(header, "X-Exa-Token");
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    await expect(await headerRename.sent("PUT", "/mcp-servers/server-exa/auth")).toEqual({
      auth_kind: "header",
      auth_header_name: "X-Exa-Token",
    });
  },
};

// the reserved list is checked in the form, with the reason, not left to a 400
export const AuthHeaderRefusesReservedNames: Story = {
  render: () => (
    <Harness fetchStub={byKind()}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Exa");
    const header = dialog.getByLabelText("Header name");
    await userEvent.clear(header);
    await userEvent.type(header, "Authorization");
    await expect(dialog.getByText(/rolter sets Authorization itself/)).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Save server" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await userEvent.clear(header);
    await userEvent.type(header, "X Api Key");
    await expect(dialog.getByText(/valid HTTP header name/)).toBeVisible();
  },
};

// the oauth branch is the existing client section, shown for oauth alone
export const AuthOAuthShowsTheClient: Story = {
  render: () => (
    <Harness fetchStub={byKind()}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Linear");
    await expect(dialog.getByRole("radio", { name: /^OAuth consent/ })).toBeChecked();
    await expect(dialog.getByLabelText("Client ID")).toHaveValue("rolter-linear");
    await expect(dialog.queryByLabelText("Bearer token")).not.toBeInTheDocument();
  },
};

// confirm → cancel sends nothing; confirm → pending → done moves the server to
// `none`, which is the only shape the schema allows a server without a secret
let clearedOnce = false;
const clears = recording(
  routed({
    servers: async () =>
      json(
        clearedOnce
          ? [SERVERS[1], { ...BEARER, auth_kind: "none", has_credential: false }]
          : BY_KIND,
      ),
    auth: async () => {
      await new Promise((resolve) => setTimeout(resolve, 400));
      clearedOnce = true;
      return json({ ...BEARER, auth_kind: "none", has_credential: false });
    },
  }),
);
export const AuthClearCredentialConfirms: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    clearedOnce = false;
    return (
      <Harness fetchStub={clears.stub}>
        <UxScreenProvider screen="mcp-catalog">
          <McpCatalog />
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Context7");
    await userEvent.click(dialog.getByRole("button", { name: "Clear stored credential" }));
    const confirm = within(await within(document.body).findByRole("dialog"));
    await waitFor(() =>
      expect(confirm.getByText("Clear the stored credential for Context7?")).toBeVisible(),
    );
    await userEvent.click(confirm.getByRole("button", { name: "Cancel" }));
    clears.expectNotSent("PUT", "/auth");

    const again = within(await within(document.body).findByRole("dialog"));
    // the confirmation hid the sheet without ending the form (#3043): the
    // operator never left it, so neither the open nor the cancel is an abandon
    expectNoUxEvent("form_abandon", "mcp-server");
    await userEvent.click(again.getByRole("button", { name: "Clear stored credential" }));
    await confirmDestructive(/cannot be read back/, "Clear credential");
    await waitFor(() =>
      expect(
        within(document.body).getByRole("button", { name: "Clear credential" }),
      ).toBeDisabled(),
    );
    await expect(await clears.expectSentBody("PUT", "/mcp-servers/server-context7/auth")).toEqual({
      auth_kind: "none",
    });
    await waitFor(() =>
      expect(
        within(document.body).queryByRole("button", { name: "Clear credential" }),
      ).not.toBeInTheDocument(),
    );
    const after = within(await within(document.body).findByRole("dialog"));
    await waitFor(() =>
      expect(after.getByRole("radio", { name: /^None — unauthenticated/ })).toBeChecked(),
    );
    await expect(after.queryByText("Credential stored")).not.toBeInTheDocument();
  },
};

// switching a credentialed server to a kind that carries none deletes the
// secret on save, so the save asks first
const switches = bodyRecording(
  routed({
    servers: async (_input, init) => (init?.method === "PATCH" ? json(BEARER) : json(BY_KIND)),
  }),
);
export const AuthSwitchingAwayConfirms: Story = {
  render: () => (
    <Harness fetchStub={switches.stub}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Context7");
    await userEvent.click(dialog.getByRole("radio", { name: /^None — unauthenticated/ }));
    await expect(
      dialog.getByText("Saving with this method deletes the stored credential."),
    ).toBeVisible();
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    await confirmDestructive(
      /Switching to None — unauthenticated deletes the stored credential/,
      "Clear credential",
    );
    await expect(await switches.sent("PUT", "/mcp-servers/server-context7/auth")).toEqual({
      auth_kind: "none",
    });
  },
};

// a missing KEK is a deployment problem, so it is named as one
export const AuthKekMissing: Story = {
  render: () => (
    <Harness
      fetchStub={routed({
        servers: async (_input, init) =>
          init?.method === "PATCH" ? json(SERVERS[1]) : json(BY_KIND),
        auth: async () => json({ error: { message: KEK_REFUSAL } }, 400),
      })}
    >
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Sentry");
    await userEvent.click(dialog.getByRole("radio", { name: /^Bearer token/ }));
    await userEvent.type(dialog.getByLabelText("Bearer token"), "ctx7_live_abc");
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    await waitFor(() =>
      expect(dialog.getByText("The control plane has no ROLTER_KEK")).toBeVisible(),
    );
    await expect(dialog.getByText(KEK_REFUSAL)).toBeVisible();
    // the dialog stays open with the typed value, so a retry needs no re-typing
    await expect(dialog.getByLabelText("Bearer token")).toHaveValue("ctx7_live_abc");
  },
};

// blank is inherit (null), a moved value is sent, an untouched one is omitted
const overrides = bodyRecording(
  routed({
    servers: async (_input, init) => (init?.method === "PATCH" ? json(HEADER) : json(BY_KIND)),
  }),
);
export const OverridesKeepAbsentAndNullApart: Story = {
  render: () => (
    <Harness fetchStub={overrides.stub}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Exa");
    await expect(dialog.getByLabelText("Connect timeout (ms)")).toHaveValue("2000");
    await expect(dialog.getByLabelText("Request timeout (ms)")).toHaveAttribute(
      "placeholder",
      "Inherit",
    );
    const retries = dialog.getByLabelText("Maximum retries");
    await userEvent.clear(retries);
    await userEvent.type(retries, "9");
    await expect(dialog.getByRole("button", { name: "Save server" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await userEvent.clear(retries);
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    const patch = (await overrides.sent("PATCH", "/mcp-servers/server-exa")) as Record<
      string,
      unknown
    >;
    await waitFor(() =>
      expect(within(document.body).queryByRole("dialog")).not.toBeInTheDocument(),
    );
    await expect(patch.max_retries).toBeNull();
    await expect(patch).not.toHaveProperty("connect_timeout_ms");
    await expect(patch).not.toHaveProperty("request_timeout_ms");
    // nothing about auth moved, so the KEK-gated route is not called at all
    await expect(overrides.calls.some((call) => call.url.endsWith("/auth"))).toBe(false);
  },
};

// a viewer reads the registry but can neither register nor configure a server
export const GatedForViewer: Story = {
  render: () => (
    <GatedHarness role="viewer" fetchStub={byKind()}>
      <McpCatalog />
    </GatedHarness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Register server");
    await expectRefused(canvasElement, "Configure server Context7");
    await expectRefused(canvasElement, "Discover tools of server Context7");
  },
};

// ---------------------------------------------------------------------------
// discovery, issuer and what discovery found (#1415)

// a client with nothing but an id, whose endpoints discovery already resolved
const DISCOVERED: McpServerRow = {
  ...OAUTH,
  id: "server-notion",
  name: "Notion",
  slug: "notion",
  url: "https://mcp.notion.com/mcp",
  authorize_url: null,
  token_url: null,
  client_id: "rolter-notion",
  has_client_secret: false,
  default_scopes: [],
  oauth_discovered_issuer: "https://api.notion.com",
  oauth_discovered_authorize_url: "https://api.notion.com/v1/oauth/authorize",
  oauth_discovered_token_url: "https://api.notion.com/v1/oauth/token",
  oauth_discovered_iss_supported: true,
  oauth_discovered_at: "2026-09-12T08:30:00Z",
};
// the same, before anyone has connected: an id and no cache
const UNPROBED: McpServerRow = {
  ...DISCOVERED,
  id: "server-figma",
  name: "Figma",
  slug: "figma",
  url: "https://mcp.figma.com/mcp",
  client_id: "rolter-figma",
  ...UNDISCOVERED,
};
// a hand-configured server that publishes no metadata, with its issuer pinned
const MANUAL_OAUTH: McpServerRow = {
  ...OAUTH,
  id: "server-jira",
  name: "Jira",
  slug: "jira",
  url: "https://mcp.atlassian.com/v1/sse",
  authorize_url: "https://auth.atlassian.com/authorize",
  token_url: "https://auth.atlassian.com/oauth/token",
  client_id: "rolter-jira",
  oauth_discovery: "manual",
  oauth_issuer: "https://auth.atlassian.com",
};
const DISCOVERY_ROWS = [DISCOVERED, UNPROBED, MANUAL_OAUTH];
// a PATCH answers with the row it was sent to, since the client PUT that
// follows is addressed by the id that comes back
const discoverySaves = () =>
  bodyRecording(
    routed({
      servers: async (input, init) =>
        init?.method === "PATCH"
          ? json(DISCOVERY_ROWS.find((row) => urlOf(input).endsWith(`/${row.id}`)) ?? DISCOVERED)
          : json(DISCOVERY_ROWS),
    }),
  );

// an operator can see what discovery resolved instead of guessing whether it
// worked, and a server that relies on it can connect with a client id alone
const shows = discoverySaves();
export const OAuthDiscoveryShowsWhatItFound: Story = {
  render: () => (
    <Harness fetchStub={shows.stub}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByRole("button", { name: "Start the consent flow for Notion" }),
    ).toBeEnabled();
    const dialog = await openConfigure(canvasElement, "Notion");
    await expect(dialog.getByRole("radio", { name: /^Discover automatically/ })).toBeChecked();
    const found = within(dialog.getByRole("group", { name: "Discovered endpoints" }));
    await expect(found.getByText("https://api.notion.com/v1/oauth/authorize")).toBeVisible();
    await expect(found.getByText("https://api.notion.com/v1/oauth/token")).toBeVisible();
    await expect(found.getByText("https://api.notion.com")).toBeVisible();
    await expect(found.getByText("Sends iss in callbacks")).toBeVisible();
    // the heading and the timestamp both start that way; the timestamp is the second
    await expect(found.getAllByText(/^Discovered /)).toHaveLength(2);
    // the typed pair is the fallback here, so blank is a valid answer
    await expect(dialog.getByRole("group", { name: "Fallback endpoints" })).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Save server" })).toBeEnabled();
    // nothing about the client moved, so the audited PUT is not sent
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    await shows.sent("PATCH", "/mcp-servers/server-notion");
    await expect(
      shows.calls.some((call) => call.method === "PUT" && call.url.includes("/oauth-client")),
    ).toBe(false);
  },
};

// a fresh server honestly says nothing has been found, and registering it
// needs only the client id — no endpoint keys go on the wire at all
const unprobed = discoverySaves();
export const OAuthDiscoveryNothingFoundYet: Story = {
  render: () => (
    <Harness fetchStub={unprobed.stub}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Figma");
    const found = within(dialog.getByRole("group", { name: "Discovered endpoints" }));
    await expect(found.getByText(/Nothing discovered yet/)).toBeVisible();
    await userEvent.type(dialog.getByLabelText("Default scopes"), "file:read");
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    await expect(await unprobed.sent("PUT", "/mcp-servers/server-figma/oauth-client")).toEqual({
      client_id: "rolter-figma",
      discovery: "auto",
      issuer: null,
      default_scopes: ["file:read"],
    });
  },
};

// manual never probes, so the pair becomes required, the discovered panel goes
// away, and half a pair is refused with a message rather than a 400
const manual = discoverySaves();
export const OAuthManualRequiresEndpoints: Story = {
  render: () => (
    <Harness fetchStub={manual.stub}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Figma");
    await userEvent.click(dialog.getByRole("radio", { name: /^Configure manually/ }));
    await expect(
      dialog.queryByRole("group", { name: "Discovered endpoints" }),
    ).not.toBeInTheDocument();
    await expect(dialog.getByRole("group", { name: "Endpoints" })).toBeVisible();
    await expect(dialog.getByRole("alert")).toHaveTextContent(
      /Manual discovery needs an authorization URL and a token URL/,
    );
    await expect(dialog.getByRole("button", { name: "Save server" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await userEvent.type(dialog.getByLabelText("Authorization URL"), "https://www.figma.com/oauth");
    await expect(dialog.getByRole("alert")).toHaveTextContent(
      /Enter both the authorization URL and the token URL/,
    );
    await userEvent.type(
      dialog.getByLabelText("Token URL"),
      "https://api.figma.com/v1/oauth/token",
    );
    await userEvent.type(dialog.getByLabelText("Issuer"), "https://www.figma.com");
    await expect(dialog.queryByRole("alert")).not.toBeInTheDocument();
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    await expect(await manual.sent("PUT", "/mcp-servers/server-figma/oauth-client")).toEqual({
      client_id: "rolter-figma",
      discovery: "manual",
      issuer: "https://www.figma.com",
      authorize_url: "https://www.figma.com/oauth",
      token_url: "https://api.figma.com/v1/oauth/token",
      default_scopes: [],
    });
  },
};

// re-saving a manual server used to omit the mode, which the control plane
// reads as auto. the mode and the pinned issuer now round-trip untouched
const resave = discoverySaves();
export const OAuthManualResaveKeepsTheMode: Story = {
  render: () => (
    <Harness fetchStub={resave.stub}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Jira");
    await expect(dialog.getByRole("radio", { name: /^Configure manually/ })).toBeChecked();
    await expect(dialog.getByLabelText("Issuer")).toHaveValue("https://auth.atlassian.com");
    await userEvent.type(dialog.getByLabelText("Default scopes"), "read:jira-work");
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    const body = await resave.sent("PUT", "/mcp-servers/server-jira/oauth-client");
    await expect(body).toMatchObject({
      discovery: "manual",
      issuer: "https://auth.atlassian.com",
      authorize_url: MANUAL_OAUTH.authorize_url,
      token_url: MANUAL_OAUTH.token_url,
    });
  },
};

// re-pinning the issuer drops what discovery cached, so the form says so before
// the operator saves rather than leaving the panel empty afterwards unexplained
export const OAuthIssuerChangeResetsDiscovery: Story = {
  render: () => (
    <Harness fetchStub={discoverySaves().stub}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Notion");
    await expect(dialog.queryByText(/clears these results/)).not.toBeInTheDocument();
    await userEvent.type(dialog.getByLabelText("Issuer"), "http://api.notion.com");
    await expect(
      dialog.getByText("Must be an https URL (http is accepted only on loopback)."),
    ).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Save server" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await userEvent.clear(dialog.getByLabelText("Issuer"));
    await userEvent.type(dialog.getByLabelText("Issuer"), "https://api.notion.com");
    await expect(dialog.getByText(/clears these results/)).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Save server" })).toBeEnabled();
  },
};

// the store drops the discovery cache when the url moves too (#1416), so the
// url field itself says so, and the discovered panel warns the same way it
// does for an issuer or mode change (#1572). moving the url back clears both
const moved = discoverySaves();
export const OAuthUrlChangeResetsDiscovery: Story = {
  render: () => (
    <Harness fetchStub={moved.stub}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Notion");
    const url = dialog.getByLabelText("Endpoint URL");
    await expect(dialog.queryByText(/clears these results/)).not.toBeInTheDocument();
    await expect(
      dialog.queryByText(/clears the OAuth endpoints discovered/),
    ).not.toBeInTheDocument();
    await userEvent.clear(url);
    await userEvent.type(url, "https://mcp.notion.so/mcp");
    await expect(url).toHaveAccessibleDescription(
      /Saving a new URL clears the OAuth endpoints discovered for this server/,
    );
    const found = within(dialog.getByRole("group", { name: "Discovered endpoints" }));
    await expect(
      found.getByText(/Saving a new server URL, issuer or discovery mode clears these results/),
    ).toBeVisible();
    await userEvent.clear(url);
    await userEvent.type(url, DISCOVERED.url);
    await expect(dialog.queryByText(/clears these results/)).not.toBeInTheDocument();
    await expect(url).not.toHaveAccessibleDescription(/clears the OAuth endpoints/);
    await userEvent.clear(url);
    await userEvent.type(url, "https://mcp.notion.so/mcp");
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    await expect(await moved.sent("PATCH", "/mcp-servers/server-notion")).toMatchObject({
      url: "https://mcp.notion.so/mcp",
    });
  },
};

// a server nothing was ever discovered for loses nothing, so a new url is not
// worth a warning there
export const OAuthUrlChangeWithoutDiscoveryStaysQuiet: Story = {
  render: () => (
    <Harness fetchStub={discoverySaves().stub}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Figma");
    const url = dialog.getByLabelText("Endpoint URL");
    await userEvent.clear(url);
    await userEvent.type(url, "https://mcp.figma.com/v2/mcp");
    await expect(dialog.queryByText(/clears these results/)).not.toBeInTheDocument();
    await expect(
      dialog.queryByText(/clears the OAuth endpoints discovered/),
    ).not.toBeInTheDocument();
  },
};

// the client read is best-effort: refused, the section still edits off the
// server row and only the redirect uri, which nothing else can supply, is gone
export const OAuthClientReadRefused: Story = {
  render: () => (
    <Harness
      fetchStub={routed({
        servers: async () => json(DISCOVERY_ROWS),
        oauthClient: async () => json({ error: { message: "forbidden" } }, 403),
      })}
    >
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Jira");
    await expect(dialog.getByLabelText("Client ID")).toHaveValue("rolter-jira");
    await expect(dialog.queryByLabelText("Redirect URI")).not.toBeInTheDocument();
    await expect(dialog.getByRole("button", { name: "Save server" })).toBeEnabled();
  },
};

// The catalog's other two gates (#1606). `CatalogGatedForViewer` above covers
// the create and the configure; the delete and the enable toggle are separate
// controls with separate gates, and a `GatedSwitch` is not a `GatedButton`, so
// neither is reached by that story.
export const DeleteAndToggleGatedForViewer: Story = {
  render: () => (
    <GatedHarness role="viewer" fetchStub={routed()}>
      <McpCatalog />
    </GatedHarness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectRefused(canvasElement, "Delete server GitHub");
    // the switch carries the same refusal, said through `aria-disabled`: a
    // disabled Radix switch still has to be announced as a switch
    const toggle = await canvas.findByRole("switch", { name: "Enable Sentry" });
    await waitFor(() => expect(toggle).toBeDisabled());
    await expect(toggle).toHaveAttribute("title", NEEDS_ADMIN);
  },
};

// the same screen at a phone's width in both languages: Russian runs a third
// longer than English and overflowed twice as many screens (#2004)
const catalogFits = phoneFits({
  render: () => (
    <Harness fetchStub={routed()}>
      <McpCatalog />
    </Harness>
  ),
  ready: (canvas) => canvas.findByText("GitHub"),
});
export const Mobile: Story = catalogFits("mobile", "en");
export const MobileInRussian: Story = catalogFits("mobile", "ru");

// registering a server is a sheet of four sections rather than a wall of twenty
// controls (#2170): the transport overrides, which most servers never set,
// start folded, and the OAuth fields exist only under the OAuth method
export const RegisterSheetIsSectioned: Story = {
  render: () => (
    <Harness fetchStub={routed()}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", { name: "Register server" }),
    );
    const sheet = within(await within(document.body).findByRole("dialog"));
    for (const title of ["Endpoint", "Tools", "Authentication"])
      await expect(sheet.getByRole("button", { name: title })).toHaveAttribute(
        "aria-expanded",
        "true",
      );
    const overrides = sheet.getByRole("button", { name: "Transport overrides" });
    await expect(overrides).toHaveAttribute("aria-expanded", "false");
    await expect(sheet.queryByLabelText("Connect timeout (ms)")).not.toBeInTheDocument();
    await userEvent.click(overrides);
    await waitFor(() => expect(sheet.getByLabelText("Connect timeout (ms)")).toBeVisible());
    // the tool names are typed from memory, and the field says so
    await expect(sheet.getByText(/declared here, not discovered/)).toBeVisible();
    await expect(sheet.queryByLabelText("Client ID")).not.toBeInTheDocument();
    // the chosen method is marked with the selection tokens, not the brand red
    const picked = sheet.getByRole("radio", { name: /^None — unauthenticated/ });
    await expect(picked.closest("label")).toHaveClass("bg-[color:var(--red-tint)]");
    await userEvent.click(sheet.getByRole("radio", { name: /^OAuth consent/ }));
    await expect(await sheet.findByLabelText("Client ID")).toBeVisible();
  },
};

// the sheet is the whole screen below `sm`; its sections and footer stay inside it
const sheetFits = (locale: "en" | "ru") => ({
  parameters: atMobile.parameters,
  globals: { ...atMobile.globals, locale },
  render: () => (
    <Harness fetchStub={routed()}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }: { canvasElement: HTMLElement }) => {
    const buttons = await within(canvasElement).findAllByRole("button", {
      name: /Register server|Зарегистрировать сервер/,
    });
    await userEvent.click(buttons[0]);
    const panel = await within(document.body).findByRole("dialog");
    await animationsSettled();
    await expect(panel.getBoundingClientRect().width).toBeLessThanOrEqual(window.innerWidth);
    await userEvent.click(within(panel).getAllByRole("radio")[3]);
    await expect(panel.scrollWidth).toBeLessThanOrEqual(panel.clientWidth);
    await expectNoHorizontalOverflow();
  },
});
export const RegisterSheetOnAPhone: Story = sheetFits("en");
export const RegisterSheetOnAPhoneInRussian: Story = sheetFits("ru");

// ---------------------------------------------------------------------------
// tools/list discovery (#1283): a manifest is pending, answered (possibly
// empty), failed with the last answer kept, and may carry stale names

const FAILED_AT = "2026-09-13T10:00:00Z";
const oneServer = (over: Partial<McpServerRow>): McpServerRow => ({
  ...SERVERS[0],
  ...DISCOVERED_OK,
  ...over,
});
const withRows = (rows: McpServerRow[]) => (
  <Harness fetchStub={routed({ servers: async () => json(rows) })}>
    <McpCatalog />
  </Harness>
);

// never answered: the tools shown are the catalog's, and the card says so
export const DiscoveryPending: Story = {
  render: () => withRows([oneServer({ ...NOT_PROBED, tools_discovery_attempted_at: FAILED_AT })]),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/^Not discovered yet\./)).toBeVisible();
    await expect(canvas.queryByText(/Declares no tools/)).not.toBeInTheDocument();
  },
};

// OAuth server nobody has connected to: the reason is the dashboard's own words
export const DiscoveryPendingNeedsSession: Story = {
  render: () =>
    withRows([
      oneServer({
        ...NOT_PROBED,
        tools_discovery_attempted_at: FAILED_AT,
        tools_discovery_error: "needs_session",
      }),
    ]),
  play: async ({ canvasElement }) => {
    await expect(
      await within(canvasElement).findByText("Not discovered yet: waiting for a user to connect."),
    ).toBeVisible();
  },
};

// the background discovery after an install has not run yet: the list is read
// again until it has, and the result appears without a reload
export const DiscoveryPollsUntilAttempted: Story = {
  render: () => {
    let reads = 0;
    return (
      <Harness
        fetchStub={routed({
          servers: async () => {
            reads += 1;
            return json([
              reads < 2 ? oneServer({ ...NOT_PROBED }) : oneServer({ tools: ["search_code"] }),
            ]);
          },
        })}
      >
        <McpCatalog />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("Discovering tools…")).toBeVisible();
    await waitFor(() => expect(canvas.getByText(/^Last discovered /)).toBeVisible(), {
      timeout: 6000,
    });
  },
};

export const DiscoveryDiscovered: Story = {
  render: () => withRows([oneServer({})]),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/^Last discovered /)).toBeVisible();
    await expect(canvas.getByText("search_code")).toBeVisible();
  },
};

// an answer with nothing in it is not "not discovered"
export const DiscoveryDeclaresNoTools: Story = {
  render: () => withRows([oneServer({ tools: [] })]),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/^Declares no tools\./)).toBeVisible();
    await expect(canvas.queryByText(/Not discovered yet/)).not.toBeInTheDocument();
  },
};

export const DiscoveryFailedAfterSuccess: Story = {
  render: () =>
    withRows([
      oneServer({
        tools_discovery_status: "failed",
        tools_discovery_error: "unreachable",
        tools_discovery_attempted_at: FAILED_AT,
      }),
    ]),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByText(/^Last discovery failed: the server could not be reached\./),
    ).toBeVisible();
    await expect(canvas.getByText(/Last succeeded /)).toBeVisible();
    // the last manifest is kept
    await expect(canvas.getByText("search_code")).toBeVisible();
  },
};

export const DiscoveryFlagsStaleTools: Story = {
  render: () => withRows([oneServer({ tools_stale: ["create_issue"] })]),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const stale = await canvas.findByText("create_issue");
    await expect(stale.closest("span")).toHaveAttribute(
      "title",
      expect.stringMatching(/kept so tool groups naming it keep working/),
    );
    await expect(canvas.getByText(/^1 tool was not returned by the last discovery/)).toBeVisible();
  },
};

const discoverNow = (() => {
  let discovered = false;
  return recording(
    routed({
      servers: async (input, init) => {
        if (urlOf(input).endsWith("/discover") && init?.method === "POST") {
          discovered = true;
          return json(oneServer({ tools: ["search_code", "open_pr"] }));
        }
        return json([
          discovered
            ? oneServer({ tools: ["search_code", "open_pr"] })
            : oneServer({ ...NOT_PROBED, tools_discovery_attempted_at: FAILED_AT }),
        ]);
      },
    }),
  );
})();
export const DiscoverNow: Story = {
  render: () => (
    <Harness fetchStub={discoverNow.stub}>
      <McpCatalog />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/^Not discovered yet\./)).toBeVisible();
    await userEvent.click(canvas.getByRole("button", { name: "Discover tools of server GitHub" }));
    await discoverNow.expectSent("POST", "/mcp-servers/server-github/discover");
    await waitFor(() => expect(canvas.getByText("open_pr")).toBeVisible());
    await expect(canvas.getByText(/^Last discovered /)).toBeVisible();
  },
};

export const DiscoverNowFails: Story = {
  render: () => (
    <Harness
      fetchStub={routed({
        servers: async (input, init) =>
          urlOf(input).endsWith("/discover") && init?.method === "POST"
            ? json({ error: { message: "registry offline" } }, 503)
            : json([oneServer({})]),
      })}
    >
      <Toasts>
        <McpCatalog />
      </Toasts>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Discover tools of server GitHub" }),
    );
    await waitFor(() =>
      expect(within(document.body).getByText("Could not discover tools of GitHub")).toBeVisible(),
    );
  },
};

// ---------------------------------------------------------------------------
// connection status, test connection, tools and try it (#951)

const CONTACT_AT = "2026-09-13T09:00:00Z";
const ERROR_AT = "2026-09-13T11:00:00Z";
const reachableRow = (over: Partial<McpServerRow> = {}) =>
  oneServer({
    last_contact_at: CONTACT_AT,
    last_protocol_version: "2025-03-26",
    last_server_name: "github-mcp",
    ...over,
  });

const TOOL_LIST: McpServerToolsResponse = {
  status: "discovered",
  error: null,
  discovered_at: "2026-09-12T08:30:00Z",
  tools: [
    {
      name: "search_code",
      description: "Search code across repositories.",
      input_schema: {
        type: "object",
        required: ["query"],
        properties: { query: { type: "string" }, limit: { type: "integer" } },
      },
      stale: false,
    },
    { name: "create_issue", description: null, input_schema: { type: "object" }, stale: true },
  ],
};

const CALL_OK: McpToolCallResult = {
  outcome: "success",
  ok: true,
  latency_ms: 84,
  request: { jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "search_code" } },
  response: { jsonrpc: "2.0", id: 1, result: { content: [{ type: "text", text: "3 matches" }] } },
  message: null,
};
const TEST_OK: McpConnectionTest = {
  status: "reachable",
  reachable: true,
  handshake_completed: true,
  protocol_version: "2025-03-26",
  server_name: "github-mcp",
  server_version: "1.4.2",
  latency_ms: 42,
  message: null,
  checked_at: "2026-09-13T12:00:00Z",
};

interface ProbeApi {
  rows?: () => McpServerRow[];
  test?: () => Response | Promise<Response>;
  tools?: () => Response | Promise<Response>;
  call?: () => Response | Promise<Response>;
}
const probeApi = (api: ProbeApi = {}) =>
  recording(
    routed({
      servers: async (input, init) => {
        const url = urlOf(input);
        const post = init?.method === "POST";
        if (post && url.endsWith("/test")) return api.test?.() ?? json(TEST_OK);
        if (post && url.endsWith("/tools/call")) return api.call?.() ?? json(CALL_OK);
        if (url.endsWith("/tools")) return api.tools?.() ?? json(TOOL_LIST);
        return json(api.rows?.() ?? [reachableRow()]);
      },
    }),
  );
const probeStory = (api: ProbeApi = {}) => {
  const stub = probeApi(api);
  return {
    stub,
    render: () => (
      <Harness fetchStub={stub.stub}>
        <Toasts>
          <McpCatalog />
        </Toasts>
      </Harness>
    ),
  };
};
const health = async (canvas: ReturnType<typeof within>, name = "GitHub") =>
  within(await canvas.findByRole("group", { name: `Connection status of ${name}` }));

export const ConnectionNotChecked: Story = {
  render: () => withRows([oneServer({})]),
  play: async ({ canvasElement }) => {
    const status = await health(within(canvasElement));
    await expect(status.getByText("Not checked yet")).toBeVisible();
    await expect(status.getByText("No handshake has been attempted yet.")).toBeVisible();
  },
};

export const ConnectionReachable: Story = {
  render: () => withRows([reachableRow()]),
  play: async ({ canvasElement }) => {
    const status = await health(within(canvasElement));
    await expect(status.getByText("Reachable")).toBeVisible();
    await expect(
      status.getByText(/Last contact .*Protocol 2025-03-26 · Server github-mcp/),
    ).toBeVisible();
    await expect(status.queryByText(/Last failure/)).not.toBeInTheDocument();
  },
};

// each failure reads as its own words, so none of them is told by colour alone
const FAILURES = [
  ["unreachable", "Unreachable"],
  ["handshake_failed", "Handshake failed"],
  ["auth_rejected", "Auth rejected"],
  ["egress_denied", "Blocked by egress policy"],
] as const;
export const ConnectionFailureStates: Story = {
  render: () =>
    withRows(
      FAILURES.map(([kind], index) =>
        oneServer({
          id: `server-${kind}`,
          name: `Server ${index + 1}`,
          slug: `server-${index + 1}`,
          last_error_kind: kind,
          last_error_at: ERROR_AT,
        }),
      ),
    ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    for (const [index, [, label]] of FAILURES.entries()) {
      const status = await health(canvas, `Server ${index + 1}`);
      await expect(status.getByText(label)).toBeVisible();
      await expect(status.getByText(/^Failed /)).toBeVisible();
      await expect(status.queryByText("Reachable")).not.toBeInTheDocument();
    }
  },
};

// the control plane keeps the last failure after a later success: the server is
// reachable now, and the card still says what went wrong before
export const ConnectionRecoveredKeepsTheLastFailure: Story = {
  render: () =>
    withRows([
      reachableRow({
        last_contact_at: ERROR_AT,
        last_error_kind: "auth_rejected",
        last_error_at: CONTACT_AT,
      }),
    ]),
  play: async ({ canvasElement }) => {
    const status = await health(within(canvasElement));
    await expect(status.getByText("Reachable")).toBeVisible();
    await expect(status.getByText(/^Last failure: Auth rejected, /)).toBeVisible();
  },
};

const testing = probeStory({
  rows: (() => {
    let calls = 0;
    return () => {
      calls += 1;
      // the list is read again after the test, and by then the handshake is on record
      return [calls < 2 ? oneServer({}) : reachableRow({ last_contact_at: TEST_OK.checked_at })];
    };
  })(),
});
export const TestConnection: Story = {
  render: testing.render,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const status = await health(canvas);
    await expect(status.getByText("Not checked yet")).toBeVisible();
    await userEvent.click(
      canvas.getByRole("button", { name: "Test the connection of server GitHub" }),
    );
    await testing.stub.expectSent("POST", "/mcp-servers/server-github/test");
    const result = within(await canvas.findByRole("status", { name: "Connection test of GitHub" }));
    await expect(result.getByText("Reachable", { selector: "span" })).toBeVisible();
    await expect(result.getByText("2025-03-26")).toBeVisible();
    await expect(result.getByText("github-mcp")).toBeVisible();
    await expect(result.getByText("1.4.2")).toBeVisible();
    await expect(result.getByText("42 ms")).toBeVisible();
    await expect(result.getByText("Completed")).toBeVisible();
    // the list was read again, so the badge moved off "Not checked yet"
    await waitFor(() => expect(status.queryByText("Not checked yet")).not.toBeInTheDocument());
  },
};

const TEST_FAILURES: [string, McpConnectionTest][] = [
  [
    "Needs a session",
    {
      ...TEST_OK,
      status: "needs_session",
      reachable: false,
      handshake_completed: false,
      protocol_version: null,
      server_name: null,
      server_version: null,
    },
  ],
  [
    "Credential unavailable",
    {
      ...TEST_OK,
      status: "credential_unavailable",
      reachable: false,
      handshake_completed: false,
      message: "sealed credential could not be opened",
    },
  ],
];
const needsSession = probeStory({ test: () => json(TEST_FAILURES[0][1]) });
export const TestConnectionNeedsASession: Story = {
  render: needsSession.render,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Test the connection of server GitHub" }),
    );
    const result = within(await canvas.findByRole("status", { name: "Connection test of GitHub" }));
    await expect(result.getByText("Needs a session")).toBeVisible();
    await expect(result.getByText(/Connect first, then try again/)).toBeVisible();
    await expect(result.getByText("Not completed")).toBeVisible();
  },
};
const noCredential = probeStory({ test: () => json(TEST_FAILURES[1][1]) });
export const TestConnectionCredentialUnavailable: Story = {
  render: noCredential.render,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Test the connection of server GitHub" }),
    );
    const result = within(await canvas.findByRole("status", { name: "Connection test of GitHub" }));
    await expect(result.getByText("Credential unavailable")).toBeVisible();
    await expect(result.getByText("sealed credential could not be opened")).toBeVisible();
  },
};

// the request itself failing is not a status: it is announced like any other save
const testFails = probeStory({
  test: () => json({ error: { message: "control plane busy" } }, 503),
});
export const TestConnectionRequestFails: Story = {
  render: testFails.render,
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", {
        name: "Test the connection of server GitHub",
      }),
    );
    await waitFor(() =>
      expect(
        within(document.body).getByText("Could not test the connection of GitHub"),
      ).toBeVisible(),
    );
  },
};

const pendingTest = probeStory({ test: () => new Promise<Response>(() => {}) });
export const TestConnectionPending: Story = {
  render: pendingTest.render,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Test the connection of server GitHub" }),
    );
    await waitFor(() =>
      expect(
        canvas.getByRole("button", { name: "Test the connection of server GitHub" }),
      ).toBeDisabled(),
    );
    await expect(canvas.getByText("Testing…")).toBeVisible();
  },
};

const openTools = async (canvasElement: HTMLElement, name = "GitHub") => {
  await userEvent.click(
    await within(canvasElement).findByRole("button", {
      name: `Browse the tools of server ${name} and try them`,
    }),
  );
  return within(await within(document.body).findByRole("dialog"));
};

export const ToolsLoaded: Story = {
  render: probeStory().render,
  play: async ({ canvasElement }) => {
    const sheet = await openTools(canvasElement);
    const list = within(await sheet.findByRole("list", { name: "Tools" }));
    await expect(list.getByText("search_code")).toBeVisible();
    await expect(list.getByText("Search code across repositories.")).toBeVisible();
    // the manifest kept a tool the last discovery no longer returned, and says so in words
    await expect(list.getAllByText("Not returned by the last discovery")).toHaveLength(1);
    await expect(list.getByText("No description.")).toBeVisible();
    await userEvent.click(list.getAllByText("Input schema")[0]);
    await expect(
      await sheet.findByRole("region", { name: /Input schema of search_code/ }),
    ).toBeVisible();
  },
};

export const ToolsLoading: Story = {
  render: probeStory({ tools: () => new Promise<Response>(() => {}) }).render,
  play: async ({ canvasElement }) => {
    const sheet = await openTools(canvasElement);
    await waitFor(() => expect(sheet.getAllByLabelText("Loading…").length).toBeGreaterThan(0));
    await expect(sheet.queryByText(/declares no tools/)).not.toBeInTheDocument();
  },
};

export const ToolsDeclaresNone: Story = {
  render: probeStory({ tools: () => json({ ...TOOL_LIST, tools: [] }) }).render,
  play: async ({ canvasElement }) => {
    const sheet = await openTools(canvasElement);
    await expect(await sheet.findByText("GitHub declares no tools")).toBeVisible();
    await expect(sheet.queryByText("Tools not discovered yet")).not.toBeInTheDocument();
  },
};

export const ToolsNotDiscoveredYet: Story = {
  render: probeStory({
    tools: () => json({ status: "pending", error: null, discovered_at: null, tools: [] }),
  }).render,
  play: async ({ canvasElement }) => {
    const sheet = await openTools(canvasElement);
    await expect(await sheet.findByText("Tools not discovered yet")).toBeVisible();
    await expect(sheet.queryByText(/declares no tools/)).not.toBeInTheDocument();
  },
};

export const ToolsReadFails: Story = {
  render: probeStory({
    tools: () => json({ error: { message: "registry offline" } }, 503),
  }).render,
  play: async ({ canvasElement }) => {
    const sheet = await openTools(canvasElement);
    await waitFor(() =>
      expect(
        sheet
          .getAllByRole("alert")
          .some((a) => /tools of the MCP server/i.test(a.textContent ?? "")),
      ).toBe(true),
    );
    await expect(sheet.getByRole("button", { name: /try again/i })).toBeVisible();
  },
};

const pickTool = async (sheet: ReturnType<typeof within>, tool: string) => {
  await userEvent.click(await sheet.findByRole("combobox", { name: "Tool" }));
  await userEvent.click(await within(document.body).findByRole("option", { name: tool }));
};

const tryOk = probeStory();
export const TryItSucceeds: Story = {
  render: tryOk.render,
  play: async ({ canvasElement }) => {
    const sheet = await openTools(canvasElement);
    await pickTool(sheet, "search_code");
    // picking a tool seeds the arguments with its required properties
    const args = sheet.getByRole("textbox", { name: "Arguments (JSON object)" });
    await expect(JSON.parse((args as HTMLTextAreaElement).value)).toEqual({ query: "" });
    await userEvent.clear(args);
    await userEvent.click(args);
    await userEvent.paste('{"query":"retry"}');
    await userEvent.click(
      sheet.getByRole("button", { name: "Run the selected tool on server GitHub" }),
    );
    expect(
      await tryOk.stub.expectSentBody("POST", "/mcp-servers/server-github/tools/call"),
    ).toEqual({ tool: "search_code", arguments: { query: "retry" } });
    const outcome = within(await sheet.findByRole("status", { name: "Result" }));
    await expect(outcome.getByText("Succeeded")).toBeVisible();
    await expect(outcome.getByText(/84 ms/)).toBeVisible();
    await expect(sheet.getByRole("region", { name: /^Request/ })).toHaveTextContent("tools/call");
    await expect(sheet.getByRole("region", { name: /^Response/ })).toHaveTextContent("3 matches");
  },
};

// the server answered and the tool failed: not a connection problem, so it must
// not read like one
const TOOL_ERRORED: McpToolCallResult = {
  ...CALL_OK,
  outcome: "tool_errored",
  ok: false,
  message: "rate limit exceeded",
  response: { jsonrpc: "2.0", id: 1, result: { isError: true, content: [] } },
};
const toolErrored = probeStory({ call: () => json(TOOL_ERRORED) });
export const TryItToolErrored: Story = {
  render: toolErrored.render,
  play: async ({ canvasElement }) => {
    const sheet = await openTools(canvasElement);
    await pickTool(sheet, "search_code");
    await userEvent.click(
      sheet.getByRole("button", { name: "Run the selected tool on server GitHub" }),
    );
    const outcome = within(await sheet.findByRole("status", { name: "Result" }));
    await expect(outcome.getByText("Tool reported an error")).toBeVisible();
    await expect(outcome.getByText(/The connection itself is fine/)).toBeVisible();
    await expect(outcome.getByText("rate limit exceeded")).toBeVisible();
    await expect(outcome.queryByText("Unreachable")).not.toBeInTheDocument();
    await expect(outcome.queryByText("Succeeded")).not.toBeInTheDocument();
    await expect(sheet.getByRole("region", { name: /^Response/ })).toHaveTextContent("isError");
  },
};

const unreachableCall = probeStory({
  call: () =>
    json({
      ...CALL_OK,
      outcome: "unreachable",
      ok: false,
      response: null,
      message: "connection refused",
    }),
});
export const TryItUnreachable: Story = {
  render: unreachableCall.render,
  play: async ({ canvasElement }) => {
    const sheet = await openTools(canvasElement);
    await pickTool(sheet, "search_code");
    await userEvent.click(
      sheet.getByRole("button", { name: "Run the selected tool on server GitHub" }),
    );
    const outcome = within(await sheet.findByRole("status", { name: "Result" }));
    await expect(outcome.getByText("Unreachable")).toBeVisible();
    await expect(outcome.getByText("connection refused")).toBeVisible();
    await expect(sheet.getByText("No response was recorded.")).toBeVisible();
  },
};

const bigResponse = probeStory({
  call: () =>
    json({
      ...CALL_OK,
      response: { truncated: true, size_bytes: 2097152, preview: '{"jsonrpc":"2.0"' },
    }),
});
export const TryItTruncatedResponse: Story = {
  render: bigResponse.render,
  play: async ({ canvasElement }) => {
    const sheet = await openTools(canvasElement);
    await pickTool(sheet, "search_code");
    await userEvent.click(
      sheet.getByRole("button", { name: "Run the selected tool on server GitHub" }),
    );
    await expect(await sheet.findByText(/too large to keep in full.*2,097,152 B/)).toBeVisible();
  },
};

// arguments that are not a JSON object never leave the dashboard
const invalidArgs = probeStory();
export const TryItBlocksInvalidArguments: Story = {
  render: invalidArgs.render,
  play: async ({ canvasElement }) => {
    const sheet = await openTools(canvasElement);
    await pickTool(sheet, "search_code");
    const run = sheet.getByRole("button", { name: "Run the selected tool on server GitHub" });
    const args = sheet.getByRole("textbox", { name: "Arguments (JSON object)" });
    await userEvent.clear(args);
    await userEvent.click(args);
    await userEvent.paste('{"query":');
    await expect(await sheet.findByText(/^Not valid JSON: /)).toBeVisible();
    await expect(run).toBeDisabled();
    await userEvent.clear(args);
    await userEvent.paste("[1, 2]");
    await expect(await sheet.findByText(/must be a JSON object/)).toBeVisible();
    await expect(run).toBeDisabled();
    invalidArgs.stub.expectNotSent("POST", "/tools/call");
    // a blank field is no arguments, and runs
    await userEvent.clear(args);
    await waitFor(() => expect(run).toBeEnabled());
    await expect(sheet.queryByText(/must be a JSON object/)).not.toBeInTheDocument();
  },
};

export const TryItNeedsAToolFirst: Story = {
  render: probeStory().render,
  play: async ({ canvasElement }) => {
    const sheet = await openTools(canvasElement);
    await sheet.findByRole("combobox", { name: "Tool" });
    await expect(
      sheet.getByRole("button", { name: "Run the selected tool on server GitHub" }),
    ).toBeDisabled();
  },
};

// a tool outside the manifest is a 400 from the control plane
const refusedCall = probeStory({
  call: () =>
    json({ error: { code: "invalid_field", message: "tool is not in the manifest" } }, 400),
});
export const TryItRefusedByTheApi: Story = {
  render: refusedCall.render,
  play: async ({ canvasElement }) => {
    const sheet = await openTools(canvasElement);
    await pickTool(sheet, "search_code");
    await userEvent.click(
      sheet.getByRole("button", { name: "Run the selected tool on server GitHub" }),
    );
    const alert = await sheet.findByRole("alert");
    await expect(alert).toHaveTextContent("Could not run the tool");
    await expect(alert).toHaveTextContent(/manifest|not valid|invalid/i);
    await expect(sheet.queryByRole("status", { name: "Result" })).not.toBeInTheDocument();
  },
};

const pendingCall = probeStory({ call: () => new Promise<Response>(() => {}) });
export const TryItPending: Story = {
  render: pendingCall.render,
  play: async ({ canvasElement }) => {
    const sheet = await openTools(canvasElement);
    await pickTool(sheet, "search_code");
    await userEvent.click(
      sheet.getByRole("button", { name: "Run the selected tool on server GitHub" }),
    );
    await waitFor(() => expect(sheet.getByText("Running…")).toBeVisible());
    await expect(
      sheet.getByRole("button", { name: "Run the selected tool on server GitHub" }),
    ).toBeDisabled();
  },
};

// a viewer reads the card and the tools, and can neither test nor run anything
export const ProbeGatedForViewer: Story = {
  render: () => (
    <GatedHarness role="viewer" fetchStub={probeApi().stub}>
      <McpCatalog />
    </GatedHarness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Test the connection of server GitHub");
    const sheet = await openTools(canvasElement);
    await sheet.findByRole("list", { name: "Tools" });
    await pickTool(sheet, "search_code");
    await expectRefused(document.body, "Run the selected tool on server GitHub");
  },
};

export const ViewLogsLinksToTheServersCalls: Story = {
  render: probeStory().render,
  play: async ({ canvasElement }) => {
    const link = await within(canvasElement).findByRole("link", {
      name: "View the MCP call logs of server GitHub",
    });
    await expect(link).toHaveAttribute("href", "/mcp-logs?server=github");
  },
};
