import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { Toaster } from "@/components/ui/toaster";
import type {
  RbacAction,
  RbacActionView,
  RbacEffective,
  RbacMatrix,
  Role,
} from "@/lib/api";
import { AuthProvider } from "@/lib/auth";
import { CapabilityProvider } from "@/lib/can";
import en from "@/lib/i18n/locales/en.json";
import { ToastProvider } from "@/lib/toast";

// Shared fetch-stub harness for screen stories (#879).
//
// `Plugins.stories.tsx` established the shape — swap `globalThis.fetch`, clear
// the persisted scope, render the screen under a fresh QueryClient. Five more
// screens needed the same thing, and five hand-copied harnesses would be five
// places for the scope fixture to drift. This is that harness, factored out.
//
// Not a `.stories.tsx` file itself, so Storybook does not try to render it as a
// screen and `check:literals` does not scan it for copy.

export type FetchStub = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

export const ORG = { id: "org-1", name: "Rolter", slug: "rolter", created_at: "2026-01-01T00:00:00Z" };
export const TEAM = { id: "team-1", org_id: "org-1", name: "Platform", created_at: "2026-01-01T00:00:00Z" };
export const PROJECT = { id: "project-1", team_id: "team-1", name: "Gateway", created_at: "2026-01-01T00:00:00Z" };

// 204/205/304 may not carry a body: the Response constructor rejects one
// outright, and a stub that throws turns a story's success path into its
// failure path while every url-only assertion still passes (#1197)
const NO_BODY = new Set([204, 205, 304]);

