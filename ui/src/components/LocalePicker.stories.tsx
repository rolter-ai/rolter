import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { DEFAULT_LOCALE, setLocale } from "@/lib/i18n";
import en from "@/lib/i18n/locales/en.json";

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

    const list = await canvas.findByRole("menu");
    await expect(list).toBeVisible();
    // every language is listed in its own language, and the active one is marked
    await expect(canvas.getByRole("menuitemradio", { name: "English" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    await expect(canvas.getByRole("menuitemradio", { name: "Русский" })).toHaveAttribute(
      "aria-checked",
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
    await userEvent.click(canvas.getByRole("menuitemradio", { name: "Русский" }));

    await waitFor(async () => {
      // the trigger's own label is translated too, so it changes name
      await expect(canvas.getByRole("button", { name: "Сменить язык" })).toHaveTextContent("RU");
    });
    await expect(canvas.queryByRole("menu")).toBeNull();
  },
};

const CHANGE = en.locale.change;
const MENU = en.locale.label;

// the panel is the shared `AnchoredPanel`, so it carries the shared elevation:
// the `--shadow-lg` token, read through a probe so the story compares what the
// browser resolved rather than a copy of the token's value
const tokenShadow = () => {
  const probe = document.createElement("div");
  probe.style.boxShadow = "var(--shadow-lg)";
  document.body.append(probe);
  const resolved = getComputedStyle(probe).boxShadow;
  probe.remove();
  return resolved;
};

/**
 * The list is a fixed panel placed from the button's rectangle, so a scroll
 * container above it cannot clip it, and it opens upward from the footer.
 */
export const MenuIsASharedPanelAboveTheButton: Story = {
  args: { collapsed: false },
  // room over the button, which the centred layout does not leave at the top
  render: (args) => (
    <div className="pt-60">
      <LocalePicker {...args} />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const trigger = canvas.getByRole("button", { name: CHANGE });
    await expect(trigger).toHaveAttribute("aria-haspopup", "menu");
    await expect(trigger).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(trigger);
    const menu = await canvas.findByRole("menu", { name: MENU });
    await expect(trigger).toHaveAttribute("aria-expanded", "true");

    const panel = menu.parentElement as HTMLElement;
    await waitFor(() => expect(panel).toBeVisible());
    await expect(getComputedStyle(panel).position).toBe("fixed");
    await expect(getComputedStyle(panel).boxShadow).toContain(tokenShadow());
    await expect(panel.getBoundingClientRect().bottom).toBeLessThanOrEqual(
      trigger.getBoundingClientRect().top,
    );
    // the entries are radios, so none is a plain `menuitem`
    await expect(within(menu).queryAllByRole("menuitem")).toHaveLength(0);
    await expect(within(menu).getAllByRole("menuitemradio")).toHaveLength(2);
  },
};

/**
 * The menu keyboard, which is `Menu`'s now rather than a copy of it: opening
 * puts focus on the language in force, arrows wrap, Home and End are the
 * edges, Escape closes with focus back on the button, and Tab closes it too.
 */
export const MenuFromTheKeyboard: Story = {
  args: { collapsed: false },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const trigger = canvas.getByRole("button", { name: CHANGE });
    trigger.focus();
    await userEvent.keyboard("{Enter}");
    const menu = within(await canvas.findByRole("menu", { name: MENU }));
    const english = menu.getByRole("menuitemradio", { name: "English" });
    const russian = menu.getByRole("menuitemradio", { name: "Русский" });

    // focus lands on the checked entry, not on the first one by default
    await waitFor(() => expect(english).toHaveFocus());
    await userEvent.keyboard("{ArrowDown}");
    await expect(russian).toHaveFocus();
    await userEvent.keyboard("{ArrowDown}");
    await expect(english).toHaveFocus();
    await userEvent.keyboard("{ArrowUp}");
    await expect(russian).toHaveFocus();
    await userEvent.keyboard("{Home}");
    await expect(english).toHaveFocus();
    await userEvent.keyboard("{End}");
    await expect(russian).toHaveFocus();

    // Escape closes it and hands focus back to the button
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(canvas.queryByRole("menu")).toBeNull());
    await waitFor(() => expect(trigger).toHaveFocus());
    await expect(trigger).toHaveAttribute("aria-expanded", "false");

    // Tab closes it rather than walking into the page behind an open menu
    await userEvent.keyboard("{Enter}");
    await canvas.findByRole("menu", { name: MENU });
    await userEvent.tab();
    await waitFor(() => expect(canvas.queryByRole("menu")).toBeNull());
  },
};

// Enter on a language picks it, and the button it was opened from gets focus
// back under its new name instead of the page losing it
export const ChoosingFromTheKeyboard: Story = {
  args: { collapsed: false },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    canvas.getByRole("button", { name: CHANGE }).focus();
    await userEvent.keyboard("{Enter}");
    await canvas.findByRole("menu", { name: MENU });
    await userEvent.keyboard("{ArrowDown}");
    await userEvent.keyboard("{Enter}");

    await waitFor(() => expect(canvas.queryByRole("menu")).toBeNull());
    const trigger = await canvas.findByRole("button", { name: "Сменить язык" });
    await expect(trigger).toHaveTextContent("RU");
    await waitFor(() => expect(trigger).toHaveFocus());
  },
};

// a press outside puts it away, and a second press on the button does too
export const MenuClosesOnAnOutsidePress: Story = {
  args: { collapsed: false },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const trigger = canvas.getByRole("button", { name: CHANGE });
    await userEvent.click(trigger);
    await canvas.findByRole("menu", { name: MENU });
    await userEvent.click(canvasElement.ownerDocument.body);
    await waitFor(() => expect(canvas.queryByRole("menu")).toBeNull());

    await userEvent.click(trigger);
    await canvas.findByRole("menu", { name: MENU });
    await userEvent.click(trigger);
    await waitFor(() => expect(canvas.queryByRole("menu")).toBeNull());
  },
};
