import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { CopyAsCodeButton } from "./CodeSnippetDialog";
import type { ClientSettingsDto } from "@/lib/api";
import { expectGateAnswered, Harness, json, recording, routes } from "@/pages/story-harness";

const SAVED: ClientSettingsDto = {
  public_base_url: "https://gateway.example.com",
  forwarded_headers: [],
  injected_headers: {},
  request_id_header: "x-request-id",
  updated_at: "2026-08-05T12:00:00Z",
  always_propagated: ["traceparent", "tracestate", "b3"],
  reserved: ["authorization"],
};

/** a deployment with a public base URL saved on Client Settings */
const withSavedBaseUrl = () =>
  recording(async (input) =>
    String(input).includes("/api/v1/client-settings") ? json(SAVED) : json([]),
  );

/** a stub that answers nothing in particular; no default story reads client settings */
const noSettings = routes([]);

const meta = {
  title: "Overlays/CodeSnippetDialog",
  component: CopyAsCodeButton,
  parameters: { layout: "centered" },
  args: { request: { model: "llama-3.1-8b", prompt: "hello there" } },
  // no role, so no capability provider and no client-settings read: the
  // snippet falls back to the dashboard's /gw proxy, as it does for a caller
  // whose gate has not answered yet
  render: (args) => (
    <Harness fetchStub={noSettings}>
      <CopyAsCodeButton {...args} />
    </Harness>
  ),
} satisfies Meta<typeof CopyAsCodeButton>;

export default meta;
type Story = StoryObj<typeof meta>;

// the dialog renders through a portal onto document.body, so canvasElement is
// empty — query the whole document
const screen = () => within(document.body);

const open = async () => {
  await userEvent.click(await screen().findByRole("button", { name: /copy as code/i }));
  return waitFor(() => screen().getByRole("dialog"));
};

/** the snippet, once the highlighter chunk has arrived */
const highlighted = async (dialog: HTMLElement) =>
  waitFor(() => {
    const token = dialog.querySelector(".rl-code .token");
    if (!token) throw new Error("not highlighted yet");
    return token;
  });

export const Curl: Story = {
  play: async () => {
    const dialog = await open();
    // curl is the default because it needs no project to try. the snippet is
    // split across token spans now, so it is the region's text that carries it
    await expect(dialog).toHaveTextContent(/curl .*\/gw\/v1\/chat\/completions/);
    await expect(dialog).toHaveTextContent(/llama-3\.1-8b/);
  },
};

/**
 * The dialog is the moment an operator stops clicking and starts integrating,
 * and it used to wrap a long URL into unreadable fragments in a `max-w-md`
 * panel (#948). Measured rather than asserted against the class, so the fix is
 * the width an operator actually gets.
 */
export const IsWideEnoughToRead: Story = {
  play: async () => {
    const dialog = await open();
    await waitFor(() => expect(dialog.getBoundingClientRect().width).toBeGreaterThan(640));
    // and the snippet scrolls sideways rather than wrapping mid-token
    const region = within(dialog).getByRole("region", { name: /code snippet/i });
    await expect(region).toHaveStyle({ whiteSpace: "pre" });
  },
};

/** Every language is highlighted, from the same tokeniser the rest of the
 *  dashboard uses — bundled, never fetched (#948, #949). */
export const Highlighted: Story = {
  play: async () => {
    const dialog = await open();
    const token = await highlighted(dialog);
    await expect(token).toBeVisible();
  },
};

