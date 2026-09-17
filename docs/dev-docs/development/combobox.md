# Dashboard dropdowns: the Combobox primitive

Every dropdown in the dashboard used to be a native `<select>`, wrapped by a
`Select` primitive. That wrapper styled the *closed* control only; the open list
was drawn by the operating system, so it ignored the design tokens entirely —
wrong font, wrong colours, no dark mode, no grouping, no secondary line. And a
native select has no type-to-filter, only first-letter jumping, which makes a
fleet of `provider/model` addresses unusable (#968).

`ui/src/components/ui/combobox.tsx` is the one dropdown the dashboard has now.
The `Select` wrapper is gone, and with it the risk of a screen growing a second
kind of dropdown: there is nothing left to reach for.

## Using it

```tsx
import { Combobox, type ComboboxOption } from "@/components/ui/combobox";

const options: ComboboxOption[] = [
  { value: "round_robin", label: "round_robin", description: "even split" },
  { value: "cache_aware", label: "cache_aware", group: "cache" },
];

<Field label={t("pages.routing.strategy")}>
  <Combobox options={options} value={strategy} onChange={setStrategy} />
</Field>;
```

| Prop | What it is for |
|---|---|
| `options` | `{ value, label, description?, group?, disabled? }[]` |
| `value` / `onChange` | controlled; `""` means nothing selected |
| `placeholder` | shown while nothing is selected; defaults to `common.combobox.placeholder` |
| `clearable` | adds an × that resets the selection — for an optional field |
| `size` | `default` matches `Input`; `sm` is the compact toolbar control the screens wrote as `h-8 text-xs` |
| `className` | wrapper layout only (margins, width) — the control's own height comes from `size` |
| `listClassName` | the popup, mainly to widen it past the control |
| `title` | native tooltip, for a compact control whose label sits elsewhere |

`description` is the secondary line under the label. The Playground's model
picker uses it for what `owned_by` says, which the native control could not
render at all.

`group` puts the option under a header. Options are laid out in the order they
arrive, so sort them into their buckets before passing them in — the primitive
starts a new section every time `group` changes.

Labels and errors come from `Field`, as with every other control: `Field`
clones `id`, `aria-describedby` and `aria-invalid` onto the combobox, which
forwards all three to its input.

## What it owes the keyboard

The pattern is the APG *editable combobox with list autocomplete*. The input
**is** the combobox: DOM focus never leaves it, and the active option is named
with `aria-activedescendant` rather than by moving focus. That is what makes it
safe inside a `Sheet` or a `Dialog` — the modal Tab trap in `lib/modal-a11y.ts`
sees focus stay in the panel, because the popup never takes it.

| Key | Behaviour |
|---|---|
| ↓ / ↑ | opens the popup; then moves the active option, wrapping, skipping disabled ones |
| Home / End | first / last enabled option of the *filtered* list |
| Enter | commits the active option; never submits the surrounding form |
| Escape | closes the popup and restores the selected label, leaving the value alone — and stops there, so the Sheet around it stays open |
| Tab | closes the popup and moves on without selecting |
| typing | filters on substring, case- and diacritic-insensitive, over label, value and description |

Opening empties the field so the next keystroke starts a filter rather than
editing the selected label at wherever the caret landed; the selection stays
visible as the placeholder and as the ticked row.

Two structural rules are not cosmetic, and axe fails the stories when either is
broken:

- **The empty-result message is a sibling of the listbox, never a child.** A
  listbox may only own options and groups; a bare paragraph inside one is an
  `aria-required-children` violation.
- **The scroll lives on the listbox, not on the popup around it.** A scrollable
  plain `<div>` is a `scrollable-region-focusable` violation, because axe
  cannot see that the arrow keys on the combobox are what scrolls it.

Group headers are `listbox > group > option` with `aria-label` on the group —
the only shape ARIA allows a header in.

Two more choices exist so a combobox does not make the screen around it harder
to query, for a test or for a screen reader:

- **The result-count live region carries `aria-live` but not `role="status"`.**
  The role would put a second status node on every screen that has a dropdown,
  and the screens whose own notice is a `role="status"` look for it by role.
- **The listbox is named `common.combobox.options`, not after the field.**
  Naming it after the field would put two nodes with the same accessible name
  on the page, and `getByLabelText("Provider")` would stop being unambiguous.
  The combobox is announced immediately before the list, so the context is
  already there.

## Driving one from a story

`userEvent.selectOptions` only works on a native `<select>`. Use `pickOption`
from `ui/src/pages/story-harness.tsx`, which opens the popup and clicks the row
by its accessible name:

```tsx
await pickOption(dialog.getByLabelText("Provider"), "vllm-cluster");
```

To assert what is *offered* rather than pick one, `openOptions` opens the popup
and hands back the listbox — the options are not inside the control, so
`within(combobox).getAllByRole("option")` finds nothing:

```tsx
const offered = within(await openOptions(canvas.getByLabelText("Customer")));
await expect(offered.getByRole("option", { name: "Acme" })).toBeInTheDocument();
```

Note that a combobox's `value` is the option's **label**, not the value that
goes on the wire — `toHaveValue("openai-prod")`, not `toHaveValue("prov-1")`.

## Copy

The three strings the primitive owns live under `common.combobox.*` in every
catalog in `ui/src/lib/i18n/locales/`: `placeholder`, `clear`, `noMatches`, plus
the pluralised `results` that the polite live region announces after each
keystroke so a screen-reader user hears how many options are left.

## Stories

`ui/src/components/ui/combobox.stories.tsx` covers the render states — default,
empty, grouped, long list, clearable, disabled control, disabled option — and
asserts the behaviour rather than eyeballing it: listbox semantics and
`aria-controls` wiring, named groups, filter-as-you-type, the no-match message,
arrows plus Enter, Home/End, Escape reverting, a disabled option refusing both
click and Enter, and clearing. Every one of them also runs the axe gate, which
is the point of the issue this primitive closes.

## Not done yet

- Very long lists are not virtualised — filtering narrows them fast enough that
  it has not mattered, but a picker over thousands of rows will want it.
