# Dashboard navigation rail

The operator-facing description of the shell — signing in, the groups, the
scope switcher and the three breakpoints — lives in
`docs/user-docs/concepts/dashboard.mdx`. This page is the contributor's view of the
same rail: the code, the invariants and the stories that pin them.

The left rail (`ui/src/components/ui/nav-sidebar.tsx`) is the dashboard's
primary navigation. It has three shapes, one per breakpoint, and two
independent size controls within them.

## The rail's header and card (#2805)

Two things used to share one menu behind the user card: the scope switcher, which
changes what every screen shows, and the person's own entries (their account, and
Sign out). The scope sat under a "Scope" heading between the identity block and
Account, with three identical unlabelled pickers and a **+** and a trash can on
each, so creating and deleting an org lived next to Sign out. They are now two
controls with one job each:

| control                       | where                                              | what it is about                             |
| ----------------------------- | -------------------------------------------------- | -------------------------------------------- |
| `ScopeSwitcher`               | the rail header, under the brand, above the search | what the dashboard is looking at             |
| the account card and its menu | the foot of the rail, as before                    | who is signed in, their screens, signing out |

`NavSidebar` offers the first as a slot, `headerExtra(folded)`, and the second as
`userMenu: { header, items(close) }`; `App.tsx` fills both.

### The shared overlay

There was no popover primitive, so the folded rail's group flyout (#2803) wrote
its own placement and dismissal, and the scope popover and both menus would each
have written a third and fourth copy. They are one now:

- `AnchoredPanel` (`ui/src/components/ui/anchored-panel.tsx`) is the surface. It
  is `fixed` and placed from the anchor's rectangle (`placePanel`, a pure function
  with its own tests): beside the rail's edge (`right`), or under (`below`) or over
  (`above`) the anchor, aligned to its start or end, clamped to the viewport, and
  turned over when `below` has no room. It is rendered inline, as the anchor's
  sibling, so it escapes `overflow` without leaving the nav drawer's focus trap,
  and it follows the anchor when a list it sits in scrolls. A press outside, focus
  moving elsewhere, and Escape close it; Escape closes only the panel on top, so
  a menu opened from inside the scope popover does not take the popover with it.
  `openPanelCount()` is what the nav drawer asks before treating Escape as its own.
- `Menu`, `MenuItem`, `MenuItemRadio` and `MenuSeparator`
  (`ui/src/components/ui/menu.tsx`) are the WAI-ARIA menu on top of it: a
  `role="menu"` of `menuitem`s (or `menuitemradio`s for an exclusive choice, which
  read `aria-checked` and carry a tick), focus on the checked entry or else the
  first one that can be chosen, Up and Down wrapping, Home and End, Tab closing the
  menu and carrying on to the control after its anchor, Escape handing focus back.
  `header` renders above the `menu` element rather than inside it, since a menu
  may own entries, groups and separators and an identity block is none of those.
- `GroupFlyout` stays a disclosure of navigation buttons, not a menu, and keeps
  its own keyboard on top of `AnchoredPanel`'s placement and dismissal.

### The language picker

`LocalePicker` (`ui/src/components/LocalePicker.tsx`, #2822) is the last of the
rail's floating lists and stands on the same two pieces as the rest: a `Menu` of
`MenuItemRadio` entries, one per catalog, each in its own language. It opens
above its button on the full rail and in the drawer, and beside the strip with
its bottom edge on the button's once the rail is folded (the account menu's
placement), so it is `fixed` like every other overlay and no scroll container can
clip it, and its panel carries the `--shadow-lg` token. Opening puts focus on the
language in force. Escape closes it with focus back on the button, Tab closes it
and carries on, a press outside closes it, and inside the drawer the first Escape
puts the menu away and only the second closes the drawer. Picking a language
closes the menu and leaves focus on the button, now under its new name. The
sign-in card renders the same component, where it opens upward from the card's
last line.

Stories: `MenuIsASharedPanelAboveTheButton`, `MenuFromTheKeyboard`,
`ChoosingFromTheKeyboard` and `MenuClosesOnAnOutsidePress` in
`LocalePicker.stories.tsx`; `LocalePickerOpensAboveTheFullRail`,
`LocalePickerOpensBesideTheFoldedRail` and `LocalePickerInsideTheDrawerOwnsEscape`
in `nav-sidebar.stories.tsx`; `DocumentTitle` in `App.stories.tsx` for the switch
itself.