export const SwitchesLanguage: Story = {
  play: async () => {
    const dialog = await open();
    const canvas = within(dialog);

    await userEvent.click(canvas.getByRole("tab", { name: "Python" }));
    await waitFor(() => expect(dialog).toHaveTextContent(/from openai import OpenAI/));

    await userEvent.click(canvas.getByRole("tab", { name: "JavaScript" }));
    await waitFor(() => expect(dialog).toHaveTextContent(/import OpenAI from "openai"/));
    await expect(canvas.getByRole("tab", { name: "JavaScript" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  },
};

/** The tabs are buttons in a tablist, so they are reachable and operable
 *  without a pointer — the snippet below the fold is not a mouse-only region. */
export const KeyboardOperable: Story = {
  play: async () => {
    const dialog = await open();
    const canvas = within(dialog);

    const python = canvas.getByRole("tab", { name: "Python" });
    python.focus();
    await expect(python).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(python).toHaveAttribute("aria-selected", "true"));

    // the scroll region itself takes focus, so the rest of a long snippet can
    // be reached with the arrow keys (#1181)
    const region = canvas.getByRole("region", { name: /code snippet/i });
    region.focus();
    await expect(region).toHaveFocus();
  },
};

// the snippet is pasted into tickets and chat, so it must never carry the
// operator's live virtual key
export const NeverInlinesTheKey: Story = {
  play: async () => {
    const dialog = await open();
    const canvas = within(dialog);
    for (const lang of ["curl", "Python", "JavaScript"]) {
      await userEvent.click(canvas.getByRole("tab", { name: lang }));
      await waitFor(() => expect(dialog.textContent ?? "").toContain("ROLTER_API_KEY"));
      await expect(dialog.textContent ?? "").not.toContain("sk-rolter-");
    }
  },
};

const asSuperadmin = withSavedBaseUrl();

/**
 * A superadmin, who may read client settings, gets the saved public base URL
 * in every language, and no comment calling it the dashboard's proxy (#2218).
 */
export const UsesTheSavedBaseUrl: Story = {
  render: (args) => (
    <Harness fetchStub={asSuperadmin.stub} role="superadmin">
      <CopyAsCodeButton {...args} />
    </Harness>
  ),
  play: async () => {
    const dialog = await open();
    const canvas = within(dialog);
    await waitFor(() =>
      expect(dialog).toHaveTextContent(
        /curl https:\/\/gateway\.example\.com\/v1\/chat\/completions/,
      ),
    );
    for (const lang of ["Python", "JavaScript"]) {
      await userEvent.click(canvas.getByRole("tab", { name: lang }));
      await waitFor(() =>
        expect(dialog).toHaveTextContent(/"https:\/\/gateway\.example\.com\/v1"/),
      );
      await expect(dialog.textContent ?? "").not.toContain("/gw/");
      await expect(dialog.textContent ?? "").not.toContain("in production");
    }
    await asSuperadmin.expectSent("GET", "/api/v1/client-settings");
  },
};

const asAdmin = withSavedBaseUrl();

/**
 * Client settings are superadmin-only, so an org admin never asks for them —
 * the 403 would say nothing the gate did not — and gets the `/gw` proxy with
 * the comment saying so, even on a deployment that saved a public base URL.
 */
export const AnAdminKeepsTheProxy: Story = {
  render: (args) => (
    <Harness fetchStub={asAdmin.stub} role="admin">
      <CopyAsCodeButton {...args} />
    </Harness>
  ),
  play: async () => {
    await expectGateAnswered();
    const dialog = await open();
    await waitFor(() => expect(dialog).toHaveTextContent(/curl .*\/gw\/v1\/chat\/completions/));
    await expect(dialog).toHaveTextContent(/in production/);
    asAdmin.expectNotSent("GET", "/api/v1/client-settings");
  },
};

// the entry point shipped as a bare `</>` glyph, so the dialog behind it was
// undiscoverable without clicking an anonymous icon (#963)
export const TriggerIsLabelled: Story = {
  play: async () => {
    const trigger = await screen().findByRole("button", { name: /copy as code/i });
    // the visible label, not just the accessible name the aria-label supplied
    await expect(trigger).toHaveTextContent(/copy as code/i);
    await expect(trigger).toHaveAttribute("title", expect.stringMatching(/copy as code/i));
  },
};
