import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { Button } from "./button";
import { SecretRevealDialog } from "./secret-reveal";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { answerSecretClosePrompt, secretClosePrompt, stubClipboard } from "@/pages/story-harness";
import { atMobile, expectInViewport, expectNoHorizontalOverflow } from "@/lib/story-viewport";

const KEY = "sk-rolter-shown-once-demo";
// a key with no break opportunity, wider than a phone's dialog
const LONG_KEY = `sk-rolter-${"0123456789abcdef".repeat(8)}`;

const meta = {
  title: "Overlays/SecretRevealDialog",
  component: SecretRevealDialog,
  parameters: { layout: "fullscreen" },
  // the demo owns the open state; these satisfy the required-prop types
  args: {
    name: "story-secret",
    open: false,
    onOpenChange: () => {},
    title: "Key created",
    description: "This is the only time the plaintext key is shown.",
    secret: KEY,
    copyLabel: "Copy key",
  },
} satisfies Meta<typeof SecretRevealDialog>;

export default meta;
type Story = StoryObj<typeof meta>;

function Demo({ secret = KEY, next }: { secret?: string; next?: React.ReactNode }) {
  const [shown, setShown] = React.useState<string | null>(null);
  return (
    <div className="p-6">
      <Button onClick={() => setShown(secret)}>Create key</Button>
      <SecretRevealDialog
        name="story-secret"
        open={shown !== null}
        onOpenChange={(open) => !open && setShown(null)}
        title="Key created"
        description="This is the only time the plaintext key is shown."
        secret={shown ?? ""}
        copyLabel="Copy key"
        size={next ? "lg" : "md"}
      >
        {next}
      </SecretRevealDialog>
    </div>
  );
}

const screen = () => within(document.body);

/** what the stub clipboard received, so a play can read it back */
let written: string[] = [];
const record = async (value: string) => {
  written = [...written, value];
};
const refuse = () => Promise.reject(new Error("denied"));

const selected = () => window.getSelection()?.toString() ?? "";

async function reveal(name = "Key created") {
  await userEvent.click(screen().getByRole("button", { name: "Create key" }));
  return screen().findByRole("dialog", { name });
}

const copyButton = (dialog: HTMLElement) =>
  within(dialog).getByRole("button", { name: /Copy key/ });

async function expectClosed() {
  await waitFor(() => expect(screen().queryByRole("dialog")).toBeNull());
}

export const Default: Story = {
  render: () => <Demo />,
  play: async () => {
    const dialog = await reveal();
    await expect(dialog).toHaveAccessibleDescription(/only time the plaintext key/);
    const value = within(dialog).getByText(KEY);
    await waitFor(() => expect(value).toBeVisible());
    // one click takes the whole value, so copying it by hand is two gestures
    await expect(getComputedStyle(value).userSelect).toBe("all");
    // the value is mono, because someone pastes it into a terminal
    await expect(getComputedStyle(value).fontFamily).toMatch(/mono/i);
  },
};

/**
 * A value that reached the clipboard is safe to leave, so closing it asks
 * nothing: a question that always appears is one people learn to click through.
 */
export const CopyingMeansClosingAsksNothing: Story = {
  beforeEach: stubClipboard(record),
  render: () => <Demo />,
  play: async () => {
    written = [];
    const dialog = await reveal();
    const copy = copyButton(dialog);
    await userEvent.click(copy);
    // what landed on the clipboard, not merely that the icon changed
    await waitFor(() => expect(written).toEqual([KEY]));
    await waitFor(() => expect(copy).toHaveAttribute("title", en.common.copied));
    await userEvent.click(within(dialog).getByRole("button", { name: en.common.done }));
    await expectClosed();
  },
};

/**
 * Escape, the scrim, the close button and Done all close the reveal, and
 * all four ask first while the value is uncopied. Cancelling the question
 * keeps the dialog and the value; only the explicit confirm closes it.
 */
