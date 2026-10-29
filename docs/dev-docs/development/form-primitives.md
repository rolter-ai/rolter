# Dashboard form primitives

Group mappings on SSO and SCIM use `DeleteIconButton`: the pending spinner follows `remove.variables`, while every remove control stays disabled until the mutation finishes.

The prompt and skill workbench headers use `DeleteIconButton` with their existing delete capabilities. Prompt variable and decorator removal use its ungated form because they only edit the unsaved draft.

Custom label removal in `LabelSheet` uses `DeleteIconButton`; only the label being deleted shows its pending spinner, and the existing confirmation remains in place.

A sheet in the dashboard is a long, dense form: forty-odd controls in six
collapsible sections, most of them a label, a control and a conditional error.
`Field` (`ui/src/components/ui/field.tsx`) assembles one of those rows on a
screen. A sheet needs the same pieces laid out by hand — two controls on a row,
a padlock beside an input, a chip list under a group label — so the pieces
themselves are components.

They live in `ui/src/components/ui/` beside the other primitives, and their
stories are grouped under **Forms/** in Storybook.

| Primitive       | File                 | What it is                                                                               |
| --------------- | -------------------- | ---------------------------------------------------------------------------------------- |
| `FieldLabel`    | `field-label.tsx`    | the compact label a sheet row carries, with the optional required marker and (i) note    |
| `FieldError`    | `field-error.tsx`    | the message under a control, carrying the id the control points at                       |
| `describedBy`   | `field-error.tsx`    | joins the ids a control actually has into one `aria-describedby`                         |
| `FormSection`   | `form-section.tsx`   | one collapsible group of fields, open state owned by the caller                          |
| `Segmented`     | `segmented.tsx`      | a two-to-four way choice rendered inline as a `radiogroup`                               |
| `LockButton`    | `lock-button.tsx`    | the padlock toggle beside a parameter or header                                          |
| `ChipGroup`     | `chip-group.tsx`     | a multi-select over a short, fully visible list                                          |
| `SwitchRow`     | `switch-row.tsx`     | a boolean as a full-width row: title, hint, switch                                       |
| `SettingsPanel` | `settings-panel.tsx` | a titled group of settings controls that can be switched off as a block                  |
| `CopyableValue` | `copyable-value.tsx` | a labelled value to copy again: mono, `select-all`, wrapping, with hint and note         |
| `Checkbox`      | `checkbox.tsx`       | a native checkbox drawn by the tokens: neutral when ticked, with an indeterminate state  |
| `AuthCard`      | `auth-card.tsx`      | the signed-out card: `<main>`, wordmark, вышивка rule, `h1`, description, then the form  |
| `SubmitForm`    | `submit-form.tsx`    | the `<form>` an editor sheet or dialog sits in: Enter submits, a refused submit says why |

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

- A value someone copies out of the dashboard and can copy again (an address,
  an id) is `CopyableValue`, or `CopyableText` where the label is already
  there (`variant="inline"` inside a description list). A value shown once is
  `SecretValue`, which composes the same box (#2418). An address built on the
  control plane's public base is `PublicUrlValue`
  (`ui/src/components/PublicUrlValue.tsx`), which says the pending, failed and
  unset-`ROLTER_PUBLIC_URL` states once for every screen (#2366).

- A tick box is `Checkbox`, never `<input type="checkbox">` with an `accent-`
  tint (#2223). It is still a real input, so the row keeps its own wrapping
  `<label>` (or the box takes an `aria-label`) and the label click, Space and the
  form value all work as before. It fills with `--foreground` when ticked, not
  red: a selection is not a brand moment, and a red box on every ticked row
  spends the red budget in `DESIGN.md`. `indeterminate` draws the mixed state
  for a parent over partly selected children. A boolean that takes effect at
  once is a `SwitchRow`, not a checkbox.
- A screen shown before sign-in (sign-in, accepting an invitation) is an
  `AuthCard`: it owns the `<main>` landmark, the `h1` (`title`), the line under
  it (`description`, `descriptionRole="status"` for an outcome) and the layout,
  and the screen passes its form as `children`. Do not copy the card's markup
  into a new signed-out screen; AcceptInvite lost its landmark and heading that
  way (#2069, #2228).

## The (i) note beside a field

A field whose effect is not obvious from its label and control explains itself
in an (i) beside the label (#487). There is one primitive for it, `InfoHint`
(`ui/src/components/ui/info-hint.tsx`), and a form row reaches it through the
`info` prop of `Field`, `FieldLabel`, `SwitchRow` and `FormSection`, never by
laying a second copy out by hand.

- **`info` and `hint` are different jobs.** `hint` is the line always visible
  under the control, for a state of this field right now (a clash, an
  unreadable value, what the chosen option does). `info` is the note a reader
  opens to learn what the field is and which values fit. A field that already
  says all of it in a `hint` needs no `info`; do not write the same sentence in
  both.
- **The copy is short and lives in the catalogs.** One or two sentences: what
  the field does, then the sensible value or the default. Put it under
  `<screen>.<field>Info` (`pages.guardrailProviders.timeoutInfo`) in **every**
  catalog, like any other string; `check:i18n` and `check:literals` fail on a
  literal. State a default only where the code or the backend says it.
- **The note is an `AnchoredPanel`.** It is fixed and placed from the (i)'s
  rectangle, so the scroll area of a dialog or sheet cannot clip it (the old,
  absolutely placed note was cut off at the edge of a field with a short label
  or one in the last column), and it turns over near the foot of the window. It
  opens on hover and on focus, a press toggles it, Escape closes it without
  closing the dialog around it, and the pointer can travel onto it.
- **A screen's story asserts its notes** with `expectFieldHint` from
  `ui/src/lib/story-hints.ts`: it points at the button named
  `About <label>`, reads the note, checks both edges of the note are hit-testable
  (whole, not clipped) and moves away. Cover the second locale where the copy is
  long.

## What they already guarantee

Each primitive owns an accessibility detail that is invisible on screen and
easy to lose when the shape is retyped in the next sheet:

- `FieldLabel` binds its `<label>` with `htmlFor`, or names a whole group
  through `id` + the group's `aria-labelledby`. A label bound to nothing looks
  correct and leaves the control unnamed.
- `FieldError` renders nothing at all when there is no error, and carries an
  `id` so the control can describe itself with it (#1527).
- `FormSection`'s header is a real `<button>` and its (i) sits _beside_ it, not
  inside: a button inside a button is invalid HTML and an axe
  `nested-interactive` failure (#1201). A collapsed section renders no body, so
  a closed section holds nothing tabbable.
- `Segmented` is a `radiogroup` of real buttons — tabbable, `Enter`-activated,
  exactly one `aria-checked`.
- `LockButton` reports `aria-pressed` and names both the state and what a press
  will do, because the padlock icon says neither.
- `ChipGroup` is one named `group` of `aria-pressed` toggles, and says _none
  available_ rather than rendering an empty row.
- `SwitchRow` names its switch after the row title. Handed a `gate`, it is
  refused the way `GatedSwitch` is and must name itself with `control` (#1820).
- `SettingsPanel` titles itself with a real heading (`<h2>`, `headingLevel` for
  a deeper panel), caps its description at `65ch`, and takes the switch that
  governs it in `action`, outside the fieldset so a switched-off panel can be
  switched back on. It groups its controls in a `<fieldset disabled>` rather
  than a faded `<div>`, and never sets `opacity` (#2213). Fading a live div drags its labels and hints below 4.5:1 while
  telling assistive tech nothing (#1181), and a reader who tabs into a group
  that looks off should find it genuinely off.

Every one of those is asserted in the primitive's own story, so a rewrite that
drops one fails the story rather than shipping.

## Editors are forms

`EditorSheet` and a `Dialog` given an `onSubmit` render a real `<form>` (`SubmitForm`, `ui/src/components/ui/submit-form.tsx`, #1205). Enter in a field submits it, a footer button of `type="submit"` means it, and `Button` defaults to `type="button"` so a Cancel or a row's delete placed inside never does.

- `EditorSheet` takes `onSubmit` (it was `onSave`) and `canSave`. A `Dialog` takes `onSubmit`, `canSubmit` and `busy`, and its footer's primary button is `type="submit"` by hand.
- The primary button is never natively `disabled` for a validation error. While `canSave` is false it is `aria-disabled` and still pressable, because pressing it, or Enter in a field, is how the form is asked why. Only `saving` really disables it.
- A required field is `<Field required>`: the control gets the native `required` and `aria-required`, the label a marker. After a refused submit a still-empty required control says "Required" under itself (`role="alert"`, wired through `aria-describedby`), focus moves to the first field at fault, and `SubmitFormStatus` in the footer counts them (`N fields need attention`, a polite `role="status"`). Typing into the field clears its message. A value of spaces counts as empty, since every save trims it.
- A `Combobox` takes `required` and `aria-required` on its text input, so `<Field required>` works on a picker the same way (an empty pick refuses the submit and names the field). Where either of two inputs will do (a rate limit needs a requests or a tokens cap), neither is `required`: pass the sheet's `blockedMessage` for the refused submit instead. A dialog that already owns a `<form>` (the two-factor enrol step) cannot take `Dialog onSubmit`, since forms do not nest; say so in a comment there.
- A screen whose own validation already names the field (`useFieldErrors`: Pricing, Plugins) uses `<Field markRequired>`: the label's marker and `aria-required`, so a screen reader announces it before the first refused save, but no native `required`, which would make the empty control `:invalid` from the first paint and add the generic "Required" beside the screen's own message (#3451). Stories assert the marker and `aria-required`, and that the pristine form is still not flagged.
- Only `required` is read from the attributes. A range, a charset or a clash with another field stays with the screen's own check (`useFieldErrors`, `FieldError`), which says more than a browser message can, in the dashboard's language. The browser's own bubble is off (`noValidate`).
- A refusal that comes from the caller's rule rather than a field (a pick that is not an input) passes `blockedMessage` to `EditorSheet`, which the status line shows when no field is at fault.
- A native `<button>` written by hand inside an editor carries `type="button"`, or it submits the form. `filter-panel.tsx` was the one that did not.

Stories submit with Enter and assert the refused submit: `EnterSubmits` and `BlockedSubmitNamesTheField` under **Overlays/EditorSheet**, `EditorSubmitsWithEnter` under **Overlays/Dialog**, `DoesNotSubmitAFormUnlessAskedTo` under **Primitives/Button**. A story asserting that a sheet's Save is held reads `aria-disabled="true"`, not `toBeDisabled()`.

## Footers and bodies on small screens

The buttons that finish a form live in the overlay's own file, beside the
panel, not in this table. Two of them decide whether a form can be finished on
a phone or in a short window at all (#2003).

`SheetActions` (`ui/src/components/ui/sheet.tsx`) is the button row inside a
`SheetFooter`. Its children are Cancel and then the primary action. Its
`start` slot takes anything outside that pair: `ProviderSheet`'s connection
test, which pins itself left with `mr-auto`. Below `sm` the sheet is the whole screen and cannot
be scrolled sideways, so the row becomes a column. `start` goes on top, and the
pair gets the bottom line to itself with the primary action last, taking the
width Cancel leaves. From `sm` up it is one row that wraps rather than overflows.
Every sheet footer uses it. A sheet that writes its own `flex justify-end` row
brings back the single line that pushed `ModelSheet`'s Save into the gutter
and `ProviderSheet`'s connection test 151 px off the left edge in Russian.

`ModelSheet` no longer puts a reason beside the primary action, and no longer
disables it for one (#2810): it stays pressable, and a refused press puts one
line above the buttons that counts what is left to fix. A reason squeezed beside
the pair is what pushed Save into the gutter (#2003), so the line sits on a row
of its own above them.

## When a form shows its errors

A required field is not wrong before anybody has had the chance to fill it, so a
sheet computes every error on every render but shows an error only when its
field was touched or a save was refused (#2810). `useErrorVisibility`
(`ui/src/lib/error-visibility.ts`) holds that decision:

```tsx
const visibility = useErrorVisibility<"name" | "baseUrl">();
const shown = (field) => (visibility.shows(field) ? found[field] : "");
// <Input onBlur={() => visibility.touch("name")} … />
// <Button onClick={() => (problems.length ? visibility.attempt() : save())}>
```

- `touch(field)` is called when focus leaves the field (a list row's field,
  when the row has one message for all of them). A field the form filled in
  itself, such as the name a duplicated route arrives with, is touched when it
  is filled.
- `attempt()` is the refused save. From then on `shows()` is true for every
  field, and `attempts` counts them, so an effect can move focus to the first
  `[aria-invalid="true"]` after each one.
- `reset()` belongs where the form opens.
- The primary action is never disabled for a validation error: pressing it is
  what asks for them. It is disabled while the form is loading or saving.
- Each problem is said once, under its own field, with `FieldError` and the
  control's `aria-describedby`. The footer carries at most one more line, a
  count under `role="status"` that does not repeat the messages (the fields announce themselves). Sections that
  hold a problem are opened by the refused save, so the field it names is on
  screen.

`Pricing` and `Plugins` use only `attempt()`: their errors wait for a refused
save, as they always did. `Security` uses `touch()` on a list's blur, and shows a
problem in a list nobody has edited from the start, since it came from the store.

`DialogBody` (`ui/src/components/ui/dialog.tsx`) holds a dialog form's fields,
between `DialogHeader` and `DialogFooter`. A panel with a body caps itself at
the window's height and only the body scrolls, so the title, the close button
and the primary action stay on screen at 640×360. A dialog with no body keeps
its natural height, and the overlay scrolls the whole panel, top edge first.
The panel is never capped without something inside it to shrink, since that
would push the footer out past the panel's own border. Use `DialogBody` for
any dialog with more than a couple of fields. A hand-written
`max-h-[65vh] overflow-y-auto` body is the shape it replaced in
`GuardrailRules`, `GuardrailProviders`, `Plugins` and `McpManagement`.

`DialogFooter` wraps as well. The primary action is its last child, so a wrapped
footer leaves it at the bottom right, where it was.

`SheetHeader` (same file) is the title and the line under it. That line says what the sheet is for, often a whole
sentence ("the plaintext key is shown once, right after creation — copy it
then"), so it wraps and nothing clips it (#2812). It was one line with an
ellipsis, and at a 1024px window Create virtual key and Invite user lost the end
of the sentence: the part of it that mattered. A value with no place to break
(an id, a URL) breaks inside the line (`overflow-wrap`) instead of widening the
panel. A sheet passes its subtitle and never styles it, and a new sheet that
needs a shorter header shortens the sentence, not the line. Stories:
`LongDescriptionWraps` and `UnbrokenDescriptionStaysInThePanel` under
**Overlays/Sheet**, and `CreateSubtitleIsWholeAt1024` and
`InviteSubtitleIsWholeAt1024` (each also in Russian) on Virtual Keys and Users.

## Inline errors on a settings screen

A settings screen saves its fields as one request, and shows what is wrong under the field it is
wrong in. `useFieldErrors` (`ui/src/lib/use-field-errors.ts`) owns the part every such screen used
to copy (#2734): the field order, the wire-name table, focus on the first invalid field, the count
for the footer, and a 400 from the control plane pinned to the field it names.

```tsx
const fields = useFieldErrors<FieldKey>({
  order: FIELD_ORDER, // the order the fields sit in
  wire: WIRE_FIELDS, // `retention_days` -> "retentionDays"
  idOf: (key) => ids[key], // the DOM id focus moves to
  local: form ? translateErrors(validate(form), t) : {}, // the screen's own checks, translated
});
// onError: if (fields.refuse(error)) return; toast.push(...)
// an input: {...fields.inputProps("retentionDays")}  +  <FieldError id={fields.errorId(...)} error={fields.errorOf(...)} />
// an edit:  fields.edit("retentionDays")      after a save: fields.clear()
// footer:   <SaveBar invalid={fields.invalid.length} pending={save.isPending}
//             onSave={() => fields.submit(() => save.mutate(form))} />
```

- Call the hook before the screen's early returns, so `local` is `{}` until the form has loaded.
- `local` holds translated messages; `validate()` returns catalog keys and `translateErrors` renders
  them. A local error wins over a server one for the same field.
- `refuse(error)` maps a rejected save through `serverFieldError` (`ui/src/lib/field-errors.ts`): the
  exact `error.field` path first (`forwarded_headers[2]`), then its root, then the wire name the
  message opens with. It returns false when the refusal names none of the screen's fields, and that
  refusal belongs in a toast. `place` handles what the table cannot, such as one row of a list
  (`injected_headers.x-team` on Client Settings). `names(error)` asks the same without placing it.
- `skip(key)` drops a field that cannot be edited right now (a control disabled by its own switch), so
  no complaint about it holds Save. `shows(key)` hides a local error until the first refused save
  (`useErrorVisibility`), while still stopping the save.
- `submit(run)` is Save's press. With a field at fault it focuses the first one that can take focus
  and does not run; otherwise it runs the save. Save therefore stays pressable on an invalid form and
  is `aria-disabled`, not `disabled`.
- `SaveBar` (`ui/src/components/ui/save-bar.tsx`) is the footer: the "N fields need attention" status,
  the `aria-disabled` Save, a `leading` slot and secondary buttons as children. A pristine form
  (`disabled`) is `aria-disabled` too, with the press ignored: a real `disabled` button has
  `pointer-events: none`, so the press that blurs a `commitOnBlur` field would never reach it. That
  press commits the field, the bar re-renders dirty, and the click then submits (#3449). Only a save
  in flight (`pending`) is really `disabled`. Assert a pristine Save with
  `toHaveAttribute("aria-disabled", "true")`, never `toBeDisabled()`.
- Both error renderers announce the same way: `Field`'s built-in `error` and `FieldError` are
  `role="alert"`, and the control's `aria-describedby` points at it. The footer count is a
  `role="status"`, which is polite. A story that needs one specific message picks it by its text, not
  by a bare `getByRole("alert")`. Use `Field` for a screen row and `FieldError` where the input is not wrapped.

Used by Performance Tuning, Model Settings, Compatibility, Client Settings, Logs Settings, Adaptive
Routing settings, the Plugins dialog and Pricing (which takes only the server mapping).

### Showing a failed call

A toast or an inline banner never prints `error.message`. `errorText(error, t)`
(`ui/src/lib/error-copy.ts`) is `describeError` as one line: the translated sentence for the
control plane's stable `code`, then its status, and the server's own words only where nothing
translated explains them. An `invalid_field` refusal names its field from `error.field`
(`errors.api.invalidFieldNamed`). A field-bound message under an input stays the server's text, since
it states the rule that was broken. `bun run check:raw-errors` fails on `(x as Error).message`,
`x.error.message` and `errorDetail(` in `pages/` and `components/`; a display that is not a server
answer carries `// raw-error-allow: <reason>` on the line above it.

## Copy

The primitives carry the small amount of copy they own under `common.*` in the
catalogs — `common.aboutField`, `common.lock.*`, `common.noneAvailable`. A
screen's wording stays in the screen's namespace and arrives as a prop.

## The guard

`bun run check:primitives` (`ui/scripts/check-ui-primitives.ts`) is what keeps
this page from being advice. It runs in the `ui, storybook, docs` job and fails on
eleven things: a native `<button>` with no `type` and no props spread (a
button defaults to submit, so one inside an editor sheet submits the form, #3241), a bare `<select>`, a hand-written `role="listbox"` / `"option"` /
`"combobox"` (#3058), a native `<input type="checkbox">` outside
`checkbox.tsx` (#2223), a raw `<pre>`, a `window.confirm`/`alert`/
`prompt`, a component re-declared under a name `src/components/ui/` already
exports, the same element markup hand-written in three or more files, and a
`DialogFooter` holding a `"destructive"` button, which is a confirmation
assembled by hand rather than taken from `ConfirmDialog` (see
[destructive actions](destructive-actions.md)), and a bare `animate-spin` /
`animate-pulse`, which must be `motion-safe:` so `prefers-reduced-motion` stops
it (#2006), and a bare `<Switch>` in a hand-built row (below). The fourth is this page's rule
— #1044 sat undiscovered for months because nothing looked, and seven
primitives stayed trapped in one sheet's file.

The shared names are read out of `src/components/ui/*.tsx` rather than listed in
the script, so a primitive added tomorrow is covered the day it lands.

### Hand-rolled switch rows

`hand-rolled-switch-row` (#2242) catches `SwitchRow` typed again under another
name: a title, a hint and a bare `<Switch>` in a `justify-between` container.
`ToggleRow` in `GuardrailRules` and the activate and enabled rows in
`GuardrailProviders`, `Plugins` and `McpManagement` were all that, and neither
the name rule nor the shape rule could see them. It flags a `<Switch>` with a
`justify-between` container in the ten lines above it and a `font-medium` or
`text-sm` title between them.

It deliberately leaves alone `GatedSwitch` (a different tag), a switch in a
`SettingsPanel`'s `action` slot (the panel owns the title), and an inline
`<label><Switch>` with no `justify-between`. When a row genuinely is not a
`SwitchRow`, waive it with `ui-primitives-allow: <reason>` in the comment
directly above the `<Switch>` line, as with the other element rules.

### Inline two-step confirmations

`inline-two-step-confirmation` (#2345) catches a delete confirmed without a
dialog at all: a destructive `Button` whose handler sets a `useState(false)`
flag, where that flag then chooses a later destructive `Button` or relabels the
first. The Users sheet had exactly this (#1893), and `hand-rolled-confirmation`
could not see it because no `DialogFooter` was involved. Open a `ConfirmDialog`
instead; a destructive button that only sets a flag read by a `ConfirmDialog`
passes. Waive with `ui-primitives-allow: <reason>` above the first button. See
[destructive actions](destructive-actions.md).

### Repeated shapes

The name rule only catches a duplicate that happens to pick the shared
component's name. It cannot see markup re-implemented inline under no name at
all, which is what #1658 found: five sheets rendered the footer failure line as
a byte-identical `<p className="px-[22px] pt-2.5 text-xs
text-[color:var(--status-danger-text)]">`, one of them carried `role="alert"`
and four did not, and the check reported the tree clean. So there is a fifth
rule that matches a _shape_ rather than a name (#1686).

It normalises every JSX element to its tag plus its `className` with the classes
sorted, and fails when the same pair is written in more than two files. Two
knobs keep it from flagging ordinary Tailwind:

- **intrinsic elements only.** `<div>`, `<section>`, `<button>` — never `<Card>`
  or `<GatedButton>`. A shared component rendered with the same className in
  five screens is the primitive doing its job, and flagging it would punish the
  composition this page is asking for.
- **at least four classes, at least two of them arbitrary values.**
  `p-[22px]`, `text-[color:var(--text-subtle)]`, `max-w-[840px]` are the
  design system spelled out by hand; `flex items-center gap-2` is a sentence in
  Tailwind and repeats in eighteen files for good reasons. Both numbers were
  tuned against the tree: at four and two, the #1658 footer line is caught and
  not one generic flex row is.

A computed `className={cn(…)}` is not compared at all. Its value depends on
props, so two spellings that look alike may render nothing alike, and guessing
there would be wrong in the direction that costs trust.

The fix is to extract the primitive. When that has to wait, the shape goes in
`ui/scripts/repeated-shapes-allowlist.ts` with the reason a reviewer accepted
one more copy — the same shape as `literals-allowlist.ts`, and for the same
reason: the check deletes an entry the moment the duplication drops back to two
files, so the list can only shrink, and a stale entry fails the run rather than
quietly covering for a copy that came back. The eight entries it carries today
came from the rule's first pass and are tracked in #1711.

Three things are exempt, by rule rather than by filename:

- anything under `src/components/ui/` — a primitive necessarily contains the
  element it wraps, and `CodeBlock` _is_ the `<pre>`;
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

## Draft state for a settings form

A settings screen saves its fields as one request, so it cannot tell a pristine form from an edited
one by looking at the Save button. `useDraft` (`ui/src/lib/use-draft.ts`) holds the two copies such a
screen needs, what the server held at the last load or save and what is being edited, and answers
the questions Save, the field markers and Discard ask:

```tsx
const { draft, saved, changed, dirty, set, reset, commit } = useDraft(source, EQUALS);
```

- `source` is the query's data mapped into the form's shape. The first load seeds both copies, and a
  refetch behind an edit never takes the edit away.
- `changed` names the fields whose value differs from `saved`. `EQUALS` is a module constant that
  gives a field its own comparison, so a blank line in a list, or the space around a colon, is not
  an edit. A field left out is compared with `Object.is`.
- `commit(next)` adopts what the server answered as both copies after a save, and `reset()` is
  Discard.

The reducer and `changedKeys` are plain functions with unit tests beside them, since the tree has no
React test renderer. The Security screen was the first consumer. The shared field errors are `useFieldErrors` above.

### One form state for the settings screens

`useSettingsForm` (`ui/src/lib/use-settings-form.ts`, #2214) is `useDraft`, `useFieldErrors` and the
leave guard (`useLeaveGuard`) as one hook, and `SettingsSaveBar` (`ui/src/components/SettingsSaveBar.tsx`)
is the footer that reads it. Adaptive Routing, Model Settings, Performance, Client Settings, Logs
Settings, Compatibility and Feature Flags use them; a new deployment-settings screen does too.

```tsx
const source = React.useMemo(() => (query.data ? fromDto(query.data) : undefined), [query.data]);
const sf = useSettingsForm<FormState, FieldKey>({
  source,
  equals: EQUALS, // optional, as for useDraft
  fieldErrors: (draft) => ({ order, wire, idOf, local: draft ? translateErrors(validate(draft), t) : {} }),
});
const { draft: form, fields, changed, set } = sf;
// a field:   value={form.x}  onChange={(e) => set({ x: e.target.value })}
// a save:    onSuccess: (dto) => sf.commit(fromDto(dto))
// the page:  <SettingsBody onSubmit={() => fields.submit(() => save.mutate(form))}> … </SettingsBody>
// the foot:  <SettingsSaveBar form={sf} pending={save.isPending} updatedAt={query.data?.updated_at} />
```

- `set(patch)` also answers the control plane's complaint about the fields it touches. `commit` adopts
  the answer as both the form and the baseline, clears the complaints and stamps `savedAt`; `discard`
  restores the baseline.
- **Dirty, reset, leave.** Save is `aria-disabled` (and its press ignored) until a field differs from what the server holds (a value
  typed back to the stored one is no edit), Discard is the way back, and leaving with an edit asks first
  (`form.leavePrompt`, rendered by the footer).
- **Errors.** A wrong field is a `FieldError` with `aria-invalid` and `aria-describedby` through
  `fields.inputProps`, as above; the footer counts them, and Save stays pressable and moves focus to the first.
- **The line beside Save** says which state the screen is in: `N changes not saved yet`, or `Saved at
14:03.` followed by `Applied to 1 of 2 live gateways`, or `Last saved <date>` from the DTO's `updated_at`
  on a fresh load. The applied line reads the config versions the Cluster screen reads
  (`useSettingsPickup`, `gatewayPickup`), polls while a gateway is behind, and is left out with
  `gateways={false}` for a setting no running gateway reads (Compatibility, while a field there needs a restart).
- **Enter saves.** `SettingsBody` given `onSubmit` is one `<form>` (#1205); the footer's Save is its submit
  button. Feature Flags opens its confirmation from the same handler.
- **A switch is not immediate.** The panel's or row's switch moves at once and the gateways still run
  the old value until Save, so `SettingsPanel unsaved` and `SwitchRow unsaved` draw a `Not saved` chip
  (`UnsavedBadge`) beside the title while the field differs from the baseline: `unsaved={changed.includes("enabled")}`.

The Security screen uses the same pieces (#3243): its lists feed `useFieldErrors` through `local` (one message per list, with the full per-entry problems still drawn under the field), and its Save asks the loosening confirmation from `security-loosening.ts` before the request, like Feature Flags asks its own.

## The versioned-repository kit

The Prompt Repository and the Skills Repository are both a named thing with immutable versions, one of them live, and are built from one kit, `ui/src/components/versioned-repository.tsx` (#2221):

- `RepositoryShell`, `RepositoryIndex`, `VersionRail`, `VersionBadge`, `VersionMeta`, `SectionHeading` and `RepositoryLoadingState` draw the page. The rail is a folded one-line toggle above the editor under `lg` and a static panel from `lg` (#3118).
- `liveAction` is the one rule for the verb a version offers: newer than the live one is a publish, older is a roll back, and with nothing live every version is a publish. `MakeLiveDialog` is the confirmation shell and `useLatched` keeps its request through the render that closes it.
- `ChangesPanel`, `CompareButton` and `VersionCompareDialog` carry the comparison with the live version (#2112), built on `TextDiff` and `FieldDiff` in `ui/src/components/ui/text-diff.tsx` and the pure line and field diff in `ui/src/lib/text-diff.ts`. `ui/src/lib/version-diff.ts` turns a prompt or skill version pair into the rows. The diff is written in the repository rather than taken from a package, so nothing is added to the bundle.
- `DeleteBySlugDialog` is the destructive confirmation that needs the slug typed back.

What differs stays with the screen: the editor, the copy that names the thing, and what a make-live dialog says about impact. A screen passes its own already-translated title and description. Selecting a row, a version, or the row a create just made goes through `useDraftSwitch`, so unsaved edits are never replaced without the discard prompt.

## The reporting window picker

A screen that reads over a reporting window draws it with `TimeWindowPicker`
(`ui/src/components/TimeWindowPicker.tsx`), not a `Combobox` of
`useTimeWindowOptions()` (#3407). It offers the named windows and **Custom
range…**, a start and an end day. The selection is a `WindowSelection` from
`ui/src/lib/time-window.ts`: a named window, or the string
`custom:YYYY-MM-DD:YYYY-MM-DD`, so it still works as a query key and in `===`.
The address and a saved view hold the three fields (`window=custom`, `since`,
`until`); read them with `readWindowSelection` / `windowFromFilters` and write
them with `windowAddress` / `windowFilters`, never by hand, so a named window
drops the days a range left. `useAddressSelection` and `useTimeWindow` do this
for a screen that keeps the window in the address; `windowBounds` and
`previousWindowBounds` take the selection. The range is whole calendar days in
the chart zone (`chartTimeZone()`, the browser's when the account sets none,
#3500) with the last day included, so `until` is the start of the day after
it; a range that has not ended yet sends no `until` (#3506). The month windows
and `previousWindowBounds` count in the same zone: each function takes the
zone as its last argument, the chart zone by default, and does its wall-clock
arithmetic through `Intl` (`wallClock` / `zonedInstant`) rather than the
browser-local `Date` getters. Because those bounds move with the zone, a
read over the window is keyed through `useWindowQueryKey(selection)` (#3519),
spread into the query key where the window's name went: it adds the chart zone
for a window of calendar days (`null` for a rolling one) and re-renders when
the zone changes, so a change of **Time zone** in Preferences is a new read
over the new zone's midnights rather than a cached answer over the old one's.
Savings, the Dashboard, LLM Logs, MCP Logs and Cost Attribution use it; a new
screen that reads over the picker's selection does too. Where the chart zone is not the browser's, the
picker names it under the list, as the list's description. Placed in a
`Field`, the picker forwards the `id`, `aria-describedby`, `aria-invalid` and
`aria-required` the field clones onto it to the list (#3507), so the field's
hint or error is what a screen reader hears beside the zone note. The saved-view allow-list in
`crates/rolter-control/src/me_saved_views.rs` is the server's copy of the
shape. `useTimeWindowOptions` stays named-only for a control that offers no
range (the route sheet's observed-traffic window).

## Still to do

`ModelSheet` is the only consumer today. `ProviderSheet`, `ProviderGroupSheet`
and `EditorSheet` solve the same layout problems separately and should move
onto these primitives; that migration is #1658, tracked separately from the
extraction so each sheet can be diffed on its own.