### The account menu

The card opens a menu about the person and nothing else. Above the entries an
identity block gives the initials, the name, the email beneath it when the name
differs, and a **role line** that says where the role applies: _Admin · org acme_,
_Member · team platform_, _Viewer · project default_, _Superadmin · whole
deployment_. `ui/src/lib/account-role.ts` derives it from `user`, `memberships` and
the scope in view: a superadmin is one thing everywhere, and anyone else shows the
strongest grant that reaches the current org, team or project (the broader one
when two are equally strong). Each shape is one catalog string,
`shell.roleLine.{superadmin,org,team,project}`, so a locale can decline the level
as its grammar wants. The card itself says the role in a word.

The entries are **Account & keys** (`/api-keys`), **Preferences** (`/preferences`),
a separator and **Sign out**, in the danger tone. The two screens are named with
the rail's own labels (`nav.api-keys`, `nav.preferences`), so one noun stands for
one place. There is no theme toggle (the dashboard is dark-only, see `DESIGN.md`),
and language and shortcuts stay in the rail footer.

The menu opens above the card at the card's width on the full rail, and beside the
rail with its bottom edge on the card's when folded. The folded card is the
initials alone, so it is named by the account's name rather than by a lone letter.

### The scope switcher

The trigger shows the path `org / team / project` in the mono face, each level
truncating on its own with the broadest giving way first, so a long org name does
not push out the project. Its accessible name is `scope.trigger` ("Scope: <path>")
and the path is its tooltip. Folded, it is a `Building2` icon button with the same
name and tooltip. It opens a popover (`role="dialog"`, `AnchoredPanel`) of three
labelled rows, **Organization**, **Team** and **Project**, each a `Combobox`:

- the first picker takes focus on open; choosing a value keeps the popover open,
  because the level below usually changes too; Escape closes it and returns to the
  trigger, and a press outside closes it without taking focus;
- a row with nothing above it is disabled and says why underneath (`scope.needsOrg`,
  `scope.needsTeam`); a row with nothing in it keeps its **⋯** button, since
  creating the first one is the way out; the list failing is said under its row;
- the first read shows a skeleton of the three rows; a level still arriving after
  a pick keeps the popover on its rows.

Each row has one overflow button, **Organization / Team / Project actions**, which
opens a `Menu`: **New organization** and **Delete organization…**; **New team** and
**Delete team…**; **New project**, **Project settings** and **Delete project…**. A
delete or settings entry is absent when the level has no row to act on. Picking one
closes the popover, parks focus on the trigger and raises the dialog, so the
dialog's return-to-opener lands somewhere that survives the menu unmounting.
Deleting goes through `ConfirmDialog`, which names the row and the consequence.

Create and delete are gated on the capability the control plane enforces
(`org:create` is superadmin-only, the rest need admin): a refused entry is a real
`disabled` button whose `title` and whose visible second line name the role. The
reason is printed because a menu is walked with the arrow keys and a disabled
button is not a stop on that walk, so a tooltip alone would not be reachable.
Project settings is not gated: anyone on the project can open it and read where
the setting stands, and only its switch is (`project_settings:update`).

