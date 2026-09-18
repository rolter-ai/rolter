# Dashboard form primitives

A sheet in the dashboard is a long, dense form: forty-odd controls in six
collapsible sections, most of them a label, a control and a conditional error.
`Field` (`ui/src/components/ui/field.tsx`) assembles one of those rows on a
screen. A sheet needs the same pieces laid out by hand — two controls on a row,
a padlock beside an input, a chip list under a group label — so the pieces
themselves are components.

They live in `ui/src/components/ui/` beside the other primitives, and their
stories are grouped under **Forms/** in Storybook.

| Primitive | File | What it is |
|---|---|---|
| `FieldLabel` | `field-label.tsx` | the compact label a sheet row carries, with the optional required marker and (i) note |
| `FieldError` | `field-error.tsx` | the message under a control, carrying the id the control points at |
| `describedBy` | `field-error.tsx` | joins the ids a control actually has into one `aria-describedby` |
| `FormSection` | `form-section.tsx` | one collapsible group of fields, open state owned by the caller |
| `Segmented` | `segmented.tsx` | a two-to-four way choice rendered inline as a `radiogroup` |
| `LockButton` | `lock-button.tsx` | the padlock toggle beside a parameter or header |
| `ChipGroup` | `chip-group.tsx` | a multi-select over a short, fully visible list |
| `SwitchRow` | `switch-row.tsx` | a boolean as a full-width row: title, hint, switch |
| `SettingsPanel` | `settings-panel.tsx` | a titled group of settings controls that can be switched off as a block |

## Which one to reach for

- A label, a control and its error on a screen is still `Field`. It generates
  the control id, binds the label and wires `aria-describedby` itself, and none
  of that has to be repeated by hand.
- A sheet row that lays its own controls out uses `FieldLabel` + the control +
  `FieldError`, and passes `describedBy(hintId, !!error && errorId)` to the
  control so the hint and the reason are both announced.
- A choice with more than four options, or one long enough to want filtering,
  is a `Combobox` and not a `Segmented` — see
  [Dashboard dropdowns and the Combobox](combobox.md).
- A list long enough to need filtering is a `Combobox` too, not a `ChipGroup`.
  The chips exist because a handful of teams is faster to pick from when all of
  them are on screen.
- A titled group of settings on a deployment-settings screen is a
  `SettingsPanel`, not `Card`. The two are easy to confuse and used to be worse:
  `ModelSettings` and `Performance` each declared a local component called
  `Card` that had nothing to do with `ui/card.tsx`'s (#1682). `Card` is a bare
  bordered surface with `CardHeader` / `CardTitle` / `CardDescription` /
  `CardContent` parts you compose yourself; `SettingsPanel` is the settings
  shape — title, one explanatory line, and a control row that dims as a unit.

## What they already guarantee

Each primitive owns an accessibility detail that is invisible on screen and
easy to lose when the shape is retyped in the next sheet:

- `FieldLabel` binds its `<label>` with `htmlFor`, or names a whole group
  through `id` + the group's `aria-labelledby`. A label bound to nothing looks
  correct and leaves the control unnamed.
- `FieldError` renders nothing at all when there is no error, and carries an
  `id` so the control can describe itself with it (#1527).
- `FormSection`'s header is a real `<button>` and its (i) sits *beside* it, not
  inside: a button inside a button is invalid HTML and an axe
  `nested-interactive` failure (#1201). A collapsed section renders no body, so
  a closed section holds nothing tabbable.
- `Segmented` is a `radiogroup` of real buttons — tabbable, `Enter`-activated,
  exactly one `aria-checked`.
- `LockButton` reports `aria-pressed` and names both the state and what a press
  will do, because the padlock icon says neither.
- `ChipGroup` is one named `group` of `aria-pressed` toggles, and says *none
  available* rather than rendering an empty row.
- `SwitchRow` names its switch after the row title.
- `SettingsPanel` groups its controls in a `<fieldset disabled>` rather than a
  faded `<div>`. Fading a live div drags its labels and hints below 4.5:1 while
  telling assistive tech nothing (#1181), and a reader who tabs into a group
  that looks off should find it genuinely off.

Every one of those is asserted in the primitive's own story, so a rewrite that
drops one fails the story rather than shipping.

## Copy

The primitives carry the small amount of copy they own under `common.*` in the
catalogs — `common.aboutField`, `common.lock.*`, `common.noneAvailable`. A
screen's wording stays in the screen's namespace and arrives as a prop.

## The guard

`bun run check:primitives` (`ui/scripts/check-ui-primitives.ts`) is what keeps
this page from being advice. It runs in the `ui lint / build` job and fails on
four things: a bare `<select>`, a raw `<pre>`, a `window.confirm`/`alert`/
`prompt`, and a component re-declared under a name `src/components/ui/` already
exports. The last one is this page's rule — #1044 sat undiscovered for months
because nothing looked, and seven primitives stayed trapped in one sheet's file.

The shared names are read out of `src/components/ui/*.tsx` rather than listed in
the script, so a primitive added tomorrow is covered the day it lands.

Three things are exempt, by rule rather than by filename:

- anything under `src/components/ui/` — a primitive necessarily contains the
  element it wraps, and `CodeBlock` *is* the `<pre>`;
- `*.stories.tsx` and `*.test.ts(x)`, which are fixtures rather than shipped UI,
  the same carve-out `check-literals.ts` makes;
- a file that **imports** the shared component and wraps it. Adapting
  `Dialog as BaseDialog` under a local `Dialog` is reaching for the primitive,
  not duplicating it, and failing that would push screens away from the shared
  component instead of towards it.

A case the rule is genuinely not about carries an inline waiver on the comment
block directly above it:

```tsx
/* ui-primitives-allow: prose with its newlines kept, not a payload — it wraps
 * rather than scrolling sideways, so CodeBlock's copy button and highlighting
 * would both be answering a question nobody asked */
<pre className="whitespace-pre-wrap">{reply}</pre>
```

The reason is mandatory — a marker with nothing after the colon fails the run,
because an unexplained waiver is indistinguishable from the bug. Waivers live at
the point of use rather than in a central allow-list file so they cannot outlive
the code they excuse, and every one is printed on every run so the set stays
visible instead of growing quietly.

## Still to do

`ModelSheet` is the only consumer today. `ProviderSheet`, `ProviderGroupSheet`
and `EditorSheet` solve the same layout problems separately and should move
onto these primitives; that migration is #1658, tracked separately from the
extraction so each sheet can be diffed on its own.