export const json = (body: unknown, status = 200) =>
  new Response(NO_BODY.has(status) ? null : JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

/**
 * The org/team/project chain every scoped screen resolves before it can load.
 *
 * Matched on the whole path rather than on a fragment: a screen's own endpoint
 * often *contains* one of these segments — `/api/v1/projects/{id}/virtual-keys`
 * is the obvious one — and a substring match would answer it with the project
 * list, leaving the screen rendering scope rows and the story asserting nothing
 * it meant to.
 */
export const scopeResponse = (url: string): Response | null => {
  const path = new URL(url, "http://localhost").pathname;
  if (path === "/api/v1/orgs") return json([ORG]);
  if (/^\/api\/v1\/orgs\/[^/]+\/teams$/.test(path)) return json([TEAM]);
  if (/^\/api\/v1\/teams\/[^/]+\/projects$/.test(path)) return json([PROJECT]);
  return null;
};

/** A stub that resolves the scope and then answers `handler`. */
export function scoped(handler: FetchStub): FetchStub {
  return async (input, init) => scopeResponse(String(input)) ?? handler(input, init);
}

/** A stub that never settles, for the loading state. */
export const pending: FetchStub = scoped(() => new Promise<Response>(() => {}));

/**
 * Route by URL fragment. Entries are matched in order, so a longer path can be
 * listed before the prefix it shares.
 */
export function routes(table: [string, () => unknown][], status = 200): FetchStub {
  return scoped(async (input) => {
    const url = String(input);
    for (const [fragment, body] of table) {
      if (url.includes(fragment)) return json(body(), status);
    }
    return json([], status);
  });
}

export function Harness({
  fetchStub,
  role,
  children,
}: {
  fetchStub: FetchStub;
  /**
   * Answer `GET /api/v1/rbac/effective` as this role and mount the screen
   * under a `CapabilityProvider` (#1183).
   *
   * Omitted, nothing is stubbed and no provider is mounted — which is the
   * un-gated case every other story asserts, because `can()` with no provider
   * above it says "unknown" and every control renders enabled.
   */
  role?: StoryRole;
  children: React.ReactNode;
}) {
  const original = React.useRef<typeof globalThis.fetch | null>(null);
  const client = React.useMemo(() => {
    original.current ??= globalThis.fetch;
    localStorage.removeItem("rolter.scope");
    globalThis.fetch = (role ? withCapabilities(role, fetchStub) : fetchStub) as typeof globalThis.fetch;
    // no retries: a story asserting an error state should not wait out a
    // backoff schedule before the screen admits the request failed
    return new QueryClient({ defaultOptions: { queries: { retry: false } } });
  }, [fetchStub, role]);
  React.useEffect(
    () => () => {
      if (original.current) globalThis.fetch = original.current;
    },
    [],
  );
  const body = role ? <CapabilityProvider>{children}</CapabilityProvider> : children;
  return <QueryClientProvider client={client}>{body}</QueryClientProvider>;
}

/** The four kinds of caller the gating stories are written for. */
export type StoryRole = Role | "superadmin";

// the capability table, as much of it as the stories need
// (crates/rolter-control/src/rbac_matrix.rs). read is a viewer's and mutations
// are an admin's for everything with a tenancy scope; everything without one is
// the superadmin's alone
const SCOPED_RESOURCES = [
  "org",
  "team",
  "project",
  "provider",
  "provider_group",
  "plugin",
  "route",
  "virtual_key",
  "budget",
  "rate_limit",
  "business_unit",
  "customer",
  "prompt_template",
  "skill",
  "user",
  "membership",
  "custom_role",
  "access_profile",
  "access_profile_assignment",
  "mcp_server",
  "mcp_tool_group",
  "mcp_settings",
  "mcp_oauth_grant",
  "mcp_oauth_session",
];

// the two deployment-wide catalogs, which are neither scoped nor an admin's:
// the effective model list and the pricing table carry no tenant's data, so
// every authenticated caller reads them, and there is no deployment membership
// a role floor could be measured against, so every mutation is the
// superadmin's alone (crates/rolter-control/src/rbac_matrix.rs). the actions
// they do not have — a price is upserted, never created; a model row is only
// ever deleted — are absent here for the same reason they are absent there
const CATALOG_ACTIONS: Record<string, RbacAction[]> = {
  model: ["delete"],
  model_price: ["update", "delete"],
};

// an admin read: the rows name the IdPs and the invitations, not just the data
const ADMIN_RESOURCES = [
  "scim_token",
  "scim_group_mapping",
  "audit_log",
  "invitation",
  "sso_provider",
  "sso_group_mapping",
  "org_auth_policy",
  "mcp_oauth_client",
];

const DEPLOYMENT_RESOURCES = [
  "feature_flags",
  "runtime_policy",
  "logging_settings",
  "compatibility_policy",
  "client_settings",
  "model_defaults",
  "adaptive_routing_policy",
  "adaptive_routing_telemetry",
  "guardrail_rule",
  "guardrail_provider",
  "cluster_node",
  "security_settings",
  "connector",
  "alert_channel",
  "alert_rule",
  "alert_history",
  "mcp_log",
];

const ACTIONS: RbacAction[] = ["read", "create", "update", "delete"];

/** What the control plane would answer for a caller holding `role`. */
export function effectiveFor(role: StoryRole): RbacEffective {
  const allowed: string[] = [];
  if (role !== "superadmin") {
    for (const resource of SCOPED_RESOURCES) {
      allowed.push(`${resource}:read`);
      if (role === "admin") {
        for (const action of ACTIONS) allowed.push(`${resource}:${action}`);
      }
    }
    // the catalogs are every authenticated caller's read, an admin's included,
    // and nobody's write short of a superadmin
    for (const resource of Object.keys(CATALOG_ACTIONS)) allowed.push(`${resource}:read`);
    // a key a member mints for themself, which is the one create a non-admin has
    if (role !== "viewer") allowed.push("my_virtual_key:create");
    if (role === "admin") {
      for (const resource of ADMIN_RESOURCES) {
        for (const action of ACTIONS) allowed.push(`${resource}:${action}`);
      }
    }
  }
  return {
    superadmin: role === "superadmin",
    role: role === "superadmin" ? "admin" : role,
    // a superadmin's list is empty on the wire too: `decide` short-circuits on
    // the flag rather than enumerating every pair
    allowed: role === "superadmin" ? [] : allowed,
    custom_roles: [],
    model_policy: null,
  };
}

/** The published rules, which is where a disabled control reads its role from. */
export function matrixFixture(): RbacMatrix {
  const actions = (minimum: Role | null): RbacActionView[] =>
    ACTIONS.map((action) => ({
      action,
      minimum_role: minimum === null ? null : action === "read" ? "viewer" : minimum,
      superadmin_only: minimum === null,
      authenticated_only: false,
    }));
  return {
    roles: [
      { role: "viewer", rank: 0 },
      { role: "member", rank: 1 },
      { role: "admin", rank: 2 },
    ],
    resources: [
      ...SCOPED_RESOURCES.map((resource) => ({
        resource,
        scope: "org",
        actions: actions("admin"),
      })),
      ...ADMIN_RESOURCES.map((resource) => ({
        resource,
        scope: "org",
        actions: actions("admin"),
      })),
      ...Object.entries(CATALOG_ACTIONS).map(([resource, mutations]) => ({
        resource,
        scope: "deployment",
        actions: [
          {
            action: "read" as RbacAction,
            minimum_role: null,
            superadmin_only: false,
            authenticated_only: true,
          },
          ...mutations.map((action) => ({
            action,
            minimum_role: null,
            superadmin_only: true,
            authenticated_only: false,
          })),
        ],
      })),
      ...DEPLOYMENT_RESOURCES.map((resource) => ({
        resource,
        scope: "deployment",
        actions: actions(null),
      })),
    ],
    custom_roles: [],
  };
}

/** Answer the two RBAC endpoints as `role`, then fall through to `handler`. */
export function withCapabilities(role: StoryRole, handler: FetchStub): FetchStub {
  return async (input, init) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (path === "/api/v1/rbac/effective") return json(effectiveFor(role));
    if (path === "/api/v1/rbac/matrix") return json(matrixFixture());
    return handler(input, init);
  };
}

