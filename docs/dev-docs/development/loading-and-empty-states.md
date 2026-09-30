# Dashboard loading and empty states

A screen has four things it can be showing: its data, a placeholder for data
that is still coming, a placeholder for data that came back with nothing in it,
and [an error](error-states.md). Before #1180 the dashboard had one shared
component for the last of those and hand-rolled the other two, twenty-one and
eighteen times respectively.

That is not just inconsistency. The three failures it produced were each
concrete:

- **`Loading…` on one line.** Untranslated, so it was the same English word in
  every locale, and one line tall, so the layout jumped the moment the rows
  landed. It also said nothing about how much was coming — one row or forty
  read identically.
- **A screen with no loading state at all.** `Logs` and `Rbac` rendered their
  empty shape while the request was in flight, so a slow ClickHouse read was
  indistinguishable from a deployment that had served no traffic.
- **An empty state that blamed a filter nobody set.** `ProviderGroups` said
  "No provider groups match." with no search running. The reader's next move is
  to clear a search that does not exist.

## The three primitives

| State           | Component                                                                                                | Lives in                               |
| --------------- | -------------------------------------------------------------------------------------------------------- | -------------------------------------- |
| in flight       | `ListSkeleton`, `CardGridSkeleton`, `FormSkeleton`, `PanelSkeleton`, `TableSkeleton`, `StatGridSkeleton` | `ui/src/components/LoadingState.tsx`   |
| loaded, no rows | `EmptyState`                                                                                             | `ui/src/components/ui/empty-state.tsx` |
| failed          | `LoadError`                                                                                              | `ui/src/components/LoadError.tsx`      |

### Loading: the shape of what is coming

Pick the skeleton that matches the content it stands in for — a card grid for a
card grid, field pairs for a form, row bars for a list. The point is that
nothing moves when the data arrives:

```tsx
{connectors.isLoading && <CardGridSkeleton cards={3} height={186} min={380} />}
```

For a list inside a `ListTable`, put the skeleton _inside_ the table, under the
header. The column headers are real information — they say what a row will
carry — and taking them away to show a placeholder loses that. Inside the table
it goes in a `ListStateRow`, and so does the empty state: a `role="status"` or
a button placed straight in the table body belongs to no row, which breaks the
table for a screen reader (see [list tables](list-tables.md)):

```tsx
<ListLoadingRow read={keys}>
  <ListSkeleton rows={4} className="p-3" />
</ListLoadingRow>
```

`ListLoadingRow` and its sibling `ListEmptyRow` take the query itself and decide
from it, for the reason the next rule gives.

