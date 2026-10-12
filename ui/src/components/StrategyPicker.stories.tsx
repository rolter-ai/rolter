import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { MemoryRouter } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { StrategyPicker } from "./StrategyPicker";
import { openOptions, pickOption } from "@/pages/story-harness";
import { STRATEGIES } from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";

function Stage({ initial = "round_robin", disabled }: { initial?: string; disabled?: boolean }) {
  const [value, setValue] = React.useState(initial);
  return (
    <MemoryRouter>
      <div className="max-w-md p-6">
        <label htmlFor="picker">Strategy</label>
        <StrategyPicker id="picker" value={value} onChange={setValue} disabled={disabled} />
      </div>
    </MemoryRouter>
  );
}

const meta = {
  title: "Components/StrategyPicker",
  component: StrategyPicker,
  // every story renders its own stateful `Stage`; these satisfy the type
  args: { id: "picker", value: "round_robin", onChange: () => {} },
  parameters: { layout: "centered" },
} satisfies Meta<typeof StrategyPicker>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * Every strategy the API accepts is offered, adaptive included, under one of
 * four group headings and with its one-line description (#2136).
 */
export const OffersEveryStrategyWithItsDescription: Story = {
  render: () => <Stage />,
  play: async ({ canvasElement }) => {
    const picker = await within(canvasElement).findByRole("combobox", { name: "Strategy" });
    const listbox = within(await openOptions(picker));
    for (const strategy of STRATEGIES) {
      const key = strategy.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());
      const description = (en.strategies.descriptions as Record<string, string>)[key];
      await expect(description).toBeTruthy();
      // the option is named by its identifier and carries the description
      const option = listbox.getByRole("option", { name: new RegExp(`^${strategy}\\b`) });
      await expect(option).toHaveTextContent(description);
    }
    for (const heading of Object.values(en.strategies.groups)) {
      await expect(listbox.getByRole("group", { name: heading })).toBeInTheDocument();
    }
  },
};

/** `adaptive` can be put on a route from the picker. */
export const AdaptiveCanBeSelected: Story = {
  render: () => <Stage />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const picker = await canvas.findByRole("combobox", { name: "Strategy" });
    await pickOption(picker, /^adaptive\b/);
    await waitFor(() => expect(picker).toHaveValue("adaptive"));
    // the deployment-wide policy governs the blend and the kill switch, and
    // the note links where it is set
    const note = await canvas.findByRole("note");
    await expect(note).toHaveTextContent(/kill switch/);
    await expect(
      within(note).getByRole("link", { name: "Adaptive Routing settings" }),
    ).toHaveAttribute("href", "/adaptive-settings");
  },
};

/** The chosen strategy's description stays under the control, not only in the list. */
export const SelectedDescriptionStaysVisible: Story = {
  render: () => <Stage initial="cache_aware" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const picker = await canvas.findByRole("combobox", { name: "Strategy" });
    const description = en.strategies.descriptions.cacheAware;
    await expect(canvas.getByText(description)).toBeVisible();
    await expect(picker).toHaveAccessibleDescription(description);
    await pickOption(picker, /^consistent_hash\b/);
    await expect(await canvas.findByText(en.strategies.descriptions.consistentHash)).toBeVisible();
    await expect(canvas.queryByText(description)).toBeNull();
  },
};

/** The telemetry caveat sits under the description, in the warning tone. */
export const TelemetryCaveatBesideTheDescription: Story = {
  render: () => <Stage initial="precise_cache_aware" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByText(en.strategies.descriptions.preciseCacheAware),
    ).toBeVisible();
    await expect(await canvas.findByRole("note")).toHaveTextContent(/cache-telemetry source/);
  },
};

/** A value this build has never heard of is kept, and has no description to invent. */
export const UnknownValueIsKept: Story = {
  render: () => <Stage initial="strategy_from_the_future" />,
  play: async ({ canvasElement }) => {
    const picker = await within(canvasElement).findByRole("combobox", { name: "Strategy" });
    await expect(picker).toHaveValue("strategy_from_the_future");
    await expect(picker).not.toHaveAttribute("aria-describedby", expect.anything());
  },
};

/** Keyboard: arrows move through the groups and Enter picks. */
export const KeyboardPicks: Story = {
  render: () => <Stage />,
  play: async ({ canvasElement }) => {
    const picker = await within(canvasElement).findByRole("combobox", { name: "Strategy" });
    picker.focus();
    await userEvent.keyboard("{ArrowDown}{ArrowDown}{Enter}");
    await waitFor(() => expect(picker).not.toHaveValue("round_robin"));
  },
};

/** The group headings and descriptions are translated; the identifiers are not. */
export const InRussian: Story = {
  globals: { locale: "ru" },
  render: () => <Stage />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const picker = await canvas.findByRole("combobox", { name: "Strategy" });
    const listbox = within(await openOptions(picker));
    await expect(
      await listbox.findByRole("group", { name: ru.strategies.groups.cacheAware }),
    ).toBeInTheDocument();
    await expect(listbox.getByRole("option", { name: /^cache_aware\b/ })).toHaveTextContent(
      ru.strategies.descriptions.cacheAware,
    );
  },
};

export const Disabled: Story = {
  render: () => <Stage disabled initial="weighted" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("combobox", { name: "Strategy" })).toBeDisabled();
    await expect(canvas.getByText(en.strategies.descriptions.weighted)).toBeVisible();
  },
};
