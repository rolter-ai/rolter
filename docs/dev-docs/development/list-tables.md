# Dashboard list tables

Most list screens (Virtual Keys, Users, Model Providers, Provider Groups, the
Model Catalog, Alert History, MCP Logs, OAuth Grants and Auth Sessions) draw
their rows with the list-table primitives in `ui/src/components/screen.tsx`.
They are `div`s laid out as a CSS grid, one template shared by the header and
every row, so a column cannot drift between the two.

The grid only draws the columns. Until #2000 nothing in the markup said
"table", and a screen reader read every header and cell as a `generic` node:
no table navigation, no column header announced with a cell, and a sort
direction shown only by an unlabelled arrow. The roles below are what make the
grid a table to assistive technology (WCAG 1.3.1).

| Primitive            | Role                            | What it is                                                                   |
| -------------------- | ------------------------------- | ---------------------------------------------------------------------------- |
| `ListTable`          | `table`                         | the bordered, sideways-scrolling frame; `label` names it                     |
| `ListHeader`         | `rowgroup` holding one `row`    | the header band; its `className` lands on the band                           |
| (inside `ListTable`) | `rowgroup`                      | the body: every child of `ListTable` that is not the `ListHeader`            |
| `ListRow`            | `row`                           | one data row, on the same grid template as the header                        |
| `ListHeaderCell`     | `columnheader`                  | a column's visible heading                                                   |
| `SortLabel`          | `columnheader` holding a button | a sortable heading, carrying `aria-sort`                                     |
| `ListActionsHeader`  | `columnheader`                  | the heading over a row's buttons: no visible text, a name for screen readers |
| `ListCell`           | `cell`                          | one column of one row                                                        |
| `ListStateRow`       | `row` holding one `cell`        | what the body shows instead of rows: the loading skeleton, the empty state   |
| `ListLoadingRow`     | a `ListStateRow`, or nothing    | the skeleton row, shown while the screen's read is awaiting an answer        |
| `ListEmptyRow`       | a `ListStateRow`, or nothing    | the empty row, shown only once the read succeeded with no rows               |

`ListTable` puts the body rowgroup in itself, the way a browser puts a
`<tbody>` round rows written straight into a `<table>`, so a screen only writes
the header and the rows. It tells the header apart by type, which means the
`ListHeader` has to be a direct child.

## Building a table

