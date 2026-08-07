import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { DEFAULT_LOCALE, setLocale } from "@/lib/i18n";

import { LocalePicker } from "./LocalePicker";

// the switcher lives in the sidebar footer next to the version, so it is on
// screen from every route. these stories drive it directly; the footer context
// is reproduced by the surrounding padding only.
const meta = {
  title: "Components/LocalePicker",
  component: LocalePicker,
  parameters: { layout: "centered" },
  // each story starts from english regardless of what ran before it
  beforeEach: async () => {
    await setLocale(DEFAULT_LOCALE);
    return async () => {
      await setLocale(DEFAULT_LOCALE);
    };
  },
} satisfies Meta<typeof LocalePicker>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: { collapsed: false },
};

// the icon-only form the collapsed rail renders
export const Collapsed: Story = {
  args: { collapsed: true },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const trigger = canvas.getByRole("button", { name: "Change language" });
    await expect(trigger).not.toHaveTextContent("EN");
  },
};

export const MenuOpen: Story = {
  args: { collapsed: false },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "Change language" }));

    const list = await canvas.findByRole("listbox");
    await expect(list).toBeVisible();
    // every language is listed in its own language, and the active one is marked
    await expect(canvas.getByRole("option", { name: "English" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    await expect(canvas.getByRole("option", { name: "Русский" })).toHaveAttribute(
      "aria-selected",
      "false",
    );
  },
};

// the whole point of the component: picking a language swaps the catalog in
// place, with no reload and no route change
export const SwitchesLanguage: Story = {
  args: { collapsed: false },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("button", { name: "Change language" })).toHaveTextContent("EN");

    await userEvent.click(canvas.getByRole("button", { name: "Change language" }));
    await userEvent.click(canvas.getByRole("option", { name: "Русский" }));

    await waitFor(async () => {
      // the trigger's own label is translated too, so it changes name
      await expect(canvas.getByRole("button", { name: "Сменить язык" })).toHaveTextContent("RU");
    });
    await expect(canvas.queryByRole("listbox")).toBeNull();
  },
};
