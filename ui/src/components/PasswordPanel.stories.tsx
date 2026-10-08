import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { PasswordPanel } from "./PasswordPanel";
import {
  Harness,
  expectAllowed,
  expectSkeleton,
  json,
  pending,
  recording,
  type Recorder,
} from "@/pages/story-harness";
import type { MeResponse } from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";

const ME: MeResponse = {
  user: {
    id: "user-1",
    email: "ada@example.com",
    display_name: "Ada Lovelace",
    bio: null,
    is_superadmin: false,
    created_at: "2026-01-01T00:00:00Z",
  },
  memberships: [],
  has_local_password: true,
};

/** an account with no local password: it signs in through single sign-on only */
const SSO_ME: MeResponse = { ...ME, has_local_password: false };

interface Sent {
  current_password: string;
  new_password: string;
}

/**
 * The two endpoints behind the panel. `answer` receives the body of each
 * `POST /auth/password` and returns what the control plane would say, so a
 * story can answer the second attempt differently from the first; `me` is the
 * read the panel draws itself from.
 */
const server = (answer: (sent: Sent, attempt: number) => Response | Promise<Response>, me = ME) => {
  let attempt = 0;
  return recording(async (input, init) => {
    const url = String(input);
    if (url.includes("/auth/password")) {
      attempt += 1;
      return answer(JSON.parse(String(init?.body)) as Sent, attempt);
    }
    if (url.includes("/auth/me")) return json(me);
    return json({ error: { message: "unexpected request" } }, 404);
  });
};

const refusal = (code: string, message: string, field?: string, status = 400) =>
  json({ error: { message, code, ...(field ? { field } : {}) } }, status);

const panel = (recorder: Recorder) => (
  <Harness fetchStub={recorder.stub}>
    <PasswordPanel />
  </Harness>
);

/**
 * Passwords are made when the module loads, so no story carries one as a
 * literal: a fixture that looks like a credential is what secret scanners flag.
 */
const fresh = () => `pw-${crypto.randomUUID()}`;
const OLD = fresh();
const NEXT = fresh();
const SECOND = fresh();
const THIRD = fresh();
const GUESS = fresh();
const TYPO = fresh();
// under the 8 character minimum
const SHORT = fresh().slice(0, 5);

const CURRENT = "Current password";
const NEW = "New password";
const CONFIRM = "Confirm new password";
const SUBMIT = "Change password";

/** type the three fields in the order the form asks for them */
async function fill(
  canvas: ReturnType<typeof within>,
  { current, next, confirm = next }: { current: string; next: string; confirm?: string },
) {
  // the form is drawn once the account read has answered
  await userEvent.type(await canvas.findByLabelText(CURRENT), current);
  await userEvent.type(canvas.getByLabelText(NEW), next);
  await userEvent.type(canvas.getByLabelText(CONFIRM), confirm);
}

const meta = {
  title: "Components/PasswordPanel",
  component: PasswordPanel,
} satisfies Meta<typeof PasswordPanel>;
export default meta;
type Story = StoryObj<typeof meta>;

const untouched = server(() => json({ sessions_revoked: 0 }));

/**
 * Nothing typed yet. The three fields are named, masked and carry the autofill
 * hints a password manager reads; the minimum length is stated before anything
 * is submitted, and nothing has been sent.
 */
export const Pristine: Story = {
  render: () => panel(untouched),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("heading", { level: 2, name: "Password" })).toBeVisible();

    const current = canvas.getByLabelText(CURRENT);
    const next = canvas.getByLabelText(NEW);
    const confirm = canvas.getByLabelText(CONFIRM);
    for (const field of [current, next, confirm])
      await expect(field).toHaveAttribute("type", "password");
    await expect(current).toHaveAttribute("autocomplete", "current-password");
    await expect(next).toHaveAttribute("autocomplete", "new-password");
    await expect(confirm).toHaveAttribute("autocomplete", "new-password");

    // the rule is said where the field is, and the field is described by it
    await expect(canvas.getByText("At least 8 characters.")).toBeVisible();
    await expect(next).toHaveAccessibleDescription("At least 8 characters.");

    await expect(canvas.getByRole("button", { name: SUBMIT })).toBeEnabled();
    await expect(canvas.queryByRole("alert")).toBeNull();
    await expect(canvas.queryByRole("status")).toBeNull();
    untouched.expectNotSent("POST", "/auth/password");
  },
};

