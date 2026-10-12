import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, waitFor } from "storybook/test";

import { SampledLogNoticeView } from "./SampledLogNotice";

const meta = {
  title: "Components/SampledLogNotice",
  component: SampledLogNoticeView,
} satisfies Meta<typeof SampledLogNoticeView>;

export default meta;
type Story = StoryObj<typeof meta>;

/** A quarter of the traffic: the share is stated and the figures are estimates. */
export const Figures: Story = {
  args: { rate: 0.25, kind: "figures" },
  play: async ({ canvas }) => {
    const notice = await canvas.findByRole("status");
    await expect(notice).toHaveTextContent("The request log keeps about 1 in 4 requests.");
    await expect(notice).toHaveTextContent(/estimates scaled up from that sample/);
  },
};

/** The LLM Logs list is the kept rows, so it says the list is partial. */
export const Rows: Story = {
  args: { rate: 0.1, kind: "rows" },
  play: async ({ canvas }) => {
    const notice = await canvas.findByRole("status");
    await expect(notice).toHaveTextContent("The request log keeps about 1 in 10 requests.");
    await expect(notice).toHaveTextContent(/A request missing from it was still served/);
  },
};

/** All but one in twenty is stated as a share, not as 95 %. */
export const NearlyFull: Story = {
  args: { rate: 0.95, kind: "figures" },
  play: async ({ canvas }) => {
    await expect(await canvas.findByRole("status")).toHaveTextContent("19 in 20 requests");
  },
};

/** Zero keeps nothing: there are no estimates to qualify, only figures that stop growing. */
export const KeepsNothing: Story = {
  args: { rate: 0, kind: "figures" },
  play: async ({ canvas }) => {
    const notice = await canvas.findByRole("status");
    await expect(notice).toHaveTextContent("The request log keeps no requests.");
    await expect(notice).not.toHaveTextContent(/estimates/);
  },
};

export const InRussian: Story = {
  args: { rate: 0.25, kind: "figures" },
  globals: { locale: "ru" },
  play: async ({ canvas }) => {
    const notice = await canvas.findByRole("status");
    // the locale arrives from the toolbar a beat after the first paint
    await waitFor(() =>
      expect(notice).toHaveTextContent("Лог запросов сохраняет примерно 1 из 4 запросов."),
    );
  },
};
