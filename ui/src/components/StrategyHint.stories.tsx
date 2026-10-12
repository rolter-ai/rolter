import type { Meta, StoryObj } from "@storybook/react";
import { MemoryRouter } from "react-router";
import { expect, waitFor } from "storybook/test";

import { withDocsBase } from "@/pages/story-harness";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";

import { StrategyHint } from "./StrategyHint";

const meta = {
  title: "Components/StrategyHint",
  component: StrategyHint,
  // adaptive links its settings screen
  decorators: [
    (Story) => (
      <MemoryRouter>
        <Story />
      </MemoryRouter>
    ),
  ],
} satisfies Meta<typeof StrategyHint>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * Degrades to least-load without KV events / an LMCache controller, and says
 * where that source is set: the provider sheet or `rolter.toml` (#2996).
 */
export const NeedsTelemetry: Story = {
  beforeEach: withDocsBase(undefined),
  args: { strategy: "precise_cache_aware" },
  play: async ({ canvas }) => {
    const note = await canvas.findByRole("note");
    // a story before this one may have left the catalog on another locale, and
    // the decorator switches it back from an effect
    await waitFor(() =>
      expect(note).toHaveTextContent(en.pages.routing.strategyHints.needsTelemetry),
    );
    await expect(note).toHaveTextContent(/least-load/);
    // it names both places the source is set
    await expect(note).toHaveTextContent(/rolter\.toml/);
    await expect(note).toHaveTextContent(/in each provider.s sheet/);
  },
};

export const LmCacheNeedsTelemetry: Story = {
  beforeEach: withDocsBase(undefined),
  args: { strategy: "lmcache_aware" },
  play: async ({ canvas }) => {
    const note = await canvas.findByRole("note");
    await waitFor(() =>
      expect(note).toHaveTextContent(en.pages.routing.strategyHints.needsTelemetry),
    );
  },
};

/** The same caveat in Russian, the longer catalog, still naming the file. */
export const NeedsTelemetryInRussian: Story = {
  globals: { locale: "ru" },
  beforeEach: withDocsBase(undefined),
  args: { strategy: "precise_cache_aware" },
  play: async ({ canvas }) => {
    const note = await canvas.findByRole("note");
    // the locale decorator switches language from an effect, after first paint
    await waitFor(() =>
      expect(note).toHaveTextContent(ru.pages.routing.strategyHints.needsTelemetry),
    );
    await expect(note).toHaveTextContent(/rolter\.toml/);
  },
};

/**
 * With a documentation host configured, the caveat links the page that
 * explains the telemetry sources and where they are set.
 */
export const LinksTheCacheAwareRoutingDocs: Story = {
  beforeEach: withDocsBase("https://docs.example.com"),
  args: { strategy: "lmcache_aware" },
  play: async ({ canvas }) => {
    const note = await canvas.findByRole("note");
    const link = await canvas.findByRole("link", { name: /About cache-aware routing/ });
    await expect(note).toContainElement(link);
    await expect(link).toHaveAttribute(
      "href",
      "https://docs.example.com/concepts/cache-aware-routing",
    );
  },
};

/** The link label is translated with the rest of the caveat. */
export const LinksTheCacheAwareRoutingDocsInRussian: Story = {
  globals: { locale: "ru" },
  beforeEach: withDocsBase("https://docs.example.com"),
  args: { strategy: "precise_cache_aware" },
  play: async ({ canvas }) => {
    const link = await canvas.findByRole("link", { name: /О маршрутизации с учётом кэша/ });
    await expect(link).toHaveAttribute(
      "href",
      "https://docs.example.com/concepts/cache-aware-routing",
    );
  },
};

/**
 * The air-gapped default: no documentation host, so the caveat stands alone
 * and there is no link into a host the network cannot reach.
 */
export const NoLinkWithoutADocsHost: Story = {
  beforeEach: withDocsBase(undefined),
  args: { strategy: "precise_cache_aware" },
  play: async ({ canvas }) => {
    await canvas.findByRole("note");
    await expect(canvas.queryByRole("link")).toBeNull();
  },
};

/**
 * Governed by the deployment-wide policy: the note says the policy sets the
 * blend and the kill switch, and links where that is set (#2136).
 */
export const DeploymentWide: Story = {
  // a docs host is configured so the missing docs link is a decision, not the
  // air-gapped default
  beforeEach: withDocsBase("https://docs.example.com"),
  args: { strategy: "adaptive" },
  play: async ({ canvas }) => {
    const note = await canvas.findByRole("note");
    await expect(note).toHaveTextContent(/adaptive-routing policy/);
    await expect(note).toHaveTextContent(/kill switch/);
    await expect(canvas.getAllByRole("link")).toHaveLength(1);
    await expect(canvas.getByRole("link", { name: "Adaptive Routing settings" })).toHaveAttribute(
      "href",
      "/adaptive-settings",
    );
  },
};

/** Most strategies need no caveat, and a hint on every one would be noise. */
export const NoCaveat: Story = {
  args: { strategy: "round_robin" },
  play: async ({ canvas }) => {
    await expect(canvas.queryByRole("note")).toBeNull();
  },
};

/** Newly selectable in #897, and deliberately uncaveated: pure config. */
export const NewlySelectableNeedsNoCaveat: Story = {
  args: { strategy: "cheapest" },
  play: async ({ canvas }) => {
    await expect(canvas.queryByRole("note")).toBeNull();
  },
};