const wrong = server(() =>
  refusal("invalid_field", "the current password is not correct", "current_password"),
);

/**
 * A wrong current password is the server's `400 invalid_field` on that one
 * field. The message sits on the current-password input and focus goes back to
 * it; what was typed in the other two stays, and typing again clears the
 * message.
 */
export const WrongCurrentPassword: Story = {
  render: () => panel(wrong),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await fill(canvas, { current: GUESS, next: NEXT });
    await userEvent.click(canvas.getByRole("button", { name: SUBMIT }));

    await expect(await wrong.expectSentBody<Sent>("POST", "/auth/password")).toEqual({
      current_password: GUESS,
      new_password: NEXT,
    });
    const current = canvas.getByLabelText(CURRENT);
    await expect(await canvas.findByText("That is not your current password.")).toBeVisible();
    await expect(current).toBeInvalid();
    await expect(current).toHaveAccessibleDescription("That is not your current password.");
    await waitFor(() => expect(current).toHaveFocus());

    // the other fields are not blamed, and their drafts survive
    await expect(canvas.getByLabelText(NEW)).not.toBeInvalid();
    await expect(canvas.getByLabelText(NEW)).toHaveValue(NEXT);
    await expect(canvas.getByLabelText(CONFIRM)).toHaveValue(NEXT);
    await expect(canvas.queryByRole("status")).toBeNull();
    // the form is usable again for the next try
    await expect(canvas.getByRole("button", { name: SUBMIT })).toBeEnabled();

    await userEvent.type(current, "x");
    await waitFor(() =>
      expect(canvas.queryByText("That is not your current password.")).toBeNull(),
    );
    await expect(current).not.toBeInvalid();
  },
};

const tooShort = server(() => json({ sessions_revoked: 0 }));

/**
 * A password under the minimum is said on the new-password field before any
 * request is made, so a typo costs no failed attempt and no round trip.
 */
export const TooShortIsCaughtBeforeSending: Story = {
  render: () => panel(tooShort),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await fill(canvas, { current: OLD, next: SHORT });
    await userEvent.click(canvas.getByRole("button", { name: SUBMIT }));

    const next = canvas.getByLabelText(NEW);
    await expect(
      await canvas.findByText("The new password needs at least 8 characters."),
    ).toBeVisible();
    await expect(next).toBeInvalid();
    await expect(next).toHaveAccessibleDescription("The new password needs at least 8 characters.");
    await waitFor(() => expect(next).toHaveFocus());
    tooShort.expectNotSent("POST", "/auth/password");
  },
};

const checked = server(() => json({ sessions_revoked: 0 }));

/**
 * The other two things the form can know: the confirmation has to match, and
 * the new password has to differ from the current one. Both are listed at once
 * rather than one per attempt, and focus goes to the first of them.
 */
export const MismatchAndSamePasswordAreCaughtBeforeSending: Story = {
  render: () => panel(checked),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await fill(canvas, { current: NEXT, next: NEXT, confirm: TYPO });
    await userEvent.click(canvas.getByRole("button", { name: SUBMIT }));

    await expect(
      await canvas.findByText("The new password must differ from the current one."),
    ).toBeVisible();
    await expect(
      canvas.getByText("The new password and its confirmation do not match."),
    ).toBeVisible();
    await expect(canvas.getByLabelText(NEW)).toBeInvalid();
    await expect(canvas.getByLabelText(CONFIRM)).toBeInvalid();
    await waitFor(() => expect(canvas.getByLabelText(NEW)).toHaveFocus());

    // an empty current password is its own message, and takes focus first
    await userEvent.clear(canvas.getByLabelText(CURRENT));
    await userEvent.click(canvas.getByRole("button", { name: SUBMIT }));
    await expect(await canvas.findByText("Enter your current password.")).toBeVisible();
    await waitFor(() => expect(canvas.getByLabelText(CURRENT)).toHaveFocus());
    checked.expectNotSent("POST", "/auth/password");
  },
};

const rejected = server(() =>
  refusal("invalid_field", "password must be at least 8 characters", "new_password"),
);

/**
 * A `400 invalid_field` on `new_password` the form did not foresee, from a
 * control plane whose rule is stricter than the dashboard's, still lands on the
 * new-password field and not in a toast.
 */
