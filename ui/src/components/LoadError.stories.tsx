import type { Meta, StoryObj } from "@storybook/react";
import { expect, fn, userEvent } from "storybook/test";

import { AnalyticsUnavailableError, ApiError } from "@/lib/api";
import { AuthProvider } from "@/lib/auth";

import { LoadError } from "./LoadError";

const meta = {
  title: "Components/LoadError",
  component: LoadError,
  args: { resource: "virtual keys" },
  // the sign-in action only exists when there is a session to sign out of, so
  // the provider has to be present for that branch to be exercised at all
  decorators: [
    (Story) => (
      <AuthProvider>
        <Story />
      </AuthProvider>
    ),
  ],
} satisfies Meta<typeof LoadError>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * The #942 case that motivated #962. Every `/api/v1/me/*` route returned 401
 * while the screen said "Failed to load your keys.", which sent the operator
 * to check key configuration. It must now point at the session instead.
 */
export const Unauthenticated: Story = {
  args: { error: new ApiError("missing bearer token", 401), onRetry: fn() },
  play: async ({ canvas }) => {
    const alert = await canvas.findByRole("alert");
    await expect(alert).toHaveTextContent(/Sign in to see virtual keys/);
    await expect(canvas.getByRole("button", { name: /sign in again/i })).toBeInTheDocument();
    // retrying an expired session just fails again
    await expect(canvas.queryByRole("button", { name: /try again/i })).toBeNull();
  },
};

/** Signed in, wrong role. Retrying cannot help, so it is not offered. */
export const Forbidden: Story = {
  args: { error: new ApiError("insufficient role", 403), onRetry: fn() },
  play: async ({ canvas }) => {
    await expect(await canvas.findByRole("alert")).toHaveTextContent(/do not have access/);
    await expect(canvas.queryByRole("button", { name: /try again/i })).toBeNull();
    await expect(canvas.queryByRole("button", { name: /sign in/i })).toBeNull();
  },
};

/**
 * Looks like a 401 and is emphatically not one — signing in cannot fix a
 * control plane that has no accounts to sign into.
 */
export const OpenModeNoSession: Story = {
  args: {
    error: new ApiError("no session", 401, "open_mode_no_session"),
    onRetry: fn(),
  },
  play: async ({ canvas }) => {
    await expect(await canvas.findByRole("alert")).toHaveTextContent(/ROLTER_ADMIN_TOKEN/);
    await expect(canvas.queryByRole("button", { name: /sign in/i })).toBeNull();
  },
};

/**
 * A store-less control plane does not mount this endpoint at all (#1204):
 * the fix is a database, so neither retry nor sign-in is offered.
 */
export const NoStore: Story = {
  args: {
    error: new ApiError("no such endpoint: /api/v1/orgs", 404, "no_such_endpoint"),
    onRetry: fn(),
  },
  play: async ({ canvas }) => {
    await expect(await canvas.findByRole("alert")).toHaveTextContent(/ROLTER_DATABASE_URL/);
    await expect(canvas.queryByRole("button", { name: /try again/i })).toBeNull();
    await expect(canvas.queryByRole("button", { name: /sign in/i })).toBeNull();
  },
};

/** `fetch` rejected: the request never reached the control plane at all. */
export const Unreachable: Story = {
  args: { error: new TypeError("Failed to fetch"), onRetry: fn() },
  play: async ({ canvas, args }) => {
    const alert = await canvas.findByRole("alert");
    await expect(alert).toHaveTextContent(/Cannot reach the control/);
    // this kind names the resource in the body rather than the title, which is
    // where the raw {{resource}} used to surface (#1362)
    await expect(alert).toHaveTextContent(/The request for virtual keys got no answer/);
    await userEvent.click(canvas.getByRole("button", { name: /try again/i }));
    await expect(args.onRetry).toHaveBeenCalled();
  },
};

/** It answered, and the answer was a failure. Its words are kept. */
export const ServerError: Story = {
  args: { error: new ApiError("database connection pool exhausted", 500), onRetry: fn() },
  play: async ({ canvas }) => {
    const alert = await canvas.findByRole("alert");
    await expect(alert).toHaveTextContent(/failed to return/);
    // the control plane's own message is the thing that was missing before
    await expect(alert).toHaveTextContent(/database connection pool exhausted/);
  },
};

/** An unanticipated status still says something true rather than guessing. */
export const UnknownStatus: Story = {
  args: { error: new ApiError("I'm a teapot", 418), onRetry: fn() },
  play: async ({ canvas }) => {
    await expect(await canvas.findByRole("alert")).toHaveTextContent(/Could not load/);
  },
};

/** No retry handle: the button is simply absent rather than inert. */
export const WithoutRetryHandle: Story = {
  args: { error: new ApiError("boom", 500) },
  play: async ({ canvas }) => {
    await canvas.findByRole("alert");
    await expect(canvas.queryByRole("button", { name: /try again/i })).toBeNull();
  },
};

/**
 * Every kind at once, against one resource noun.
 *
 * Five of the eight bodies carry `{{resource}}` and the title of a sixth does
 * not, so the component has to interpolate both halves — before #1362 it filled
 * only the title and the reader saw the raw `{{resource}}` in the body. Catalog
 * parity cannot see this: the placeholder is in every locale, it was the call
 * site that dropped it. The assertion is therefore on the rendered DOM.
 */
export const EveryKind: Story = {
  args: { error: new ApiError("boom", 500) },
  render: (args) => (
    <div className="flex flex-col gap-3">
      {[
        new ApiError("missing bearer token", 401),
        new ApiError("insufficient role", 403),
        new ApiError("no session", 401, "open_mode_no_session"),
        new ApiError("no such endpoint: /api/v1/orgs", 404, "no_such_endpoint"),
        new AnalyticsUnavailableError("analytics is not configured"),
        new TypeError("Failed to fetch"),
        new ApiError("database connection pool exhausted", 500),
        new ApiError("I'm a teapot", 418),
      ].map((error, i) => (
        <LoadError key={i} {...args} error={error} />
      ))}
    </div>
  ),
  play: async ({ canvas }) => {
    const alerts = await canvas.findAllByRole("alert");
    await expect(alerts).toHaveLength(8);
    for (const alert of alerts) {
      // no unresolved interpolation anywhere on screen, in either half
      await expect(alert.textContent).not.toContain("{{");
      await expect(alert.textContent).not.toContain("}}");
      // and the noun really did land, not just the braces disappear — every
      // kind names it in its title, its body, or both
      await expect(alert).toHaveTextContent(/virtual keys/);
    }
  },
};
