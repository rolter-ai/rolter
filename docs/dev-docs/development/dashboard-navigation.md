# Dashboard navigation rail

The operator-facing description of the shell — signing in, the groups, the
scope switcher and the three breakpoints — lives in
`docs/user-docs/concepts/dashboard.mdx`. This page is the contributor's view of the
same rail: the code, the invariants and the stories that pin them.

The left rail (`ui/src/components/ui/nav-sidebar.tsx`) is the dashboard's
primary navigation. It has three shapes, one per breakpoint, and two
independent size controls within them.

## Breakpoints

The shape is chosen in javascript, not only in CSS: below `md` the rail is a
modal drawer, and a modal owes a focus trap, an Escape handler and a scroll
lock that no class can supply. `ui/src/lib/use-media-query.ts` exports the two
queries (`BELOW_MD`, `BELOW_LG`), which are tailwind's `md` and `lg` so the
javascript and the classes cannot disagree.

| Viewport                        | Shape             | Behaviour                                                                                                                                                                                                                                                                                                                                            |
| ------------------------------- | ----------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `< 768px` (below `md`)          | off-canvas drawer | Hidden by default and out of the flow entirely — a closed drawer takes no width. Opened by the hamburger in `ScreenHeader`, which renders only at this width; drawn over a scrim, with labels; dismissed by Escape, a scrim click, or navigating. Focus, the Tab trap and the scroll lock come from `useModalA11y`, the same contract `Sheet` signs. |
| `768px`–`1023px` (`md` to `lg`) | icon rail         | On screen, folded to the 52px icon strip. The collapse toggle still works — this only picks the starting state. No splitter.                                                                                                                                                                                                                         |
| `≥ 1024px` (`lg` and up)        | full rail         | The resizable, collapsible rail described below. The remembered width applies here and nowhere else.                                                                                                                                                                                                                                                 |

Below `md` the rail's persisted width is not read and not written: the drawer
is sized by the viewport, and a width dragged on a desktop must not decide how
much of a phone screen the navigation eats.

Stories: `MobileDrawer` and `TabletIconRail` in `nav-sidebar.stories.tsx` pin
the first two rows, including that nothing overflows the viewport at either
width. They set their size through `src/lib/story-viewport.ts`, which
`.storybook/test-runner.ts` turns into a real `page.setViewportSize` — the
viewport addon only sizes the preview iframe inside the Storybook UI, so
without that hook a "fits at 375px" story would be measured at 1280 and assert
nothing.

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

Ranking lives in `ui/src/lib/command-palette.ts` and is unit-tested there:
subsequence matching, so "rr" reaches Routing Rules, with word-boundary and
head-of-label bonuses that keep initialisms above incidental hits. Recently
visited screens are remembered per browser under `rolter.recent-screens`;
storage is passed in rather than reached for, so the same test covers a browser
that refuses it.

Stories: `CommandPaletteShortcut`, `NavSearchShortcut` and `SkipLink` in
`App.stories.tsx` pin the three keys against the assembled shell;
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
  screen reader gets the marker without having to find a sibling element.
- **Folded to icons** — no room for a word, so the marker is a decorative dot
  on the corner of the entry's icon, mirroring the footer's update hint. The
  word moves into `shell.experimentalItem`, the button's `title`; on a button
  with no text content that tooltip is also its accessible name.

The page header deliberately carries no marker. #1386 asked for the nav alone,
and the rail is where an operator is choosing what to rely on.

Stories: `ExperimentalItems`, `ExperimentalItemsCollapsed` and
`ExperimentalItemsNarrow` in `nav-sidebar.stories.tsx` cover the two shapes and
the narrowest width the rail can be dragged to; `ExperimentalMarker` and
`ExperimentalMarkerOnIconRail` in `App.stories.tsx` cover the whole path from
the endpoint's answer to the marked entry.

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
