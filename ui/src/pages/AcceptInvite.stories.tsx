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
};

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
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("heading", { name: "Join Acme" })).toBeVisible();
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
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/This invitation link is not valid/)).toBeVisible();
    await expect(canvas.queryByRole("button")).not.toBeInTheDocument();
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
 * accepted. The server's reason is shown as-is: it is the only thing that
 * distinguishes this from a typed password the form would have caught.
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
