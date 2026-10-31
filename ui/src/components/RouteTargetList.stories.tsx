import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, within } from "storybook/test";

import { RouteTargetList } from "./RouteTargetList";

const FLEET = [
  { provider: "vllm-a", upstream: "meta-llama/Llama-3.1-70B", weight: 1 },
  { provider: "vllm-b", upstream: "meta-llama/Llama-3.1-70B", weight: 1 },
  { provider: "vllm-c", upstream: "meta-llama/Llama-3.1-70B", weight: 2 },
];

const meta = {
  title: "Components/RouteTargetList",
  component: RouteTargetList,
  args: { label: "Targets of llama-70b", strategy: "weighted", targets: FLEET },
  decorators: [
    (Story) => (
      <div className="max-w-[640px] p-4">
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof RouteTargetList>;

export default meta;
type Story = StoryObj<typeof meta>;

/** Under a strategy that reads weights, each one is stated as a share of traffic. */
export const Weighted: Story = {
  play: async ({ canvas }) => {
    const lines = within(canvas.getByRole("list", { name: "Targets of llama-70b" })).getAllByRole(
      "listitem",
    );
    await expect(lines).toHaveLength(3);
    await expect(lines[0]).toHaveTextContent(/vllm-a.*weight 1.*25% of traffic/);
    await expect(lines[2]).toHaveTextContent(/vllm-c.*weight 2.*50% of traffic/);
    await expect(canvas.queryByText(/does not read weights/)).toBeNull();
  },
};

/**
 * A fallback target (#3319) takes no ordinary traffic: it is marked as one,
 * states no share, and the ordinary targets split the whole route between
 * them.
 */
export const WithAFallbackTarget: Story = {
  args: {
    targets: [
      FLEET[0],
      FLEET[2],
      { ...FLEET[1], provider: "openai-burst", takesOver: ["throttled", "timeout"] },
    ],
  },
  play: async ({ canvas }) => {
    const lines = within(canvas.getByRole("list", { name: "Targets of llama-70b" })).getAllByRole(
      "listitem",
    );
    await expect(lines[0]).toHaveTextContent(/vllm-a.*33% of traffic/);
    await expect(lines[1]).toHaveTextContent(/vllm-c.*67% of traffic/);
    await expect(lines[2]).toHaveTextContent(/openai-burst.*fallback/);
    await expect(lines[2]).not.toHaveTextContent(/of traffic/);
    await expect(within(lines[2]).getByTestId("fallback-badge").getAttribute("title")).toContain(
      "throttled, timeout",
    );
  },
};

/** Under a strategy that ignores weights a fallback is still marked as one. */
export const FallbackUnderIgnoredWeights: Story = {
  args: {
    strategy: "cache_aware",
    targets: [FLEET[0], { ...FLEET[1], takesOver: ["refusal:violence"] }],
  },
  play: async ({ canvas }) => {
    await expect(canvas.getByTestId("fallback-badge")).toBeVisible();
    await expect(canvas.queryByText(/of traffic/)).toBeNull();
  },
};

/**
 * `cache_aware` is built from the target count alone, so the weights are
 * listed as stored and no split is claimed for them.
 */
export const StrategyIgnoresWeights: Story = {
  args: { strategy: "cache_aware" },
  play: async ({ canvas }) => {
    await expect(canvas.getByText(/does not read weights/)).toHaveTextContent(/^cache_aware/);
    await expect(canvas.queryByText(/of traffic/)).toBeNull();
    await expect(canvas.getAllByText(/^weight \d$/)).toHaveLength(3);
  },
};

/**
 * Health where the rollup has it: within the SLA, below it, and a target
 * nothing has observed yet, each said in words beside its dot.
 */
export const WithHealth: Story = {
  args: {
    strategy: "cache_aware",
    health: true,
    targets: [
      { ...FLEET[0], health: { uptime: 0.9995, breached: false, state: "within" } },
      { ...FLEET[1], health: { uptime: 0.97, breached: true, state: "breaching" } },
      { ...FLEET[2], health: null },
    ],
  },
  play: async ({ canvas }) => {
    const lines = canvas.getAllByRole("listitem");
    await expect(lines[0]).toHaveTextContent("99.95% uptime");
    await expect(lines[1]).toHaveTextContent("97.00% uptime, below SLA");
    await expect(lines[2]).toHaveTextContent("no health data");
    await expect(canvas.getByText(/last 7 days, against a 99% SLA/)).toBeVisible();
  },
};

/** One target takes all the traffic, and a note about weights would be noise. */
export const SingleTarget: Story = {
  args: {
    strategy: "round_robin",
    targets: [{ provider: "openai-prod", upstream: "gpt-4o", weight: 1 }],
  },
  play: async ({ canvas }) => {
    await expect(canvas.getAllByRole("listitem")).toHaveLength(1);
    await expect(canvas.queryByText(/does not read weights/)).toBeNull();
  },
};