export const EveryWayOutAsksWhileUncopied: Story = {
  render: () => <Demo />,
  play: async () => {
    let dialog = await reveal();
    const ways: [string, () => Promise<unknown>][] = [
      ["Escape", () => userEvent.keyboard("{Escape}")],
      [
        "the close button",
        () => userEvent.click(within(dialog).getByRole("button", { name: en.common.close })),
      ],
      [
        "the scrim",
        async () => {
          const scrim = dialog.previousElementSibling as HTMLElement;
          await expect(scrim).toHaveAttribute("aria-hidden", "true");
          await userEvent.click(scrim);
        },
      ],
      ["Done", () => userEvent.click(within(dialog).getByRole("button", { name: en.common.done }))],
    ];
    for (const [, close] of ways) {
      await close();
      const prompt = await secretClosePrompt();
      await waitFor(() =>
        expect(within(prompt).getByText(en.common.secret.closeBody)).toBeVisible(),
      );
      await answerSecretClosePrompt(false);
      // cancelling keeps the reveal, and the value is still there to copy
      dialog = await screen().findByRole("dialog", { name: "Key created" });
      await expect(within(dialog).getByText(KEY)).toBeVisible();
    }
    await userEvent.click(within(dialog).getByRole("button", { name: en.common.done }));
    await answerSecretClosePrompt(true);
    await expectClosed();
  },
};

/**
 * The clipboard is withheld on a plain-http dashboard. The copy fails, the
 * dialog says so in a line that stays, the value is selected ready for a
 * keystroke, and the button that selects it again works (#2327).
 */
export const AFailedCopyLeavesAMessage: Story = {
  beforeEach: stubClipboard(refuse),
  render: () => <Demo />,
  play: async () => {
    const dialog = await reveal();
    const copy = copyButton(dialog);
    await userEvent.click(copy);

    const alert = await within(dialog).findByRole("alert");
    await waitFor(() => expect(alert).toBeVisible());
    await expect(alert).toHaveTextContent(en.common.copyFailed);
    await expect(alert).toHaveTextContent(en.common.secret.copyByHand);

    // the button's own glyph resets after 1.6 s; this line does not
    await new Promise((resolve) => setTimeout(resolve, 1800));
    await expect(within(dialog).getByRole("alert")).toBeVisible();
    await expect(copy).toHaveAttribute("title", en.common.copyFailed);

    // the value stays, and is already selected so Ctrl+C is one keystroke
    await expect(within(dialog).getByText(KEY)).toBeVisible();
    await expect(selected()).toBe(KEY);

    window.getSelection()?.removeAllRanges();
    await expect(selected()).toBe("");
    await userEvent.click(within(dialog).getByRole("button", { name: en.common.secret.select }));
    await expect(selected()).toBe(KEY);
  },
};

let attempts = 0;

/** The message belongs to the press that failed: the next press clears it. */
export const AFailedCopyClearsOnTheNextPress: Story = {
  beforeEach: stubClipboard(async (value) => {
    attempts += 1;
    if (attempts === 1) throw new Error("denied");
    written = [...written, value];
  }),
  render: () => <Demo />,
  play: async () => {
    attempts = 0;
    written = [];
    const dialog = await reveal();
    const copy = copyButton(dialog);
    await userEvent.click(copy);
    await within(dialog).findByRole("alert");

    await userEvent.click(copy);
    await waitFor(() => expect(within(dialog).queryByRole("alert")).toBeNull());
    await waitFor(() => expect(copy).toHaveAttribute("title", en.common.copied));
    await expect(written).toEqual([KEY]);

    // and a copy that worked closes without the question
    await userEvent.click(within(dialog).getByRole("button", { name: en.common.done }));
    await expectClosed();
  },
};

/**
 * With the clipboard api refused, selecting the value and pressing Ctrl+C is
 * the way out, and it counts: the dialog says it was copied and closes without
 * asking whether it was.
 */
