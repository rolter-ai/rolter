import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { CopyableText, CopyableValue } from "./copyable-value";
import { Skeleton } from "./skeleton";
import en from "@/lib/i18n/locales/en.json";
import { stubClipboard } from "@/pages/story-harness";
import { atMobile, expectNoHorizontalOverflow } from "@/lib/story-viewport";

const ADDRESS = "https://rolter.example.com/scim/v2";
// an address with no break opportunity, wider than a phone
const LONG = `https://rolter.example.com/${"0123456789abcdef".repeat(8)}`;
const ID = "req_01JABCDEF0123456789XYZ";

const meta = {
  title: "Forms/CopyableValue",
  component: CopyableValue,
  parameters: { layout: "padded" },
  args: {
    label: "SCIM base URL",
    value: ADDRESS,
    copyLabel: "Copy SCIM base URL",
    hint: "Point your identity provider's SCIM connector here.",
    testId: "copyable",
  },
  render: (args) => (
    <div className="max-w-xl">
      <CopyableValue {...args} />
    </div>
  ),
} satisfies Meta<typeof CopyableValue>;

export default meta;
type Story = StoryObj<typeof meta>;

/** what the stub clipboard received, so a play can read it back */
let written: string[] = [];
const record = async (value: string) => {
  written = [...written, value];
};

/**
 * The value is named by its label and described by its hint, mono, and taken
 * whole by one click, and the button copies exactly the value.
 */
export const Default: Story = {
  beforeEach: stubClipboard(record),
  play: async ({ canvasElement }) => {
    written = [];
    const canvas = within(canvasElement);
    const group = canvas.getByRole("group", { name: "SCIM base URL" });
    await expect(group).toHaveAccessibleDescription(/SCIM connector here/);
    const value = within(group).getByTestId("copyable");
    await expect(value.textContent).toBe(ADDRESS);
    // one click takes the whole value, so copying it by hand on a plain-http
    // dashboard, where the clipboard api is withheld, is one more keystroke
    await expect(getComputedStyle(value).userSelect).toBe("all");
    await expect(getComputedStyle(value).fontFamily).toMatch(/mono/i);
    // named for what it copies, value included
    const copy = within(group).getByRole("button", { name: `Copy SCIM base URL: ${ADDRESS}` });
    await userEvent.click(copy);
    await waitFor(() => expect(written).toEqual([ADDRESS]));
    await waitFor(() => expect(copy).toHaveAttribute("title", en.common.copied));
    // a repeatable value raises no note and no alert
    await expect(canvas.queryByRole("note")).toBeNull();
    await expect(canvas.queryByRole("alert")).toBeNull();
  },
};

/**
 * A value that can be copied again does not hold a failed copy the way a
 * secret does: the icon says so and resets, and the value is still on screen to
 * select by hand.
 */
export const CopyFailsAndResets: Story = {
  beforeEach: stubClipboard(() => Promise.reject(new Error("denied"))),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const copy = canvas.getByRole("button", { name: /Copy SCIM base URL/ });
    await userEvent.click(copy);
    await waitFor(() => expect(copy).toHaveAttribute("title", en.common.copyFailed));
    await expect(canvas.queryByRole("alert")).toBeNull();
    await waitFor(() => expect(copy).toHaveAttribute("title", "Copy SCIM base URL"));
    await expect(canvas.getByTestId("copyable")).toHaveTextContent(ADDRESS);
  },
};

/** A caution about the value sits under the hint as a note, inside the group. */
export const WithNote: Story = {
  args: {
    note: "ROLTER_PUBLIC_URL is not set, so this uses the default address.",
  },
  play: async ({ canvasElement }) => {
    const group = within(canvasElement).getByRole("group", { name: "SCIM base URL" });
    await expect(within(group).getByRole("note")).toHaveTextContent("ROLTER_PUBLIC_URL is not set");
    await expect(within(group).getByTestId("copyable")).toHaveTextContent(ADDRESS);
  },
};

/**
 * Nothing to copy yet: the box says why at its own height, and offers no copy
 * button, so the row does not jump when the value arrives.
 */
export const Empty: Story = {
  args: { value: null, empty: "Type a slug to see the redirect URI." },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("Type a slug to see the redirect URI.")).toBeVisible();
    await expect(canvas.queryByRole("button")).toBeNull();
    await expect(canvas.queryByTestId("copyable")).toBeNull();
  },
};

/**
 * A value still being read is a skeleton in the box's place, and offers
 * nothing to copy.
 */
export const Loading: Story = {
  args: { value: null, status: <Skeleton height={46} radius={6} /> },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("group", { name: "SCIM base URL" })).toBeVisible();
    await expect(canvas.queryByTestId("copyable")).toBeNull();
    await expect(canvas.queryByRole("button")).toBeNull();
  },
};

/** A value that could not be read says so in the box's place, never a guess. */
export const Failed: Story = {
  args: {
    value: null,
    status: (
      <p role="alert" className="text-sm text-[color:var(--status-danger-text)]">
        The control plane failed to return the public URL.
      </p>
    ),
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("alert")).toHaveTextContent(/failed to return/);
    await expect(canvas.queryByTestId("copyable")).toBeNull();
  },
};

/** The label takes a `Field` row's size when the value sits among them. */
export const BesideFields: Story = {
  args: { besideFields: true, label: "Redirect URI" },
  play: async ({ canvasElement }) => {
    const group = within(canvasElement).getByRole("group", { name: "Redirect URI" });
    await expect(within(group).getByText("Redirect URI")).toHaveClass("text-sm");
  },
};

/**
 * Wrapped, never truncated: an address is checked by its end, so a long one
 * breaks onto more lines rather than running off a phone.
 */
export const LongValueWraps: Story = {
  ...atMobile,
  args: { value: LONG },
  play: async ({ canvasElement }) => {
    const value = within(canvasElement).getByTestId("copyable");
    await expect(value.textContent).toBe(LONG);
    await expect(value.scrollWidth).toBeLessThanOrEqual(value.clientWidth);
    await expect(
      within(canvasElement).getByRole("button", { name: /Copy SCIM base URL/ }),
    ).toBeVisible();
    await expectNoHorizontalOverflow();
  },
};

/**
 * Inline, for a value that is already one cell of a description list: no box,
 * the row's own text size, and the same one-click selection.
 */
export const Inline: Story = {
  beforeEach: stubClipboard(record),
  render: () => (
    <dl className="grid max-w-sm grid-cols-[minmax(0,1fr)_minmax(0,2fr)] gap-x-3 text-xs">
      <dt className="text-muted-foreground">Request ID</dt>
      <dd className="min-w-0">
        <CopyableText variant="inline" value={ID} copyLabel="Copy request ID" testId="inline" />
      </dd>
    </dl>
  ),
  play: async ({ canvasElement }) => {
    written = [];
    const canvas = within(canvasElement);
    const value = canvas.getByTestId("inline");
    await expect(getComputedStyle(value).userSelect).toBe("all");
    await expect(getComputedStyle(value).fontFamily).toMatch(/mono/i);
    await userEvent.click(canvas.getByRole("button", { name: `Copy request ID: ${ID}` }));
    await waitFor(() => expect(written).toEqual([ID]));
  },
};