`CreateProjectHost`, from the same file, is still mounted once by the shell in
`App.tsx` and registers itself with `useCreateProjectOpener`, and any screen opens
it with `openCreateProject()` from `ui/src/lib/scope.ts` (#2611). It is not part
of the switcher because the rail is out of the document whenever it is a closed
drawer, so an opener registered inside it would be gone by the time a screen called
it. It is the dialog **New project** raises, under the team in scope, and does
nothing when no team is in scope. The Getting started card is the first caller; the
`GettingStartedOpensCreateProject` story in `ui/src/App.stories.tsx` opens it
through the whole shell.

Stories: `ui/src/components/ScopeSwitcher.stories.tsx` (the path and the labelled
rows, folded, loading, no org, no team, failing orgs, keeping the popover open on a
pick, Escape and an outside press, Tab through the rows, the row menu's keyboard,
what each level's menu offers, create and delete through the menus, and the gating
as a viewer, an admin and a superadmin); `UserMenu…` and `HeaderSlot…` in
`nav-sidebar.stories.tsx` (the menu's shape, keyboard, folded placement, Escape
inside the drawer); and `AccountMenu…`, `RoleLine…`, `ScopeSwitcherSitsUnderTheBrand`,
`ScopePopover…` in `App.stories.tsx` through the whole shell, including Russian at
1024px, a superadmin with no membership, and the folded and drawer shapes.

## Breakpoints

The shape is chosen in javascript, not only in CSS: below `md` the rail is a
modal drawer, and a modal owes a focus trap, an Escape handler and a scroll
lock that no class can supply. `ui/src/lib/use-media-query.ts` exports the two
queries (`BELOW_MD`, `BELOW_LG`), which are tailwind's `md` and `lg` so the
javascript and the classes cannot disagree.

| Viewport                        | Shape             | Behaviour                                                                                                                                                                                                                                                                                                                                                                        |
| ------------------------------- | ----------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `< 768px` (below `md`)          | off-canvas drawer | Hidden by default and out of the flow entirely — a closed drawer takes no width. Opened by the hamburger in `ScreenHeader`, which renders only at this width; drawn over a scrim, with labels; dismissed by Escape, a scrim click, or navigating. Focus, the Tab trap, the scroll lock and the inert screen beside it come from `useModalA11y`, the same contract `Sheet` signs. |
| `768px`–`1023px` (`md` to `lg`) | icon rail         | On screen, folded to the 52px icon strip. The collapse toggle still works — this only picks the starting state. No splitter.                                                                                                                                                                                                                                                     |
| `≥ 1024px` (`lg` and up)        | full rail         | The resizable, collapsible rail described below. The remembered width applies here and nowhere else.                                                                                                                                                                                                                                                                             |

Below `md` the rail's persisted width is not read and not written: the drawer
is sized by the viewport, and a width dragged on a desktop must not decide how
much of a phone screen the navigation eats.

Stories: `MobileDrawer` and `TabletIconRail` in `nav-sidebar.stories.tsx` pin
the first two rows, including that nothing overflows the viewport at either
width. They set their size through `src/lib/story-viewport.ts`
(`globals.viewport`), which `@storybook/addon-vitest` turns into a real
`page.viewport` call before the story renders — the viewport addon only sizes the
preview iframe inside the Storybook UI, so without that a "fits at 375px" story
would be measured at the default 1280 and assert nothing.

## The assembled shell

The three shapes above are the rail's. The shell is the rail _plus_ the screen
header that opens it and the route that dismisses it, and until #1239 nothing
mounted all three together: the drawer's open state is owned by `App`, its
trigger lives in `ScreenHeader`, and the `useEffect` on `location.pathname`
that closes it is a third place again. Each half had a story; the composition
had none.

`ui/src/pages/shell-harness.tsx` is the fixture that mounts it. It stacks the
providers in the order `main.tsx` does — query client, toasts, a session
already in `localStorage`, a `MemoryRouter` at the requested route, `App` —
over a fetch stub that answers `/auth/me`, the RBAC pair, the org/team/project
chain, `/version` and the landing screen's own data. Anything it does not
name falls through to `[]`, so a screen the drawer navigates to lands in its
empty state rather than an error. It is a sibling of `pages/story-harness.tsx`
rather than more of it: that module is imported by every screen story, and
pulling `App` into it would pull every page into every one of those bundles.

`ui/src/App.stories.tsx` uses it for the three widths — `Desktop` (full rail,
splitter, the booted route marked `aria-current`), `Tablet` (52px icon strip,
no splitter) and `Mobile` (no rail at all, the hamburger opens the drawer, and
picking an entry both navigates and closes it). All three read their copy out
of the `en` catalog rather than repeating it.

The story caught a real defect the moment it existed: the screen's scroll
container in `App.tsx` was scrollable without being focusable, so on any
viewport where a screen overflows, everything below the fold was mouse-only.
It now carries `tabIndex={0}` and a `role="region"` named by the screen title,
the same contract `ListTable` and `CodeBlock` already sign.

## Collapse

`collapsible` puts a toggle in the brand row that folds the rail down to a
52px icon-only strip. Labels, the search box, group headings and the version
become titles or disappear; the active item keeps its folk-red thread.

### A group on the folded rail

A group's children are drawn under it only when the rail has the width for it,
so folded to the 52px strip a group has nowhere to unfold in place. Until #2803
the click still toggled the group's expanded state, which nothing drew, and the
group did nothing at all. Folded, a group opens a flyout (`GroupFlyout` in
`nav-sidebar.tsx`) instead:

- **What it is.** A `fixed` panel beside the rail, level with the icon,
  headed by the group's label and listing its screens with the same item styles
  the full rail uses (the experimental badge and the count included). It is
  `fixed` rather than absolute because the rail's list is a scroll container and
  would clip it; the position comes from the icon's rectangle, nudged up when the
  viewport is too short to hold it below. It follows the icon while the list
  scrolls and goes once the icon has scrolled out of view. The surface, its
  placement and its dismissal are `AnchoredPanel`'s (see "The shared overlay"
  above), shared with the account menu and the scope popover.
