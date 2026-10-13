import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { MemoryRouter } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import AcceptInvite from "./AcceptInvite";
import { Harness, json, recording, type FetchStub } from "./story-harness";
import { AuthProvider } from "@/lib/auth";
import { withPageA11y } from "@/lib/story-a11y";

const TOKEN = "inv_2f9c41";

const PREVIEW = {
  org_name: "Acme",
  email: "anya@acme.co",
  role: "admin",
  scope_type: "org",
  expires_at: "2026-09-30T00:00:00Z",
  has_account: false,
};

/** the same invitation, to an address that already has an account (#1935) */
const EXISTING = { ...PREVIEW, has_account: true };

/** the answer to an accept that granted the role but minted no session */
const signInRequired = (reason: "existing_account" | "second_factor") => () =>
  json({ sign_in_required: true, email: PREVIEW.email, reason });

/**
 * The invitee has no session — the token in the url is the only credential the
 * screen has — so this is deliberately *not* the scoped harness stub: nothing
 * under `/api/v1/orgs` is reachable yet.
 */
const invite =
  (
    preview: () => Response | Promise<Response>,
    accept: () => Response | Promise<Response> = () =>
      json({ token: "session-token", user: { email: PREVIEW.email, is_superadmin: false } }),
  ): FetchStub =>
  async (input) =>
    String(input).endsWith("/accept") ? accept() : preview();

/** the recorder the story under way installed, read back by its play function */
let calls: ReturnType<typeof recording>;

function Stage({ stub }: { stub: FetchStub }) {
  const recorder = recording(stub);
  calls = recorder;
  // accepting an invite signs the invitee in, which persists a session; drop it
  // on unmount so a later story does not boot into someone else's account
  React.useEffect(
    () => () => {
      localStorage.removeItem("rolter.session.token");
      localStorage.removeItem("rolter.session.email");
      localStorage.removeItem("rolter.session.user");
    },
    [],
  );
  return (
    <MemoryRouter initialEntries={[`/invite/${TOKEN}`]}>
      <Harness fetchStub={recorder.stub}>
        <AuthProvider>
          <AcceptInvite token={TOKEN} />
        </AuthProvider>
      </Harness>
    </MemoryRouter>
  );
}

// the tab title is document state, which outlives a story: blank it first, so
// a title the previous story left behind can never pass for this one's
const blankTitle = () => {
  document.title = "";
};

const meta = {
  title: "Screens/AcceptInvite",
  component: AcceptInvite,
  // signed out this is a whole page, so it is gated on its landmark and heading (#2069)
  parameters: { layout: "fullscreen", ...withPageA11y },
  args: { token: TOKEN },
} satisfies Meta<typeof AcceptInvite>;

export default meta;
type Story = StoryObj<typeof meta>;

/** The link checks out: who invited whom, and as what. */
export const Loaded: Story = {
  render: () => <Stage stub={invite(() => json(PREVIEW))} />,
  beforeEach: blankTitle,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("heading", { level: 1, name: "Join Acme" })).toBeVisible();
    // the card owns the landmark, as sign-in's does (#2069, #2228)
    await expect(canvas.getByRole("main")).toBeVisible();
    // outside the shell, so the page names the tab itself (#2002)
    await waitFor(() => expect(document.title).toBe("Join Acme · rolter"));
    // the address and the role are stated before a password is chosen: an
    // invite to the wrong account is only catchable here
    await expect(canvas.getByText(PREVIEW.email)).toBeVisible();
    await expect(canvas.getByText("Admin")).toBeVisible();
  },
};

/**
 * The role is named the way the inviter saw it in the invite dialog, from the
 * same `shell.roles` catalog, and not as the id the control plane stores: the
 * invitee was told "Member", not `member` (#2813).
 */
