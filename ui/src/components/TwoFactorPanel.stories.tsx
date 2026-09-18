import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { TwoFactorPanel } from "./TwoFactorPanel";
import {
  Harness,
  Toasted,
  cancelConfirmation,
  confirmDestructive,
  expectEmptyState,
  expectLoadError,
  expectSkeleton,
  expectToast,
  json,
  pending,
  recording,
  type FetchStub,
} from "@/pages/story-harness";
import type { MfaStatus } from "@/lib/api";

const OFF: MfaStatus = {
  enabled: false,
  enrolment_pending: false,
  recovery_codes_remaining: 0,
  policy: "off",
  required: false,
};

const ON: MfaStatus = {
  enabled: true,
  enrolment_pending: false,
  recovery_codes_remaining: 7,
  policy: "optional",
  required: false,
};

const SECRET = {
  otpauth_uri:
    "otpauth://totp/rolter:anya@acme.co?secret=JBSWY3DPEHPK3PXP&issuer=rolter&digits=6&period=30",
  secret: "JBSWY3DPEHPK3PXP",
  digits: 6,
  period: 30,
};

const CODES = Array.from({ length: 10 }, (_, i) => `4KJH2Q${String(i).padStart(2, "0")}ZXRT9WM`);

/**
 * One stub for the four endpoints behind the panel.
 *
 * `status` is a function rather than a value so a story can answer the reload
 * after a mutation differently from the first read — which is the only way to
 * assert that the panel went from "off" to "on" rather than that a fixture
 * changed under it.
 */
const mfa =
  (
    status: () => Response,
    { enroll = () => json(SECRET), confirm = () => json({ recovery_codes: CODES }) } = {},
  ): FetchStub =>
  async (input, init) => {
    const url = String(input);
    const method = (init?.method ?? "GET").toUpperCase();
    if (url.includes("/me/mfa/enroll")) return enroll();
    if (url.includes("/me/mfa/confirm")) return confirm();
    if (url.includes("/me/mfa/recovery-codes")) return json({ recovery_codes: CODES });
    if (url.includes("/me/mfa") && method === "DELETE") return json(null, 204);
    return status();
  };

// the recorders the two destructive stories assert against. Declared here
// rather than beside their stories so the play function and the render share
// one object without depending on module evaluation order
let regenerations = recording(mfa(() => json(ON)));
let removals = recording(mfa(() => json(ON)));

const meta = {
  title: "Components/TwoFactorPanel",
  component: TwoFactorPanel,
} satisfies Meta<typeof TwoFactorPanel>;
export default meta;
type Story = StoryObj<typeof meta>;

/** No factor armed, and no policy asking for one: an invitation, not a warning. */
export const NotEnrolled: Story = {
  render: () => (
    <Harness fetchStub={mfa(() => json(OFF))}>
      <TwoFactorPanel />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(
      canvasElement,
      /No second factor on this account/,
      /Set up two-factor authentication/,
    );
  },
};

/**
 * The same state under `required_all`. The copy changes because the stakes do:
 * the *next* sign-in is refused, which is a deadline rather than advice.
 */
export const NotEnrolledButRequired: Story = {
  render: () => (
    <Harness fetchStub={mfa(() => json({ ...OFF, policy: "required_all", required: true }))}>
      <TwoFactorPanel />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/your next sign-in is refused/i)).toBeInTheDocument();
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <TwoFactorPanel />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

export const LoadFailed: Story = {
  render: () => (
    <Harness fetchStub={mfa(() => json({ error: { message: "boom" } }, 500))}>
      <TwoFactorPanel />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /two-factor authentication/i);
  },
};

export const Enrolled: Story = {
  render: () => (
    <Harness fetchStub={mfa(() => json(ON))}>
      <TwoFactorPanel />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/7 recovery codes left/)).toBeInTheDocument();
    await expect(canvas.getByRole("button", { name: /remove second factor/i })).toBeEnabled();
  },
};

/**
 * Zero codes left is the state one lost phone away from needing an
 * administrator, so it says that rather than printing a `0`.
 */
export const NoRecoveryCodesLeft: Story = {
  render: () => (
    <Harness fetchStub={mfa(() => json({ ...ON, recovery_codes_remaining: 0 }))}>
      <TwoFactorPanel />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/No recovery codes left/)).toBeInTheDocument();
  },
};

/**
 * Under a `required_*` policy the control plane refuses the removal, so the
 * button is refused here too — with a `title` saying why, because a disabled
 * control that explains nothing is the same non-answer the 403 was.
 */
export const RemovalRefusedByPolicy: Story = {
  render: () => (
    <Harness fetchStub={mfa(() => json({ ...ON, policy: "required_all", required: true }))}>
      <TwoFactorPanel />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const button = await canvas.findByRole("button", { name: /remove second factor/i });
    await waitFor(() => expect(button).toBeDisabled());
    await expect(button).toHaveAttribute(
      "title",
      "Your organization requires a second factor on this account",
    );
  },
};