- **A disclosure, not an ARIA menu.** The icon is a button with
  `aria-expanded`, the flyout a `role="group"` named by its heading, and the
  entries are the same navigation buttons the full rail has, so they sit in the
  tab order right after the icon and `aria-current="page"` marks the screen
  you are on.
- **Keyboard.** Enter or Space (the button's click), or the right or down
  arrow, opens it and moves focus to the screen you are on, else to the first.
  Up and down arrows move through the screens and wrap; Home and End jump to the
  ends. Escape or the left arrow closes it and returns focus to the icon, as does
  picking a screen. A press outside, Tab past either end, the rail unfolding, or
  a route change closes it without moving focus. One flyout is open at a time.
- **The current section.** The screen you are on is out of sight when folded, so
  its group carries the active thread and `aria-current="true"` (not `"page"`:
  the group is not the page). The full rail is unchanged.
- **Gating.** The flyout renders `item.children` as handed in, and `visibleNav`
  has already dropped the screens a role cannot read and the groups left
  empty, so a refused screen is neither in the flyout nor behind an icon.

Stories: the `FoldedGroup…` set in `nav-sidebar.stories.tsx` (open, current
section, keyboard, navigate, outside press, focus leaving, one at a time, the
experimental marker, following the list's scroll), `NavAsViewerFlyout` and
`NavAsSuperadminFlyout` in `CapabilityGating.stories.tsx`, and
`FoldedRailOpensAGroup` and `FoldedRailFlyoutLeavesOutRefusedScreens` in
`App.stories.tsx` for the assembled shell.

## A label the rail cuts

A label wider than the rail ends in an ellipsis. The `ru` catalog runs a third
longer than `en`, so at the default 232px "Адаптивная маршрутизация" read
"Адаптивная маршрут…" with the rest of it nowhere on screen (#2830). The folded
rail already names each icon through `title`; the full rail now does the same for
the label it has cut, and only for that one:

- The rail's list and the group flyout both hand `revealClippedText`
  (`ui/src/lib/reveal-clipped-text.ts`, the function `ListTable` uses for its cells)
  to `onPointerOver`. On pointer over, the label under the pointer gets its full
  text as a `title` if, and only if, it is cut by an ellipsis; one that fits gets
  nothing, and a label that has since been given room (a wider rail, a longer
  window) loses the title it was given. The text is in the document in full either
  way, so the accessible name never depended on it.
- An experimental entry does not clip at all: its name wraps beside or above its
  badge (#2812), so there is nothing to reveal.
- The footer cuts two more things the same way, and answers the same way (#2861).
  The account card's name and role are cut at the default width by a long e-mail
  address or a long role name, so the card button hands `revealClippedText` to its
  own `onPointerOver`. The update pill's text ("v0.2.0 available") is cut when
  the footer's four icon links, the language code and the version have taken the
  row, so the row that holds them does the same. The pill's link keeps the
  sentence it already carries as `title` and `aria-label`; the text inside it
  names only itself, and an icon link's author-written `title` is left alone.
- Folded, the card has no text left to cut: its `title` and `aria-label` already
  carry the whole name, and the handler is not attached.

Stories: `ClippedRailLabelNamesItself` and `ClippedLabelInAFlyoutNamesItself` in
`nav-sidebar.stories.tsx`, and `RailLabelsNameThemselvesWhenCutInRussian` in
`App.stories.tsx`, which opens every group of the real navigation at 1024px in
`ru` and checks each label that is cut and one that is not. For the footer,
`ClippedAccountCardNamesItself` and `ClippedUpdatePillNamesItself` cut a long
e-mail, a long role and the crowded footer at 232px, and
`AccountCardThatFitsNamesNothing`, `UpdatePillThatFitsNamesNothing` and
`FoldedAccountCardNamesTheWholeAccount` hold the cases that must stay quiet.

## Resize

`resizable` turns the right edge into a splitter. It exists because a fixed
width cannot suit every locale: the `ru` catalog's labels are consistently
longer than `en`'s, and at the shipped 232px several of them truncate with no
way to read the whole label (#950).

- **Bounds** — `NAV_MIN_WIDTH` (180px) to `NAV_MAX_WIDTH` (420px), exported
  from the component. Every path clamps: drag, keyboard, and the value read
  back from storage, so a stale or hand-edited entry cannot restore a rail too
  narrow to click or wide enough to bury the content.
- **Mouse** — drag the edge. Double-click resets to `NAV_DEFAULT_WIDTH`
  (232px, the value `--sidebar-width` carries in `index.css`).
- **Keyboard** — the splitter is a focusable `role="separator"` with
  `aria-orientation="vertical"` and live `aria-valuenow`/`valuemin`/`valuemax`.
  `←`/`→` move it 16px, `Shift` multiplies that by four, `Home` and `End` jump
  to the bounds, and `Enter`/`Space` return it to the default. A mouse-only
  affordance would not be acceptable on a primary nav control.
- **Persistence** — the settled width is written to `localStorage` under
  `storageKey` (`rolter.nav.width` by default), so it survives a reload per
  browser. Reads and writes are both wrapped: a browser that refuses storage
  still resizes, it just forgets. The stored value is applied after mount
  rather than in the state initializer, so the first paint is the same
  everywhere.
- **Collapse wins.** While collapsed there is no splitter at all — a 52px icon
  rail has no edge worth dragging, and leaving it behind would let a keyboard
  user stretch a rail whose labels are hidden. The same applies below `lg`,
  where the rail is folded or a drawer.

The width transition is disabled for the duration of a drag; animating it
would fight the pointer.

Stories: `Resizable`, `DraggedNarrow`, `DraggedWide` and
`CollapsedHasNoSplitter` in `nav-sidebar.stories.tsx` cover the bounds, the
keyboard path, the ARIA contract and the collapsed case.

## Keyboard: the shortcut table

Every shortcut the dashboard binds is declared once, in `SHORTCUTS` in
`ui/src/lib/shortcuts.ts` (#1676). The shell dispatches by walking that table
and the reference sheet renders by mapping it, so neither side names a key of
its own — a shortcut that works cannot be missing from the sheet, and a row in
the sheet cannot name a keystroke nothing listens for.

| Key                            | What it does                                  | Where it lives                                                                     |
| ------------------------------ | --------------------------------------------- | ---------------------------------------------------------------------------------- |
| `⌘K` / `Ctrl-K`                | toggles the command palette                   | `isPaletteShortcut` in `ui/src/lib/command-palette.ts`                             |
| `/`                            | puts the caret in the rail's search box       | `isNavSearchShortcut`, which focuses `NAV_SEARCH_ID`                               |
| `?`                            | opens the keyboard shortcut reference         | `isHelpShortcut`, which opens `ShortcutHelp`                                       |
| `Tab` from the top of the page | reveals the skip link, which focuses `<main>` | the first child of the shell in `ui/src/App.tsx`; not a chord, so not in the table |

**Adding one** is three edits in one file plus its copy: a `ShortcutId`, a row
in `SHORTCUTS`, and the action in the shell's `ShortcutHandlers` — a `Record`
over the id union, so the build fails until the shell handles it. The name goes
under `shell.shortcuts.items.<id>` in **every** catalog;
`ui/src/lib/shortcuts.test.ts` fails on a locale that is short one, and on copy
left behind for a shortcut that has been removed.

`/` and `?` are ignored while the caller is typing — `isTextEntry` checks the
event target — or the characters would be unwritable in every field in the
dashboard, model names and prompts included. The rule is duck-typed rather than
an `instanceof HTMLElement` check so the unit tests can exercise it without a
DOM. `?` rejects Meta, Control and Alt but _not_ Shift: on most layouts Shift is
how `?` is typed at all.

The printed chord is the same table's data. `MOD` resolves to `⌘` on an Apple
keyboard and `Ctrl` elsewhere (`isApplePlatform` reads `userAgentData.platform`
and the deprecated `navigator.platform`, and guesses `Ctrl` when it has
neither), and `KbdChord` in `ui/src/components/ui/kbd.tsx` prints it — one
`<kbd>` per key, with the whole group carrying `⌘K` as its accessible name.
Getting the platform wrong only changes a label: the matchers accept Meta _and_
Control regardless. Stories pin `apple` rather than letting the runner's own
platform decide what they assert.

The hints beside the controls read their chord from the table too:
`shortcutChord("palette")` inside the palette's own field, and
`shortcutChord("navSearch")` in the rail's search box, where it gives way to the
clear button once there is a query.

A chord is not a discovery path on its own, so the reference has a mouse path
as well (#1697): a `footerLinks` entry in `ui/src/App.tsx`, beside the
magnifier that opens the palette, opening the same `ShortcutHelp`. Its title is
`shell.shortcuts.open` interpolated with `chordText(shortcutChord("help"))`, so
the entry names the key it stands in for and cannot drift from the table
either. The footer rides the rail at every width, which is why
`ShortcutReferenceFromRail`, `…FromIconRail` and `…FromDrawer` in
`App.stories.tsx` cover all three.

The skip link is an anchor whose `href` is the `<main>` id, but it calls
`preventDefault` and focuses the element itself: letting the fragment land
would rewrite the router's URL, and a fragment target only takes focus in some
browsers. `<main>` therefore carries `tabIndex={-1}`, which is what makes it a
focusable target at all. The link sits inside a `<header>` so it is content
within a landmark — axe's `region` rule is on for `Shell/App`, and a bare
anchor above the rail fails it.

`ui/src/components/CommandPalette.tsx` is not built on the `Combobox`
primitive: that one is a value picker that writes a pick back into a form,
while the palette navigates and edits nothing. It owes the same roles all the
same — a `combobox` input over a `listbox` of `option`s, moved with
`aria-activedescendant` so focus never leaves the field being typed into. The
container drops the `listbox` role while there is nothing in it, since a
`listbox` with no `option` inside fails `aria-required-children`.

The record half is deliberately cheap: virtual keys, providers and routes, read
with the same endpoints their screens use, only once the palette is open and
only for a caller whose capabilities do not say no. There is no search
endpoint, and the palette must not grow one without one being built first.

The one thing the palette looks up rather than lists is a pasted request or trace
id (#1861). `pastedIdLookup` in `ui/src/lib/command-palette.ts` reads the query
with `parseLogLookup` from `ui/src/lib/log-lookup.ts`, the same rule LLM Logs
applies to its own field: 32 lowercase hex characters, or a `traceparent` value
holding them, is a trace id, and anything else is a request id. A request id
also has to be one token of at least `MIN_PASTED_ID_LENGTH` characters with a
digit, so a typed word is never mistaken for one. The offer is a `lookup`
section with one entry, shown only when no screen and no record matched (a
route called `gpt-4o-mini` still opens Routing Rules), only when `logs` is in
the caller's nav, and for a request id only once the scope and the record
lists have settled (the lists are not fetching while the scope resolves, so
waiting on them alone would show the offer and then take it away). Its
`PaletteEntry` carries a `search`, which `onNavigate(screen, search)` appends, so the palette opens `/logs?request_id=…`. The records' own
skeleton and error sit beside the `listbox` rather than in it: a `listbox` may
hold options only.

Ranking lives in `ui/src/lib/command-palette.ts` and is unit-tested there:
subsequence matching, so "rr" reaches Routing Rules, with word-boundary and
head-of-label bonuses that keep initialisms above incidental hits. Recently
visited screens are remembered per browser under `rolter.recent-screens`;
storage is passed in rather than reached for, so the same test covers a browser
that refuses it.

Stories: `CommandPaletteShortcut`, `NavSearchShortcut` and `SkipLink` in
`App.stories.tsx` pin the three keys against the assembled shell, and
`APastedIdInThePaletteOpensLlmLogs` follows a pasted id from the palette into
the open drawer;
`Shell/CommandPalette` pins the palette's own keyboard, its records, and its
loading, error and empty states; `SearchMatchesGroupLabel` and
`SearchMatchesNothing` in `nav-sidebar.stories.tsx` pin the two nav-search bugs
#1198 named — a group-label match used to expand the group and then filter
every child out of it, and a query matching nothing used to empty the rail
rather than say so.

## Every leaf is a screen

`NAV` in `ui/src/lib/nav.tsx` names the entries; `SCREENS` in `ui/src/App.tsx`
maps each navigable leaf key to the element rendered at `/<key>`. The two are
one list written twice, and `ui/src/lib/nav.test.ts` holds them to each other:
a nav entry with no screen, or a screen no entry reaches, fails there.

Until #1201 that invariant was assumed rather than checked. `App` looked the key
up in a `BUILT` set and fell back to a branded `Stub` screen ("TODO — we'll come
back to this screen") for anything missing. Every leaf had been built long
before, so the fallback rendered nowhere — dead code that still advertised that
the rail was allowed to point at a screen which does not exist. Both the set and
the placeholder are gone; the test is what keeps the table complete.

## The document title

Each screen names the browser tab `{title} · rolter` (#2002). Before that every
screen shared the one `<title>` in `ui/index.html`, so tabs, history entries and
a screen reader's page announcement could not tell one screen from another
(WCAG 2.4.2).

`useDocumentTitle` in `ui/src/lib/document-title.ts` writes it. The name is
appended in code rather than taken from a catalog, because it is lowercase in
every locale, the tab title included (see the brand guidelines). The title part
is the caller's already-translated string, so switching language re-renders the
caller and renames the tab without a reload.

Three places call it:

- `Screen` in `ui/src/App.tsx`, with the header's own `screens.<key>.title`.
  That covers every routed leaf. The refused state renders inside `Screen`, so a
  screen the caller may not read keeps its name in the tab, and an unknown or
  legacy path redirects to a leaf that names itself.
- `Login`, with `auth.title`. Signed out there is no `Screen` around it.
- `AcceptInvite`, with `pages.acceptInvite.title` once the preview names the
  org. While the preview is in flight, or when the link is dead, it uses
  `pages.acceptInvite.genericTitle`.

A new page rendered outside `Screen` calls the hook itself. Stories:
`DocumentTitle`, `DocumentTitleOnARefusedScreen` and
`DocumentTitleOnAnUnknownPath` in `App.stories.tsx`, `DocumentTitle` in
`Login.stories.tsx`, and the `Loaded` and `InvalidLink` stories in
`AcceptInvite.stories.tsx`. Each blanks `document.title` in `beforeEach`,
because the title outlives a story and one left behind would pass for the next.

## The experimental marker

`SUBSYSTEMS` in `crates/rolter-core/src/stability.rs` is the only list of what
this build ships as experimental (#1385). It travels on `GET /api/v1/version`
as `experimental`, and each entry carries the `nav_keys` it surfaces on — so
the mapping from subsystem to nav leaf has one owner, and `ui/` never keeps a
second copy that can drift out of step with the backend's.

`useStability` in `ui/src/lib/version.ts` turns that into a map of nav leaf key
→ note. It shares `useVersionStatus`'s query key, so the shell still makes one
request for the two things it reads out of that answer, and it is tolerant on
every path that can fail: an older control plane with no `experimental` field,
a network error, or a session still being checked all yield an empty map. The
rail then renders with no markers, which is the correct answer rather than an
error — a rail that will not render is a far worse outcome than a rail missing
a badge.

`toNavItem` in `ui/src/App.tsx` sets `experimental` and `experimentalNote` on
the `NavItem`s whose key the map names. The marker rides along with the
individual entry: the grouping is untouched and there is no "experimental"
section, which was an explicit constraint of #1386.

Two shapes, because the rail has two widths:

- **Full width** — a `Badge` beside the label carrying `shell.experimental`,
  with the build's own one-line note as its `title`. The badge is inside the
  button, so the entry's accessible name is "Tool groups Experimental" and a
  screen reader gets the marker without having to find a sibling element. The
  name never gives way to it (#2812): `NavLabel` puts the two on one wrapping
  line, so they share a row while both fit and the badge drops under the name
  when they do not. The rail's default 232px left "Репозиторий навыков" four
  letters ("Репоз…") beside "Экспериментально" when the badge held its size and
  the label truncated, and a name longer than the rail wraps instead of being
  clipped. An entry with no marker keeps the single truncating line. The
  folded rail's flyout draws the same `NavLabel`, so the two cannot disagree.
- **Folded to icons** — no room for a word, so the marker is a decorative dot
  on the corner of the entry's icon, mirroring the footer's update hint. The
  word moves into `shell.experimentalItem`, the button's `title`; on a button
  with no text content that tooltip is also its accessible name.

The page header deliberately carries no marker. #1386 asked for the nav alone,
and the rail is where an operator is choosing what to rely on.

Stories: `ExperimentalItems`, `ExperimentalItemsCollapsed` and
`ExperimentalItemsNarrow` in `nav-sidebar.stories.tsx` cover the two shapes and
the narrowest width the rail can be dragged to, with
`ExperimentalItemWithALongNameWraps` and
`ExperimentalMarkerBesideTheNameWhenItFits` for the wrap; `ExperimentalMarker` and
`ExperimentalMarkerOnIconRail` in `App.stories.tsx` cover the whole path from
the endpoint's answer to the marked entry, and
`ExperimentalEntriesStayWholeAt1024` (also `…InRussian` and
`…InTheFlyoutInRussian`) measure the name and the badge in the full rail, under a
group and in the flyout at a 1024px window.

## The tab strip

`ui/src/components/ui/tabs.tsx` is the in-page counterpart to the rail: an
underline strip of `role="tab"` buttons inside a `role="tablist"`, used by
`Rbac`, `Playground` and `CodeSnippetDialog`. It follows the WAI-ARIA tablist
pattern, which is a keyboard contract, not styling:

- **One tab stop.** A roving tabindex puts `tabIndex={0}` on the selected tab
  and `-1` on the rest, so `Tab` walks past the whole strip in one press
  instead of one per tab. A `value` matching no tab still leaves the first tab
  reachable — a strip with no tab stop is a keyboard trap in reverse.
- **Arrows move selection.** `←`/`→` step and wrap at both ends, `Home` and
  `End` jump to the edges. Selection follows focus (automatic activation),
  which is the pattern's default for panels that are cheap to render; all three
  call sites are.
- **Panels are optional.** A `TabItem` may carry `id` and `panelId`; the tab
  then gets `aria-controls` and the caller's `role="tabpanel"` points back with
  `aria-labelledby`. Call sites that render no panel omit both and are
  unchanged — the relationship is opt-in, so adding it to the primitive did not
  ripple through the screens (#1273).

Stories: `WalksWithArrowKeys` and `LinkedToPanel` in `tabs.stories.tsx` cover
the roving tabindex, the wrap, `Home`/`End` and the panel wiring.

## Every screen is its own chunk

`SCREENS` in `ui/src/App.tsx` maps a navigable leaf to the element the shell
renders for it, and every entry goes through `React.lazy` — `screen(() =>
import("@/pages/Foo"))`, or `named(…, "Bar")` for the files that export several
screens side by side. Vite emits one chunk per lazy boundary, so a screen's
code is fetched when that screen is first opened.

It used to be one bundle. The dashboard emitted a single 1.26 MB chunk, so the
first paint of the sign-in screen downloaded the playground, the charts, the
highlighter grammars and forty-odd screens the reader had not asked for — on a
slow link to a self-hosted deployment that is the whole wait, and the
`chunks are larger than 500 kB` warning had been normalised into build noise
that would have hidden the next regression (#1709). Splitting takes the entry
chunk to ~374 kB and leaves the build warning-free without touching
`chunkSizeWarningLimit`.

Two screens stay statically imported on purpose: `Login` and `AcceptInvite`
_are_ the first paint for a signed-out reader, so deferring them would add a
round trip to the one screen that cannot spare one.

The shell wraps the screen region in a `React.Suspense` whose fallback is a
`ListSkeleton`, so the first visit to a screen shows the same `role="status"`
placeholder its own queries use rather than going blank.

Nothing about the air-gapped guarantee changes. Every split chunk is emitted
into `dist/assets` and served by the control plane from there, exactly as the
single bundle was — there is no runtime fetch to anything outside the
deployment.

`ui/src/lib/screens.test.ts` holds this: a static `import Foo from
"@/pages/Foo"` added for one new screen pulls that screen and everything it
imports back into the entry chunk, the build still succeeds, and nothing in the
output says which screen did it. The test names it.
