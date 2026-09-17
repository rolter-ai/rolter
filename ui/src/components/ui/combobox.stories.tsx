import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { Combobox, type ComboboxOption } from "./combobox";
import { Field } from "./field";
import { Sheet, SheetBody, SheetHeader } from "./sheet";

const meta = {
  title: "Primitives/Combobox",
  component: Combobox,
  parameters: { layout: "padded" },
  // the stories drive their own state through a wrapper; these only satisfy
  // the required props on the type
  args: { options: [], value: "", onChange: () => {} },
} satisfies Meta<typeof Combobox>;

export default meta;
type Story = StoryObj<typeof meta>;

const STRATEGIES: ComboboxOption[] = [
  { value: "round_robin", label: "round_robin", description: "even split across targets" },
  { value: "random", label: "random", description: "uniform random pick" },
  { value: "power_of_two", label: "power_of_two", description: "least loaded of two" },
  { value: "cache_aware", label: "cache_aware", description: "prefix-cache affinity" },
  { value: "weighted", label: "weighted", description: "static weights" },
];

const GROUPED: ComboboxOption[] = [
  { value: "fake-llm", label: "fake-llm", group: "rolter" },
  { value: "gpt-4o", label: "gpt-4o", group: "routes" },
  { value: "gpt-4o-mini", label: "gpt-4o-mini", group: "routes" },
  { value: "openai/gpt-4.1", label: "openai/gpt-4.1", group: "openai" },
  { value: "openai/o3", label: "openai/o3", group: "openai" },
  { value: "anthropic/claude-sonnet", label: "anthropic/claude-sonnet", group: "anthropic" },
  { value: "anthropic/claude-haiku", label: "anthropic/claude-haiku", group: "anthropic" },
];

// a fleet-sized list — the case the native <select> made unusable (#968), at a
// size that used to put every row in the document (#1579). the descriptions are
// uneven on purpose: a described row is taller, which is what makes the window
// variable-height rather than a multiplication
const LONG: ComboboxOption[] = Array.from({ length: 2400 }, (_, i) => ({
  value: `provider-${i % 12}/model-${i}`,
  label: `provider-${i % 12}/model-${i}`,
  description: i % 3 === 0 ? "provider pin" : undefined,
  group: `provider-${i % 12}`,
}));

// an ungrouped list of the same size, for the window's own arithmetic
const LONG_FLAT: ComboboxOption[] = LONG.map(({ group: _group, ...rest }) => rest);

const WITH_DISABLED: ComboboxOption[] = [
  { value: "round_robin", label: "round_robin" },
  { value: "cache_aware", label: "cache_aware", description: "needs a kv-event source", disabled: true },
  { value: "weighted", label: "weighted" },
];

function Controlled({
  options = STRATEGIES,
  initial = "",
  clearable = false,
  disabled = false,
  label = "Strategy",
}: {
  options?: ComboboxOption[];
  initial?: string;
  clearable?: boolean;
  disabled?: boolean;
  label?: string;
}) {
  const [value, setValue] = React.useState(initial);
  return (
    <div className="w-80">
      <Field label={label} hint={`current: ${value || "(none)"}`}>
        <Combobox
          options={options}
          value={value}
          onChange={setValue}
          clearable={clearable}
          disabled={disabled}
        />
      </Field>
    </div>
  );
}

export const Default: Story = { render: () => <Controlled initial="round_robin" /> };

export const Empty: Story = { render: () => <Controlled /> };

export const Grouped: Story = {
  render: () => <Controlled options={GROUPED} initial="fake-llm" label="Model" />,
};

export const LongList: Story = {
  render: () => <Controlled options={LONG} initial="provider-0/model-0" label="Model" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("combobox", { name: "Model" }));
    const listbox = canvas.getByRole("listbox");
    // a window, not the list: 2400 options would be 2400 nodes, and the popup
    // is 240px tall (#1579)
    const rendered = within(listbox).getAllByRole("option");
    await expect(rendered.length).toBeGreaterThan(0);
    await expect(rendered.length).toBeLessThan(80);
    // the scroll still spans every row, so the scrollbar tells the truth about
    // how much list there is
    await expect(listbox.scrollHeight).toBeGreaterThan(50_000);
  },
};

