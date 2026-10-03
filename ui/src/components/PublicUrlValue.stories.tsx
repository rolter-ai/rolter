import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, within } from "storybook/test";

import { PublicUrlValue } from "./PublicUrlValue";
import type { PublicUrl } from "@/lib/api";
import {
  expectLoadError,
  expectSkeleton,
  Harness,
  json,
  pending,
  routes,
} from "@/pages/story-harness";

const PUBLIC_BASE = "https://rolter.example.com";
const DEFAULT_BASE = "http://localhost:4001";
const CONFIGURED: PublicUrl = { public_url: PUBLIC_BASE, configured: true };
const UNSET: PublicUrl = { public_url: DEFAULT_BASE, configured: false };

const address = (base: string) => `${base}/scim/v2`;

const meta = {
  title: "Forms/PublicUrlValue",
  component: PublicUrlValue,
  parameters: { layout: "padded" },
  args: {
    address,
    label: "SCIM base URL",
    copyLabel: "Copy SCIM base URL",
    hint: "Point your identity provider's SCIM connector here.",
    testId: "public-url-value",
  },
} satisfies Meta<typeof PublicUrlValue>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * Built on the base the control plane reports, never on this page's origin, and
 * a configured base raises no note.
 */
export const Configured: Story = {
  render: (args) => (
    <Harness fetchStub={routes([["/public-url", () => CONFIGURED]])}>
      <PublicUrlValue {...args} />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const value = await canvas.findByTestId("public-url-value");
    await expect(value.textContent).toBe(`${PUBLIC_BASE}/scim/v2`);
    await expect(value.textContent).not.toContain(window.location.origin);
    await expect(canvas.queryByRole("note")).toBeNull();
  },
};

/**
 * `ROLTER_PUBLIC_URL` unset: the default address is still shown and copyable,
 * with one note, worded the same on every screen, naming it and what to do.
 */
export const Unset: Story = {
  render: (args) => (
    <Harness fetchStub={routes([["/public-url", () => UNSET]])}>
      <PublicUrlValue {...args} />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const value = await canvas.findByTestId("public-url-value");
    await expect(value.textContent).toBe(`${DEFAULT_BASE}/scim/v2`);
    const note = canvas.getByRole("note");
    await expect(note).toHaveTextContent("ROLTER_PUBLIC_URL is not set");
    await expect(note).toHaveTextContent(DEFAULT_BASE);
    await expect(note).toHaveTextContent(/restart the control plane/);
  },
};

/** Still in flight: the box's space is held by a skeleton, and nothing is claimed. */
export const Loading: Story = {
  render: (args) => (
    <Harness fetchStub={pending}>
      <PublicUrlValue {...args} />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectSkeleton(canvasElement);
    await expect(canvas.queryByTestId("public-url-value")).toBeNull();
    await expect(canvas.queryByRole("note")).toBeNull();
  },
};

/**
 * The base could not be read: the failure is said with a retry, never a URL
 * guessed from the browser, and the value appears once the retry lands.
 */
export const Failed: Story = {
  render: (args) => {
    let reads = 0;
    return (
      <Harness
        fetchStub={async (input) =>
          String(input).includes("/public-url") && ++reads === 1
            ? json({ error: { message: "upstream unavailable" } }, 502)
            : json(CONFIGURED)
        }
      >
        <PublicUrlValue {...args} />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /failed to return the public URL/i);
    await expect(canvas.queryByTestId("public-url-value")).toBeNull();
    await userEvent.click(canvas.getByRole("button", { name: "Try again" }));
    const value = await canvas.findByTestId("public-url-value");
    await expect(value.textContent).toBe(`${PUBLIC_BASE}/scim/v2`);
  },
};
