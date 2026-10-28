import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { Combobox, type ComboboxOption } from "./combobox";
import { Field } from "./field";
import { ProviderLogo } from "./provider-logo";
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
  {
    value: "cache_aware",
    label: "cache_aware",
    description: "needs a kv-event source",
    disabled: true,
  },
  { value: "weighted", label: "weighted" },
];

function Controlled({
  options = STRATEGIES,
  initial = "",
  clearable = false,
  disabled = false,
  label = "Strategy",
  width = "w-80",
}: {
  options?: ComboboxOption[];
  initial?: string;
  clearable?: boolean;
  disabled?: boolean;
  label?: string;
  width?: string;
}) {
  const [value, setValue] = React.useState(initial);
  return (
    <div className={width}>
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

// a selected label longer than the control ends in an ellipsis rather than being
// cut mid-glyph under the chevron, and the full label stays on the title (#2261)
export const LongSelectedLabelEllipsizes: Story = {
  render: () => (
    <Controlled
      width="w-40"
      options={[
        { value: "block", label: "Блокировать при сбое выполнения плагина" },
        { value: "allow", label: "Пропускать" },
      ]}
      initial="block"
      label="Failure policy"
    />
  ),
  play: async ({ canvasElement }) => {
    const input = within(canvasElement).getByRole("combobox") as HTMLInputElement;
    await expect(getComputedStyle(input).textOverflow).toBe("ellipsis");
    await expect(input.scrollWidth).toBeGreaterThan(input.clientWidth);
    await expect(input.title).toBe("Блокировать при сбое выполнения плагина");
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
    // equal matches: the shorter label is listed first
    await expect(options[0]).toHaveTextContent("anthropic/claude-haiku");
    await expect(options[1]).toHaveTextContent("anthropic/claude-sonnet");
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

// keyboard path: Backspace (or Delete) with nothing typed removes the selection
export const ClearsFromTheKeyboard: Story = {
  render: () => <Controlled initial="weighted" clearable />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const input = canvas.getByRole("combobox", { name: "Strategy" });
    await userEvent.tab();
    await expect(input).toHaveFocus();
    await userEvent.keyboard("{Backspace}");
    await expect(canvas.getByText("current: (none)")).toBeInTheDocument();
    await expect(input).toHaveValue("");
    await expect(input).toHaveFocus();
  },
};

// a field that is mostly free text and only suggests (#2810): the control
// plane's list of what a provider serves is a hint, and an id outside it is
// still the operator's to type
function Suggesting({
  options = STRATEGIES,
  commitOnBlur = true,
  commitOnInput = false,
  emptyText,
}: {
  options?: ComboboxOption[];
  commitOnBlur?: boolean;
  commitOnInput?: boolean;
  emptyText?: string;
}) {
  const [value, setValue] = React.useState("");
  const [events, setEvents] = React.useState<string[]>([]);
  const note = (event: string) => setEvents((seen) => [...seen, event]);
  return (
    <div className="w-80 space-y-2">
      <Field label="Upstream model" hint={`current: ${value || "(none)"}`}>
        <Combobox
          allowCustom
          clearable
          commitOnBlur={commitOnBlur}
          commitOnInput={commitOnInput}
          emptyText={emptyText}
          options={options}
          value={value}
          onChange={setValue}
          onFocus={() => note("focus")}
          onBlur={() => note("blur")}
        />
      </Field>
      <p>{`events: ${events.join(",") || "(none)"}`}</p>
      <button type="button">elsewhere</button>
    </div>
  );
}

// leaving the field keeps what was typed, with no "Use …" row to find first
export const KeepsWhatWasTypedWhenFocusLeaves: Story = {
  render: () => <Suggesting />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const input = canvas.getByRole("combobox", { name: "Upstream model" });
    await userEvent.click(input);
    await userEvent.keyboard("acme/support-ft-v2");
    await userEvent.tab();
    await waitFor(() =>
      expect(canvas.getByText("current: acme/support-ft-v2")).toBeInTheDocument(),
    );
    await expect(input).toHaveValue("acme/support-ft-v2");
    await expect(input).toHaveAttribute("aria-expanded", "false");
  },
};

// the same on a pointer: clicking away is leaving, and a click on an option is not
export const KeepsWhatWasTypedWhenAnotherControlIsClicked: Story = {
  render: () => <Suggesting />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("combobox", { name: "Upstream model" }));
    await userEvent.keyboard("acme/support-ft-v2");
    await userEvent.click(canvas.getByRole("button", { name: "elsewhere" }));
    await waitFor(() =>
      expect(canvas.getByText("current: acme/support-ft-v2")).toBeInTheDocument(),
    );
  },
};

