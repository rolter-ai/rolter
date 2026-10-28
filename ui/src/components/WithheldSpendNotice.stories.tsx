import type { Meta, StoryObj } from "@storybook/react";
import { MemoryRouter } from "react-router";
import { expect, within } from "storybook/test";

import { Harness, routes } from "@/pages/story-harness";
import { formattersFor } from "@/lib/i18n/format";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";

import { WithheldSpendNotice } from "./WithheldSpendNotice";

const fmt = formattersFor("en");
const D = en.pages.dashboard;

const meta = {
  title: "Components/WithheldSpendNotice",
  component: WithheldSpendNotice,
  args: { withheld: 3, withheldCost: 1.25, unknownUsage: 2, search: "" },
  decorators: [
    (Story) => (
      <MemoryRouter>
        <Harness
          fetchStub={routes([
            ["/api/v1/currency", () => ({ base: "USD", codes: ["USD"], rates: {} })],
          ])}
        >
          <div className="flex flex-col gap-3">
            <Story />
          </div>
        </Harness>
      </MemoryRouter>
    ),
  ],
} satisfies Meta<typeof WithheldSpendNotice>;
export default meta;
type Story = StoryObj<typeof meta>;

/** #3302: withheld spend is billed spend nobody received, and it says how much. */
export const WithheldSpend: Story = {
  args: { unknownUsage: 0 },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const notice = await canvas.findByRole("status");
    await expect(notice).toHaveTextContent(D.withheld.title_other.replace("{{count}}", "3"));
    await expect(notice).toHaveTextContent(
      D.withheld.body.replace("{{cost}}", fmt.currency(1.25, "USD")),
    );
    await expect(canvas.queryByText(/reported no usage/)).toBeNull();
  },
};

/** One request reads in the singular. */
export const OneWithheldRequest: Story = {
  args: { withheld: 1, unknownUsage: 0 },
  play: async ({ canvasElement }) => {
    await expect(await within(canvasElement).findByRole("status")).toHaveTextContent(
      D.withheld.title_one.replace("{{count}}", "1"),
    );
  },
};

/** Successes whose upstream reported no usage: their cost is unknown, not zero. */
export const UnknownUsage: Story = {
  args: { withheld: 0 },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const notice = await canvas.findByRole("status");
    await expect(notice).toHaveTextContent(D.usageUnknown.title_other.replace("{{count}}", "2"));
    await expect(notice).toHaveTextContent(D.usageUnknown.body);
    await expect(canvas.queryByText(/withheld/)).toBeNull();
  },
};

/** Both are in the window: two notices, each with its own link. */
export const Both: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByRole("status");
    await expect(canvas.getAllByRole("status")).toHaveLength(2);
  },
};

/** Nothing withheld and nothing unknown: no notice at all, not an empty one. */
export const NothingToReport: Story = {
  args: { withheld: 0, unknownUsage: 0 },
  play: async ({ canvasElement }) => {
    await expect(within(canvasElement).queryByRole("status")).toBeNull();
  },
};

/**
 * Each link opens LLM Logs over the window and the filters the figures were read
 * with, narrowed by its own flag; the other flag is not carried.
 */
export const LinksCarryTheWindowAndFilters: Story = {
  args: { search: "?window=7d&model=gpt-4o" },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const withheld = await canvas.findByRole("link", { name: D.withheld.link });
    await expect(withheld).toHaveAttribute("href", "/logs?window=7d&model=gpt-4o&withheld=true");
    await expect(canvas.getByRole("link", { name: D.usageUnknown.link })).toHaveAttribute(
      "href",
      "/logs?window=7d&model=gpt-4o&usage_unknown=true",
    );
  },
};

/** Russian has four plural forms for the count. */
export const InRussian: Story = {
  globals: { locale: "ru" },
  args: { withheld: 3, unknownUsage: 0 },
  play: async ({ canvasElement }) => {
    await expect(await within(canvasElement).findByRole("status")).toHaveTextContent(
      ru.pages.dashboard.withheld.title_few.replace("{{count}}", "3"),
    );
  },
};