// the rows the window places have to be exactly as tall as the arithmetic that
// placed them, or the popup drifts further out of register the further it is
// scrolled — and a described row is taller than a plain one
export const WindowRowsMatchTheirMeasuredHeight: Story = {
  render: () => <Controlled options={LONG_FLAT} initial="provider-0/model-0" label="Model" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("combobox", { name: "Model" }));
    const listbox = canvas.getByRole("listbox");
    const rendered = within(listbox).getAllByRole("option");
    const described = rendered.find((o) => o.textContent?.includes("provider pin"));
    const plain = rendered.find((o) => !o.textContent?.includes("provider pin"));
    await expect(described?.getBoundingClientRect().height).toBe(48);
    await expect(plain?.getBoundingClientRect().height).toBe(32);
    // and the total is the sum of every row's height, not a row count times one
    const described_count = LONG_FLAT.filter((o) => o.description).length;
    await expect(listbox.scrollHeight).toBe(
      described_count * 48 + (LONG_FLAT.length - described_count) * 32,
    );
  },
};

// the one thing a virtual list must not break: the active option is named by
// aria-activedescendant, and a name pointing at an unmounted node announces
// nothing at all (#968)
export const TheActiveOptionStaysMounted: Story = {
  render: () => <Controlled options={LONG} initial="provider-0/model-0" label="Model" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const combobox = canvas.getByRole("combobox", { name: "Model" });
    await userEvent.click(combobox);
    const named = () => {
      const id = combobox.getAttribute("aria-activedescendant");
      return id ? canvasElement.ownerDocument.getElementById(id) : null;
    };
    await expect(named()).not.toBeNull();
    // End jumps past the window to the last option; the node it names has to
    // exist there too
    await userEvent.keyboard("{End}");
    await expect(named()).not.toBeNull();
    await expect(named()).toHaveTextContent("model-2399");
    await userEvent.keyboard("{Home}");
    await expect(named()).not.toBeNull();
    await expect(named()).toHaveTextContent("model-0");
    // and arrowing on from Home walks the list rather than restarting it
    await userEvent.keyboard("{ArrowDown}{ArrowDown}");
    await expect(named()).toHaveTextContent("model-2");
  },
};

// filtering reaches rows the window never rendered: the filter runs over every
// option, not over the ones currently in the document
export const FilterReachesUnrenderedRows: Story = {
  render: () => <Controlled options={LONG} initial="provider-0/model-0" label="Model" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const combobox = canvas.getByRole("combobox", { name: "Model" });
    await userEvent.click(combobox);
    await userEvent.keyboard("model-2399");
    const options = within(canvas.getByRole("listbox")).getAllByRole("option");
    await expect(options).toHaveLength(1);
    await expect(options[0]).toHaveTextContent("provider-11/model-2399");
    await userEvent.keyboard("{Enter}");
    await expect(combobox).toHaveValue("provider-11/model-2399");
  },
};

// a group whose heading has scrolled out of the window still names its rows
export const ScrolledGroupsKeepTheirName: Story = {
  render: () => <Controlled options={LONG} initial="provider-0/model-0" label="Model" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("combobox", { name: "Model" }));
    const listbox = canvas.getByRole("listbox");
    listbox.scrollTop = 4_000;
    listbox.dispatchEvent(new Event("scroll", { bubbles: true }));
    await waitFor(async () => {
      const groups = within(listbox).getAllByRole("group");
      await expect(groups.length).toBeGreaterThan(0);
      for (const group of groups) {
        await expect(group.getAttribute("aria-label")).toMatch(/^provider-\d+$/);
        await expect(within(group).getAllByRole("option").length).toBeGreaterThan(0);
      }
    });
  },
};

