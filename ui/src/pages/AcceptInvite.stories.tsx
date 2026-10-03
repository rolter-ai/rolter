import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { MemoryRouter } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import AcceptInvite from "./AcceptInvite";
import { Harness, json, recording, type FetchStub } from "./story-harness";
import { AuthProvider } from "@/lib/auth";

const TOKEN = "inv_2f9c41";

const PREVIEW = {
  org_name: "Acme",
  email: "anya@acme.co",
  role: "admin",
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
  parameters: { layout: "fullscreen" },
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
    await expect(await canvas.findByRole("heading", { name: "Join Acme" })).toBeVisible();
    // outside the shell, so the page names the tab itself (#2002)
    await waitFor(() => expect(document.title).toBe("Join Acme · rolter"));
    // the address and the role are stated before a password is chosen: an
    // invite to the wrong account is only catchable here
    await expect(canvas.getByText(PREVIEW.email)).toBeVisible();
    await expect(canvas.getByText("admin")).toBeVisible();
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
    await expect(canvas.queryByRole("button")).not.toBeInTheDocument();
    // a dead link names no org, so the tab falls back to the plain noun
    await expect(document.title).toBe("Invitation · rolter");
  },
};

/** Under eight characters is refused by the field, before the round trip. */
export const PasswordTooShort: Story = {
  render: () => <Stage stub={invite(() => json(PREVIEW))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await canvas.findByLabelText(/^Password/), "short");
    await expect(canvas.getByRole("button", { name: /accept invitation/i })).toBeDisabled();
  },
};

/** The two boxes disagree: said here rather than after the account is created. */
export const PasswordsDoNotMatch: Story = {
  render: () => <Stage stub={invite(() => json(PREVIEW))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await canvas.findByLabelText(/^Password/), "correct-horse");
    await userEvent.type(canvas.getByLabelText(/confirm password/i), "correct-hors");
    await expect(canvas.getByRole("alert")).toHaveTextContent(/do not match/);
    await expect(canvas.getByRole("button", { name: /accept invitation/i })).toBeDisabled();
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
      /now on your account.*usual password/,
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
