# Dashboard dropdowns: the Combobox primitive

Every dropdown in the dashboard used to be a native `<select>`, wrapped by a
`Select` primitive. That wrapper styled the _closed_ control only; the open list
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

| Prop                 | What it is for                                                                                                                                                                                                                                                                                                                                                                                            |
| -------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `options`            | `{ value, label, description?, group?, icon?, disabled? }[]`                                                                                                                                                                                                                                                                                                                                              |
| `value` / `onChange` | controlled; `""` means nothing selected                                                                                                                                                                                                                                                                                                                                                                   |
| `placeholder`        | shown while nothing is selected; defaults to `common.combobox.placeholder`                                                                                                                                                                                                                                                                                                                                |
| `allowCustom`        | accepts a value outside `options`: a typed string with no exact match gets a "Use …" row (`common.combobox.use`)                                                                                                                                                                                                                                                                                          |
| `commitOnBlur`       | with `allowCustom`, keeps what was typed when focus leaves, instead of dropping it unless "Use …" was picked                                                                                                                                                                                                                                                                                              |
| `commitOnInput`      | with `allowCustom`, reports what is typed as the value on every keystroke; for a field whose text is the value as it is typed, the empty text included: `commitOnBlur` keeps typed text on blur but not an erased field, so clearing it and moving on commits nothing (#3244). Escape closes the list but cannot discard the text, which was reported on each keystroke; under `commitOnBlur` it drops it |
| `required`           | `required` and `aria-required` on the text input, so `<Field required>` and a refused submit see an empty pick (#3242)                                                                                                                                                                                                                                                                                    |
| `emptyText`          | said in place of "No options match your filter" when the popup has no option to list at all (#2810)                                                                                                                                                                                                                                                                                                       |
| `onQueryChange`      | what is typed, on every keystroke, for a list that is only part of what exists and asks its source for the rest (#3385); filtering the options on screen stays the combobox's own                                                                                                                                                                                                                         |
| `onFocus` / `onBlur` | focus came in / left the control as a whole; a move between its own parts is neither                                                                                                                                                                                                                                                                                                                      |
| `clearable`          | adds an × that resets the selection — for an optional field                                                                                                                                                                                                                                                                                                                                               |
| `size`               | `default` matches `Input`; `sm` is the compact toolbar control the screens wrote as `h-8 text-xs`                                                                                                                                                                                                                                                                                                         |
| `className`          | wrapper layout only (margins, width) — the control's own height comes from `size`                                                                                                                                                                                                                                                                                                                         |
| `listClassName`      | the popup, mainly to widen it past the control                                                                                                                                                                                                                                                                                                                                                            |
| `title`              | native tooltip, for a compact control whose label sits elsewhere                                                                                                                                                                                                                                                                                                                                          |

`description` is the secondary line under the label. The Playground's model
picker uses it for what `owned_by` says, which the native control could not
render at all.

`icon` is a leading mark drawn before the label, in the row and in the closed
control once the option is chosen (#3059). It is decorative (give it
`aria-hidden`; the label names the option). The provider pickers put
`<ProviderLogo kind={…} />` there; see "Provider logos" below.

`group` puts the option under a header. Options are laid out in the order they
arrive, so sort them into their buckets before passing them in — the primitive
starts a new section every time `group` changes.

Labels and errors come from `Field`, as with every other control: `Field`
clones `id`, `aria-describedby` and `aria-invalid` onto the combobox, which
forwards all three to its input.

## What it owes the keyboard

The pattern is the APG _editable combobox with list autocomplete_. The input
**is** the combobox: DOM focus never leaves it, and the active option is named
with `aria-activedescendant` rather than by moving focus. That is what makes it
safe inside a `Sheet` or a `Dialog` — the modal Tab trap in `lib/modal-a11y.ts`
sees focus stay in the panel, because the popup never takes it.

| Key        | Behaviour                                                                                                                      |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------ |
| ↓ / ↑      | opens the popup; then moves the active option, wrapping, skipping disabled ones                                                |
| Home / End | first / last enabled option of the _filtered_ list                                                                             |
| Enter      | commits the active option; never submits the surrounding form                                                                  |
| Escape     | closes the popup and restores the selected label, leaving the value alone — and stops there, so the Sheet around it stays open |
| Tab        | closes the popup and moves on without selecting                                                                                |
| typing     | filters on substring, case- and diacritic-insensitive, over label, value and description                                       |

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

To assert what is _offered_ rather than pick one, `openOptions` opens the popup
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

## Long lists

Past 120 filtered options the popup renders a window of rows rather than all of
them (#1579). Two things make that cheaper here than a virtual list usually is:

- **The heights are known, not measured.** A row is tall exactly when its option
  carries a `description`, so `measure()` computes every offset from the data in
  one pass. The same numbers are applied to the rows as an inline `height`, so
  the arithmetic and the layout cannot drift — a row is what the table says it
  is, rather than the table being a guess at what the classes produce. Changing
  a row's padding or type scale means changing `ROW` with it, and
  `WindowRowsMatchTheirMeasuredHeight` fails when they disagree.
- **The window never drops the active option.** Whatever the scroll says, the
  row named by `aria-activedescendant` is rendered, because a name pointing at
  an unmounted node announces nothing at all — the one regression #968 must not
  have. `TheActiveOptionStaysMounted` walks End, Home and an arrow to prove it.

The rows above and below the window are one `role="none"` box each, so the
listbox still owns nothing but options and groups. A group whose header has
scrolled out keeps its wrapper and its `aria-label`, so a run of rows is still
announced under the right name.

Below the threshold nothing windows: filtering narrows a list to a handful in
two keystrokes, and the plain path is the one every other story exercises.

## Fuzzy filtering

What is typed is matched as a subsequence, not a substring, and the matches are
ranked best first (#3058). The scorer is `fuzzyMatch` in `ui/src/lib/fuzzy.ts`,
the same one the command palette uses, so `oai` finds `openai` and `rr` finds
`Routing Rules`:

- a hit at the head of the label, or on a word boundary (`-`, `_`, `/`, `.`,
  space), beats one inside a word
- adjacent characters beat scattered ones
- a shorter label wins a tie
- the value and the description are searched too, at a discount, so they never
  outrank a hit on the label
- case and diacritics do not matter

The characters that matched are drawn with `HighlightedText`
(`ui/src/components/ui/highlighted-text.tsx`). A grouped list stays grouped
while ranked: groups are ordered by their best row, so a header is printed once.
With no query the options keep the order they were passed in.

Menus filter the same way: `Menu` (`ui/src/components/ui/menu.tsx`) has a search
field above its entries. Typing while an entry has focus moves to the field,
ArrowDown returns to the entries, and Enter takes the best match. Entries rank
themselves with a CSS `order`, so a menu's children stay arbitrary nodes; a
`MenuItem` whose children are not plain text passes `searchLabel`. Nothing else
in the dashboard is a dropdown: `bun run check:primitives` fails a bare
`<select>` and a hand-written `role="listbox"`, `option` or `combobox`
(`hand-rolled-listbox`). The command palette is waived, with its reason in the
source, because it opens routes and records rather than choosing a value.

## Provider logos

`ProviderLogo` (`ui/src/components/ui/provider-logo.tsx`) draws a provider
kind's mark in the picker's `icon` slot: the provider sheet's kind picker, the
provider pickers in the model and provider group sheets. The marks are the
single-colour paths of [`simple-icons`](https://simpleicons.org) (CC0-1.0), a
pinned dependency imported icon by icon in `ui/src/lib/provider-logos.ts`, so
the bundle carries only the ones named there and nothing is fetched at runtime.
They are painted in `currentColor` from the secondary text token. A kind the set
has no mark for gets a monogram (its first letter in a quiet frame) rather than
a gap. The dashboard theme is dark-only, so there is no light variant to keep.

When a provider kind is added, add its mark to `PROVIDER_LOGOS` or name it in
the monogram list of `ui/src/lib/provider-logos.test.ts`; the test fails until
one of the two is done.