export const CopyingByHandCounts: Story = {
  beforeEach: stubClipboard(refuse),
  render: () => <Demo />,
  play: async () => {
    const dialog = await reveal();
    await userEvent.click(copyButton(dialog));
    await within(dialog).findByRole("alert");
    await expect(selected()).toBe(KEY);

    // what the keyboard does with that selection
    document.dispatchEvent(new ClipboardEvent("copy", { bubbles: true }));
    const status = await within(dialog).findByRole("status");
    await expect(status).toHaveTextContent(en.common.copied);
    await expect(within(dialog).queryByRole("alert")).toBeNull();

    await userEvent.click(within(dialog).getByRole("button", { name: en.common.done }));
    await expectClosed();
  },
};

/** Half a key on the clipboard is not a copy of it, so closing still asks. */
export const APartialSelectionIsNotACopy: Story = {
  beforeEach: stubClipboard(refuse),
  render: () => <Demo />,
  play: async () => {
    const dialog = await reveal();
    const text = within(dialog).getByText(KEY).firstChild as Text;
    const range = document.createRange();
    range.setStart(text, 0);
    range.setEnd(text, 6);
    const selection = window.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(range);
    await expect(selected()).toBe("sk-rol");

    document.dispatchEvent(new ClipboardEvent("copy", { bubbles: true }));
    await expect(within(dialog).queryByRole("status")).toBeNull();

    await userEvent.click(within(dialog).getByRole("button", { name: en.common.done }));
    await secretClosePrompt();
    await answerSecretClosePrompt(true);
    await expectClosed();
  },
};

/** What comes next sits under the value, and a document-sized dialog makes room for it. */
export const WithANextStep: Story = {
  render: () => <Demo next={<p>Send this key to the team that will run the batch job.</p>} />,
  play: async () => {
    const dialog = await reveal();
    await waitFor(() =>
      expect(within(dialog).getByText(/Send this key to the team/)).toBeVisible(),
    );
    // the wide size: the narrow one is 448 px
    await waitFor(() => expect(dialog.getBoundingClientRect().width).toBeGreaterThan(480));
  },
};

/** The question, the message and the select button follow the locale. */
export const InRussian: Story = {
  globals: { locale: "ru" },
  beforeEach: stubClipboard(refuse),
  render: () => <Demo />,
  play: async () => {
    const dialog = await reveal();
    await userEvent.click(within(dialog).getByRole("button", { name: /Copy key/ }));
    const alert = await within(dialog).findByRole("alert");
    await expect(alert).toHaveTextContent(ru.common.copyFailed);
    await expect(alert).toHaveTextContent(ru.common.secret.copyByHand);
    await expect(
      within(dialog).getByRole("button", { name: ru.common.secret.select }),
    ).toBeVisible();

    await userEvent.click(within(dialog).getByRole("button", { name: ru.common.done }));
    const prompt = await screen().findByRole("dialog", { name: ru.common.secret.closeTitle });
    await expect(within(prompt).getByText(ru.common.secret.closeBody)).toBeVisible();
    await userEvent.click(
      within(prompt).getByRole("button", { name: ru.common.secret.closeConfirm }),
    );
    await expectClosed();
  },
};

/**
 * 375 px, the Russian copy and a key with no place to break: the worst line
 * the dialog has to hold. The value wraps inside the panel, the failed-copy
 * message and its button fit, and nothing is pushed past the window.
 */
export const FitsAPhoneAfterAFailedCopy: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  beforeEach: stubClipboard(refuse),
  render: () => <Demo secret={LONG_KEY} />,
  play: async () => {
    const dialog = await reveal();
    await userEvent.click(within(dialog).getByRole("button", { name: /Copy key/ }));
    await within(dialog).findByRole("alert");
    await expectInViewport(dialog);
    await expect(within(dialog).getByText(LONG_KEY)).toBeVisible();
    await expectInViewport(within(dialog).getByRole("button", { name: ru.common.secret.select }));
    await expectNoHorizontalOverflow();
  },
};