export const RoleIsNamedAsTheInviterSawIt: Story = {
  render: () => <Stage stub={invite(() => json({ ...PREVIEW, role: "member" }))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const role = await canvas.findByText("Member");
    await expect(role).toBeVisible();
    await expect(canvas.queryByText("member")).not.toBeInTheDocument();
    await expect(role.closest("p")).toHaveTextContent(
      "You were invited as anya@acme.co with the Member role.",
    );
  },
};

/**
 * A role this dashboard has no name for, such as one a newer control plane
 * added, is shown as the server sent it rather than as a missing key.
 */
export const UnknownRoleIsShownAsSent: Story = {
  render: () => <Stage stub={invite(() => json({ ...PREVIEW, role: "auditor" }))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("auditor")).toBeVisible();
    await expect(canvas.queryByText(/shell\.roles/)).not.toBeInTheDocument();
  },
};

/** The preview request is in flight — one line, not an empty card. */
export const Loading: Story = {
  render: () => <Stage stub={invite(() => new Promise<Response>(() => {}))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("Checking your invitation…")).toBeVisible();
    await expect(canvas.queryByLabelText(/^Password/)).not.toBeInTheDocument();
  },
};

/**
 * Used, revoked or expired all look the same from here, so the copy names all
 * three and says who to ask — the invitee cannot fix any of them alone.
 */
export const InvalidLink: Story = {
  render: () => <Stage stub={invite(() => json({ error: { message: "not found" } }, 404))} />,
  beforeEach: blankTitle,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/This invitation link is not valid/)).toBeVisible();
    // a heading and a way on: someone who already accepted lands here from their mail
    await expect(canvas.getByRole("heading", { level: 1 })).toBeVisible();
    await expect(canvas.getByRole("main")).toBeVisible();
    await expect(canvas.getByRole("button", { name: /go to sign in/i })).toBeVisible();
    await expect(canvas.queryByLabelText(/^Password/)).not.toBeInTheDocument();
    // a dead link names no org, so the tab falls back to the plain noun
    await expect(document.title).toBe("Invitation · rolter");
  },
};

/**
 * The length rule is a hint under the field that becomes the error once the
 * person leaves it, and the button is not silently off.
 */
export const PasswordTooShort: Story = {
  render: () => <Stage stub={invite(() => json(PREVIEW))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const password = await canvas.findByLabelText(/^Password/);
    await expect(password).toHaveAccessibleDescription("At least 8 characters.");
    await userEvent.type(password, "short");
    // still typing: no error yet
    await expect(canvas.queryByRole("alert")).not.toBeInTheDocument();
    await userEvent.tab();
    await expect(await canvas.findByRole("alert")).toHaveTextContent("Use at least 8 characters.");
    await expect(password).toHaveAccessibleDescription("Use at least 8 characters.");
    await expect(canvas.getByRole("button", { name: /accept invitation/i })).toBeEnabled();
  },
};

/** The two boxes disagree: said once the confirmation is as long as the password. */
export const PasswordsDoNotMatch: Story = {
  render: () => <Stage stub={invite(() => json(PREVIEW))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await canvas.findByLabelText(/^Password/), "correct-horse");
    const confirm = canvas.getByLabelText(/confirm password/i);
    await userEvent.type(confirm, "correct-hors");
    // not on the first keystrokes
    await expect(canvas.queryByRole("alert")).not.toBeInTheDocument();
    await userEvent.type(confirm, "x");
    await expect(await canvas.findByRole("alert")).toHaveTextContent(/do not match/);
    await expect(confirm).toBeInvalid();
    await expect(confirm).toHaveAccessibleDescription(/do not match/);
    // a mismatch never reaches the server
    await userEvent.click(canvas.getByRole("button", { name: /accept invitation/i }));
    calls.expectNotSent("POST", "/accept");
  },
};

/** The same toggle as sign-in, so a typo is checkable before it becomes the password. */
export const PasswordCanBeShown: Story = {
  render: () => <Stage stub={invite(() => json(PREVIEW))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const password = await canvas.findByLabelText(/^Password/);
    await expect(password).toHaveAttribute("type", "password");
    await userEvent.click(canvas.getAllByRole("button", { name: /show password/i })[0]);
    await expect(password).toHaveAttribute("type", "text");
  },
};