export const ServerRefusesTheNewPassword: Story = {
  render: () => panel(rejected),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await fill(canvas, { current: OLD, next: NEXT });
    await userEvent.click(canvas.getByRole("button", { name: SUBMIT }));

    await expect(
      await canvas.findByText("This password was not accepted. Try a different one."),
    ).toBeVisible();
    await expect(canvas.getByLabelText(NEW)).toBeInvalid();
    await waitFor(() => expect(canvas.getByLabelText(NEW)).toHaveFocus());
    await expect(canvas.getByLabelText(CURRENT)).not.toBeInvalid();
  },
};

const throttled = server(
  () =>
    new Response(
      JSON.stringify({
        error: {
          message: "too many rejected attempts; try again later",
          code: "too_many_attempts",
        },
      }),
      { status: 429, headers: { "Content-Type": "application/json", "Retry-After": "90" } },
    ),
);

/**
 * A spent attempt budget answers `429` with the wait in `Retry-After`. The
 * panel says how long, as an alert on the form rather than on one field, since
 * the right password is refused too while the lock holds; the drafts stay.
 */
export const ThrottledAfterTooManyWrongPasswords: Story = {
  render: () => panel(throttled),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await fill(canvas, { current: GUESS, next: NEXT });
    await userEvent.click(canvas.getByRole("button", { name: SUBMIT }));

    const alert = await canvas.findByRole("alert");
    await expect(alert).toHaveTextContent("Too many wrong passwords. Try again in 90 seconds.");
    await expect(alert).toBeVisible();
    // no field is blamed for a lock, and nothing the person typed is lost
    await expect(canvas.getByLabelText(CURRENT)).not.toBeInvalid();
    await expect(canvas.getByLabelText(CURRENT)).toHaveValue(GUESS);
    await expect(canvas.getByLabelText(NEW)).toHaveValue(NEXT);
    await expect(canvas.queryByRole("status")).toBeNull();
  },
};

const throttledNoHeader = server(() =>
  refusal("too_many_attempts", "too many rejected attempts; try again later", undefined, 429),
);

/** A `429` that carries no readable wait says to try later rather than inventing one. */
export const ThrottledWithoutAWait: Story = {
  render: () => panel(throttledNoHeader),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await fill(canvas, { current: GUESS, next: NEXT });
    await userEvent.click(canvas.getByRole("button", { name: SUBMIT }));
    await expect(await canvas.findByRole("alert")).toHaveTextContent(
      "Too many wrong passwords. Try again later.",
    );
  },
};

const changed = server((_sent, attempt) => json({ sessions_revoked: [3, 1, 0][attempt - 1] ?? 0 }));

/**
 * The change went through. The panel says how many other sessions were signed
 * out, in the right plural, empties all three fields so no password lingers in
 * the page, and stays on screen: the caller is still signed in. Typing the
 * next password hides the confirmation, which was about the last one.
 */
export const ChangedAndOtherSessionsSignedOut: Story = {
  render: () => panel(changed),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await fill(canvas, { current: OLD, next: NEXT });
    // Enter in the last field submits, the way the button does
    await userEvent.type(canvas.getByLabelText(CONFIRM), "{enter}");

    await expect(await changed.expectSentBody<Sent>("POST", "/auth/password")).toEqual({
      current_password: OLD,
      new_password: NEXT,
    });
    const status = await canvas.findByRole("status");
    await expect(status).toHaveTextContent("Password changed. 3 other sessions were signed out.");
    await expect(status).toBeVisible();
    for (const label of [CURRENT, NEW, CONFIRM])
      await expect(canvas.getByLabelText(label)).toHaveValue("");
    await expect(canvas.queryByRole("alert")).toBeNull();

    // the next change: one session reads in the singular
    await fill(canvas, { current: NEXT, next: SECOND });
    await expect(canvas.queryByRole("status")).toBeNull();
    await userEvent.click(canvas.getByRole("button", { name: SUBMIT }));
    await waitFor(() =>
      expect(canvas.getByRole("status")).toHaveTextContent(
        "Password changed. 1 other session was signed out.",
      ),
    );

    // and with nothing else signed in, it says so instead of "0 other sessions"
    await fill(canvas, { current: SECOND, next: THIRD });
    await userEvent.click(canvas.getByRole("button", { name: SUBMIT }));
    await waitFor(() =>
      expect(canvas.getByRole("status")).toHaveTextContent(
        "Password changed. You had no other sessions to sign out.",
      ),
    );
  },
};

const failed = server(() => json({ error: { message: "store unavailable" } }, 500));

