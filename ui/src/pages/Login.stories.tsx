import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, within } from "storybook/test";

import Login from "./Login";
import {
  Harness,
  StaleSession,
  expectSkeleton,
  json,
  type FetchStub,
} from "./story-harness";
import { AuthProvider } from "@/lib/auth";
import { withPageA11y } from "@/lib/story-a11y";

/**
 * The login screen used to answer every failure the same way: it signed the
 * user in through the email-only gate, with no session token. A wrong
 * password, a locked account and an SSO-only org were indistinguishable from
 * success until the first `/me/*` call failed for a reason nothing on screen
 * explained. These stories are what stops that coming back (#1160).
 */
const meta: Meta<typeof Login> = {
  title: "Screens/Login",
  component: Login,
  // the signed-out page is a page: no story mounts the shell around it, so it
  // carries its own landmark, <main> and <h1> and is gated on them (#1353)
  parameters: { ...withPageA11y },
};
export default meta;
type Story = StoryObj<typeof Login>;

const METHODS = { password: true, sso: [] };

/** Password login answers `methods`, then `login` fails with `code`. */
function loginFails(
  status: number,
  code: string,
  extra: { retryAfter?: string } = {},
): FetchStub {
  return async (input) => {
    const url = String(input);
    if (url.includes("/auth/methods")) return json(METHODS);
    if (url.includes("/auth/login")) {
      return new Response(
        JSON.stringify({ error: { message: "refused", code } }),
        {
          status,
          headers: {
            "Content-Type": "application/json",
            ...(extra.retryAfter ? { "Retry-After": extra.retryAfter } : {}),
          },
        },
      );
    }
    return json({});
  };
}

// the form starts empty (it once shipped a fake demo account pre-filled), so a
// sign-in has to type an account first — `required` blocks an empty submit
async function signIn(canvasElement: HTMLElement) {
  const canvas = within(canvasElement);
  await userEvent.type(await canvas.findByLabelText(/email/i), "anya@acme.co");
  await userEvent.type(canvas.getByLabelText(/^password/i), "correct-horse");
  await userEvent.click(canvas.getByRole("button", { name: /sign in/i }));
}

export const WrongPassword: Story = {
  render: () => (
    <Harness fetchStub={loginFails(401, "invalid_credentials")}>
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await signIn(canvasElement);
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("alert")).toHaveTextContent(
      /do not match an account/i,
    );
    // and crucially: the form is still on screen. the old code would have
    // navigated away, having "signed in" with no session
    await expect(canvas.getByRole("button", { name: /sign in/i })).toBeVisible();
  },
};

export const OrgRequiresSso: Story = {
  render: () => (
    <Harness fetchStub={loginFails(403, "password_login_disabled")}>
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await signIn(canvasElement);
    await expect(
      await within(canvasElement).findByRole("alert"),
    ).toHaveTextContent(/single sign-on/i);
  },
};

export const LockedOut: Story = {
  render: () => (
    <Harness
      fetchStub={loginFails(429, "too_many_attempts", { retryAfter: "90" })}
    >
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await signIn(canvasElement);
    // the lock is a clock: the wait it carries is rendered, not swallowed
    await expect(
      await within(canvasElement).findByRole("alert"),
    ).toHaveTextContent(/90 seconds/i);
  },
};

export const ControlPlaneUnreachable: Story = {
  render: () => (
    <Harness
      fetchStub={async (input) => {
        if (String(input).includes("/auth/methods")) return json(METHODS);
        throw new TypeError("network error");
      }}
    >
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await signIn(canvasElement);
    await expect(
      await within(canvasElement).findByRole("alert"),
    ).toHaveTextContent(/could not be reached/i);
  },
};

/**
 * The session in localStorage was already dead when the tab reopened, and
 * `/auth/me` said so (#1196). The screen has to explain why it is asking
 * again — a sign-in form that appears with no reason reads like the dashboard
 * lost the session on its own.
 */
export const SessionExpired: Story = {
  render: () => (
    <Harness
      fetchStub={async (input) => {
        const url = String(input);
        if (url.includes("/auth/methods")) return json(METHODS);
        if (url.includes("/auth/me")) {
          return json(
            { error: { message: "missing or invalid session", code: "unauthenticated" } },
            401,
          );
        }
        return json({});
      }}
    >
      <StaleSession>
        <Login />
      </StaleSession>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // by text, not by role: the form's own loading region is a `status` too
    // while /auth/methods is in flight
    await expect(await canvas.findByText(/session expired/i)).toBeVisible();
    // and the way back in is right there, not behind a reload
    await expect(canvas.getByRole("button", { name: /sign in/i })).toBeVisible();
  },
};

/**
 * The counterpart: the control plane refused the *credentials*, not the
 * session. Only one of the two messages may be on screen, or the screen is
 * telling the user two different stories about the same click.
 */
export const ExpiredNoticeYieldsToLoginError: Story = {
  render: () => (
    <Harness
      fetchStub={async (input) => {
        const url = String(input);
        if (url.includes("/auth/methods")) return json(METHODS);
        if (url.includes("/auth/me")) {
          return json(
            { error: { message: "missing or invalid session", code: "unauthenticated" } },
            401,
          );
        }
        if (url.includes("/auth/login")) {
          return json(
            { error: { message: "refused", code: "invalid_credentials" } },
            401,
          );
        }
        return json({});
      }}
    >
      <StaleSession>
        <Login />
      </StaleSession>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/session expired/i)).toBeVisible();
    await signIn(canvasElement);
    await expect(await canvas.findByRole("alert")).toHaveTextContent(
      /do not match an account/i,
    );
    await expect(canvas.queryByRole("status")).toBeNull();
  },
};

/**
 * `/auth/methods` has not answered yet.
 *
 * The screen used to assume password login was on while the answer was in
 * flight, so an SSO-only deployment flashed a password form and then took it
 * away — half the operators who saw it had started typing (#1180). The form is
 * held until the deployment has said what it offers.
 */
export const Loading: Story = {
  render: () => (
    <Harness fetchStub={() => new Promise<Response>(() => {})}>
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectSkeleton(canvasElement);
    await expect(canvas.queryByLabelText(/^password/i)).not.toBeInTheDocument();
    // the branding and the heading are not deployment-dependent, so they stay
    await expect(canvas.getByRole("heading")).toBeVisible();
  },
};

/**
 * The SSO-only deployment the flash was worst on: once the answer lands there
 * is no password field at all, only the identity provider.
 */
export const SsoOnlyNeverShowsAPasswordField: Story = {
  render: () => (
    <Harness
      fetchStub={async (input) =>
        String(input).includes("/auth/methods")
          ? json({
              password: false,
              sso: [{ slug: "okta", name: "Okta", start_url: "/api/v1/auth/sso/okta" }],
            })
          : json({})
      }
    >
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("link", { name: /Okta/ })).toBeVisible();
    await expect(canvas.queryByLabelText(/^password/i)).not.toBeInTheDocument();
  },
};
