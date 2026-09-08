import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { CopyAsCodeButton } from "./CodeSnippetDialog";

const meta = {
  title: "Overlays/CodeSnippetDialog",
  component: CopyAsCodeButton,
  parameters: { layout: "centered" },
  args: { request: { model: "llama-3.1-8b", prompt: "hello there" } },
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