- **Name it.** `label` is required and becomes the table's `aria-label`. Pass
  the screen's own title, `t("screens.<key>.title")`. The table is also the
  focusable scroll frame (#1181), so the name is what focus announces.
- **Every child of a row is a cell.** A `ListHeader` holds `ListHeaderCell`,
  `SortLabel` or `ListActionsHeader`; a `ListRow` holds `ListCell`. A bare
  `span` in a row is read with no column header.
- **Replace an element, wrap a component.** Where a row used to hold a `span`
  or `div`, change the tag to `ListCell` and keep its classes: the cell becomes
  the grid item and nothing moves. Where it held a component (a `Pill`, a
  `Badge`, a `Combobox`, a `Switch`, a `RowIconButton`), wrap it in
  `<ListCell className="grid">`. A plain block cell would shrink a pill to its
  text and a button to its icon; `grid` lays the component out exactly as it
  was when it sat in the row's grid itself, stretched across its column. Add
  `min-w-0` when the component's own root carried it, or long content widens
  the column.
- **Name the button column.** A column of row buttons takes a
  `ListActionsHeader`, never an empty `<span />`: a column header with no name
  is announced as an empty column and fails axe `empty-table-header`. It reads
  `common.rowActions` by default; a table whose buttons take more than one
  column passes `label` so each has its own name (Auth Sessions has
  `colRenew` and `colRevoke`).
- **A row that opens something holds a control for it.** A `ListRow` is a
  `row`, which takes no focus, so an `onClick` on it is a mouse shortcut and
  nothing more: without a button inside it, Tab walks past every row (WCAG
  2.1.1). MCP Logs is the model (#2022). Its last column is a
  `ListActionsHeader` labelled `analytics.details`, and each row's chevron
  button is named after the event (tool, server and time, since a tool repeats
  down the page). The button stops propagation so the row's own click does not
  fire a second time, and its story opens the drawer from the keyboard and
  asserts, in `waitFor`, that focus moves into the drawer and back to the
  button on close.
- **Loading and empty go in a `ListStateRow`.** A skeleton's `role="status"` or
  an empty state's heading and button placed straight in the body is content no
  row owns, which a screen reader reads outside the table and axe fails as
  `aria-required-children`. The header stays on screen above it, which is the
  point: see [loading and empty states](loading-and-empty-states.md).
- **The state row is as wide as what the reader sees.** Below its column floor
  (`minWidth`, 760 by default) the table scrolls sideways inside its card, and
  a state row the width of the floor centred the empty title and its button in
  a band that began past the card's right edge: on a 375px phone a first-run
  admin saw a header and an empty card (#2362). `ListTable` is a size container
  (`container-type: inline-size`) and `ListStateRow` is `sticky left-0` and
  `100cqw` wide, so it fills the visible frame and holds there while the
  header and the rows scroll beneath it. The header band and the rows keep the
  floor, so the columns stay aligned, and at desktop width, where nothing
  scrolls, the state row is exactly as wide as the header. Put anything a
  screen draws in place of rows into a `ListStateRow` (a `LoadError` included)
  and it is held to the same edge; do not size it yourself. The table's own
  width must not come from its content, which is true in a block or a column
  flex (every list screen today) and is not in a row flex, where the table
  needs `flex-1 min-w-0`.
- **The native `Table` holds its placeholder to the frame too.** The data-driven
  `Table` in `ui/src/components/ui/table.tsx` is a real `<table>` inside a
  scrolling `div`, and its `empty` placeholder is one `<td colSpan>` the width
  of the whole table. Below its columns' width the table scrolls sideways and
  that cell was centred on a band wider than the card: at 375px Cluster's title
  ran 18px past the card's edge, and Audit Log's and User Provisioning's sat off
  to one side (#2420). The scroller is a size container and the cell holds a
  `sticky left-0` box `100cqw` wide, the same answer as `ListStateRow`: the
  placeholder is centred on what the reader sees and stays there while the
  columns scroll beneath it. The header and the body rows are untouched, so
  the columns stay aligned, and at desktop width the box is exactly as wide as
  the table. The same rule applies: the scroller's width must not come from its
  content, which holds while it keeps `w-full` and does not in a row flex with
  that swapped for `w-auto`.
- **Hand the state rows the query, not a condition.** Write the skeleton as
  `<ListLoadingRow read={query}>` and the empty state as
  `<ListEmptyRow read={query} rows={rows.length}>`, where `query` is the
  screen's `useQuery` result and `rows` is what survived its filters. The rows
  alone cannot tell a list that is still coming, or one whose read failed,
  from one that answered with nothing, and a hand-written
  `!query.isLoading && rows.length === 0` put "No providers yet" and its create
  button under the list's own `LoadError` (#2211).

## Sorting

`SortLabel` is the header cell and the button in one. The sort state is
`aria-sort` on the header, where a screen reader announces it with the column:
`ascending`, `descending`, or `none` for a sortable column that is not sorted.
A column that cannot be sorted carries no `aria-sort` at all.

The button's name is the column's label, which the caller has already
translated. It deliberately has no `aria-label` such as "Sort by name": a
header's name is computed from its content, so that label would become the
column's name as well, and a screen reader reads the column name before every
cell in it. The arrow draws the same state for the eye and is `aria-hidden`.

## Testing

Every story is an axe test, but axe walks straight through a generic `span`
inside a row. A column that lost its `ListCell` and holds only text passes the
axe gate while a screen reader reads it with no header. So
`ui/src/pages/story-harness.tsx` exports `expectListTable(canvasElement, name)`,
which checks the table by name: two rowgroups, a header row of column headers,
and body rows whose every child is a cell, one per column (or one across the
table for a `ListStateRow`). Each list screen's loaded story calls it.

`expectListStateInViewport(canvasElement, name, { says, cta })` is the phone
half (#2362): `toBeVisible` does not see a box a scroll container has pushed
past its own edge, so it measures the state row's title and button (or, with
neither, the skeleton) against the window with `expectInViewport`. It scrolls
the table into view and never the state, because scrolling the title into view
would slide the table sideways and pass on the fault. Pair it with an `atMobile`
story: Users, Providers and the primitive stories below do.

`expectTableStateInFrame(canvasElement, { says, body, cta })` is the same check
for the native `Table` (#2420), which has no accessible name to look it up by:
`says` is the empty title and the helper finds the table from it. It measures
the title, the description and the button against the scroller's own frame with
`expectInFrame` (`ui/src/lib/story-viewport.ts`) and not against the window,
because the frame is narrower than the window by the page gutters and a title the
card's edge had clipped still sat inside the window. The title and the
description are measured by their text, through a `Range`, since the block that
holds a line of centred text is as wide as its row whether or not the text
fits, and the title must also be centred in the frame, which catches a
placeholder that is inside it and off to one side. Cluster, Audit Log and User
Provisioning each assert it at 375px in `en` and `ru` (`EmptyFitsThePhone`,
`EmptyFitsThePhoneInRussian`), and the primitive's stories under **Display/Table**
hold the placeholder in the frame while the table scrolls
(`EmptyStaysInFrameWhenTheTableScrolls`) and as wide as the table and its header
row at desktop width (`EmptySpansTheTableAtDesktopWidth`).

The primitive's own stories, under **Display/ScreenPrimitives**, assert the
roles (`TableSemantics`), the `aria-sort` cycle and the hidden arrow
(`SortIsAnnouncedOnTheHeader`), the loading and empty rows
(`LoadingRowKeepsTheTableWhole`, `NoRows`), and the read they wait on
(`ParkedReadKeepsTheSkeleton`, `NoEmptyRowUntilTheReadSucceeds`). At phone
width `EmptyRowFitsThePhone`, `LoadingRowFitsThePhone` and
`LoadErrorInTheStateRowFitsThePhone` hold the state row in the frame,
`StateRowStaysInFrameWhenTheTableScrolls` scrolls the table to its end and
checks the state has not moved while the header band has, and
`StateRowSpansTheTableAtDesktopWidth` checks the state row, the header and the
body share one left edge and one width when nothing scrolls.

The Logs screen is not built from these primitives: it renders a native
`<table>` and gets its semantics from the elements. It does not scroll to fit
its columns either. Its scroll area is a size container, and Provider, Tokens
and Latency are `display: none` below 840, 720 and 600px of the table's own
width (`@min-[…]` variants on the `th`, the `td` and the `col`), so Time, Model,
Status and Cost are in the frame however much the sidebar, the filter rail and
the detail drawer have taken. The width that decides is the table's, not the
window's (#1986). `TheColumnsFollowTheWidthTheTableHas` and the 375px stories
read the drawn columns and assert no cell sits past the frame. So does the Roles &
Permissions matrix (#2081), which needs what the list primitives do not have: a
row header per resource (`th scope="row"`) and a `tbody` per scope under its own
full-width header. Each of its chips pairs an `aria-hidden` mark with the same
statement in `sr-only` text, so a cell is read as "Read: allowed Create: not
allowed …" with its role and resource announced; the `Loaded` story asserts
that name and the mark's glyph.
