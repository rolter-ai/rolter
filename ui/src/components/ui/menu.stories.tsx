import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { Menu, MenuItem, MenuItemRadio, MenuSeparator } from "./menu";

// the menu's filter (#3058): a search field above the entries that narrows and
// ranks them with the same scorer as the command palette and the comboboxes
const meta = {
  title: "Primitives/Menu",
  component: Menu,
  parameters: { layout: "centered" },
  args: { anchor: document.body, label: "Actions", onClose: () => {}, children: null },
} satisfies Meta<typeof Menu>;

export default meta;
type Story = StoryObj<typeof meta>;

function Host({ onPick = () => {} }: { onPick?: (what: string) => void }) {
  const [anchor, setAnchor] = React.useState<HTMLButtonElement | null>(null);
  const [open, setOpen] = React.useState(true);
  const [lang, setLang] = React.useState("en");
  return (
    <>
      <button ref={setAnchor} type="button" onClick={() => setOpen(true)}>
        Open actions
      </button>
      {open && anchor && (
        <Menu anchor={anchor} label="Actions" onClose={() => setOpen(false)}>
          <MenuItem onSelect={() => onPick("Edit route")}>Edit route</MenuItem>
          <MenuItem onSelect={() => onPick("Duplicate route")}>Duplicate route</MenuItem>
          <MenuSeparator />
          <MenuItemRadio checked={lang === "en"} onSelect={() => setLang("en")}>
            English
          </MenuItemRadio>
          <MenuItemRadio checked={lang === "ru"} onSelect={() => setLang("ru")}>
            Русский
          </MenuItemRadio>
          <MenuSeparator />
          <MenuItem tone="danger" onSelect={() => onPick("Delete route")}>
            Delete route
          </MenuItem>
        </Menu>
      )}
    </>
  );
}

// every entry the menu draws, radios included, in the order they are drawn
const entries = (root: HTMLElement) =>
  Array.from(root.querySelectorAll('[role^="menuitem"]'), (e) => e.textContent);

export const Filterable: Story = {
  render: () => <Host />,
  play: async ({ canvasElement }) => {
    const body = within(canvasElement.ownerDocument.body);
    const menu = await body.findByRole("menu", { name: "Actions" });
    await expect(entries(menu)).toHaveLength(5);
    await expect(body.getByRole("textbox", { name: "Filter Actions" })).toBeVisible();
  },
};

export const FilterRanksAndMarks: Story = {
  render: () => <Host />,
  play: async ({ canvasElement }) => {
    const body = within(canvasElement.ownerDocument.body);
    const menu = await body.findByRole("menu", { name: "Actions" });
    // typing while an entry has focus starts the search in the field
    await waitFor(() => expect(menu.contains(document.activeElement)).toBe(true));
    await userEvent.keyboard("dr");
    const field = body.getByRole("textbox", { name: "Filter Actions" });
    await expect(field).toHaveValue("dr");
    // "dr" is a subsequence of the route entries, not of the languages
    const found = entries(menu);
    await expect(found.length).toBeGreaterThan(0);
    await expect(found).not.toContain("English");
    await expect(menu.querySelector("[data-match]")).not.toBeNull();
    // separators between groups the filter pulled apart are gone
    await expect(within(menu).queryAllByRole("separator")).toHaveLength(0);
  },
};

export const FilterNoMatch: Story = {
  render: () => <Host />,
  play: async ({ canvasElement }) => {
    const body = within(canvasElement.ownerDocument.body);
    await body.findByRole("menu", { name: "Actions" });
    await userEvent.type(body.getByRole("textbox", { name: "Filter Actions" }), "zzz");
    await expect(body.queryAllByRole("menuitem")).toHaveLength(0);
    await expect(await body.findByText("No entries match your filter")).toBeVisible();
  },
};

export const FilterKeyboard: Story = {
  render: () => <Host />,
  play: async ({ canvasElement }) => {
    const body = within(canvasElement.ownerDocument.body);
    const menu = await body.findByRole("menu", { name: "Actions" });
    const field = body.getByRole("textbox", { name: "Filter Actions" });
    await userEvent.type(field, "dele");
    // ArrowDown leaves the field for the best entry
    await userEvent.keyboard("{ArrowDown}");
    const first = within(menu).getAllByRole("menuitem")[0];
    await waitFor(() => expect(first).toHaveFocus());
    await expect(first).toHaveTextContent("Delete route");
    // End still walks to the last entry drawn
    await userEvent.keyboard("{End}");
    const all = within(menu).getAllByRole("menuitem");
    await waitFor(() => expect(all[all.length - 1]).toHaveFocus());
    // Escape closes the menu
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(body.queryByRole("menu")).toBeNull());
  },
};