/**
 * An `AuthProvider` that boots with a session token already in localStorage,
 * the way a reloaded tab does (#1196).
 *
 * The token is written during render, before the provider mounts and reads it,
 * and the whole session is cleared again on unmount so a story that leaves a
 * dead token behind cannot change what the next story sees.
 */
export function StaleSession({
  token = "stale-session-token",
  email = "anya@acme.co",
  children,
}: {
  token?: string;
  email?: string;
  children: React.ReactNode;
}) {
  React.useState(() => {
    localStorage.setItem("rolter.session.token", token);
    localStorage.setItem("rolter.session.email", email);
    return null;
  });
  React.useEffect(
    () => () => {
      localStorage.removeItem("rolter.session.token");
      localStorage.removeItem("rolter.session.email");
      localStorage.removeItem("rolter.session.user");
    },
    [],
  );
  return <AuthProvider>{children}</AuthProvider>;
}

/**
 * Run `body` with `window.confirm` answering `answer`, then restore it.
 *
 * The editor sheets guard discarding a dirty draft with `window.confirm`, which
 * is a real modal in a browser and would hang the test runner. Stubbing it is
 * also the only way to assert *both* answers — that "cancel" keeps the sheet
 * open is the half a manual click-through never checks.
 */
export async function withConfirm(answer: boolean, body: () => Promise<void>): Promise<void> {
  const original = window.confirm;
  window.confirm = () => answer;
  try {
    await body();
  } finally {
    window.confirm = original;
  }
}

/**
 * Click a button once it is actually clickable.
 *
 * `findByRole` waits for the element to *exist*, not to be enabled, and most of
 * these screens disable their primary action until the org/team/project chain
 * has resolved — three sequential requests. Clicking in between throws
 * `pointer-events: none`, which reads like a layout bug and is really a race.
 */