/**
 * A failure that is not about the password (the control plane is down) is said
 * once on the form in the dashboard's own words, keeps every draft, and leaves
 * the button live for another try.
 */
export const ServerFailureKeepsTheDraft: Story = {
  render: () => panel(failed),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await fill(canvas, { current: OLD, next: NEXT });
    await userEvent.click(canvas.getByRole("button", { name: SUBMIT }));

    const alert = await canvas.findByRole("alert");
    await expect(alert).toHaveTextContent("Could not change your password.");
    await expect(alert).toHaveTextContent(en.errors.api.server);
    await expect(canvas.getByLabelText(CURRENT)).toHaveValue(OLD);
    await expect(canvas.getByLabelText(NEW)).toHaveValue(NEXT);
    await expect(canvas.getByRole("button", { name: SUBMIT })).toBeEnabled();
  },
};

const slow = server(() => new Promise<Response>(() => {}));

/** While a change is on the wire the button is off, so a double press sends one request. */
export const SubmitsOnce: Story = {
  render: () => panel(slow),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await fill(canvas, { current: OLD, next: NEXT });
    await userEvent.click(canvas.getByRole("button", { name: SUBMIT }));
    await waitFor(() => expect(canvas.getByRole("button", { name: SUBMIT })).toBeDisabled());
    // Enter in a field submits the form too, and must not start a second change
    await userEvent.type(canvas.getByLabelText(CONFIRM), "{enter}");
    await expect(
      slow.calls.filter((c) => c.method === "POST" && c.url.includes("/auth/password")),
    ).toHaveLength(1);
  },
};

const sso = server(() => json({ sessions_revoked: 0 }), SSO_ME);

/**
 * An account that signs in through single sign-on has no local password. It is
 * told so, and where its password lives, rather than offered a form that can
 * only answer `409 no_local_password`.
 */
export const SsoOnly: Story = {
  render: () => panel(sso),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("heading", { level: 2, name: "Password" })).toBeVisible();
    await expect(
      canvas.getByText(/You sign in through single sign-on, so this account has no local password/),
    ).toBeVisible();
    await expect(canvas.getByText(/Change it with your identity provider/)).toBeVisible();
    await expect(canvas.queryByLabelText(CURRENT)).toBeNull();
    await expect(canvas.queryByLabelText(NEW)).toBeNull();
    await expect(canvas.queryByRole("button", { name: SUBMIT })).toBeNull();
    sso.expectNotSent("POST", "/auth/password");
  },
};

/** The same explanation in Russian, with the layout holding the longer sentence. */
export const SsoOnlyInRussian: Story = {
  globals: { locale: "ru" },
  render: () => panel(sso),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("heading", { level: 2, name: "Пароль" })).toBeVisible();
    await expect(
      canvas.getByText(/Вы входите через единый вход, поэтому у этой учётной записи нет/),
    ).toBeVisible();
    await expect(canvas.queryByRole("button", { name: "Сменить пароль" })).toBeNull();
  },
};

let staleMe = 0;
const noLocalPassword = recording(async (input) => {
  const url = String(input);
  if (url.includes("/auth/password")) {
    return refusal(
      "no_local_password",
      "this account signs in through single sign-on",
      undefined,
      409,
    );
  }
  // the first read says the account has a password; the one after the refusal
  // knows better
  if (url.includes("/auth/me")) {
    staleMe += 1;
    return json(staleMe === 1 ? ME : SSO_ME);
  }
  return json({ error: { message: "unexpected request" } }, 404);
});

/**
 * An account whose password was removed since the page loaded gets the `409`,
 * and the panel reads `/auth/me` again and turns into the explanation instead
 * of leaving a form that can never work.
 */
export const NoLocalPasswordTurnsIntoTheExplanation: Story = {
  beforeEach: () => {
    staleMe = 0;
  },
  render: () => panel(noLocalPassword),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await fill(canvas, { current: OLD, next: NEXT });
    await userEvent.click(canvas.getByRole("button", { name: SUBMIT }));
    await expect(
      await canvas.findByText(/You sign in through single sign-on, so this account has no local/),
    ).toBeVisible();
    await expect(canvas.queryByLabelText(CURRENT)).toBeNull();
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <PasswordPanel />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectSkeleton(canvasElement);
    // the form is not drawn until the account is known: an account with no
    // local password would otherwise see a form flash and vanish
    await expect(canvas.getByRole("heading", { level: 2, name: "Password" })).toBeVisible();
    await expect(canvas.queryByLabelText(CURRENT)).toBeNull();
  },
};