export const Clearable: Story = {
  render: () => <Controlled initial="weighted" clearable />,
};

export const Disabled: Story = {
  render: () => <Controlled initial="random" disabled />,
};

export const DisabledOption: Story = {
  render: () => <Controlled options={WITH_DISABLED} initial="round_robin" />,
};

// the control is a combobox, the popup is a listbox of options, and the label
// from Field names it — this is the native-select baseline the primitive has
// to keep (#968)
export const HasListboxSemantics: Story = {
  render: () => <Controlled options={GROUPED} initial="fake-llm" label="Model" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const combobox = canvas.getByRole("combobox", { name: "Model" });
    await expect(combobox).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(combobox);
    await expect(combobox).toHaveAttribute("aria-expanded", "true");
    const listbox = canvas.getByRole("listbox");
    await expect(listbox).toHaveAttribute("id", combobox.getAttribute("aria-controls"));
    const options = within(listbox).getAllByRole("option");
    await expect(options).toHaveLength(GROUPED.length);
    // the selected option is the one aria-selected names
    await expect(within(listbox).getByRole("option", { selected: true })).toHaveTextContent(
      "fake-llm",
    );
  },
};

// a grouped list is `listbox > group > option`, so the header names the run of
// rows under it instead of being a stray paragraph among the options
export const GroupsAreNamed: Story = {
  render: () => <Controlled options={GROUPED} initial="fake-llm" label="Model" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("combobox", { name: "Model" }));
    const listbox = canvas.getByRole("listbox");
    const groups = within(listbox).getAllByRole("group");
    await expect(groups.map((g) => g.getAttribute("aria-label"))).toEqual([
      "rolter",
      "routes",
      "openai",
      "anthropic",
    ]);
    await expect(within(groups[1]).getAllByRole("option")).toHaveLength(2);
  },
};

// typing narrows the list to substring matches, in any position
export const FiltersAsYouType: Story = {
  render: () => <Controlled options={GROUPED} initial="fake-llm" label="Model" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const combobox = canvas.getByRole("combobox", { name: "Model" });
    await userEvent.click(combobox);
    await userEvent.keyboard("claude");
    const options = within(canvas.getByRole("listbox")).getAllByRole("option");
    await expect(options).toHaveLength(2);
    await expect(options[0]).toHaveTextContent("anthropic/claude-sonnet");
  },
};

// a filter that matches nothing says so instead of showing an empty box
export const NoMatches: Story = {
  render: () => <Controlled options={GROUPED} label="Model" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("combobox", { name: "Model" }));
    await userEvent.keyboard("zzzz");
    await expect(canvas.queryAllByRole("option")).toHaveLength(0);
    // the message sits beside the listbox, never inside it (a listbox may only
    // own options and groups)
    await expect(canvas.getByText("No options match your filter")).toBeInTheDocument();
  },
};

// keyboard: arrows move the active option (named by aria-activedescendant,
// never by moving DOM focus), Enter commits it
export const ArrowKeysAndEnter: Story = {
  render: () => <Controlled initial="round_robin" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const combobox = canvas.getByRole("combobox", { name: "Strategy" });
    combobox.focus();
    await userEvent.keyboard("{ArrowDown}");
    await expect(combobox).toHaveAttribute("aria-expanded", "true");
    // focus stayed on the input; the active option is named instead
    await expect(combobox).toHaveFocus();
    await userEvent.keyboard("{ArrowDown}");
    const active = combobox.getAttribute("aria-activedescendant");
    await expect(document.getElementById(active ?? "")).toHaveTextContent("random");
    await userEvent.keyboard("{Enter}");
    await expect(combobox).toHaveAttribute("aria-expanded", "false");
    await expect(canvas.getByText("current: random")).toBeInTheDocument();
  },
};

