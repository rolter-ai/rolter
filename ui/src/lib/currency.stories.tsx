import type { Meta, StoryObj } from "@storybook/react";
import { expect, waitFor, within } from "storybook/test";

import { useCurrencyCode } from "@/lib/currency";
import { Harness, type FetchStub, json, recording } from "@/pages/story-harness";

// `useCurrencyCode` is what stops a screen printing `$` in front of euros
// (#1182), and its one interesting rule is the fallback: an amount is never
// rendered with no unit at all. A hook has no DOM under `bun test`, so it is
// asserted here, the way `lib/scope.stories.tsx` asserts `useScope`.

const settings = (base: string) => ({ base, codes: ["USD", "EUR"], rates: { USD: 1, EUR: 1.08 } });

/** answer `/api/v1/currency` with `respond`, and nothing else */
const currency =
  (respond: () => Promise<Response>): FetchStub =>
  async (input) => {
    const path = new URL(String(input), "http://localhost").pathname;
    return path === "/api/v1/currency" ? respond() : json({});
  };

function CurrencyProbe({ name = "probe" }: { name?: string }) {
  return <p data-testid={name}>{useCurrencyCode()}</p>;
}

const meta = {
  title: "Session/Currency",
  component: CurrencyProbe,
  parameters: { layout: "padded" },
} satisfies Meta<typeof CurrencyProbe>;

export default meta;
type Story = StoryObj<typeof meta>;

/** the deployment settles in euros, so that is the unit every amount carries */
export const ReadsTheDeploymentBase: Story = {
  render: () => (
    <Harness fetchStub={currency(async () => json(settings("EUR")))}>
      <CurrencyProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByTestId("probe")).toHaveTextContent("EUR"));
  },
};

const failing = recording(currency(async () => json({ error: "boom" }, 500)));

/** a failed read falls back to USD rather than leaving the amount with no unit */
export const FallsBackToUsdWhenTheReadFails: Story = {
  render: () => (
    <Harness fetchStub={failing.stub}>
      <CurrencyProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await failing.expectSent("GET", "/api/v1/currency");
    // let the 500 land: "USD" before the answer and "USD" after it look alike
    await new Promise((resolve) => setTimeout(resolve, 100));
    await expect(within(canvasElement).getByTestId("probe")).toHaveTextContent("USD");
  },
};

/** and says USD while the answer is still out, instead of flashing a bare number */
export const SaysUsdWhileTheAnswerIsOut: Story = {
  render: () => (
    <Harness fetchStub={currency(() => new Promise<Response>(() => {}))}>
      <CurrencyProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expect(within(canvasElement).getByTestId("probe")).toHaveTextContent("USD");
  },
};

/** an empty base is not a currency: an older control plane that sends one still gets a unit */
export const TreatsAnEmptyBaseAsUnset: Story = {
  render: () => (
    <Harness fetchStub={currency(async () => json(settings("")))}>
      <CurrencyProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await new Promise((resolve) => setTimeout(resolve, 100));
    await expect(within(canvasElement).getByTestId("probe")).toHaveTextContent("USD");
  },
};
