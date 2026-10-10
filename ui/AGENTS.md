# ui/AGENTS.md

Dashboard-specific guidance, loaded when working under `ui/`. The repository-wide rules in the root `AGENTS.md` still apply. `CLAUDE.md` in this directory is a symlink to this file.

## Dashboard design

The product and visual record lives at the repository root: `PRODUCT.md` (who
the dashboard is for, the constraints) and `DESIGN.md` (the tokens and rules,
derived from `ui/src/index.css`). Read both before building or reshaping a
screen, and build against the tokens. Never hard-code a hex or font the tokens
already carry.

The design skill is [impeccable](https://github.com/pbakaus/impeccable),
declared for the project in `.claude/settings.json`. It loads both files before
UI work, and its detector hook checks each edit to a UI file. Plan a new screen
with `/impeccable shape <screen>`, review one with `/impeccable critique <screen>`
and `/impeccable audit <screen>`, finish it with `/impeccable polish <screen>`,
and refresh `DESIGN.md` with `/impeccable document` when a token or primitive
changes. Setup and triage rules are in
`docs/dev-docs/development/impeccable.md`.

The repository declares only generic, open-source skills like this one. Skills
native to one vendor's product (Anthropic's `frontend-design` plugin, Claude
Design and its DesignSync) stay out of the repository and its instructions;
install them for yourself if you want them.

When working on dashboard UI, consult the project MCP server
(`rolter-storybook` in `.mcp.json`) before writing components:

- run `docs-list` first to discover available primitives and their doc ids
- run `docs-show` / `docs-show-story` before using component props
- run `get-storybook-story-instructions` before creating or editing stories
- run `stories-find-by-component` and `stories-preview` after changing how UI
  looks, and include the returned URLs in your reply — but only when your own
  checkout is the one serving port 6006 (see below)

The story tests themselves are not an MCP tool: run them with
`bun run test:stories <files>`, which runs vitest against your checkout (it starts
its own server, so the Storybook on port 6006 is neither needed nor used).

That MCP server _is_ the Storybook dev server on port 6006, so it has to be
listening before the agent session starts — a session that begins with port
6006 down has no `rolter-storybook` tools, and starting Storybook mid-session
does not attach them on its own; reconnect the server from `/mcp` or start a
new session. The `post-start` hook in `.config/wt.toml` starts it for every new
worktree and tears it down with the worktree, so this is handled as long as
hooks are approved (`wt config approvals add`). The project `SessionStart` hook
in `.claude/settings.json` (`.claude/hooks/start-storybook.sh`) covers the rest:
when nothing listens on 6006 and `ui/node_modules` exists it starts the server
detached, logging to `.claude/storybook-session.log`, and returns at once; it
says nothing when the server is already up. Because MCP servers attach while a
session starts, that server is there for the next session, not the one that
started it. To do it by hand, run in `ui/` before launching the session:
`nohup bun run storybook --ci --no-open --exact-port &`.

Only one checkout can hold port 6006 at a time; the first worktree to start
takes it and the hook fails fast in the others. The docs tools describe the
shared primitives, which rarely differ between checkouts, but `stories-preview`
and `stories-changed` describe the checkout that is serving, not necessarily
yours. For that reason the `rolter-ui` subagent (`.claude/agents/rolter-ui.md`)
is given the three `docs-*` tools and `get-storybook-story-instructions` only.

This applies to every state a screen has, empty/loading/error included. Assets
stay vendored locally — the dashboard must work air-gapped, so no runtime CDN
fonts or images.

Every user-facing string goes through the i18n catalogs (`en` is the base, `ru`
ships beside it) — `t("pages.<screen>.<key>")`, never a literal in JSX — and
numbers, money and dates go through `useFormat()` rather than a bare
`toLocaleString()`, which silently follows the browser locale instead of the
dashboard's. `bun run check:i18n` fails on a key that is missing, orphaned,
empty, short a plural form, or that dropped an interpolation placeholder. See
`docs/dev-docs/development/i18n.md` for key naming and how to add a locale.

## Maintenance matrix (dashboard)

When you change the thing in bold, the entries after it must change with it.