/** An org-wide invitation says so, and when the link stops working. */
export const NamesTheOrgScopeAndExpiry: Story = {
  render: () => <Stage stub={invite(() => json(PREVIEW))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/gives you the Admin role across Acme\./)).toBeVisible();
    await expect(canvas.getByText(/The link works until/)).toBeVisible();
  },
};

/** A team invitation is not an org one: the team is named. */
export const NamesATeamScope: Story = {
  render: () => (
    <Stage stub={invite(() => json({ ...PREVIEW, scope_type: "team", scope_name: "Platform" }))} />
  ),
  play: async ({ canvasElement }) => {
    await expect(
      await within(canvasElement).findByText(/Admin role on the team Platform in Acme\./),
    ).toBeVisible();
  },
};

/** A project-admin invite must not read like org admin. */
export const NamesAProjectScope: Story = {
  render: () => (
    <Stage
      stub={invite(() => json({ ...PREVIEW, scope_type: "project", scope_name: "Gateway" }))}
    />
  ),
  play: async ({ canvasElement }) => {
    await expect(
      await within(canvasElement).findByText(/Admin role on the project Gateway in Acme\./),
    ).toBeVisible();
  },
};

/** The scope sentence in Russian, in the dashboard's own role word. */
export const NamesAProjectScopeInRussian: Story = {
  render: () => (
    <Stage
      stub={invite(() =>
        json({ ...PREVIEW, role: "member", scope_type: "project", scope_name: "Gateway" }),
      )}
    />
  ),
  globals: { locale: "ru" },
  play: async ({ canvasElement }) => {
    await expect(
      await within(canvasElement).findByText(/роль «Участник» в проекте Gateway организации Acme/),
    ).toBeVisible();
  },
};

/** The happy path: the password is posted against the token from the url. */
export const Accepts: Story = {
  render: () => <Stage stub={invite(() => json(PREVIEW))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await canvas.findByLabelText(/^Password/), "correct-horse");
    await userEvent.type(canvas.getByLabelText(/confirm password/i), "correct-horse");
    await userEvent.click(canvas.getByRole("button", { name: /accept invitation/i }));
    const body = (await calls.expectSentBody("POST", `/invitations/accept/${TOKEN}/accept`)) as {
      password: string;
    };
    await expect(body.password).toBe("correct-horse");
  },
};

/**
 * The link was still valid when it was previewed and spent by the time it was
 * accepted. The server's message is not one the dashboard has a translation
 * for, so it leads with a generic translated line and keeps the message below
 * as detail: it is the only thing that distinguishes this from a typed
 * password the form would have caught.
 */
