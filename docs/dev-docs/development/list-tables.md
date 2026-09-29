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
- **Loading and empty go in a `ListStateRow`.** A skeleton's `role="status"` or
  an empty state's heading and button placed straight in the body is content no
  row owns, which a screen reader reads outside the table and axe fails as
  `aria-required-children`. The header stays on screen above it, which is the
  point: see [loading and empty states](loading-and-empty-states.md).

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

The primitive's own stories, under **Display/ScreenPrimitives**, assert the
roles (`TableSemantics`), the `aria-sort` cycle and the hidden arrow
(`SortIsAnnouncedOnTheHeader`), and the loading and empty rows
(`LoadingRowKeepsTheTableWhole`, `NoRows`).

The Logs screen is not built from these primitives: it renders a native
`<table>` and gets its semantics from the elements.
