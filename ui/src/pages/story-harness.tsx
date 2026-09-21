import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { Toaster } from "@/components/ui/toaster";
import type { RbacEffective, Role } from "@/lib/api";
import { AuthProvider } from "@/lib/auth";
import { CapabilityProvider, useCapabilities } from "@/lib/can";
import en from "@/lib/i18n/locales/en.json";
import { effectiveFor as effectiveFromTable, matrixFixture } from "@/lib/rbac-capabilities";
import { ToastProvider } from "@/lib/toast";
import type { UiEvent } from "@/lib/api";
import { pendingUxEvents, resetUxForTests } from "@/lib/ux";

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

export const ORG = {
  id: "org-1",
  name: "Rolter",
  slug: "rolter",
  created_at: "2026-01-01T00:00:00Z",
};
export const TEAM = {
  id: "team-1",
  org_id: "org-1",
  name: "Platform",
  created_at: "2026-01-01T00:00:00Z",
};
export const PROJECT = {
  id: "project-1",
  team_id: "team-1",
  name: "Gateway",
  created_at: "2026-01-01T00:00:00Z",
};

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
  // the org-wide list `OrgScopePicker` reads, which carries the owning team's
  // name so the picker can group without a request per team (#1357)
  if (/^\/api\/v1\/orgs\/[^/]+\/projects$/.test(path)) {
    return json([{ ...PROJECT, team_name: TEAM.name }]);
  }
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
    globalThis.fetch = (
      role ? withCapabilities(role, fetchStub) : fetchStub
    ) as typeof globalThis.fetch;
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
  const body = role ? (
    <CapabilityProvider>
      <GateProbe />
      {children}
    </CapabilityProvider>
  ) : (
    children
  );
  return <QueryClientProvider client={client}>{body}</QueryClientProvider>;
}

/**
 * The testid of the harness's own gate probe.
 *
 * Exported for `expectAllowed`; a story never queries it directly.
 */
export const GATE_PROBE = "rolter-gate-probe";

/**
 * How far `/api/v1/rbac/effective` got, published into the DOM (#1707).
 *
 * A refusal is visible — the control is disabled and carries a `title` — but
 * being *allowed* looks exactly like not having asked yet, because `undefined`
 * renders as allowed by design. So there is nothing on a permitted control for
 * a play to wait on, and `toBeEnabled()` on it is true from the first paint
 * however long it retries.
 *
 * This is the missing observable, and it is the harness's rather than the
 * dashboard's: no production component learns a test-only attribute. `answered`
 * means the query settled *with a payload*, so a story cannot pass against a
 * control plane that 404s the endpoint and leaves every capability unknown —
 * which is the case a bare enabled assertion is least able to tell apart.
 *
 * The state rides on a data attribute with no text content, so a `getByText`
 * anywhere in the tree cannot match it.
 */
function GateProbe() {
  const value = useCapabilities();
  const state = !value?.resolved ? "pending" : value.effective ? "answered" : "unanswered";
  return <span data-testid={GATE_PROBE} data-gate={state} hidden />;
}

/** The four kinds of caller the gating stories are written for. */
export type StoryRole = Role | "superadmin";

/**
 * What the control plane would answer for a caller holding `role`, derived
 * from its own capability table (#1298).
 *
 * The table used to be copied out by hand here, and the copy drifted: #1258
 * found it calling `model` and `model_price` org-scoped admin resources when
 * both are deployment-wide catalogs a superadmin alone writes, which let two
 * screens gate on capabilities the control plane does not define while their
 * stories passed. `src/lib/rbac-capabilities.ts` derives both payloads from a
 * generated copy of `CAPABILITIES` instead, and a test fails the build when
 * that copy and `crates/rolter-control/src/rbac_matrix.rs` disagree.
 */
export function effectiveFor(role: StoryRole): RbacEffective {
  return role === "superadmin" ? effectiveFromTable(null, true) : effectiveFromTable(role);
}