/**
 * The whole ceremony: secret → code → codes → acknowledgement.
 *
 * The assertion that matters most is the last one — `Done` stays refused until
 * the box is ticked, because the codes are shown exactly once and a dialog
 * dismissed by a stray click is a user with no way back into their account.
 */
export const EnrolsAndShowsRecoveryCodes: Story = {
  render: () => {
    let armed = false;
    return (
      <Harness
        fetchStub={mfa(() => json(armed ? ON : OFF), {
          confirm: () => {
            armed = true;
            return json({ recovery_codes: CODES });
          },
        })}
      >
        <Toasted>
          <TwoFactorPanel />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /set up two-factor/i }));
    const dialog = await within(document.body).findByRole("dialog");
    // the secret is offered as text as well as as a QR: a user on a desktop
    // authenticator, or without a camera, has no other way through this step
    await expect(within(dialog).getByText(SECRET.secret)).toBeInTheDocument();
    // and the spent-code rule is said before it bites, not after
    await expect(
      within(dialog).getByText(/next sign-in needs the following one/i),
    ).toBeInTheDocument();

    await userEvent.type(within(dialog).getByLabelText(/code from the app/i), "123456");
    await userEvent.click(within(dialog).getByRole("button", { name: "Turn on" }));

    const codes = await waitFor(() => within(document.body).getByRole("dialog"));
    await waitFor(() => expect(within(codes).getByText(/Save your recovery codes/)).toBeVisible());
    await expect(within(codes).getByText(new RegExp(CODES[0]))).toBeInTheDocument();
    const done = within(codes).getByRole("button", { name: "Done" });
    await expect(done).toBeDisabled();
    await userEvent.click(within(codes).getByRole("checkbox"));
    await expect(done).toBeEnabled();
    await userEvent.click(done);

    await expectToast(canvasElement, /Two-factor authentication is on/);
  },
};

/** A wrong code leaves the dialog open with the control plane's own words. */
export const AWrongEnrolmentCodeIsReported: Story = {
  render: () => (
    <Harness
      fetchStub={mfa(() => json(OFF), {
        confirm: () =>
          json(
            {
              error: {
                message: "that code did not match; check the clock on the device and try again",
              },
            },
            400,
          ),
      })}
    >
      <TwoFactorPanel />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /set up two-factor/i }));
    const dialog = await within(document.body).findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText(/code from the app/i), "000000");
    await userEvent.click(within(dialog).getByRole("button", { name: "Turn on" }));
    await waitFor(() =>
      expect(within(dialog).getByText(/check the clock on the device/i)).toBeVisible(),
    );
    // the dialog survives, so the next attempt does not need a fresh secret
    await expect(within(dialog).getByLabelText(/code from the app/i)).toBeVisible();
  },
};

/** Regenerating kills the batch the user is holding, so it asks first. */
export const RegeneratingRecoveryCodesConfirmsFirst: Story = {
  render: () => {
    const calls = recording(mfa(() => json(ON)));
    regenerations = calls;
    return (
      <Harness fetchStub={calls.stub}>
        <Toasted>
          <TwoFactorPanel />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const button = await canvas.findByRole("button", { name: /regenerate recovery codes/i });
    await userEvent.click(button);
    await cancelConfirmation();
    regenerations.expectNotSent("POST", "/me/mfa/recovery-codes");

    await userEvent.click(button);
    await confirmDestructive(/stops working immediately/, "Regenerate");
    await regenerations.expectSent("POST", "/me/mfa/recovery-codes");
    await waitFor(() =>
      expect(within(document.body).getByText(/Save your recovery codes/)).toBeVisible(),
    );
  },
};

/**
 * Removal needs a code as well as consent — the server refuses it without one
 * — so the confirm button waits for the field rather than spending a round
 * trip to be told the body was empty.
 */
export const RemovingTheFactorNeedsACode: Story = {
  render: () => {
    const calls = recording(mfa(() => json(ON)));
    removals = calls;
    return (
      <Harness fetchStub={calls.stub}>
        <Toasted>
          <TwoFactorPanel />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: /remove second factor/i }));
    const dialog = await within(document.body).findByRole("dialog");
    await expect(within(dialog).getByRole("button", { name: "Remove" })).toBeDisabled();
    await userEvent.type(
      within(dialog).getByLabelText(/code from the app, or a recovery code/i),
      "654321",
    );
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    const body = await removals.expectSentBody<{ code: string }>("DELETE", "/me/mfa");
    await expect(body.code).toBe("654321");
    await expectToast(canvasElement, /Two-factor authentication removed/);
  },
};