- **Added a dashboard screen** — add `ui/src/pages/<Screen>.tsx`; register the route in `ui/src/App.tsx` as a lazy entry — `screen(() => import("@/pages/<Screen>"))`, never a static import, or the screen rejoins the entry chunk everyone downloads at sign-in (#1709; `ui/src/lib/screens.test.ts` fails on one); add the nav entry in `ui/src/lib/nav.tsx`; add `nav.<key>` and `screens.<key>.title`/`.subtitle` to **every** catalog in `ui/src/lib/i18n/locales/`; a screen is not done until all of its copy is in the catalogs and translated, which `bun run check:literals` enforces; add a `.stories.tsx` and run it with `bun run test:stories <file>`, which also fails a run that picked the file up for fewer tests than it has stories (the bare `bun run test-storybook` is the same vitest run without that check); cover empty/loading/error states; fake the API inside that story with the fetch stubs from `ui/src/pages/story-harness.tsx` (`Harness` around `routes`/`scoped`/`json`, `pending` for loading, `recording` to assert what was sent) — there is no shared mock module, see `docs/dev-docs/development/testing.md`
- **Added a dashboard loading, empty or error state** — never hand-roll one: a skeleton from `ui/src/components/LoadingState.tsx`, `EmptyState` with an `actions` CTA wherever the screen can create the missing row, `LoadError` with an `errors.resources.*` noun and a `target` naming the region, which records `error_state` itself, so never add a `useErrorState` beside it (#2444); branch the empty copy on whether a filter is actually active; gate the empty state and any count on the read, never on the rows — a failed or pending read holds no rows either, so derive nothing from `data?.length ?? 0` and write a count beside a list as `ListSummary` (#2211); cover all three in the screen's story with `expectSkeleton` / `expectEmptyState` / `expectLoadError`, and follow the error and loading assertions with `expectNoFalseEmpty`; a read that can answer `AnalyticsUnavailableError` (no ClickHouse) is a supported deployment, so state it with `AnalyticsUnavailable` and the screen's own `noAnalytics` copy, asserted by `expectAnalyticsUnavailable`, never as a `LoadError` alert (#2016); see `docs/dev-docs/development/loading-and-empty-states.md` and `docs/dev-docs/development/error-states.md`
- **Added a dashboard dropdown** — use `Combobox` from `ui/src/components/ui/combobox.tsx` — never a bare `<select>`, whose open list is drawn by the operating system and ignores the tokens (there is no `Select` wrapper any more, #968 removed it); label it with `Field`, put any new copy under `common.combobox.*` in **every** catalog, and assert the listbox roles and the keyboard in the story. `bun run check:primitives` fails on a bare `<select>`. See `docs/dev-docs/development/combobox.md`
- **Added a dashboard list table, or a column to one** — build it from the list-table primitives in `ui/src/components/screen.tsx`: a `ListTable` whose `label` is the screen's `t("screens.<key>.title")`, `ListHeaderCell` / `SortLabel` / `ListActionsHeader` inside the `ListHeader`, a `ListCell` for every child of a `ListRow` (`className="grid"` when it wraps a component rather than replacing a `span`), and the loading and empty states in `ListLoadingRow` / `ListEmptyRow`, which take the screen's query and wrap a `ListStateRow`; a bare `span` in a row is read with no column header and axe misses one that holds only text, so the screen's loaded story calls `expectListTable` (#2000); the grid's first column is the one a row is found by, so write it `${primaryColumn(<weight>)}` and never a bare `fr` (`scripts/list-grids.test.ts` fails it, #2812), and a `truncate` cell needs no `title` of its own, the table names what it clipped. See `docs/dev-docs/development/list-tables.md`
- **Added an audited action or target type in `crates/rolter-control`** — regenerate the Audit Log filters' list with `bun run gen:audit` and commit `ui/src/lib/audit-vocabulary.json`; `ui/scripts/audit-vocabulary-source.test.ts` fails while it disagrees with the source, and fails on an action `auditGroup()` in `ui/src/lib/audit-vocabulary.ts` cannot place under a group (add its family there, #2127). A page the new target type lives on goes in `TARGET_PATH` in `ui/src/pages/AuditLog.tsx`
- **Added a floating panel, popover or actions menu** — never position or dismiss one by hand: a surface that floats beside or under a control is `AnchoredPanel` from `ui/src/components/ui/anchored-panel.tsx` (fixed, placed from the anchor's rectangle so it escapes `overflow`, closed by a press outside, Escape and focus leaving, with Escape closing only the panel on top), and a short list of verbs behind one control is `Menu` with `MenuItem` / `MenuItemRadio` (an exclusive choice, like the language picker) / `MenuSeparator` from `ui/src/components/ui/menu.tsx` (a real `role="menu"`: arrows, Home/End, Tab closes, Escape returns focus). A gated entry is `<MenuItem gate="resource:action" control="<noun>-<verb>">`, refused as a real `disabled` button that prints the role it needs under its label; assert it with `expectRefused(…, "menuitem")` / `expectAllowed(…, "menuitem")`. Assert the keyboard path and Escape in the story, in both rail states where the control lives in the rail; see `docs/dev-docs/development/dashboard-navigation.md`
- **Hand-wrote dashboard markup a second time** — `bun run check:primitives` also fails the same intrinsic element with the same literal `className` in three or more files (#1686) — extract the primitive into `ui/src/components/ui/`, give it a story and import it; when the extraction has to wait, record the shape with its reason in `ui/scripts/repeated-shapes-allowlist.ts`, which only ever shrinks. See `docs/dev-docs/development/form-primitives.md`
- **Added a dashboard surface showing JSON, YAML, TOML, CSV or logs** — never a raw `<pre>`: render it with `CodeBlock` from `ui/src/components/ui/code-block.tsx`, which owns the focusable scroll region, the copy button and the palette; pass a `label` wherever more than one block shares a screen; a new language means a grammar in `ui/src/lib/code-highlight.ts` plus an entry in `CODE_LANGUAGES`, a story and a `--code-*` rule; `bun run check:primitives` fails on a raw `<pre>` — see `docs/dev-docs/development/highlighting.md`
- **Added a snippet, example or placeholder that hands out a gateway URL** — build the address with `useGatewayBase()` from `ui/src/lib/use-gateway-base.ts`, or `gatewayBase()` from `ui/src/lib/gateway.ts` where the saved value is already in hand, never from `window.location.origin`: the control plane serves the gateway only under `/gw/*`, and the public base URL saved on Client Settings wins over both (#2218); assert the address in the surface's story. See `docs/dev-docs/development/highlighting.md#the-gateway-address-in-a-snippet`
- **Added or re-worded dashboard copy** — put the string in `ui/src/lib/i18n/locales/en.json` and translate it in every sibling catalog in the same PR; `bun run check:i18n` and `bun run check:literals` are both merge gates. Never hardcode user-facing English in a component — `check:literals` enforces it with no baseline; only notation that is never translated goes in `ui/src/lib/i18n/literals-allowlist.ts`, with its reason; see `docs/dev-docs/development/i18n.md`
- **Added a story that asserts focus** — wrap it in `waitFor` whenever the focus was moved by a dialog, drawer or sheet opening or closing — those hand focus over from an effect, a step after the element enters or leaves the document, so an assertion with no retry reads a value that may not be written yet and passes only by timing (#1675). Straight after `.focus()` or a `userEvent` call it needs no waiter. `bun run check:focus` enforces it; #1672's 5s `asyncUtilTimeout` does not help here, since an assertion that never polls waits zero milliseconds at any budget
- **Added a story that asserts a gated control or a sheet's rows** — never assert it once: a gated control renders **enabled** until `/api/v1/rbac/effective` answers, so use `expectRefused` rather than a bare `toBeDisabled()` and `expectAllowed` rather than a `toBeEnabled()` — enabled is the state the control starts in, so that one is already true before the gate speaks and a `waitFor` around it changes nothing (#1707) — and a sheet fires its own query as it opens, so its rows want `await findByRole(…)` and not a `getByRole` on the line after the dialog appeared (#1689). `bun run check:waits` fails on all three; a control disabled by its own prop from the first paint carries `// story-wait-allow: <reason>` above the line. See `docs/dev-docs/development/testing.md`
- **Added a story that asserts `React.StrictMode` behaviour** — render the `StrictMode` with the subject absent and mount the subject from the play function (a click that flips the host's state). A `<React.StrictMode>` rendered in the same commit as its subject double-invokes no effects, so the assertion passes against broken code (#1744). Anchor any "did not happen" assertion on something that can only follow the remount, and run the story once against deliberately broken code. See `docs/dev-docs/development/testing.md#a-strictmode-story-mounts-its-subject-in-a-later-commit-1744`
- **Added a destructive dashboard action** — back it with `ui/src/components/ConfirmDialog.tsx` — never `window.confirm`; name the row in the title and state the consequence in the body; put both under `pages.<screen>.confirm.*` in **every** catalog; keep the dialog mounted with `open={!!target}` and reset the mutation on close; cover confirm → pending → done in the screen's story, with the `form_submit`/`form_abandon`/`save_confirmed` rows; `bun run check:primitives` fails on `window.confirm`/`alert`/`prompt` and on a `DialogFooter` holding a destructive button (#1760); see `docs/dev-docs/development/destructive-actions.md`
- **Showed a value someone copies and can copy again (an address, an id)** — never a hand-built box beside a `CopyButton`: use `CopyableValue` from `ui/src/components/ui/copyable-value.tsx`, or `CopyableText` where the label is already there (`variant="inline"` in a description list); it owns the mono, `select-all`, wrapping value and the copy button. An address on the control plane's public base is `PublicUrlValue` (`ui/src/components/PublicUrlValue.tsx`), which owns the pending, failed and unset-`ROLTER_PUBLIC_URL` states and their wording (#2418, #2366)
- **Showed a value the control plane shows once (a key, a token, an invitation link)** — never lay the reveal out by hand: use `SecretRevealDialog` from `ui/src/components/ui/secret-reveal.tsx`, or `SecretValue` and `useSecretCloseGuard` where the reveal lives inside a sheet. It owns the copy button, the failed-copy message that stays, the value selectable in one click, and the "Close without copying?" question that Escape, the scrim, the close button and Done all raise while the value is uncopied; put the step that comes next in its `children` (`KeyNextStep` for a key, which takes the gateway address from `useGatewayBase()`). Cover copy succeeds, copy fails and close-uncopied in the screen's story with `stubClipboard` and `answerSecretClosePrompt` from `story-harness.tsx`; see `docs/dev-docs/development/secret-reveal.md`
