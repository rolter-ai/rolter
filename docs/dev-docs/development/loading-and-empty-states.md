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

| State | Component | Lives in |
| --- | --- | --- |
| in flight | `ListSkeleton`, `CardGridSkeleton`, `FormSkeleton`, `PanelSkeleton`, `TableSkeleton`, `StatGridSkeleton` | `ui/src/components/LoadingState.tsx` |
| loaded, no rows | `EmptyState` | `ui/src/components/ui/empty-state.tsx` |
| failed | `LoadError` | `ui/src/components/LoadError.tsx` |

### Loading: the shape of what is coming

Pick the skeleton that matches the content it stands in for — a card grid for a
card grid, field pairs for a form, row bars for a list. The point is that
nothing moves when the data arrives:

```tsx
{connectors.isLoading && <CardGridSkeleton cards={3} height={186} min={380} />}
```

For a list inside a `ListTable`, put the skeleton *inside* the table, under the
header. The column headers are real information — they say what a row will
carry — and taking them away to show a placeholder loses that.

Every shape wraps itself in one `role="status"` region labelled with
`common.loading`, so a screen reader hears one announcement rather than one per
bar, and a story can assert the screen is busy without reaching for a class
name. `story-harness.tsx` exports `expectSkeleton` for exactly that.

**A screen must not render its content shape while a request is in flight.**
That is the `Logs` bug: an empty table and a loading table looked the same.

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
  anything, and no amount of traffic will fill the screen. That is a
  `noAnalytics` [load error](error-states.md), not an `EmptyState`; the
  Dashboard rendered it as the latter until #1236.
- **No CTA where no action exists.** `McpOAuth` grants are created by a user
  completing an OAuth flow in a client; `Cluster` nodes enrol themselves on
  their snapshot poll. Inventing a button for those would be worse than none.
  Where the action lives on *another* screen — a complexity policy needs a route
  first — link there instead.

`Table` takes an `empty` prop rendered in a full-width row, so the placeholder
sits inside the table's border with the column headers above it rather than
floating beneath a header row over nothing.

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