/** The published rules, which is where a disabled control reads its role from. */
export { matrixFixture };

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
 * The open discard prompt (#1463).
 *
 * Looked up by its accessible name rather than positionally: the editor is
 * still mounted behind it, so there are two `role="dialog"` nodes on the body
 * and `sheet()` cannot tell them apart.
 */
export async function discardPrompt(): Promise<HTMLElement> {
  return within(document.body).findByRole("dialog", { name: /discard unsaved changes/i });
}

/**
 * Answer the discard prompt — `true` throws the draft away, `false` keeps
 * editing. Asserting both answers matters: that "cancel" leaves the draft
 * intact is the half a manual click-through never checks.
 */
export async function answerDiscardPrompt(discard: boolean): Promise<void> {
  const prompt = await discardPrompt();
  await userEvent.click(
    within(prompt).getByRole("button", { name: discard ? "Discard" : "Cancel" }),
  );
  await waitFor(() => expect(prompt).not.toBeInTheDocument());
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
  // re-queried on every poll for the same reason `expectRefused` is: a screen
  // that re-renders between the lookup and the click leaves a captured
  // reference detached, and clicking a node that is no longer in the document
  // fires nothing (#1670)
  const button = await waitFor(() => {
    const found = canvas.getByRole("button", { name });
    expect(found).toBeEnabled();
    return found;
  });
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

/**
 * What a refused control says it would take, read out of the catalog.
 *
 * Built from `rbac.needsRole` rather than written out again, so rewording the
 * refusal cannot leave the gating stories asserting a sentence the dashboard
 * no longer renders.
 */
export const NEEDS_ADMIN = en.rbac.needsRole.replace("{{role}}", en.shell.roles.admin);

/**
 * What a refused deployment-wide control says it would take.
 *
 * A capability the table marks `superadmin_only` names no role at all — there
 * is no grant at an org scope that reaches it — so its refusal reads
 * differently, and a story that asserted the admin sentence there would pass
 * against a gate keyed to the wrong capability.
 */
export const NEEDS_SUPERADMIN = en.rbac.needsSuperadmin;

/** The same sentence for a capability the table opens at `member`. */
export const NEEDS_MEMBER = en.rbac.needsRole.replace("{{role}}", en.shell.roles.member);

/**
 * Assert a gated control is refused, and that it names what would allow it.
 *
 * Both halves matter: `disabled` on its own is the same non-answer the 403
 * was, so the `title` has to carry the role (#1183). The disabled state is
 * awaited rather than asserted at once, because the effective-permissions
 * query is one request behind the first paint and the control renders enabled
 * until it lands.
 *
 * `role` is the ARIA role to look the control up by, as on `expectAllowed` —
 * a gated picker is a `combobox` (#1759).
 */
export async function expectRefused(
  canvasElement: HTMLElement,
  name: RegExp | string,
  reason: string = NEEDS_ADMIN,
  role: string = "button",
): Promise<void> {
  const canvas = within(canvasElement);
  // the control is looked up again on every poll rather than captured once
  // (#1670). a screen re-renders while the gate is still in flight — the scope
  // chain resolving re-keys the query behind it, which sends the screen back to
  // its skeleton for a frame — and React builds a *new* button when it comes
  // back. A captured reference then reports the detached node's attributes
  // forever: `title` stays null however long the assertion waits, so the story
  // fails identically at 50ms of latency and at 900ms, and reads as a gate that
  // never resolved when the live control is refused exactly as it should be
  //
  // both flags in one wait: a control can already be disabled for a reason of
  // its own — an unsaved draft that does not validate yet — so asserting the
  // disabled flag first would pass before the gate has answered and then read
  // a `title` that is still null
  await waitFor(() => {
    // every match, not the first: a screen repeats its primary action in the
    // empty state, and a gate that refused one of the two and not the other
    // would be a gate that leaks
    const buttons = canvas.getAllByRole(role, { name });
    for (const button of buttons) {
      expect(button).toBeDisabled();
      expect(button).toHaveAttribute("title", reason);
    }
  });
}

/**
 * Every sentence a refused control puts in its `title`.
 *
 * Read out of the catalog rather than written out again, and covering all three
 * shapes the gate can produce, so `expectAllowed` cannot call a refusal it does
 * not recognise "allowed".
 */
const REFUSALS = [NEEDS_ADMIN, NEEDS_MEMBER, NEEDS_SUPERADMIN, en.rbac.needsPermission];

/**
 * Assert a gated control is *permitted*, once the gate has actually answered.
 *
 * The mirror of `expectRefused`, and it needs the extra half for a reason
 * (#1707): a refusal is a state change a play can wait for, but being allowed
 * is the state the control is already in — `undefined` is "not known yet" and
 * renders as allowed on purpose — so `await waitFor(() => expect(button)
 * .toBeEnabled())` is satisfied on its first poll, before the request has left.
 * It cannot fail at any latency, which makes it a story that asserts nothing.
 *
 * So the gate is awaited through the harness's own probe: `answered` means the
 * effective-permissions query settled *with a payload*, not merely that it
 * stopped being pending. Only then is the control read — enabled, and carrying
 * none of the refusal sentences, because a control disabled by its own form
 * state would otherwise report as a gate that refused.
 *
 * `role` is the ARIA role to look the control up by, for the toggles and row
 * controls that are not buttons.
 */
export async function expectAllowed(
  canvasElement: HTMLElement,
  name: RegExp | string,
  role: string = "button",
): Promise<void> {
  const canvas = within(canvasElement);
  // looked up again on every poll for the reason `expectRefused` is (#1670): a
  // screen that re-renders while the gate is in flight leaves a captured node
  // detached, and the detached copy answers forever with the attributes it had
  await waitFor(() => {
    const probe = within(document.body).getByTestId(GATE_PROBE);
    expect(probe).toHaveAttribute("data-gate", "answered");
    // every match, not the first: a screen repeats its primary action in the
    // empty state, and half a gate is not an answer
    const controls = canvas.getAllByRole(role, { name });
    for (const control of controls) {
      expect(control).toBeEnabled();
      expect(REFUSALS).not.toContain(control.getAttribute("title"));
    }
  });
}

/** Assert the screen is standing in a skeleton for content it does not have yet. */
export async function expectSkeleton(canvasElement: HTMLElement): Promise<void> {
  const canvas = within(canvasElement);
  await waitFor(() => expect(canvas.getAllByLabelText(LOADING_LABEL).length).toBeGreaterThan(0));
}

/**
 * Assert the skeletons marked with `testId` sit inside a `role="status"`
 * region.
 *
 * `expectSkeleton` only asks whether *something* on the screen is announced as
 * loading, which a second panel elsewhere can satisfy on its own — so it cannot
 * tell a hand-rolled sub-panel of bare `Skeleton`s from a `LoadingRegion`, and
 * `Skeleton` is `aria-hidden` (#1618). This names the panel.
 */
export async function expectInStatusRegion(
  canvasElement: HTMLElement,
  testId: string,
): Promise<void> {
  await waitFor(() => {
    const node = within(canvasElement).getByTestId(testId);
    expect(node.closest('[role="status"]')).not.toBeNull();
  });
}

/**
 * Assert a `LoadError` is on screen and says `says`.
 *
 * `getAllByRole` rather than `getByRole`: a screen can carry a second alert —
 * a failed mutation, a warning banner — and the story should not become
 * order-dependent on that.
 */
export async function expectLoadError(canvasElement: HTMLElement, says: RegExp): Promise<void> {
  const canvas = within(canvasElement);
  // a screen whose query retries before it gives up needs longer than the
  // shared budget in `.storybook/preview.ts` — Logs runs its own retry policy
  // over the shared one
  await waitFor(
    () =>
      expect(canvas.getAllByRole("alert").some((a) => says.test(a.textContent ?? ""))).toBe(true),
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
   *
   * `T` names the shape the story is about to assert on, so a field check reads
   * as `body.client_secret` rather than through a cast at every call site.
   */
  expectSentBody: <T = unknown>(method: string, fragment: string) => Promise<T>;
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
    expectSentBody: async <T,>(method: string, fragment: string): Promise<T> => {
      await waitFor(() => expect(match(method, fragment)?.body).toBeDefined());
      return JSON.parse(match(method, fragment)!.body as string) as T;
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
  await userEvent.click(within(sheet()).getByRole("button", { name: closeLabel }));
  await expectSheetClosed();
  expect(
    within(document.body).queryByRole("dialog", { name: /discard unsaved changes/i }),
  ).toBeNull();
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
    const found = canvas.getAllByRole(role).find((node) => says.test(node.textContent ?? ""));
    expect(found).toBeDefined();
    return found as HTMLElement;
  });
  await waitFor(() => expect(within(region).getByText(says)).toBeVisible());
}

/**
 * Pick an option in a `Combobox`, the way a person does.
 *
 * `userEvent.selectOptions` only drives a native `<select>`; the styled
 * combobox that replaced it (#968) is an input plus a listbox, so a story
 * opens it and clicks the row. `option` is matched on the row's accessible
 * name, which is its label plus any secondary line.
 */
export async function pickOption(combobox: HTMLElement, option: string | RegExp): Promise<void> {
  const listbox = await openOptions(combobox);
  await userEvent.click(within(listbox).getByRole("option", { name: option }));
}

/**
 * Open a `Combobox` and hand back its listbox, for a story that asserts what is
 * *offered* rather than picking one. The options are not inside the control —
 * the listbox is a separate element the combobox points at with `aria-controls`
 * — so `within(combobox).getAllByRole("option")` finds nothing.
 */
export async function openOptions(combobox: HTMLElement): Promise<HTMLElement> {
  await userEvent.click(combobox);
  const listId = combobox.getAttribute("aria-controls");
  const listbox = listId ? document.getElementById(listId) : null;
  if (!listbox) throw new Error("openOptions: the control is not a combobox with a listbox");
  return listbox;
}

// --- the UX event stream ---------------------------------------------------
//
// `EditorSheet` and `ConfirmDialog` emit through `useFormTelemetry` for every
// screen that renders them (#1730), so the assertion that they *do* belongs
// beside them rather than in each of the twenty-eight call sites. The queue in
// `ux.ts` is never flushed under a story — nothing stubs the endpoint — so it
// is readable directly, which is also what makes the negative assertion
// ("cancelling emitted no submit") possible at all.

/**
 * Drop the queue before and after a story, for a `beforeEach` in the meta.
 *
 * Both ends matter: a story that inherited the previous one's events would
 * pass on somebody else's signal, and one that left its own behind would hand
 * that signal to whatever ran next.
 */
export function recordUxEvents(): () => void {
  resetUxForTests();
  return () => resetUxForTests();
}

/** Every event queued so far, in the order it was emitted. */
export function uxEvents(): readonly UiEvent[] {
  return pendingUxEvents();
}

// `target` is omitted for the events that name no form — `time_to_interactive`
// is the screen, not a control on it — so an absent one matches on the action
const matches = (event: UiEvent, action: UiEvent["action"], target?: string) =>
  event.action === action && (target === undefined || event.target === target);

/**
 * Wait for one event and hand it back, so a story can go on to assert the
 * duration or the outcome on it.
 *
 * Matched on `action` and `target` rather than on the whole queue: a screen
 * emits `screen_view` and `time_to_interactive` of its own, and a story that
 * asserted an exact queue would break every time an unrelated emitter was
 * added.
 */
export async function expectUxEvent(action: UiEvent["action"], target?: string): Promise<UiEvent> {
  return waitFor(() => {
    const found = pendingUxEvents().find((e) => matches(e, action, target));
    expect(found).toBeDefined();
    return found as UiEvent;
  });
}

/**
 * Assert no such event was emitted — the half that says a cancel is a cancel.
 * A dialog that emitted `form_submit` on the way out would pass every
 * positive assertion above and still make the dogfood data say the delete went
 * through.
 */
export function expectNoUxEvent(action: UiEvent["action"], target?: string): void {
  expect(pendingUxEvents().find((e) => matches(e, action, target))).toBeUndefined();
}
