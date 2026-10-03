import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Security from "./Security";
import {
  Harness,
  Toasted,
  expectLoadError,
  expectSkeleton,
  expectToast,
  json,
} from "./story-harness";
import type { SecuritySettingsDto } from "@/lib/api";
import { atMobile, atTablet, expectNoHorizontalOverflow } from "@/lib/story-viewport";

const BASE: SecuritySettingsDto = {
  virtual_key_required: true,
  allowed_origins: ["https://app.example.com"],
  allowed_headers: ["x-request-id"],
  required_headers: { "x-tenant": "acme" },
  auth_bypass_routes: ["/healthz"],
  dashboard_auth_enabled: true,
  dashboard_credential_ref: "ROLTER_DASHBOARD_SECRET",
  dashboard_secret_configured: true,
  updated_at: "2026-08-01T09:00:00Z",
};

const meta = {
  title: "Screens/Security",
  component: Security,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Security>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={async () => json(BASE)}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Password protect the dashboard")).toBeVisible());
  },
};

// a settings form's honest placeholder is the shape of the panels it is about
// to render, not the word "Loading" on one line
export const Loading: Story = {
  render: () => (
    <Harness fetchStub={() => new Promise<Response>(() => {})}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

/**
 * The #962 failure this screen had verbatim: every cause rendered "Security
 * settings need superadmin access", so a 500 read as a permissions problem.
 * A 403 is the one case where that sentence was true, and it is now the only
 * case where it is said.
 */
export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={async () => json({ error: { message: "forbidden" } }, 403)}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /You do not have access to security settings/);
    // a 403 is not transient, so no retry is offered
    await expect(canvas.queryByRole("button", { name: /Try again/ })).not.toBeInTheDocument();
  },
};

export const Error_: Story = {
  name: "Error",
  render: () => (
    <Harness fetchStub={async () => json({ error: { message: "sqlx: pool timed out" } }, 500)}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /failed to return security settings/i);
    // the control plane's own words survive rather than being swallowed
    await expect(canvas.getByText(/pool timed out/)).toBeVisible();
    await expect(canvas.getByRole("button", { name: /Try again/ })).toBeInTheDocument();
  },
};

/**
 * A save that went through used to be a line of text in the footer that the
 * next keystroke wiped. It is a toast now, so it survives the sheet, the
 * scroll and the operator looking away (#1197).
 */
export const SavesChanges: Story = {
  render: () => (
    <Harness
      fetchStub={async (_input, init) =>
        init?.method === "PUT" ? json({ ...BASE, ...JSON.parse(String(init.body)) }) : json(BASE)
      }
    >
      <Toasted>
        <Security />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const origins = await canvas.findByDisplayValue("https://app.example.com");
    await userEvent.clear(origins);
    await userEvent.type(origins, "https://console.example.com");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /security settings updated/i);
  },
};

/**
 * The save is refused (#1607).
 *
 * This screen is a form, not a sheet, so "the draft survives" means the edited
 * field still holds what was typed rather than snapping back to the value the
 * server last confirmed — and the refusal reaches the toast queue, which is the
 * only place a rejected write is reported here.
 */
export const SaveRejectedByTheServer: Story = {
  render: () => (
    <Harness
      fetchStub={async (_input, init) =>
        init?.method === "PUT"
          ? json({ error: { message: "https://console.example.com is not a valid origin" } }, 422)
          : json(BASE)
      }
    >
      <Toasted>
        <Security />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const origins = await canvas.findByDisplayValue("https://app.example.com");
    await userEvent.clear(origins);
    await userEvent.type(origins, "https://console.example.com");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /is not a valid origin/, "error");
    await waitFor(() => expect(origins).toHaveValue("https://console.example.com"));
  },
};

/** The settings panels are one column below `sm`, and nothing spills (#1203). */
export const Mobile: Story = {
  ...atMobile,
  render: () => (
    <Harness fetchStub={async () => json(BASE)}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Password protect the dashboard")).toBeVisible());
    await expectNoHorizontalOverflow();
  },
};

export const Tablet: Story = {
  ...atTablet,
  render: () => (
    <Harness fetchStub={async () => json(BASE)}>
      <Security />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Password protect the dashboard")).toBeVisible());
    await expectNoHorizontalOverflow();
  },
};