// typed in another case, the text names the option it matches rather than a second spelling
export const TypedTextNamesTheOptionItMatches: Story = {
  render: () => <Suggesting />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("combobox", { name: "Upstream model" }));
    await userEvent.keyboard("ROUND_ROBIN");
    await userEvent.tab();
    await waitFor(() => expect(canvas.getByText("current: round_robin")).toBeInTheDocument());
  },
};

// what the flag is for: without it a typed value that was not picked is dropped
export const WithoutTheFlagTypedTextIsDropped: Story = {
  render: () => <Suggesting commitOnBlur={false} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const input = canvas.getByRole("combobox", { name: "Upstream model" });
    await userEvent.click(input);
    await userEvent.keyboard("acme/support-ft-v2");
    await userEvent.tab();
    await waitFor(() => expect(input).toHaveValue(""));
    await expect(canvas.getByText("current: (none)")).toBeInTheDocument();
  },
};

// beside a Save that waits for the draft to differ (#3244): the value follows
// the keystrokes, so there is nothing left to commit when the press arrives
export const CommitOnInputFollowsTheKeystrokes: Story = {
  render: () => <Suggesting commitOnBlur={false} commitOnInput />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const input = canvas.getByRole("combobox", { name: "Upstream model" });
    await userEvent.click(input);
    await userEvent.keyboard("acme/support");
    await expect(canvas.getByText("current: acme/support")).toBeInTheDocument();
    // a typed id that names an option exactly takes the option's own spelling
    await userEvent.clear(input);
    await userEvent.keyboard("ROUND_ROBIN");
    await expect(canvas.getByText("current: round_robin")).toBeInTheDocument();
    await userEvent.clear(input);
    await expect(canvas.getByText("current: (none)")).toBeInTheDocument();
  },
};

// a required combobox says so to assistive technology, and an empty one is
// what a refused submit finds (#3242)
export const RequiredIsOnTheTextInput: Story = {
  render: () => (
    <Field label="Upstream model" required>
      <Combobox options={STRATEGIES} value="" onChange={() => {}} />
    </Field>
  ),
  play: async ({ canvasElement }) => {
    const input = within(canvasElement).getByRole("combobox", { name: /Upstream model/ });
    await expect(input).toBeRequired();
    await expect(input).toHaveAttribute("aria-required", "true");
  },
};

// Escape is a way out, not a way to keep: it discards what was typed
export const EscapeDiscardsWhatWasTyped: Story = {
  render: () => <Suggesting />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const input = canvas.getByRole("combobox", { name: "Upstream model" });
    await userEvent.click(input);
    await userEvent.keyboard("acme/support-ft-v2{Escape}");
    await userEvent.tab();
    await expect(canvas.getByText("current: (none)")).toBeInTheDocument();
    await expect(input).toHaveValue("");
  },
};

// nothing was typed: focus passing through changes nothing
export const FocusPassingThroughKeepsTheValue: Story = {
  render: () => <Suggesting />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const input = canvas.getByRole("combobox", { name: "Upstream model" });
    await userEvent.click(input);
    await userEvent.keyboard("weighted{Enter}");
    await userEvent.tab();
    await userEvent.tab({ shift: true });
    await userEvent.tab();
    await expect(canvas.getByText("current: weighted")).toBeInTheDocument();
  },
};

// the owner is told when focus arrives and when it leaves the control as a
// whole, once each: picking an option is a move inside it
export const ReportsFocusAndBlurOnce: Story = {
  render: () => <Suggesting />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("combobox", { name: "Upstream model" }));
    await userEvent.click(await canvas.findByRole("option", { name: /^weighted/ }));
    await expect(canvas.getByText("events: focus")).toBeInTheDocument();
    await userEvent.click(canvas.getByRole("button", { name: "elsewhere" }));
    await waitFor(() => expect(canvas.getByText("events: focus,blur")).toBeInTheDocument());
  },
};

// a list with nothing to offer says why instead of "No options match your filter",
// which would blame a filter nobody typed
export const SaysWhyThereIsNothingToOffer: Story = {
  render: () => <Suggesting options={[]} emptyText="This provider listed no models." />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("combobox", { name: "Upstream model" }));
    await expect(canvas.getByText("This provider listed no models.")).toBeInTheDocument();
    await expect(canvas.queryByText("No options match your filter")).not.toBeInTheDocument();
    // typing a value gives the "Use …" row, and the message makes way for it
    await userEvent.keyboard("acme/ft");
    await expect(canvas.queryByText("This provider listed no models.")).not.toBeInTheDocument();
    await expect(canvas.getByRole("option", { name: "Use “acme/ft”" })).toBeInTheDocument();
  },
};