The state row is as wide as the part of the table the reader sees, not as its
column floor (#2362). Below the floor the table scrolls sideways, and a row the
width of the floor put the empty title and its call to action centred past the
right edge of a 375px card, so a phone showed a header over a blank body. The
row sticks to the frame's left edge and fills it; see
[list tables](list-tables.md) for how, and for the story that measures it.

Every shape wraps itself in one `role="status"` region labelled with
`common.loading`, so a screen reader hears one announcement rather than one per
bar, and a story can assert the screen is busy without reaching for a class
name. `story-harness.tsx` exports `expectSkeleton` for exactly that.

**A screen must not render its content shape while a request is in flight.**
That is the `Logs` bug: an empty table and a loading table looked the same.

**An empty state needs a successful answer.** Gate it on `isSuccess`, not on
`!isLoading && !error`. The two differ exactly when react-query parks a retry:
in a hidden tab, or with the browser offline, a failed first attempt leaves the
query `pending` but not fetching, so it is neither loading nor failed. In that
window LLM Logs said "Nothing logged yet" about a load that had failed (#1984).
For the same reason the skeleton keys on `isPending`, which covers the parked
retry, rather than `isLoading`, which does not. `isAwaiting` in
`ui/src/lib/read-state.ts` is that test with one refinement: a query that is
disabled (a screen with no org to read yet) is also `isPending`, forever, so it
asks for `isPending` with a fetch that is running or parked. `isEmptyAnswer` is
the empty-state half. The list-table rows, `Table` and the summary below are
built on the two, so a screen hands over its query rather than restating them.

### A read that has not answered holds no rows either

The rule above came from one screen; the review for #2049 found seventeen
others breaking it (#2211). A list screen derived its placeholder from the rows
(`!query.isLoading && rows.length === 0`), and a failed read holds exactly as
many rows as an empty one. So Providers, Users, Keys, Models, Provider Groups,
User Provisioning and the Audit Log said "No X yet", often beside a create
button, directly under the `LoadError` explaining that the list could not be
read. On the evidence screen that is a false statement. An operator reads an
outage or a permissions problem as "nothing configured", and may create a
duplicate.

Counts had the same fault in a smaller type: `data?.length ?? 0` printed
"0 teams", "All 0" and "0 of 0 routes" while the read was in flight and again
after it failed.

The pieces that carry the rule:

- **`ListEmptyRow` / `ListLoadingRow`** (`ui/src/components/screen.tsx`) —
  `<ListEmptyRow read={query} rows={rows.length}>` renders its `EmptyState`
  only on a successful answer. `rows` is what survived the screen's filters,
  so a search that matched nothing still gets its no-match copy.
- **`Table`** takes `read` beside `empty`, and the type will not accept one
  without the other.
- **`ListSummary`** is the count beside a list: `<ListSummary data={query.data}>`
  hands its render function the data, so there is no `?? 0` left to write, and
  renders nothing until the data is held. A summary that also explains the
  screen passes a `fallback` with the explanation and no count, as User
  Provisioning does for its SCIM lead. A failed refetch keeps the rows on
  screen, so the count keys on the data rather than on `isSuccess` and stays
  with them.
- **Anything derived from the list waits too.** A card that reads "No spend in
  this window" when its unit has no row in the rollup only says so once the
  rollup succeeded (Cost Attribution, #2105); an Export CSV or a "collector
  config" button that renders the list waits for a list that answered.

A card-grid screen with no `ListTable` gates its `EmptyState` the same way:
`query.isSuccess && query.data.length === 0`.

A screen made of cards that each read their own query gives every card all three
states (#1976). The Dashboard's `CardRead` decides from `isAwaiting` and the
read's data before it lets a card draw anything: a skeleton in the card's own
shape, then either its own `LoadError` or the content. The empty copy sits
inside the content, where the read is known to hold data, so an empty answer
still says so after a failed refresh. When every read has failed holding nothing
the Dashboard draws no cards and shows one `LoadError` for the screen
([error states](error-states.md)). An average or a rate over an empty window
is undefined rather than zero: the Dashboard's latency and error-rate tiles read
"—" with "No requests in this window" under them, where "0 ms" and "0.00 %"
claimed a measurement of a quiet deployment.

### Empty: what it is, and what to do about it

An `EmptyState` carries an icon, a title, one sentence of description, and —
wherever the screen has a control that would create the missing thing — an
`actions` button that opens it:

```tsx
<EmptyState
  uxTarget="connectors"
  icon={<Cable />}
  title={t("pages.connectors.emptyTitle")}
  description={t("pages.connectors.emptyBody")}
  actions={<Button onClick={() => setAddOpen(true)}>{t("pages.connectors.emptyAction")}</Button>}
/>
```

Two rules the wording depends on:

- **"Nothing here" and "nothing matched" are different answers.** They want
  different sentences and different buttons: one offers to create the first row,
  the other offers to clear the filter. Screens with a search or a filter bar
  branch on whether one is actually active. Deriving the copy from the row count
  alone is what produced "No provider groups match." on a screen with no query.
- **A deployment answer is not an empty state.** A control plane with no
  ClickHouse has not "served nothing yet" — it was never asked to record
  anything, and no amount of traffic will fill the screen. That is an
  `AnalyticsUnavailableError`, not an `EmptyState`; the Dashboard rendered it as
  the latter until #1236. Nor is it an outage: LLM Logs, the Dashboard, MCP
  Logs, Cost Attribution's spend strip and Account's usage figures all show it
  as the informational `AnalyticsUnavailable` panel rather than a red alert
  (#1984, #1976, #2016; see [error states](error-states.md)).
- **No CTA where no action exists.** `McpOAuth` grants are created by a user
  completing an OAuth flow in a client; `Cluster` nodes enrol themselves on
  their snapshot poll. Inventing a button for those would be worse than none.
  Where the action lives on _another_ screen — a complexity policy needs a route
  first — link there instead.

`Table` takes an `empty` prop rendered in a full-width row, so the placeholder
sits inside the table's border with the column headers above it rather than
floating beneath a header row over nothing. It comes with `read`, the query the
rows came from, and renders only once that read succeeded.

An empty result is never routed through `LoadError`; see
[error states](error-states.md) for why.

## Copy and stories

Every string goes through the catalogs as `pages.<screen>.emptyTitle`,
`.emptyBody` and `.emptyAction` (`noMatchTitle` / `noMatchBody` for the
filtered variant), in **every** locale under `ui/src/lib/i18n/locales/`; see
[i18n](i18n.md). Loading needs no per-screen copy — the shapes share
`common.loading`.

Each touched screen's stories cover all three states with play assertions.
`story-harness.tsx` supplies `expectSkeleton`, `expectEmptyState` (which checks
the CTA is there) and `expectLoadError` / `expectForbidden`, so a story asserts
the state rather than a sentence that is free to be reworded.

The error and loading stories also call `expectNoFalseEmpty(canvasElement,
/No teams yet/)`: no empty state with that title, and no count of zero anywhere
on the screen. It asserts an absence, and absent is also what a screen looks
like before it rendered, so it goes after `expectLoadError` or
`expectSkeleton`. A story whose list renders after some other read has to wait
for the list first: Cost Attribution's spend stories wait for the roster before
asserting that no card claims "no spend".
