# ui/AGENTS.md

Dashboard-specific guidance, loaded when working under `ui/`. The repository-wide rules in the root `AGENTS.md` still apply. `CLAUDE.md` in this directory is a symlink to this file.

## Dashboard design

Before building or reshaping any dashboard screen, run the design skill:

```
/frontend-design:frontend-design rolter
```

It sets the aesthetic direction — palette, typography, layout — so a screen is a
deliberate call for rolter rather than shadcn defaults. It composes with the
existing rolter design system (DesignSync / the Claude Design project), which
supplies the tokens and primitives the dashboard already ships: run the skill
first, then build against the tokens. Never hard-code a hex or font the tokens
already carry.
When working on dashboard UI, consult the project MCP server
(`rolter-storybook` in `.mcp.json`) before writing components:

- run `list-all-documentation` first to discover available primitives
- run `get-documentation` / `get-documentation-for-story` before using
  component props
- run `get-storybook-story-instructions` before creating or editing stories
- run `preview-stories` after generating UI or stories, and include the
  returned URLs in your reply

That MCP server _is_ the Storybook dev server on port 6006, so it has to be
listening before the agent session starts — a session that begins with port
6006 down has no `rolter-storybook` tools for its entire lifetime, and starting
Storybook mid-session does not attach them. The `post-start` hook in
`.config/wt.toml` starts it for every new worktree and tears it down with the
worktree, so this is handled as long as hooks are approved
(`wt config approvals add`). Outside a Worktrunk worktree, start it yourself
with `bun run storybook` in `ui/` before launching the session. Only one
worktree can hold port 6006 at a time.

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

- **Added a dashboard screen** — add `ui/src/pages/<Screen>.tsx`; register the route in `ui/src/App.tsx` as a lazy entry — `screen(() => import("@/pages/<Screen>"))`, never a static import, or the screen rejoins the entry chunk everyone downloads at sign-in (#1709; `ui/src/lib/screens.test.ts` fails on one); add the nav entry in `ui/src/lib/nav.tsx`; add `nav.<key>` and `screens.<key>.title`/`.subtitle` to **every** catalog in `ui/src/lib/i18n/locales/`; a screen is not done until all of its copy is in the catalogs and translated, which `bun run check:literals` enforces; add a `.stories.tsx` and run it with `bun run test:stories <file>`, never `storybook dev` plus `test-storybook` by hand — a taken port makes the second pass against another worktree's build and report green (#1684); cover empty/loading/error states; fake the API inside that story with the fetch stubs from `ui/src/pages/story-harness.tsx` (`Harness` around `routes`/`scoped`/`json`, `pending` for loading, `recording` to assert what was sent) — there is no shared mock module, see `docs/dev-docs/development/testing.md`
- **Added a dashboard loading, empty or error state** — never hand-roll one: a skeleton from `ui/src/components/LoadingState.tsx`, `EmptyState` with an `actions` CTA wherever the screen can create the missing row, `LoadError` with an `errors.resources.*` noun; branch the empty copy on whether a filter is actually active; cover all three in the screen's story with `expectSkeleton` / `expectEmptyState` / `expectLoadError`; see `docs/dev-docs/development/loading-and-empty-states.md` and `docs/dev-docs/development/error-states.md`
- **Added a dashboard dropdown** — use `Combobox` from `ui/src/components/ui/combobox.tsx` — never a bare `<select>`, whose open list is drawn by the operating system and ignores the tokens (there is no `Select` wrapper any more, #968 removed it); label it with `Field`, put any new copy under `common.combobox.*` in **every** catalog, and assert the listbox roles and the keyboard in the story. `bun run check:primitives` fails on a bare `<select>`. See `docs/dev-docs/development/combobox.md`
- **Hand-wrote dashboard markup a second time** — `bun run check:primitives` also fails the same intrinsic element with the same literal `className` in three or more files (#1686) — extract the primitive into `ui/src/components/ui/`, give it a story and import it; when the extraction has to wait, record the shape with its reason in `ui/scripts/repeated-shapes-allowlist.ts`, which only ever shrinks. See `docs/dev-docs/development/form-primitives.md`
- **Added a dashboard surface showing JSON, YAML, TOML, CSV or logs** — never a raw `<pre>`: render it with `CodeBlock` from `ui/src/components/ui/code-block.tsx`, which owns the focusable scroll region, the copy button and the palette; pass a `label` wherever more than one block shares a screen; a new language means a grammar in `ui/src/lib/code-highlight.ts` plus an entry in `CODE_LANGUAGES`, a story and a `--code-*` rule; `bun run check:primitives` fails on a raw `<pre>` — see `docs/dev-docs/development/highlighting.md`
- **Added or re-worded dashboard copy** — put the string in `ui/src/lib/i18n/locales/en.json` and translate it in every sibling catalog in the same PR; `bun run check:i18n` and `bun run check:literals` are both merge gates. Never hardcode user-facing English in a component — `check:literals` enforces it with no baseline; only notation that is never translated goes in `ui/src/lib/i18n/literals-allowlist.ts`, with its reason; see `docs/dev-docs/development/i18n.md`
- **Added a story that asserts focus** — wrap it in `waitFor` whenever the focus was moved by a dialog, drawer or sheet opening or closing — those hand focus over from an effect, a step after the element enters or leaves the document, so an assertion with no retry reads a value that may not be written yet and passes only by timing (#1675). Straight after `.focus()` or a `userEvent` call it needs no waiter. `bun run check:focus` enforces it; #1672's 5s `asyncUtilTimeout` does not help here, since an assertion that never polls waits zero milliseconds at any budget
- **Added a story that asserts a gated control or a sheet's rows** — never assert it once: a gated control renders **enabled** until `/api/v1/rbac/effective` answers, so use `expectRefused` rather than a bare `toBeDisabled()` and `expectAllowed` rather than a `toBeEnabled()` — enabled is the state the control starts in, so that one is already true before the gate speaks and a `waitFor` around it changes nothing (#1707) — and a sheet fires its own query as it opens, so its rows want `await findByRole(…)` and not a `getByRole` on the line after the dialog appeared (#1689). `bun run check:waits` fails on all three; a control disabled by its own prop from the first paint carries `// story-wait-allow: <reason>` above the line. See `docs/dev-docs/development/testing.md`
- **Added a destructive dashboard action** — back it with `ui/src/components/ConfirmDialog.tsx` — never `window.confirm`; name the row in the title and state the consequence in the body; put both under `pages.<screen>.confirm.*` in **every** catalog; cover confirm → pending → done in the screen's story; `bun run check:primitives` fails on `window.confirm`/`alert`/`prompt`; see `docs/dev-docs/development/destructive-actions.md`
