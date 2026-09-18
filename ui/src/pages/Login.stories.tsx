import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, within } from "storybook/test";

import Login from "./Login";
import {
  Harness,
  StaleSession,
  expectSkeleton,
  json,
  recording,
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
function loginFails(status: number, code: string, extra: { retryAfter?: string } = {}): FetchStub {
  return async (input) => {
    const url = String(input);
    if (url.includes("/auth/methods")) return json(METHODS);
    if (url.includes("/auth/login")) {
      return new Response(JSON.stringify({ error: { message: "refused", code } }), {
        status,
        headers: {
          "Content-Type": "application/json",
          ...(extra.retryAfter ? { "Retry-After": extra.retryAfter } : {}),
        },
      });
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
    await expect(await canvas.findByRole("alert")).toHaveTextContent(/do not match an account/i);
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
    await expect(await within(canvasElement).findByRole("alert")).toHaveTextContent(
      /single sign-on/i,
    );
  },
};

export const LockedOut: Story = {
  render: () => (
    <Harness fetchStub={loginFails(429, "too_many_attempts", { retryAfter: "90" })}>
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await signIn(canvasElement);
    // the lock is a clock: the wait it carries is rendered, not swallowed
    await expect(await within(canvasElement).findByRole("alert")).toHaveTextContent(/90 seconds/i);
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
    await expect(await within(canvasElement).findByRole("alert")).toHaveTextContent(
      /could not be reached/i,
    );
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
          return json({ error: { message: "refused", code: "invalid_credentials" } }, 401);
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
    await expect(await canvas.findByRole("alert")).toHaveTextContent(/do not match an account/i);
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

// --- the second-factor step (#1078) ---------------------------------------

const CHALLENGE = {
  mfa_required: true,
  // deliberately a word rather than a random-looking string: the secret
  // scanner cannot tell a story fixture from a leaked token, and it is right
  // not to try
  mfa_token: "rolter-mfa-example-token",
  // five minutes out, which is what the control plane issues
  expires_at: new Date(Date.now() + 5 * 60_000).toISOString(),
};

const SESSION = {
  token: "rolter-session-example-token",
  expires_at: new Date(Date.now() + 7 * 24 * 3600_000).toISOString(),
  user: {
    id: "user-1",
    email: "anya@acme.co",
    is_superadmin: false,
    deactivated_at: null,
    created_at: "2026-01-01T00:00:00Z",
  },
};

/**
 * Password right, factor armed: `/auth/login` answers a challenge rather than
 * a session, and `verify` is what mints one.
 *
 * `challenge` decides what the login step answers, so a story can send the
 * user into the second step and then answer the code differently on each
 * attempt — the budget is the part worth testing.
 */
function stepUp(verify: (attempt: number) => Response, challenge = CHALLENGE): FetchStub {
  let attempts = 0;
  return async (input) => {
    const url = String(input);
    if (url.includes("/auth/methods")) return json(METHODS);
    if (url.includes("/auth/mfa/verify")) {
      attempts += 1;
      return verify(attempts);
    }
    if (url.includes("/auth/login")) return json(challenge);
    return json({});
  };
}

const rejected = () =>
  json({ error: { message: "invalid credentials", code: "invalid_credentials" } }, 401);

async function submitCode(canvasElement: HTMLElement, code: string) {
  const canvas = within(canvasElement);
  await userEvent.type(await canvas.findByLabelText(/authentication code/i), code);
  await userEvent.click(canvas.getByRole("button", { name: /^verify$/i }));
}

/**
 * The happy path, and the one the dashboard could not represent at all before
 * this: an org that set `mfa_policy` past `off` used to lock every dashboard
 * user out, because `LoginResponse` modelled only the session branch.
 */
export const SecondFactorChallenge: Story = {
  render: () => (
    <Harness fetchStub={stepUp(() => json(SESSION))}>
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await signIn(canvasElement);
    // the password step is gone: one step at a time, and going back is the
    // button below rather than the field left on screen
    await expect(await canvas.findByLabelText(/authentication code/i)).toBeVisible();
    await expect(canvas.queryByLabelText(/^password/i)).not.toBeInTheDocument();
    // the field takes a recovery code too, and says so — the server tells the
    // two apart by shape, so hard-limiting this to six digits would lock out
    // exactly the user who lost their phone
    await expect(canvas.getByText(/one of your recovery codes/i)).toBeVisible();
    await submitCode(canvasElement, "123456");
  },
};

/** A recovery code goes in the same field, and reaches the same endpoint. */
export const ARecoveryCodeRedeemsTheChallenge: Story = {
  render: () => {
    stepUpCalls = recording(stepUp(() => json(SESSION)));
    return (
      <Harness fetchStub={stepUpCalls.stub}>
        <AuthProvider>
          <Login />
        </AuthProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await signIn(canvasElement);
    await submitCode(canvasElement, "4KJH2Q00ZXRT9WM");
    const body = await stepUpCalls.expectSentBody<{ mfa_token: string; code: string }>(
      "POST",
      "/auth/mfa/verify",
    );
    await expect(body.mfa_token).toBe(CHALLENGE.mfa_token);
    await expect(body.code).toBe("4KJH2Q00ZXRT9WM");
  },
};

let stepUpCalls = recording(stepUp(() => json(SESSION)));

/**
 * A wrong code keeps the challenge, and says how much of the budget is left.
 *
 * The count is kept here because the control plane will not say: a wrong code,
 * an expired challenge and an exhausted one are all `invalid_credentials`, so
 * a guesser cannot tell "keep going" from "start over". The person who
 * mistyped still needs to know.
 */
export const AWrongCodeSpendsOneOfThree: Story = {
  render: () => (
    <Harness fetchStub={stepUp((attempt) => (attempt === 1 ? rejected() : json(SESSION)))}>
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await signIn(canvasElement);
    await submitCode(canvasElement, "000000");
    // one line carries both halves: what happened, and how much of the budget
    // is left. `Field` renders an error instead of its hint, so a count under
    // the field would be hidden by the rejection that changed it
    await expect(await canvas.findByRole("alert")).toHaveTextContent(
      /check the clock on your device.*2 attempts left/i,
    );
    // the field is cleared and still there: the challenge survives a miss
    await expect(canvas.getByLabelText(/authentication code/i)).toHaveValue("");
    await submitCode(canvasElement, "123456");
  },
};

/**
 * The third miss takes the challenge with it. Leaving the prompt up would have
 * the user typing codes into a token that can no longer redeem anything, so
 * the screen goes back to the password step and says why.
 */
export const AnExhaustedChallengeReturnsToThePassword: Story = {
  render: () => (
    <Harness fetchStub={stepUp(() => rejected())}>
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await signIn(canvasElement);
    await submitCode(canvasElement, "000000");
    await submitCode(canvasElement, "111111");
    await submitCode(canvasElement, "222222");
    await expect(await canvas.findByRole("alert")).toHaveTextContent(
      /sign in with your password again/i,
    );
    await expect(canvas.getByLabelText(/^password/i)).toBeVisible();
  },
};

/** An expired challenge is the same dead end, reached by waiting instead. */
export const AnExpiredChallengeReturnsToThePassword: Story = {
  render: () => (
    <Harness
      fetchStub={stepUp(() => json(SESSION), {
        ...CHALLENGE,
        // already dead when it arrives: the timer fires on the next tick
        expires_at: new Date(Date.now() - 1000).toISOString(),
      })}
    >
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await signIn(canvasElement);
    await expect(await canvas.findByRole("alert")).toHaveTextContent(/expired/i);
    await expect(canvas.getByLabelText(/^password/i)).toBeVisible();
  },
};

/**
 * The org requires a factor this account has not armed. The password was
 * right, there is nothing to retype, and the remedy belongs to an
 * administrator — so this must not read like a mistyped password.
 */
export const OrgRequiresAFactorThisAccountLacks: Story = {
  render: () => (
    <Harness fetchStub={loginFails(403, "mfa_enrolment_required")}>
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await signIn(canvasElement);
    const alert = await within(canvasElement).findByRole("alert");
    await expect(alert).toHaveTextContent(/An administrator has to/i);
    await expect(alert).not.toHaveTextContent(/do not match an account/i);
  },
};