const unreadable = recording(async (input) =>
  String(input).includes("/auth/me")
    ? json({ error: { message: "boom" } }, 500)
    : json({ sessions_revoked: 0 }),
);

/**
 * A failed read of the account does not take the form away: the route decides
 * for itself, and the profile card above already reports the failed read, so
 * this panel adds no second error.
 */
export const AccountReadFailedStillOffersTheForm: Story = {
  render: () => panel(unreadable),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByLabelText(CURRENT)).toBeVisible();
    await expect(canvas.getByRole("button", { name: SUBMIT })).toBeEnabled();
    await expect(canvas.queryByRole("alert")).toBeNull();
  },
};

const openMode = recording(async () =>
  json({ error: { message: "no local account session", code: "open_mode_no_session" } }, 401),
);

/** Open mode has no accounts, so there is no password and the panel is not drawn. */
export const HiddenInOpenMode: Story = {
  render: () => panel(openMode),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await openMode.expectSent("GET", "/auth/me");
    await waitFor(() => expect(canvas.queryByRole("heading", { name: "Password" })).toBeNull());
    await expect(canvas.queryByLabelText(CURRENT)).toBeNull();
  },
};

const viewer = server(() => json({ sessions_revoked: 0 }));

/**
 * `my_password:update` is open to every signed-in account, so even a viewer, the
 * lowest role, can change their own password: the button is live once the gate
 * has answered.
 */
export const AllowedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={viewer.stub} role="viewer">
      <PasswordPanel />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expect(await within(canvasElement).findByLabelText(CURRENT)).toBeVisible();
    await expectAllowed(canvasElement, SUBMIT);
  },
};

const russian = server((_sent, attempt) => json({ sessions_revoked: [1, 2, 5][attempt - 1] ?? 0 }));

/**
 * The success line in Russian takes four plural forms, and 1, 2 and 5 land on
 * three of them; the field names, the hint and the empty-field message are the
 * Russian ones too.
 */
export const ChangedInRussian: Story = {
  globals: { locale: "ru" },
  render: () => panel(russian),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const [CUR, NXT, CNF, GO] = [
      "Текущий пароль",
      "Новый пароль",
      "Повторите новый пароль",
      "Сменить пароль",
    ];
    await expect(await canvas.findByText("Не короче 8 символов.")).toBeVisible();

    await userEvent.click(canvas.getByRole("button", { name: GO }));
    await expect(await canvas.findByText("Введите текущий пароль.")).toBeVisible();

    const answers = [
      "Пароль изменён. 1 другая сессия завершена.",
      "Пароль изменён. 2 другие сессии завершены.",
      "Пароль изменён. 5 других сессий завершено.",
    ];
    let previous = "";
    for (const expected of answers) {
      const next = fresh();
      await userEvent.clear(canvas.getByLabelText(CUR));
      await userEvent.type(canvas.getByLabelText(CUR), previous || OLD);
      await userEvent.clear(canvas.getByLabelText(NXT));
      await userEvent.type(canvas.getByLabelText(NXT), next);
      await userEvent.clear(canvas.getByLabelText(CNF));
      await userEvent.type(canvas.getByLabelText(CNF), next);
      await userEvent.click(canvas.getByRole("button", { name: GO }));
      await waitFor(() => expect(canvas.getByRole("status")).toHaveTextContent(expected));
      previous = next;
    }
  },
};

const throttledRussian = server(
  () =>
    new Response(JSON.stringify({ error: { message: "x", code: "too_many_attempts" } }), {
      status: 429,
      headers: { "Content-Type": "application/json", "Retry-After": "21" },
    }),
);

/** The wait in a lock takes the Russian plural form too: 21 is "секунду". */
export const ThrottledInRussian: Story = {
  globals: { locale: "ru" },
  render: () => panel(throttledRussian),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await canvas.findByLabelText("Текущий пароль"), OLD);
    await userEvent.type(canvas.getByLabelText("Новый пароль"), NEXT);
    await userEvent.type(canvas.getByLabelText("Повторите новый пароль"), NEXT);
    await userEvent.click(canvas.getByRole("button", { name: "Сменить пароль" }));
    await expect(await canvas.findByRole("alert")).toHaveTextContent(
      "Слишком много неверных паролей. Повторите через 21 секунду.",
    );
  },
};