// keyboard: Home and End jump to the ends of the filtered list
export const HomeAndEnd: Story = {
  render: () => <Controlled initial="round_robin" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const combobox = canvas.getByRole("combobox", { name: "Strategy" });
    combobox.focus();
    await userEvent.keyboard("{ArrowDown}{End}");
    let active = combobox.getAttribute("aria-activedescendant");
    await expect(document.getElementById(active ?? "")).toHaveTextContent("weighted");
    await userEvent.keyboard("{Home}");
    active = combobox.getAttribute("aria-activedescendant");
    await expect(document.getElementById(active ?? "")).toHaveTextContent("round_robin");
  },
};

// keyboard: Escape closes the popup and restores the selected label, leaving
// the value untouched
export const EscapeReverts: Story = {
  render: () => <Controlled initial="weighted" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const combobox = canvas.getByRole("combobox", { name: "Strategy" });
    await userEvent.click(combobox);
    await userEvent.keyboard("rand");
    await expect(combobox).toHaveValue("rand");
    await userEvent.keyboard("{Escape}");
    await expect(combobox).toHaveAttribute("aria-expanded", "false");
    await expect(combobox).toHaveValue("weighted");
    await expect(canvas.getByText("current: weighted")).toBeInTheDocument();
  },
};

// a disabled option is announced as such and refuses both click and Enter
export const DisabledOptionIsNotSelectable: Story = {
  render: () => <Controlled options={WITH_DISABLED} initial="round_robin" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const combobox = canvas.getByRole("combobox", { name: "Strategy" });
    await userEvent.click(combobox);
    const blocked = within(canvas.getByRole("listbox")).getByRole("option", {
      name: /cache_aware/,
    });
    await expect(blocked).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(blocked);
    await expect(canvas.getByText("current: round_robin")).toBeInTheDocument();
    // arrowing skips it too — one step down from the first option lands on the
    // third
    await userEvent.keyboard("{ArrowDown}");
    const active = combobox.getAttribute("aria-activedescendant");
    await expect(document.getElementById(active ?? "")).toHaveTextContent("weighted");
  },
};

// the clear affordance resets an optional field back to nothing selected
export const ClearsSelection: Story = {
  render: () => <Controlled initial="weighted" clearable />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "Clear selection" }));
    await expect(canvas.getByText("current: (none)")).toBeInTheDocument();
    await expect(canvas.getByRole("combobox", { name: "Strategy" })).toHaveValue("");
  },
};

function InSheet() {
  const [open, setOpen] = React.useState(true);
  const [value, setValue] = React.useState("round_robin");
  return (
    <Sheet open={open} onOpenChange={setOpen}>
      <SheetHeader title="Edit route" subtitle="gpt-4o" onClose={() => setOpen(false)} />
      <SheetBody>
        <Field label="Strategy" hint={`current: ${value}`}>
          <Combobox options={STRATEGIES} value={value} onChange={setValue} />
        </Field>
      </SheetBody>
    </Sheet>
  );
}

export const InsideASheet: Story = { render: () => <InSheet /> };

// the popup is operated from inside a modal without the modal fighting it:
// focus never leaves the input, so the Tab trap is satisfied, and Escape takes
// the popup down without taking the Sheet with it
export const EscapeInsideASheetKeepsTheSheet: Story = {
  render: () => <InSheet />,
  play: async ({ canvasElement }) => {
    void canvasElement;
    // the Sheet portals to <body>, so the query root is the document
    const body = within(document.body);
    const combobox = body.getByRole("combobox", { name: "Strategy" });
    await userEvent.click(combobox);
    await expect(combobox).toHaveAttribute("aria-expanded", "true");
    await expect(combobox).toHaveFocus();
    await userEvent.keyboard("{Escape}");
    await expect(combobox).toHaveAttribute("aria-expanded", "false");
    await expect(body.getByRole("dialog")).toBeInTheDocument();
    // a second Escape, with the popup already closed, is the Sheet's again
    await userEvent.keyboard("{Escape}");
    await expect(body.queryByRole("dialog")).toBeNull();
  },
};
