import type { Meta, StoryObj } from "@storybook/react";
import { expect } from "storybook/test";

import { DocsLink } from "./DocsLink";

/**
 * The documentation link a screen renders from a page key alone (#1164).
 *
 * The two states that matter are "a documentation host is configured" and "one
 * is not". The second is the default, and the air-gapped case: it must render
 * literally nothing rather than a link into a host the network cannot reach.
 */
const meta = {
  title: "Components/DocsLink",
  component: DocsLink,
  args: { page: "whichKey" },
} satisfies Meta<typeof DocsLink>;

export default meta;
type Story = StoryObj<typeof meta>;

/** The build-time base URL, which is what a distribution build bakes in. */
export const Configured: Story = {
  args: { config: {}, fallback: "https://docs.example.com" },
  play: async ({ canvas }) => {
    const link = await canvas.findByRole("link");
    await expect(link).toHaveAttribute("href", "https://docs.example.com/security/which-key");
    // the destination is a third party; it learns nothing about this deployment
    await expect(link).toHaveAttribute("rel", "noreferrer");
    await expect(link).toHaveAttribute("target", "_blank");
  },
};

/** The control plane's injected block wins over whatever the build baked in. */
export const RuntimeOverrideWins: Story = {
  args: {
    config: { docsBaseUrl: "https://docs.internal.example/rolter/" },
    fallback: "https://docs.example.com",
  },
  play: async ({ canvas }) => {
    const link = await canvas.findByRole("link");
    await expect(link).toHaveAttribute(
      "href",
      "https://docs.internal.example/rolter/security/which-key",
    );
  },
};

/**
 * The default, and the air-gapped case: no base URL anywhere, so no link —
 * not a disabled one, not a `#`, nothing to click and nothing to 404.
 */
export const NoDocsHostConfigured: Story = {
  args: { config: {}, fallback: "" },
  play: async ({ canvas }) => {
    await expect(canvas.queryByRole("link")).toBeNull();
  },
};

/**
 * A base URL the operator mistyped into something unusable suppresses the link
 * too, rather than falling back to the host it meant to replace.
 */
export const UnusableBaseSuppressesTheLink: Story = {
  args: {
    config: { docsBaseUrl: "javascript:alert(1)" },
    fallback: "https://docs.example.com",
  },
  play: async ({ canvas }) => {
    await expect(canvas.queryByRole("link")).toBeNull();
  },
};

/** Screens pass their own translated text; the icon stays decorative. */
export const CustomLabel: Story = {
  args: { page: "virtualKeys", label: "About virtual keys", fallback: "https://docs.example.com" },
  play: async ({ canvas }) => {
    const link = await canvas.findByRole("link", { name: /About virtual keys/ });
    await expect(link).toHaveAttribute("href", "https://docs.example.com/concepts/virtual-keys");
    await expect(link).toHaveTextContent("About virtual keys");
    // the accessible name says the link leaves the dashboard, which the visible
    // text cannot
    await expect(link).toHaveAccessibleName(/new tab/i);
  },
};

/** With no label the generic catalog string is used, never a bare URL. */
export const DefaultLabel: Story = {
  args: { fallback: "https://docs.example.com" },
  play: async ({ canvas }) => {
    const link = await canvas.findByRole("link");
    await expect(link).toHaveTextContent("Documentation");
    await expect(link).not.toHaveTextContent("https://");
  },
};
