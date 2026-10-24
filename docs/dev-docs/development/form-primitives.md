# Dashboard form primitives

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

| Primitive       | File                 | What it is                                                                              |
| --------------- | -------------------- | --------------------------------------------------------------------------------------- |
| `FieldLabel`    | `field-label.tsx`    | the compact label a sheet row carries, with the optional required marker and (i) note   |
| `FieldError`    | `field-error.tsx`    | the message under a control, carrying the id the control points at                      |
| `describedBy`   | `field-error.tsx`    | joins the ids a control actually has into one `aria-describedby`                        |
| `FormSection`   | `form-section.tsx`   | one collapsible group of fields, open state owned by the caller                         |
| `Segmented`     | `segmented.tsx`      | a two-to-four way choice rendered inline as a `radiogroup`                              |
| `LockButton`    | `lock-button.tsx`    | the padlock toggle beside a parameter or header                                         |
| `ChipGroup`     | `chip-group.tsx`     | a multi-select over a short, fully visible list                                         |
| `SwitchRow`     | `switch-row.tsx`     | a boolean as a full-width row: title, hint, switch                                      |
| `SettingsPanel` | `settings-panel.tsx` | a titled group of settings controls that can be switched off as a block                 |
| `CopyableValue` | `copyable-value.tsx` | a labelled value to copy again: mono, `select-all`, wrapping, with hint and note        |
| `Checkbox`      | `checkbox.tsx`       | a native checkbox drawn by the tokens: neutral when ticked, with an indeterminate state |
| `AuthCard`      | `auth-card.tsx`      | the signed-out card: `<main>`, wordmark, вышивка rule, `h1`, description, then the form |

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
  the `aria-disabled` Save, a `leading` slot and secondary buttons as children.
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
ten things: a bare `<select>`, a hand-written `role="listbox"` / `"option"` /
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
  `rounded-[10px]`, `text-[color:var(--text-subtle)]`, `max-w-[840px]` are the
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
React test renderer. The Security screen is the first consumer. The shared field errors are `useFieldErrors` above. The saved-at line that #2214 asks every settings
screen to carry is not here yet; it belongs beside this hook rather than in each screen.

## The versioned-repository kit

The Prompt Repository and the Skills Repository are both a named thing with immutable versions, one of them live, and are built from one kit, `ui/src/components/versioned-repository.tsx` (#2221):

- `RepositoryShell`, `RepositoryIndex`, `VersionRail`, `VersionBadge`, `VersionMeta`, `SectionHeading` and `RepositoryLoadingState` draw the page. The rail is a folded one-line toggle above the editor under `lg` and a static panel from `lg` (#3118).
- `liveAction` is the one rule for the verb a version offers: newer than the live one is a publish, older is a roll back, and with nothing live every version is a publish. `MakeLiveDialog` is the confirmation shell and `useLatched` keeps its request through the render that closes it.
- `ChangesPanel`, `CompareButton` and `VersionCompareDialog` carry the comparison with the live version (#2112), built on `TextDiff` and `FieldDiff` in `ui/src/components/ui/text-diff.tsx` and the pure line and field diff in `ui/src/lib/text-diff.ts`. `ui/src/lib/version-diff.ts` turns a prompt or skill version pair into the rows. The diff is written in the repository rather than taken from a package, so nothing is added to the bundle.
- `DeleteBySlugDialog` is the destructive confirmation that needs the slug typed back.

What differs stays with the screen: the editor, the copy that names the thing, and what a make-live dialog says about impact. A screen passes its own already-translated title and description. Selecting a row, a version, or the row a create just made goes through `useDraftSwitch`, so unsaved edits are never replaced without the discard prompt.

## Still to do

`ModelSheet` is the only consumer today. `ProviderSheet`, `ProviderGroupSheet`
and `EditorSheet` solve the same layout problems separately and should move
onto these primitives; that migration is #1658, tracked separately from the
extraction so each sheet can be diffed on its own.
