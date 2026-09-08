# Dashboard navigation rail

The operator-facing description of the shell — signing in, the groups, the
scope switcher and the three breakpoints — lives in
`user-docs/concepts/dashboard.mdx`. This page is the contributor's view of the
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

| Viewport | Shape | Behaviour |
|---|---|---|
| `< 768px` (below `md`) | off-canvas drawer | Hidden by default and out of the flow entirely — a closed drawer takes no width. Opened by the hamburger in `ScreenHeader`, which renders only at this width; drawn over a scrim, with labels; dismissed by Escape, a scrim click, or navigating. Focus, the Tab trap and the scroll lock come from `useModalA11y`, the same contract `Sheet` signs. |
| `768px`–`1023px` (`md` to `lg`) | icon rail | On screen, folded to the 52px icon strip. The collapse toggle still works — this only picks the starting state. No splitter. |
| `≥ 1024px` (`lg` and up) | full rail | The resizable, collapsible rail described below. The remembered width applies here and nowhere else. |

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
