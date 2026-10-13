import type { Meta, StoryObj } from "@storybook/react-vite";
import { MemoryRouter } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import ResetPassword from "./ResetPassword";
import { Harness, json, recording, type FetchStub } from "./story-harness";
import { AuthProvider } from "@/lib/auth";

const TOKEN = "rolter_reset_2f9c41";

const PREVIEW = { email: "anya@acme.co", expires_at: "2026-09-30T00:00:00Z" };

/**
 * The member has no session: the token in the url is the only credential the
 * screen has, so nothing under `/api/v1/orgs` is reachable.
 */
const reset =
  (
    preview: () => Response | Promise<Response>,
    redeem: () => Response | Promise<Response> = () =>
      json({ email: PREVIEW.email, sign_in_required: true }),
  ): FetchStub =>
  async (_input, init) =>
    (init?.method ?? "GET").toUpperCase() === "POST" ? redeem() : preview();

let calls: ReturnType<typeof recording>;

function Stage({ stub }: { stub: FetchStub }) {
  const recorder = recording(stub);
  calls = recorder;
  return (
    <MemoryRouter initialEntries={[`/reset-password/${TOKEN}`]}>
      <Harness fetchStub={recorder.stub}>
        <AuthProvider>
          <ResetPassword token={TOKEN} />
        </AuthProvider>
      </Harness>
    </MemoryRouter>
  );
}

const meta = {
  title: "Screens/ResetPassword",
  component: ResetPassword,
  parameters: { layout: "fullscreen" },
  args: { token: TOKEN },
} satisfies Meta<typeof ResetPassword>;

export default meta;
type Story = StoryObj<typeof meta>;

/** The link checks out: the address it is for is stated before a password is chosen. */
export const Loaded: Story = {
  render: () => <Stage stub={reset(() => json(PREVIEW))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByRole("heading", { level: 1, name: "Reset your password" }),
    ).toBeVisible();
    await expect(canvas.getByRole("main")).toBeVisible();
    await expect(await canvas.findByText(PREVIEW.email)).toBeVisible();
    await waitFor(() => expect(document.title).toBe("Reset your password · rolter"));
  },
};

/** The preview request is in flight: one line, not an empty card. */
export const Loading: Story = {
  render: () => <Stage stub={reset(() => new Promise<Response>(() => {}))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("Checking your reset link…")).toBeVisible();
    await expect(canvas.queryByLabelText(/^New password/)).not.toBeInTheDocument();
  },
};

/** Used, replaced, expired and wrong all look the same, and the copy says who to ask. */
export const InvalidLink: Story = {
  render: () => <Stage stub={reset(() => json({ error: { message: "unauthenticated" } }, 401))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/This reset link is not valid/)).toBeVisible();
    await expect(canvas.queryByRole("button")).not.toBeInTheDocument();
  },
};

/** Under eight characters is refused by the field, before the round trip. */
export const PasswordTooShort: Story = {
  render: () => <Stage stub={reset(() => json(PREVIEW))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await canvas.findByLabelText(/^New password/), "short");
    await expect(canvas.getByRole("button", { name: /set new password/i })).toBeDisabled();
  },
};

/** The two boxes disagree: said here rather than after the password is stored. */
export const PasswordsDoNotMatch: Story = {
  render: () => <Stage stub={reset(() => json(PREVIEW))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await canvas.findByLabelText(/^New password/), "correct-horse");
    await userEvent.type(canvas.getByLabelText(/confirm password/i), "correct-hors");
    await expect(canvas.getByRole("alert")).toHaveTextContent(/do not match/);
    await expect(canvas.getByRole("button", { name: /set new password/i })).toBeDisabled();
  },
};

/**
 * The password is posted against the token from the url, and the member is sent
 * on to sign in: redeeming starts no session.
 */
export const SetsThePasswordAndSendsTheMemberToSignIn: Story = {
  render: () => <Stage stub={reset(() => json(PREVIEW))} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await canvas.findByLabelText(/^New password/), "correct-horse");
    await userEvent.type(canvas.getByLabelText(/confirm password/i), "correct-horse");
    await userEvent.click(canvas.getByRole("button", { name: /set new password/i }));
    const body = (await calls.expectSentBody("POST", `/password-reset/${TOKEN}`)) as {
      password: string;
    };
    await expect(body.password).toBe("correct-horse");
    await expect(
      await canvas.findByText(/Your password is set\. Sign in with it to continue\./),
    ).toBeVisible();
    await expect(canvas.getByRole("button", { name: /continue to sign in/i })).toBeVisible();
  },
};

/** The link was spent between the preview and the submit: the refusal is shown and the form stays. */
export const SpentLinkIsRefusedOnSubmit: Story = {
  render: () => (
    <Stage
      stub={reset(
        () => json(PREVIEW),
        () => json({ error: { message: "unauthenticated" } }, 401),
      )}
    />
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await canvas.findByLabelText(/^New password/), "correct-horse");
    await userEvent.type(canvas.getByLabelText(/confirm password/i), "correct-horse");
    await userEvent.click(canvas.getByRole("button", { name: /set new password/i }));
    await expect(await canvas.findByRole("alert")).toBeVisible();
    await expect(canvas.getByLabelText(/^New password/)).toBeVisible();
  },
};
