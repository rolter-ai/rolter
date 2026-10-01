import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Login from "./Login";
import {
  Harness,
  StaleSession,
  Toasted,
  expectLoadError,
  expectSkeleton,
  expectToast,
  json,
  recording,
  type FetchStub,
} from "./story-harness";
import { AuthProvider, useAuth } from "@/lib/auth";
import en from "@/lib/i18n/locales/en.json";
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
 * Signed out there is no shell around this page to name the tab, so the page
 * names it itself: its own heading, then the name, lowercase (#2002).
 */
export const DocumentTitle: Story = {
  // blanked first: the tab title outlives a story, and one left behind by the
  // previous story must not pass for this one's
  beforeEach: () => {
    document.title = "";
  },
  render: () => (
    <Harness fetchStub={async () => json(METHODS)}>
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await within(canvasElement).findByRole("heading", { level: 1, name: en.auth.title });
    await waitFor(() => expect(document.title).toBe(`${en.auth.title} · rolter`));
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
  expires_in: 300,
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

/**
 * An expired challenge is the same dead end, reached by waiting instead.
 *
 * The prompt is timed from `expires_in`, so that is the field that says it is
 * already dead. `expires_at` stays in the future on purpose: a screen that
 * still read it would keep the prompt up, and this story would fail.
 */
export const AnExpiredChallengeReturnsToThePassword: Story = {
  render: () => (
    <Harness
      fetchStub={stepUp(() => json(SESSION), {
        ...CHALLENGE,
        // already dead when it arrives: the timer fires on the next tick
        expires_in: 0,
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
 * The org requires a factor, the account has none, and this control plane
 * cannot enrol one because it has no `ROLTER_KEK` to seal the secret with.
 * The password was right, there is nothing to retype, and the remedy belongs
 * to an operator — so this must not read like a mistyped password.
 */
export const NoKeyToEnrolWithIsAnOperatorsProblem: Story = {
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
    await expect(alert).toHaveTextContent(/ROLTER_KEK/);
    await expect(alert).not.toHaveTextContent(/do not match an account/i);
  },
};

// --- enrolling at sign-in (#1852) -----------------------------------------

const ENROLMENT_CHALLENGE = {
  mfa_enrolment_required: true,
  // a word, not a random-looking string, for the same reason as CHALLENGE
  enrolment_token: "rolter-enrol-example-token",
  // ten minutes out, which is what the control plane issues
  expires_at: new Date(Date.now() + 10 * 60_000).toISOString(),
  expires_in: 600,
};

const SETUP_KEY = {
  otpauth_uri:
    "otpauth://totp/rolter:anya@acme.co?secret=JBSWY3DPEHPK3PXP&issuer=rolter&digits=6&period=30",
  secret: "JBSWY3DPEHPK3PXP",
  digits: 6,
  period: 30,
};

const RECOVERY = ["ALPHA-EXAMPLE-01", "BRAVO-EXAMPLE-02", "CHARLIE-EXAMPLE-03"];

const TOKEN_STORAGE_KEY = "rolter.session.token";

/**
 * Password right, org requires a factor, none armed: `/auth/login` answers an
 * enrolment challenge. `enroll` mints the setup key and `confirm` proves it;
 * each can be answered differently so a story can break one step at a time.
 */
function enrolAtSignIn(
  {
    enroll = () => json(SETUP_KEY),
    confirm = () => json({ ...SESSION, recovery_codes: RECOVERY }),
  }: {
    enroll?: () => Response | Promise<Response>;
    confirm?: (attempt: number) => Response;
  } = {},
  challenge = ENROLMENT_CHALLENGE,
): FetchStub {
  let attempts = 0;
  return async (input) => {
    const url = String(input);
    if (url.includes("/auth/methods")) return json(METHODS);
    if (url.includes("/auth/mfa/enroll")) return enroll();
    if (url.includes("/auth/mfa/confirm")) {
      attempts += 1;
      return confirm(attempts);
    }
    if (url.includes("/auth/login")) return json(challenge);
    return json({});
  };
}

let enrolCalls = recording(enrolAtSignIn());

/**
 * The whole path this used to refuse: set up a factor from the sign-in card,
 * save the codes it issued, and only then go in.
 *
 * The session already exists once the code is accepted, but it is held back
 * until the codes are acknowledged — they are shown exactly once, and the
 * dashboard behind the card is the thing that would make them easy to skip.
 */
export const EnrolsASecondFactorAtSignIn: Story = {
  render: () => {
    enrolCalls = recording(enrolAtSignIn());
    return (
      <Harness fetchStub={enrolCalls.stub}>
        <AuthProvider>
          <Login />
        </AuthProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    localStorage.removeItem(TOKEN_STORAGE_KEY);
    const canvas = within(canvasElement);
    await signIn(canvasElement);

    // one step at a time: the password form is gone, the setup is here
    await expect(await canvas.findByText(/set one up to finish signing in/i)).toBeVisible();
    await expect(canvas.queryByLabelText(/^password/i)).not.toBeInTheDocument();
    // the QR and, for a phone with no camera, the same secret as text
    await expect(await canvas.findByText(SETUP_KEY.secret)).toBeInTheDocument();
    const enrolBody = await enrolCalls.expectSentBody<{ enrolment_token: string }>(
      "POST",
      "/auth/mfa/enroll",
    );
    await expect(enrolBody.enrolment_token).toBe(ENROLMENT_CHALLENGE.enrolment_token);

    await userEvent.type(canvas.getByLabelText(/code from the app/i), "123456");
    await userEvent.click(canvas.getByRole("button", { name: /turn on and sign in/i }));
    const confirmBody = await enrolCalls.expectSentBody<{ enrolment_token: string; code: string }>(
      "POST",
      "/auth/mfa/confirm",
    );
    await expect(confirmBody).toEqual({
      enrolment_token: ENROLMENT_CHALLENGE.enrolment_token,
      code: "123456",
    });

    // the codes, once, and the way in stays shut until they are saved
    await expect(await canvas.findByText(/save your recovery codes/i)).toBeVisible();
    await expect(canvas.getByText(new RegExp(RECOVERY[0]))).toBeInTheDocument();
    const go = canvas.getByRole("button", { name: /continue to the dashboard/i });
    await expect(go).toBeDisabled();
    await expect(localStorage.getItem(TOKEN_STORAGE_KEY)).toBeNull();

    await userEvent.click(canvas.getByRole("checkbox", { name: /i have saved these codes/i }));
    await expect(go).toBeEnabled();
    await userEvent.click(go);
    await waitFor(() => expect(localStorage.getItem(TOKEN_STORAGE_KEY)).toBe(SESSION.token));
    localStorage.removeItem(TOKEN_STORAGE_KEY);
  },
};

/**
 * A wrong code keeps the setup on screen — the secret and the challenge are
 * both still good — and says why under the field, in the reader's language
 * rather than the control plane's English 400.
 */
export const AWrongEnrolmentCodeKeepsTheSetup: Story = {
  render: () => (
    <Harness
      fetchStub={enrolAtSignIn({
        confirm: () => json({ error: { message: "that code did not match" } }, 400),
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
    await submitEnrolmentCode(canvasElement, "000000");
    // what happened and how much of the budget is left, in one line
    await expect(
      await canvas.findByText(/check the clock on your device.*4 attempts left/i),
    ).toBeVisible();
    await expect(canvas.getByLabelText(/code from the app/i)).toHaveValue("");
    await expect(canvas.getByText(SETUP_KEY.secret)).toBeInTheDocument();
  },
};

async function submitEnrolmentCode(canvasElement: HTMLElement, code: string) {
  const canvas = within(canvasElement);
  await userEvent.type(await canvas.findByLabelText(/code from the app/i), code);
  await userEvent.click(canvas.getByRole("button", { name: /turn on and sign in/i }));
}

/**
 * The challenge takes five codes. The server's answer to a wrong one does not
 * say how many are left, and its answer to the sixth looks like any dead
 * token — so the card counts, and after the fifth it goes back to the password
 * step and says what to do about the entry already saved in the app, rather
 * than inviting one more code that can only fail.
 */
export const TheLastWrongEnrolmentCodeReturnsToThePassword: Story = {
  render: () => (
    <Harness
      fetchStub={enrolAtSignIn({
        confirm: () => json({ error: { message: "that code did not match" } }, 400),
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
    for (const left of [4, 3, 2, 1]) {
      await submitEnrolmentCode(canvasElement, "000000");
      await expect(
        await canvas.findByText(new RegExp(`${left} attempts? left`, "i")),
      ).toBeVisible();
    }
    await submitEnrolmentCode(canvasElement, "000000");
    const alert = await canvas.findByRole("alert");
    await expect(alert).toHaveTextContent(/last code/i);
    await expect(alert).toHaveTextContent(/delete the entry/i);
    await expect(canvas.getByLabelText(/^password/i)).toBeVisible();
  },
};

/**
 * The token died on the server — spent, or the account finished enrolling in
 * another tab. Nothing typed here can revive it, so the card goes back to the
 * password, which hands out whatever challenge now fits.
 */
export const ADeadEnrolmentTokenReturnsToThePassword: Story = {
  render: () => (
    <Harness fetchStub={enrolAtSignIn({ confirm: () => rejected() })}>
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await signIn(canvasElement);
    await userEvent.type(await canvas.findByLabelText(/code from the app/i), "123456");
    await userEvent.click(canvas.getByRole("button", { name: /turn on and sign in/i }));
    await expect(await canvas.findByRole("alert")).toHaveTextContent(/no longer be used/i);
    await expect(canvas.getByLabelText(/^password/i)).toBeVisible();
  },
};

/**
 * An expired setup prompt is the same dead end, reached by waiting instead.
 * As with the step-up, `expires_in` is what says so and `expires_at` stays in
 * the future, so a card still reading the absolute time would fail this.
 */
export const AnExpiredEnrolmentReturnsToThePassword: Story = {
  render: () => (
    <Harness fetchStub={enrolAtSignIn({}, { ...ENROLMENT_CHALLENGE, expires_in: 0 })}>
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await signIn(canvasElement);
    await expect(await canvas.findByRole("alert")).toHaveTextContent(/setup prompt expired/i);
    await expect(canvas.getByLabelText(/^password/i)).toBeVisible();
  },
};

/**
 * A laptop clock that runs fast must not expire the prompt on arrival.
 *
 * Here the browser believes `expires_at` passed a quarter of an hour ago — the
 * server's clock and this one disagree — while the server says the prompt has
 * its full ten minutes. Compared with the local clock, the card would send the
 * member back before the setup key could even load, every sign-in, and under
 * a `required_*` policy they would never get in.
 */
export const AFastBrowserClockKeepsTheSetupPrompt: Story = {
  render: () => (
    <Harness
      fetchStub={enrolAtSignIn(
        {},
        { ...ENROLMENT_CHALLENGE, expires_at: new Date(Date.now() - 15 * 60_000).toISOString() },
      )}
    >
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await signIn(canvasElement);
    // the setup key only arrives after the card has mounted and fetched it, so
    // a timer read off `expires_at` would already have fired by now
    await expect(await canvas.findByText(SETUP_KEY.secret)).toBeInTheDocument();
    await expect(canvas.getByLabelText(/code from the app/i)).toBeVisible();
    await expect(canvas.queryByLabelText(/^password/i)).not.toBeInTheDocument();
  },
};

/** The setup key is on its way: a skeleton stands where the QR will be. */
export const TheSetupKeyIsLoading: Story = {
  render: () => (
    <Harness fetchStub={enrolAtSignIn({ enroll: () => new Promise<Response>(() => {}) })}>
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await signIn(canvasElement);
    await expectSkeleton(canvasElement);
    // nothing to prove yet, so nothing to submit
    await expect(
      within(canvasElement).getByRole("button", { name: /turn on and sign in/i }),
    ).toBeDisabled();
  },
};

/**
 * Minting the key failed for a reason a retry can fix. The challenge is still
 * good, so the card offers the retry rather than sending the user back.
 */
export const TheSetupKeyFailsToLoad: Story = {
  render: () => (
    <Harness
      fetchStub={enrolAtSignIn({
        enroll: () => json({ error: { message: "database unavailable" } }, 503),
      })}
    >
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await signIn(canvasElement);
    await expectLoadError(canvasElement, /setup key/i);
    await expect(within(canvasElement).queryByLabelText(/^password/i)).not.toBeInTheDocument();
  },
};

/**
 * The org announced the requirement ahead of time. Until the date the
 * password is enough, and the one moment every bound member passes through —
 * this one — says by when.
 */
export const AnAnnouncedRequirementIsSaidOnTheWayIn: Story = {
  render: () => (
    <Harness
      fetchStub={async (input) => {
        const url = String(input);
        if (url.includes("/auth/methods")) return json(METHODS);
        if (url.includes("/auth/login")) {
          return json({ ...SESSION, mfa_enrol_by: "2026-10-03T12:00:00Z" });
        }
        return json({});
      }}
    >
      <Toasted>
        <AuthProvider>
          <Login />
        </AuthProvider>
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await signIn(canvasElement);
    await expectToast(canvasElement, /required from/i);
    localStorage.removeItem(TOKEN_STORAGE_KEY);
  },
};

// --- ending a browser sign-in (#2411) -------------------------------------
//
// the callback redirects to `/login?sso_code=…` or `/login?sso_error=…`; the
// stories put that query on the iframe's own address and restore it after

// a session left in storage by an earlier story would read as signed in
function clearSession() {
  for (const key of ["token", "email", "user"]) localStorage.removeItem(`rolter.session.${key}`);
}

function atQuery(query: string) {
  return () => {
    const original = window.location.search;
    const params = new URLSearchParams(original);
    for (const [k, v] of new URLSearchParams(query)) params.set(k, v);
    window.history.replaceState(null, "", `?${params.toString()}`);
    clearSession();
    return () => {
      window.history.replaceState(null, "", original || window.location.pathname);
      clearSession();
    };
  };
}

/** what the shell would render once the session is stored */
function SignedInProbe({ children }: { children: React.ReactNode }) {
  const { email } = useAuth();
  // a page of its own, as the shell would be: landmark and heading (#1353)
  return email ? (
    <main>
      <h1>shell</h1>
      <p role="status">signed in as {email}</p>
    </main>
  ) : (
    <>{children}</>
  );
}

const SSO_METHODS = {
  password: true,
  sso: [{ slug: "okta", name: "Okta", start_url: "/auth/sso/okta/start" }],
};

function ssoStub(exchange: () => Response): FetchStub {
  return async (input) => {
    const url = String(input);
    if (url.includes("/auth/methods")) return json(SSO_METHODS);
    if (url.includes("/auth/sso/exchange")) return exchange();
    return json({});
  };
}

let exchangeCalls = recording(ssoStub(() => json({})));
// delegates to whichever recorder is current when the call lands: a render pass
// can run before `beforeEach` has made this run's recorder, and twice
const viaExchangeCalls: FetchStub = (input, init) => exchangeCalls.stub(input, init);

/** The code is redeemed, the session stored, the address bar cleaned. */
export const SsoCodeSignsIn: Story = {
  beforeEach: () => {
    // made once per run, not in `render`, which a render pass may call twice
    exchangeCalls = recording(ssoStub(() => json({ ...SESSION, granted_roles: [] })));
    return atQuery("sso_code=one-time-example-code")();
  },
  render: () => {
    return (
      <Harness fetchStub={viaExchangeCalls}>
        <AuthProvider>
          <SignedInProbe>
            <Login />
          </SignedInProbe>
        </AuthProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await expect(await within(canvasElement).findByText(/signed in as anya@acme.co/)).toBeVisible();
    const body = await exchangeCalls.expectSentBody<{ code: string }>("POST", "/auth/sso/exchange");
    await expect(body.code).toBe("one-time-example-code");
    await expect(localStorage.getItem(TOKEN_STORAGE_KEY)).toBe(SESSION.token);
    await expect(window.location.search).toBe("");
  },
};

/**
 * The code is single use, so the exchange must run once even though
 * `React.StrictMode` runs the screen's effects twice. The screen mounts in a
 * later commit, under a StrictMode that is already there (#1744).
 */
function LateLogin() {
  const [mounted, setMounted] = React.useState(false);
  return (
    <>
      <button onClick={() => setMounted(true)}>open the login</button>
      {mounted && (
        <AuthProvider>
          <SignedInProbe>
            <Login />
          </SignedInProbe>
        </AuthProvider>
      )}
    </>
  );
}

export const SsoCodeIsRedeemedOnceUnderStrictMode: Story = {
  beforeEach: () => {
    exchangeCalls = recording(ssoStub(() => json({ ...SESSION, granted_roles: [] })));
    return atQuery("sso_code=one-time-example-code")();
  },
  render: () => {
    return (
      <React.StrictMode>
        <Harness fetchStub={viaExchangeCalls}>
          <LateLogin />
        </Harness>
      </React.StrictMode>
    );
  },
  play: async ({ canvasElement }) => {
    await userEvent.click(within(canvasElement).getByRole("button", { name: "open the login" }));
    // the sign-in landing can only follow the remount, so the count is read after it
    await expect(await within(canvasElement).findByText(/signed in as/)).toBeVisible();
    await expect(
      exchangeCalls.calls.filter((c) => c.url.includes("/auth/sso/exchange")),
    ).toHaveLength(1);
  },
};

/** A spent or expired code says so, keeps the form, and signs nobody in. */
export const SsoCodeRefused: Story = {
  beforeEach: atQuery("sso_code=spent-example-code"),
  render: () => (
    <Harness
      fetchStub={ssoStub(() =>
        json({ error: { message: "invalid", code: "invalid_exchange_code" } }, 400),
      )}
    >
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("alert")).toHaveTextContent(/expired before it finished/i);
    await expect(canvas.getByRole("button", { name: /sign in/i })).toBeVisible();
    await expect(localStorage.getItem(TOKEN_STORAGE_KEY)).toBeNull();
    await expect(window.location.search).toBe("");
  },
};

/** The exchange could not be reached: the form stays and the outage is named. */
export const SsoCodeExchangeUnreachable: Story = {
  beforeEach: atQuery("sso_code=any-example-code"),
  render: () => (
    <Harness
      fetchStub={async (input) => {
        if (String(input).includes("/auth/methods")) return json(SSO_METHODS);
        throw new TypeError("network error");
      }}
    >
      <AuthProvider>
        <Login />
      </AuthProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expect(await within(canvasElement).findByRole("alert")).toHaveTextContent(
      /could not be reached/i,
    );
  },
};

const SSO_MESSAGES: Record<string, RegExp> = {
  idp_error: /declined the sign-in/i,
  state_expired: /took too long or was already used/i,
  sso_disabled: /turned off for this organization/i,
  no_mapped_group: /not in any group/i,
  account_deactivated: /has been deactivated/i,
  idp_verification_failed: /could not be reached or its response/i,
  not_configured: /cannot finish a single sign-on/i,
  internal_error: /failed on our side/i,
};

const refusalRender: Story["render"] = () => (
  <Harness fetchStub={ssoStub(() => json({}))}>
    <AuthProvider>
      <Login />
    </AuthProvider>
  </Harness>
);

/** the message, the cleaned address bar, and the form still there to try again */
function refusalPlay(message: RegExp, provider?: string): Story["play"] {
  return async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("alert")).toHaveTextContent(message);
    if (provider) await expect(await canvas.findByText(provider)).toBeVisible();
    await expect(window.location.search).toBe("");
    await expect(await canvas.findByLabelText(/^password/i)).toBeVisible();
  };
}

export const SsoErrorIdpError: Story = {
  beforeEach: atQuery("sso_error=idp_error"),
  render: refusalRender,
  play: refusalPlay(SSO_MESSAGES.idp_error),
};
export const SsoErrorStateExpired: Story = {
  beforeEach: atQuery("sso_error=state_expired"),
  render: refusalRender,
  play: refusalPlay(SSO_MESSAGES.state_expired),
};
export const SsoErrorDisabled: Story = {
  beforeEach: atQuery("sso_error=sso_disabled"),
  render: refusalRender,
  play: refusalPlay(SSO_MESSAGES.sso_disabled),
};
export const SsoErrorNoMappedGroup: Story = {
  beforeEach: atQuery("sso_error=no_mapped_group&sso=okta"),
  render: refusalRender,
  play: refusalPlay(SSO_MESSAGES.no_mapped_group, "Identity provider: Okta"),
};
export const SsoErrorAccountDeactivated: Story = {
  beforeEach: atQuery("sso_error=account_deactivated"),
  render: refusalRender,
  play: refusalPlay(SSO_MESSAGES.account_deactivated),
};
export const SsoErrorVerificationFailed: Story = {
  beforeEach: atQuery("sso_error=idp_verification_failed"),
  render: refusalRender,
  play: refusalPlay(SSO_MESSAGES.idp_verification_failed),
};
export const SsoErrorNotConfigured: Story = {
  beforeEach: atQuery("sso_error=not_configured"),
  render: refusalRender,
  play: refusalPlay(SSO_MESSAGES.not_configured),
};
export const SsoErrorInternal: Story = {
  beforeEach: atQuery("sso_error=internal_error"),
  render: refusalRender,
  play: refusalPlay(SSO_MESSAGES.internal_error),
};

/** A code this dashboard has no wording for falls back to the internal error. */
export const SsoErrorUnknownCode: Story = {
  beforeEach: atQuery("sso_error=something_new"),
  render: refusalRender,
  play: refusalPlay(SSO_MESSAGES.internal_error),
};

/** The slug is looked up, never printed: one methods does not list is ignored. */
export const SsoErrorUnknownProviderIsNotRendered: Story = {
  beforeEach: atQuery("sso_error=idp_error&sso=%3Cb%3Eevil%3C%2Fb%3E"),
  render: refusalRender,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("alert")).toHaveTextContent(SSO_MESSAGES.idp_error);
    await expect(canvas.queryByText(/evil/)).toBeNull();
    await expect(canvas.queryByText(/identity provider:/i)).toBeNull();
  },
};