export async function clickWhenEnabled(
  container: HTMLElement,
  name: RegExp | string,
): Promise<void> {
  const canvas = within(container);
  const button = await canvas.findByRole("button", { name });
  await waitFor(() => expect(button).toBeEnabled());
  await userEvent.click(button);
}

/**
 * The label every shared skeleton shape carries (#1180).
 *
 * Asserting on the label rather than a class name keeps the story tied to what
 * a screen reader is told, which is the part that has to stay true. Read out
 * of the catalog rather than written out again, so rewording the copy cannot
 * leave the stories asserting a string the dashboard no longer renders.
 */
export const LOADING_LABEL = en.common.loading;

/** Assert the screen is standing in a skeleton for content it does not have yet. */
export async function expectSkeleton(canvasElement: HTMLElement): Promise<void> {
  const canvas = within(canvasElement);
  await waitFor(() =>
    expect(canvas.getAllByLabelText(LOADING_LABEL).length).toBeGreaterThan(0),
  );
}

/**
 * Assert a `LoadError` is on screen and says `says`.
 *
 * `getAllByRole` rather than `getByRole`: a screen can carry a second alert —
 * a failed mutation, a warning banner — and the story should not become
 * order-dependent on that.
 */
export async function expectLoadError(
  canvasElement: HTMLElement,
  says: RegExp,
): Promise<void> {
  const canvas = within(canvasElement);
  // a screen whose query retries before it gives up needs longer than the
  // 1s default — Logs runs its own retry policy over the shared one
  await waitFor(
    () =>
      expect(
        canvas.getAllByRole("alert").some((a) => says.test(a.textContent ?? "")),
      ).toBe(true),
    { timeout: 6000 },
  );
}

/** The `forbidden` LoadError, which is what a non-superadmin gets. */
export async function expectForbidden(canvasElement: HTMLElement): Promise<void> {
  await expectLoadError(canvasElement, /You do not have access to/);
}

/**
 * Assert an empty state with `title` is on screen, and that it offers `cta`.
 *
 * A placeholder that names nothing to do next is half an empty state — the CTA
 * is the half #1180 was filed over.
 */
export async function expectEmptyState(
  canvasElement: HTMLElement,
  title: RegExp,
  cta?: RegExp,
): Promise<void> {
  const canvas = within(canvasElement);
  await waitFor(() => expect(canvas.getByText(title)).toBeVisible());
  // `getAll`: most screens carry the same action in the toolbar as well, and
  // the placeholder repeating it there is the point, not a duplicate
  if (cta) await expect(canvas.getAllByRole("button", { name: cta }).length).toBeGreaterThan(0);
}

/** The open editor sheet. Sheets portal to the body, not into the canvas. */
export function sheet(): HTMLElement {
  return within(document.body).getByRole("dialog");
}

/**
 * Every request the stub was given, so a story can assert the DELETE actually
 * left rather than that a row vanished from a fixture it controls (#1179).
 */
export interface RecordedCall {
  method: string;
  url: string;
  /** the request body as sent, when there was one */
  body?: string;
}

export interface Recorder {
  stub: FetchStub;
  calls: RecordedCall[];
  /** wait for a request with `method` whose URL contains `fragment` */
  expectSent: (method: string, fragment: string) => Promise<void>;
  /** assert none was sent — the half a "cancel" test exists to check */
  expectNotSent: (method: string, fragment: string) => void;
  /**
   * Wait for a matching request and return its parsed JSON body.
   *
   * Asserting the *body* is the difference between "the screen sent a PUT" and
   * "the screen sent the attribution the operator picked": a mutation that
   * fires with the wrong payload passes every url-only assertion (#1193).
   */
  expectSentBody: (method: string, fragment: string) => Promise<unknown>;
}

