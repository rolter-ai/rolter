# Dashboard design context for agents (impeccable)

[impeccable](https://github.com/pbakaus/impeccable) is a design skill for coding agents. Before it
touches UI it loads two files from the repository root, and it ships a detector that flags design
anti-patterns and drift from the design system after each edit. rolter uses it so every agent that
works on the dashboard starts from the same product and visual record.

## What is in the repository

| File                      | What it holds                                                                                                     | Who changes it                                               |
| ------------------------- | ----------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| `PRODUCT.md`              | Users, positioning, operating context, constraints, voice. No visual decisions                                    | `/impeccable init` when product truth changes                |
| `DESIGN.md`               | The visual system: tokens in YAML frontmatter, then prose and named rules                                         | `/impeccable document` after the tokens or primitives change |
| `.impeccable/design.json` | Sidecar to `DESIGN.md`: tonal ramps, shadows, motion, breakpoints, component snippets for impeccable's live panel | regenerated together with `DESIGN.md`                        |
| `.impeccable/config.json` | Project settings: the detector hook is on, detector ignores                                                       | `impeccable hooks …` only, never by hand                     |

`DESIGN.md` is derived from `ui/src/index.css`, `ui/tailwind.config.js` and
`ui/src/components/ui/`. When it disagrees with the code, the code wins: fix the drift, then
refresh the file. The reasoning behind each token and its contrast numbers stays in
[Dashboard theme](dashboard-theme.md), and the mark and wordmark rules stay in the
[brand guidelines](../../user-docs/community/brand.mdx). `DESIGN.md` points at both rather than
restating them.

Per-developer state is gitignored: `.impeccable/config.local.json` (your consent to run the hook),
the detector's caches, live-mode sessions and critique reports.

## Setting it up on a machine

Install the plugin once per machine, at user scope:

```bash
claude plugin marketplace add pbakaus/impeccable
```

```bash
claude plugin install impeccable@impeccable
```

Start a new session so the skill and its hooks load, then record your consent to run the detector
in this checkout:

```bash
"$HOME/.claude/plugins/cache/impeccable/impeccable/<version>/skills/impeccable/scripts/impeccable" hooks on
```

The shared config already has the hook enabled, so on a new machine `hooks on` only adds the
consent to the gitignored local file. The
launcher downloads its engine binary from the project's GitHub releases on first use and refuses
to run it unless the `.sha256` sidecar matches.

## Using it

- `/impeccable critique <target>` and `/impeccable audit <target>` review a screen: critique for
  hierarchy, clarity and fit with `PRODUCT.md`, audit for accessibility, performance and
  responsive behaviour. Findings become issues on the board like any other out-of-scope find.
- `/impeccable polish`, `harden`, `clarify`, `layout`, `typeset` and the rest refine an existing
  screen. The full list is `/impeccable` with no argument.
- Screen work still starts with `/frontend-design:frontend-design rolter` (see `ui/AGENTS.md`);
  impeccable reads the same `DESIGN.md` and checks the result.

The detector runs on every `Edit`/`Write` to a UI file and prints a short reminder when it finds a
mechanical problem: clipped content, a contrast failure, gradient text, a colour that is not a
token. A deeper pass runs once when the session stops. Triage a finding the way the skill's
`reference/hooks.md` describes: fix a real problem; record a confirmed
false positive with the narrowest `impeccable hooks ignore-value … --reason "…"`; ask when unsure.
Never add an ignore to get past a finding you have not looked at.

A one-off scan without the hook:

```bash
"$HOME/.claude/plugins/cache/impeccable/impeccable/<version>/skills/impeccable/scripts/impeccable" detect ui/src/pages/Models.tsx
```
