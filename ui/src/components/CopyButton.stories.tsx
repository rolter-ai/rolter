import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { CopyButton, type CopyState } from "./CopyButton";
import en from "@/lib/i18n/locales/en.json";
import { stubClipboard } from "@/pages/story-harness";

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

/**
 * Left to itself a failure is brief: the glyph and the tooltip go back to
 * normal after a moment, which is enough for an address that can be copied
 * again. Nothing changes for a call site that does not ask for more (#2327).
 */
export const AFailureResetsByDefault: Story = {
  beforeEach: stubClipboard(() => Promise.reject(new Error("denied"))),
  render: (args) => <CopyButton {...args} />,
  play: async ({ canvasElement }) => {
    const button = within(canvasElement).getByRole("button");
    await userEvent.click(button);
    await waitFor(() => expect(button).toHaveAttribute("title", en.common.copyFailed));
    await waitFor(() => expect(button).not.toHaveAttribute("title", en.common.copyFailed), {
      timeout: 3000,
    });
  },
};

/**
 * Where the value is shown once, `persistFailure` holds the failure past the
 * timer, and a different value clears it: a value that has not been copied yet
 * has not failed to copy.
 */
export const PersistFailureHoldsUntilTheValueChanges: Story = {
  beforeEach: stubClipboard(() => Promise.reject(new Error("denied"))),
  render: (args) => {
    const Demo = () => {
      const [value, setValue] = React.useState(args.value);
      return (
        <div className="flex items-center gap-2">
          <CopyButton value={value} persistFailure />
          <button type="button" onClick={() => setValue("openai-prod/gpt-4o-mini")}>
            Another address
          </button>
        </div>
      );
    };
    return <Demo />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const button = canvas.getByRole("button", { name: /openai-prod\/gpt-4o$/ });
    await userEvent.click(button);
    await waitFor(() => expect(button).toHaveAttribute("title", en.common.copyFailed));

    // well past the 1.6 s the default reset would have taken
    await new Promise((resolve) => setTimeout(resolve, 1800));
    await expect(button).toHaveAttribute("title", en.common.copyFailed);

    await userEvent.click(canvas.getByRole("button", { name: "Another address" }));
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: /gpt-4o-mini/ })).toHaveAttribute(
        "title",
        en.common.copy,
      ),
    );
  },
};

let attempts = 0;

/**
 * `onStateChange` hears every press, including a second failure on the same
 * value, which is how a caller draws a message that clears on the next press
 * and comes back if that press fails too.
 */
export const OnStateChangeHearsEveryPress: Story = {
  beforeEach: stubClipboard(async () => {
    attempts += 1;
    if (attempts !== 3) throw new Error("denied");
  }),
  render: (args) => {
    const Demo = () => {
      const [heard, setHeard] = React.useState<CopyState[]>([]);
      return (
        <div className="flex items-center gap-2">
          <CopyButton
            value={args.value}
            persistFailure
            onStateChange={(state) => setHeard((all) => [...all, state])}
          />
          <output aria-label="heard">{heard.join(",")}</output>
        </div>
      );
    };
    return <Demo />;
  },
  play: async ({ canvasElement }) => {
    attempts = 0;
    const canvas = within(canvasElement);
    const button = canvas.getByRole("button");
    const heard = canvas.getByLabelText("heard");

    await userEvent.click(button);
    await waitFor(() => expect(heard).toHaveTextContent(/^failed$/));
    // the same failure again is still told, after the press cleared the first
    await userEvent.click(button);
    await waitFor(() => expect(heard).toHaveTextContent(/^failed,idle,failed$/));
    // and a press that works clears the failure before it reports
    await userEvent.click(button);
    await waitFor(() => expect(heard).toHaveTextContent(/^failed,idle,failed,idle,copied$/));
    await expect(button).toHaveAttribute("title", en.common.copied);
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