export const AcceptRejected: Story = {
  render: () => (
    <Stage
      stub={invite(
        () => json(PREVIEW),
        () => json({ error: { message: "this invitation has already been accepted" } }, 409),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await canvas.findByLabelText(/^Password/), "correct-horse");
    await userEvent.type(canvas.getByLabelText(/confirm password/i), "correct-horse");
    await userEvent.click(canvas.getByRole("button", { name: /accept invitation/i }));
    await waitFor(() => expect(canvas.getByText(/already been accepted/)).toBeVisible());
    // the form stays, because a different link can still be pasted into it
    await expect(canvas.getByLabelText(/^Password/)).toBeVisible();
  },
};

/** fills the form and submits, for a refusal story */
const submitEn = async (canvasElement: HTMLElement) => {
  const canvas = within(canvasElement);
  await userEvent.type(await canvas.findByLabelText(/^Password/), "correct-horse");
  await userEvent.type(canvas.getByLabelText(/confirm password/i), "correct-horse");
  await userEvent.click(canvas.getByRole("button", { name: /accept invitation/i }));
  return canvas;
};

/**
 * A link spent, revoked or expired between opening and submitting answers 401.
 * The control plane's "missing or invalid credentials" is not what is said; and
 * since retrying cannot help, the button goes off and sign-in is offered.
 */
export const LinkSpentBetweenOpeningAndSubmitting: Story = {
  render: () => (
    <Stage
      stub={invite(
        () => json(PREVIEW),
        () => json({ error: { message: "missing or invalid credentials" } }, 401),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = await submitEn(canvasElement);
    await expect(await canvas.findByRole("alert")).toHaveTextContent(
      /already used, was revoked or has expired/,
    );
    await expect(canvas.queryByText(/missing or invalid credentials/)).not.toBeInTheDocument();
    await expect(canvas.getByRole("button", { name: /accept invitation/i })).toBeDisabled();
    await expect(canvas.getByRole("button", { name: /go to sign in/i })).toBeVisible();
  },
};

/** A deactivated account answers 403; "insufficient role" means nothing to an invitee. */
export const DeactivatedAccount: Story = {
  render: () => (
    <Stage
      stub={invite(
        () => json(PREVIEW),
        () => json({ error: { message: "insufficient role for this resource" } }, 403),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = await submitEn(canvasElement);
    await expect(await canvas.findByRole("alert")).toHaveTextContent(/has been deactivated/);
    await expect(canvas.queryByText(/insufficient role/)).not.toBeInTheDocument();
    await expect(canvas.getByRole("button", { name: /accept invitation/i })).toBeDisabled();
    await expect(canvas.getByRole("button", { name: /go to sign in/i })).toBeVisible();
  },
};

/** A throttled token says how long to wait, and the button stays: waiting does fix this. */
export const ThrottledWithAWait: Story = {
  render: () => (
    <Stage
      stub={invite(
        () => json(PREVIEW),
        () =>
          new Response(
            JSON.stringify({ error: { message: "slow down", code: "too_many_attempts" } }),
            {
              status: 429,
              headers: { "Content-Type": "application/json", "Retry-After": "45" },
            },
          ),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = await submitEn(canvasElement);
    await expect(await canvas.findByRole("alert")).toHaveTextContent(/Try again in 45 seconds/);
    await expect(canvas.getByRole("button", { name: /accept invitation/i })).toBeEnabled();
  },
};

/** The control plane cannot be reached: said in the dashboard's words, retry stays open. */
export const ControlPlaneUnreachable: Story = {
  render: () => (
    <Stage
      stub={invite(
        () => json(PREVIEW),
        () => {
          throw new TypeError("network error");
        },
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = await submitEn(canvasElement);
    await expect(await canvas.findByRole("alert")).toHaveTextContent(/could not reach the server/i);
    await expect(canvas.getByRole("button", { name: /accept invitation/i })).toBeEnabled();
  },
};

/**
 * The address already has an account, so the screen asks for no password: an
 * invite link never signs anyone in to an existing account, since the inviter
 * holds the same token (#1935). Accepting grants the role and hands over to
 * the ordinary sign-in, where the account's own password and factor apply.
 */
export const ExistingAccount: Story = {
  render: () => <Stage stub={invite(() => json(EXISTING), signInRequired("existing_account"))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/already has a rolter account/)).toBeVisible();
    // "choose a password" would be untrue here: the account keeps its own
    await expect(canvas.queryByLabelText(/^Password/)).not.toBeInTheDocument();
    await userEvent.click(canvas.getByRole("button", { name: /accept invitation/i }));
    const body = await calls.expectSentBody<Record<string, unknown>>(
      "POST",
      `/invitations/accept/${TOKEN}/accept`,
    );
    await expect(body).toEqual({});
    await expect(await canvas.findByRole("status")).toHaveTextContent(
      /The Admin role in Acme is now on your account.*usual password/,
    );
    await expect(canvas.getByRole("button", { name: /continue to sign in/i })).toBeVisible();
    // nothing to sign in with: the answer carried no session
    await expect(localStorage.getItem("rolter.session.token")).toBeNull();
  },
};

/**
 * A new account in an org whose policy requires a second factor: the account
 * is created, but the session waits for the sign-in that enrols the factor.
 */
export const SecondFactorRequired: Story = {
  render: () => <Stage stub={invite(() => json(PREVIEW), signInRequired("second_factor"))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await canvas.findByLabelText(/^Password/), "correct-horse");
    await userEvent.type(canvas.getByLabelText(/confirm password/i), "correct-horse");
    await userEvent.click(canvas.getByRole("button", { name: /accept invitation/i }));
    await expect(await canvas.findByRole("status")).toHaveTextContent(
      /Acme requires a second factor/,
    );
    await expect(canvas.queryByLabelText(/^Password/)).not.toBeInTheDocument();
    await expect(localStorage.getItem("rolter.session.token")).toBeNull();
  },
};

/** the accept form filled in and sent, in whatever locale the story set */
const submitRu = async (canvasElement: HTMLElement) => {
  const canvas = within(canvasElement);
  await userEvent.type(await canvas.findByLabelText(/^Пароль/), "correct-horse");
  await userEvent.type(canvas.getByLabelText(/Повторите пароль/), "correct-horse");
  await userEvent.click(canvas.getByRole("button", { name: "Принять приглашение" }));
  return canvas;
};

/**
 * A coded refusal is translated: the server's English message never reaches
 * the screen when the dashboard has the sentence for its `code` (#2216).
 */
export const AcceptRejectedInRussian: Story = {
  render: () => (
    <Stage
      stub={invite(
        () => json(PREVIEW),
        () =>
          json(
            {
              error: {
                message: "too many rejected attempts; try again later",
                code: "too_many_attempts",
              },
            },
            429,
          ),
      )}
    />
  ),
  // the toolbar global is what switches the catalog
  globals: { locale: "ru" },
  play: async ({ canvasElement }) => {
    const canvas = await submitRu(canvasElement);
    await expect(await canvas.findByText(/Слишком много неудачных попыток/)).toBeVisible();
    await expect(canvas.queryByText(/too many rejected/)).not.toBeInTheDocument();
  },
};

/** An unknown message falls back to a generic line, the raw words tucked below it. */
export const AcceptUnknownErrorInRussian: Story = {
  render: () => (
    <Stage
      stub={invite(
        () => json(PREVIEW),
        () => json({ error: { message: "invitation seat limit reached" } }, 409),
      )}
    />
  ),
  // the toolbar global is what switches the catalog
  globals: { locale: "ru" },
  play: async ({ canvasElement }) => {
    const canvas = await submitRu(canvasElement);
    await expect(await canvas.findByText(/Сервер отклонил этот запрос/)).toBeVisible();
    await expect(canvas.getByText("invitation seat limit reached")).toBeVisible();
  },
};

/**
 * In Russian the role is «Участник», the word the invite dialog showed the
 * inviter, in the page the invitee opens and again once the role is granted
 * to an account that already existed (#2813).
 */
export const RoleIsTranslatedInRussian: Story = {
  render: () => <Stage stub={invite(() => json({ ...PREVIEW, role: "member" }))} />,
  // the toolbar global is what switches the catalog
  globals: { locale: "ru" },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const role = await canvas.findByText("Участник");
    await expect(role).toBeVisible();
    await expect(canvas.queryByText("member")).not.toBeInTheDocument();
    await expect(role.closest("p")).toHaveTextContent(
      "Вас пригласили как anya@acme.co с ролью Участник.",
    );
  },
};

/** The same for the sentence that follows an accepted invitation to an existing account. */
export const GrantedRoleIsTranslatedInRussian: Story = {
  render: () => (
    <Stage
      stub={invite(() => json({ ...EXISTING, role: "viewer" }), signInRequired("existing_account"))}
    />
  ),
  globals: { locale: "ru" },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Принять приглашение" }));
    await expect(await canvas.findByRole("status")).toHaveTextContent(
      "Роль Наблюдатель в Acme добавлена к вашей учётной записи.",
    );
    await expect(canvas.queryByText(/viewer/)).not.toBeInTheDocument();
  },
};