export function recording(handler: FetchStub): Recorder {
  const calls: RecordedCall[] = [];
  const match = (method: string, fragment: string) =>
    calls.find((c) => c.method === method && c.url.includes(fragment));
  return {
    stub: async (input, init) => {
      calls.push({
        method: (init?.method ?? "GET").toUpperCase(),
        url: String(input),
        body: typeof init?.body === "string" ? init.body : undefined,
      });
      return handler(input, init);
    },
    calls,
    expectSent: async (method, fragment) => {
      await waitFor(() => expect(match(method, fragment)).toBeDefined());
    },
    expectNotSent: (method, fragment) => {
      expect(match(method, fragment)).toBeUndefined();
    },
    expectSentBody: async (method, fragment) => {
      await waitFor(() => expect(match(method, fragment)?.body).toBeDefined());
      return JSON.parse(match(method, fragment)!.body as string) as unknown;
    },
  };
}

/**
 * The open confirmation. Like every dialog it portals onto the body, so it is
 * never in `canvasElement`.
 */
export async function confirmation(): Promise<HTMLElement> {
  return within(document.body).findByRole("dialog");
}

/**
 * Confirm a destructive action, checking the dialog names the thing first.
 *
 * Naming is the whole point of #1179: a confirmation that says "Are you sure?"
 * is a click-through, not a decision, so the story asserts the item's own name
 * is on screen before it presses the button.
 */
export async function confirmDestructive(
  names: RegExp | string,
  confirmLabel: RegExp | string,
): Promise<void> {
  const dialog = await confirmation();
  await expect(within(dialog).getByText(names)).toBeInTheDocument();
  const button = within(dialog).getByRole("button", { name: confirmLabel });
  await waitFor(() => expect(button).toBeEnabled());
  await userEvent.click(button);
}

/** Dismiss the open confirmation without running the action. */
export async function cancelConfirmation(): Promise<void> {
  const dialog = await confirmation();
  await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
  await expectSheetClosed();
}

/** Assert the sheet closed. */
export async function expectSheetClosed(): Promise<void> {
  await waitFor(() => expect(within(document.body).queryByRole("dialog")).not.toBeInTheDocument());
}

/**
 * Close the sheet and assert it did *not* ask to discard — the untouched-draft
 * case. A confirm on a form nobody edited trains people to click through the
 * one that matters.
 */
export async function expectClosesWithoutPrompting(closeLabel = "Cancel"): Promise<void> {
  let asked = false;
  const original = window.confirm;
  window.confirm = () => {
    asked = true;
    return true;
  };
  try {
    await userEvent.click(within(sheet()).getByRole("button", { name: closeLabel }));
    await expectSheetClosed();
    expect(asked).toBe(false);
  } finally {
    window.confirm = original;
  }
}

/**
 * A screen under the shell's toast queue (#1197).
 *
 * `useToast()` is a no-op outside a provider, so a screen story renders
 * perfectly well without this — which is exactly why a story that means to
 * assert the outcome has to opt in. It is not folded into `Harness` because
 * the `Toaster` contributes its own `role="status"` and `role="alert"`
 * regions, and the stories that already query those by role would stop being
 * able to.
 */
export function Toasted({ children }: { children: React.ReactNode }) {
  return (
    <ToastProvider>
      {children}
      <Toaster />
    </ToastProvider>
  );
}

/**
 * Assert the toast queue is announcing `says`.
 *
 * `tone` picks the live region: a success is polite (`status`), a failure
 * assertive (`alert`). The card fades in, so visibility is awaited rather than
 * asserted at once — and the region is looked up among all of them, because a
 * screen carries live regions of its own that have nothing to do with this.
 */
export async function expectToast(
  canvasElement: HTMLElement,
  says: RegExp,
  tone: "success" | "error" = "success",
): Promise<void> {
  const canvas = within(canvasElement);
  const role = tone === "error" ? "alert" : "status";
  const region = await waitFor(() => {
    const found = canvas
      .getAllByRole(role)
      .find((node) => says.test(node.textContent ?? ""));
    expect(found).toBeDefined();
    return found as HTMLElement;
  });
  await waitFor(() => expect(within(region).getByText(says)).toBeVisible());
}
