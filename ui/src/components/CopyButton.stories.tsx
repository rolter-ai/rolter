import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { CopyButton } from "./CopyButton";
import en from "@/lib/i18n/locales/en.json";

/**
 * A clipboard the story owns.
 *
 * The real one is unavailable in a headless browser — and deliberately
 * withheld by the platform on an insecure origin, which is the failure state
 * the button has a branch for — so neither outcome can be observed without
 * standing one in. Installed during render rather than in an effect, since a
 * child's effect runs before the parent's and the first click would otherwise
 * reach the real API.
 */
function WithClipboard({
  writeText,
  children,
}: {
  writeText: (value: string) => Promise<void>;
  children: React.ReactNode;
}) {
  const original = React.useRef<unknown>(undefined);
  React.useState(() => {
    original.current = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    return null;
  });
  React.useEffect(
    () => () => {
      const descriptor = original.current as PropertyDescriptor | undefined;
      if (descriptor) Object.defineProperty(navigator, "clipboard", descriptor);
      else Reflect.deleteProperty(navigator, "clipboard");
    },
    [],
  );
  return <>{children}</>;
}

const ADDRESS = "openai-prod/gpt-4o";

/** what the stub clipboard received, so a play function can read it back */
let copied: string[] = [];

const meta = {
  title: "Components/CopyButton",
  component: CopyButton,
  parameters: { layout: "padded" },
  args: { value: ADDRESS },
} satisfies Meta<typeof CopyButton>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  render: (args) => (
    <span className="inline-flex items-center gap-1 font-mono text-sm">
      {ADDRESS}
      <CopyButton {...args} />
    </span>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the accessible name names the value, so a row of these is not eleven
    // buttons all called "Copy"
    await expect(canvas.getByRole("button", { name: /openai-prod\/gpt-4o/ })).toBeVisible();
  },
};

/** The label is overridable where "Copy" alone would not say copy *what*. */
export const CustomLabel: Story = {
  args: { label: "Copy address prefix" },
  render: (args) => <CopyButton {...args} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("button", { name: /copy address prefix/i })).toBeVisible();
  },
};

/** The happy path: the value reaches the clipboard and the tick confirms it. */
export const Copies: Story = {
  render: (args) => {
    const written: string[] = [];
    return (
      <WithClipboard
        writeText={async (value) => {
          written.push(value);
          copied = written;
        }}
      >
        <span className="inline-flex items-center gap-1 font-mono text-sm">
          {ADDRESS}
          <CopyButton {...args} />
        </span>
      </WithClipboard>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const button = canvas.getByRole("button");
    copied = [];
    await userEvent.click(button);
    // what actually landed on the clipboard, not merely that the icon changed
    await waitFor(() => expect(copied).toEqual([ADDRESS]));
    // and the confirmation is announced, not only drawn: the icon swap alone
    // is invisible to a screen reader
    await expect(button).toHaveAttribute("title", en.common.copied);
  },
};

/**
 * The clipboard API is withheld on an insecure origin — a plain-http dashboard
 * on a LAN is the common case — and the button says so instead of silently
 * doing nothing.
 */
export const ClipboardRefused: Story = {
  render: (args) => (
    <WithClipboard writeText={async () => Promise.reject(new Error("denied"))}>
      <CopyButton {...args} />
    </WithClipboard>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const button = canvas.getByRole("button");
    await userEvent.click(button);
    await waitFor(() => expect(button).toHaveAttribute("title", en.common.copyFailed));
  },
};

/** In a list the buttons stay distinguishable, each named by its own row. */
export const InAList: Story = {
  render: () => (
    <ul className="space-y-1 text-sm">
      {["openai-prod/gpt-4o", "anthropic-prod/claude-sonnet", "vllm-cluster/llama-3.1-70b"].map(
        (address) => (
          <li key={address} className="flex items-center gap-1 font-mono">
            {address}
            <CopyButton value={address} />
          </li>
        ),
      )}
    </ul>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getAllByRole("button")).toHaveLength(3);
    await expect(
      canvas.getByRole("button", { name: /vllm-cluster\/llama-3\.1-70b/ }),
    ).toBeVisible();
  },
};