// the message is for an empty list only: a filter that matches nothing in a
// list that has options still blames the filter
export const AFilterThatMatchesNothingStillSaysSo: Story = {
  render: () => (
    <div className="w-80">
      <Field label="Model">
        <Combobox
          options={GROUPED}
          value=""
          onChange={() => {}}
          emptyText="This provider listed no models."
        />
      </Field>
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("combobox", { name: "Model" }));
    await userEvent.keyboard("zzzz");
    await expect(canvas.getByText("No options match your filter")).toBeInTheDocument();
    await expect(canvas.queryByText("This provider listed no models.")).not.toBeInTheDocument();
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

// the points of #3058: typed text is a subsequence match ranked best-first by
// the command palette's scorer, not a substring filter that keeps list order
const PROVIDERS: ComboboxOption[] = [
  { value: "anthropic", label: "Anthropic", description: "claude models" },
  { value: "openai_compatible", label: "OpenAI-compatible" },
  { value: "openai", label: "OpenAI" },
  { value: "ollama", label: "Ollama" },
  { value: "openrouter", label: "OpenRouter" },
];

export const FuzzyRanksBestFirst: Story = {
  render: () => <Controlled options={PROVIDERS} label="Provider" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const combobox = canvas.getByRole("combobox", { name: "Provider" });
    await userEvent.click(combobox);
    // "oai" is not a substring of anything, but a subsequence of both openai
    // rows; the shorter label wins, though it sits later in the list
    await userEvent.keyboard("oai");
    const rows = within(canvas.getByRole("listbox")).getAllByRole("option");
    await expect(rows.map((r) => r.textContent)).toEqual(["OpenAI", "OpenAI-compatible"]);
    // the characters that matched are marked
    await expect(rows[0].querySelectorAll("[data-match]").length).toBeGreaterThan(0);
    // Enter takes the best match
    await userEvent.keyboard("{Enter}");
    await expect(combobox).toHaveValue("OpenAI");
  },
};

export const FuzzyFindsByValueAndDescription: Story = {
  render: () => <Controlled options={PROVIDERS} label="Provider" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("combobox", { name: "Provider" }));
    // the description is searched too, at a discount to the label
    await userEvent.keyboard("claude");
    const rows = within(canvas.getByRole("listbox")).getAllByRole("option");
    await expect(rows).toHaveLength(1);
    await expect(rows[0]).toHaveTextContent("Anthropic");
  },
};

export const FuzzyKeepsGroupsTogether: Story = {
  render: () => <Controlled options={GROUPED} label="Model" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("combobox", { name: "Model" }));
    await userEvent.keyboard("o");
    // each header is printed once however the rows were reordered
    const groups = within(canvas.getByRole("listbox")).getAllByRole("group");
    const names = groups.map((g) => g.getAttribute("aria-label"));
    await expect(new Set(names).size).toBe(names.length);
  },
};

export const FuzzyNoMatch: Story = {
  render: () => <Controlled options={PROVIDERS} label="Provider" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("combobox", { name: "Provider" }));
    await userEvent.keyboard("zzz");
    await expect(within(canvas.getByRole("listbox")).queryAllByRole("option")).toHaveLength(0);
    await expect(canvas.getByText("No options match your filter")).toBeVisible();
  },
};

// the option's leading icon slot (#3059): the mark is in the row and, once an
// option is chosen, in the closed control. a kind without a mark shows a
// monogram in the same box so the labels still line up
const WITH_LOGOS: ComboboxOption[] = [
  "anthropic",
  "ollama",
  "gemini",
  "groq",
  "a_kind_the_set_has_never_heard_of",
].map((kind) => ({
  value: kind,
  label: kind,
  icon: <ProviderLogo kind={kind} />,
}));

export const WithLeadingIcons: Story = {
  render: () => <Controlled options={WITH_LOGOS} initial="anthropic" label="Provider" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const combobox = canvas.getByRole("combobox", { name: "Provider" });
    // the closed control carries the selection's mark
    await expect(canvasElement.querySelector('[data-provider-logo="anthropic"]')).not.toBeNull();
    await userEvent.click(combobox);
    const rows = within(canvas.getByRole("listbox")).getAllByRole("option");
    await expect(rows).toHaveLength(5);
    for (const row of rows) await expect(row.querySelector("[data-provider-logo]")).not.toBeNull();
    // a drawn mark follows the text colour, so it follows the theme
    const mark = rows[0].querySelector("[data-provider-logo]")!;
    await expect(mark.getAttribute("fill")).toBe("currentColor");
    // the unknown kind falls back to a monogram, not an empty box
    await expect(rows[4].querySelector("[data-monogram]")).toHaveAttribute("data-monogram", "A");
    await expect(rows[4]).toHaveTextContent(/^a_kind_the_set_has_never_heard_of$/);
    // and the icon does not leak into the option's accessible name
    await expect(rows[0]).toHaveAccessibleName("anthropic");
  },
};
