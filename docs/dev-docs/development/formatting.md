# Formatting markdown, MDX, JSON and YAML

Every Rust file in this repository is `rustfmt`-formatted, every TOML file is
`taplo`-formatted, and everything under `ui/` is prettier-formatted against
`ui/.prettierrc`. Until [#1695] the prose and data files outside `ui/` were the
one part of the tree nothing owned: `docs/dev-docs/`, `docs/user-docs/`,
`AGENTS.md`, `README.md` and the ADRs were wrapped, padded and bulleted however
each author or agent happened to type them. A one-word edit to a hand-padded
table showed up in review as a rewritten table.

They are now formatted by prettier too, from a root `.prettierrc` and
`.prettierignore`.

## Running it

```
just fmt-docs           # rewrite
just fmt-docs-check     # check only, the way CI does
```

Both call `scripts/format-docs.sh`, which is also what the
`prettier (markdown / json / yaml outside ui/)` prek hook and the
`docs formatting (prettier)` job in `quality.yml` run. Never invoke prettier
here with ad-hoc flags: the hook, the CI job and a local run all read the same
two config files, and a flag that only one of them passes turns into a diff
nobody can reproduce.

The prettier version is not pinned in a second place. `scripts/format-docs.sh`
reads it out of `ui/package.json`, so the whole repository formats with one
build and a dependabot bump moves both halves at once.

## The style, and how it was chosen

```json
{
  "printWidth": 100,
  "embeddedLanguageFormatting": "off"
}
```

Derived from the tree rather than imposed on it, the same way `ui/.prettierrc`
was — each candidate was applied to all 164 markdown and MDX files outside
`ui/` and scored by the size of the diff it produced:

| `proseWrap` | `embeddedLanguageFormatting` | Files touched | Lines changed |
| ----------- | ---------------------------- | ------------- | ------------- |
| `preserve`  | `auto` (prettier's default)  | 141           | 4840          |
| `always`    | `auto`                       | 162           | 23942         |
| `always`    | `off`                        | 162           | 22423         |
| `preserve`  | `off` — **chosen**           | 129           | 3596          |

The JSON and YAML outside `ui/` were measured separately and cost 2 files and
34 lines, so they are in scope too rather than left as a second unformatted
island.

Three things follow from that table.

- **`proseWrap` stays at prettier's `preserve` default.** `always` rewraps every
  paragraph in the repository — a six times larger diff, and every later prose edit
  reflows its neighbours, which is the failure this was meant to end rather than
  a new form of it. Line wrapping in prose is therefore still the author's
  choice; what is normalized is structure: table padding, list markers,
  emphasis delimiters, heading and fence spacing.
- **`embeddedLanguageFormatting` is `off`.** Prettier's default reaches inside
  fenced code blocks and reformats the JSON, YAML and TSX in them. A code block
  in the docs is a quotation of real code or real config, and rewriting it is
  wrong in both directions: it made a `prometheus.yml` snippet disagree with the
  file it is quoting, and it turned documented JSX fragments such as
  `{keys.error && (<LoadError … />)}` into statements with a trailing semicolon.
  Turning it off also removed the only case where prettier rewrapped prose
  against `proseWrap: preserve` — inside a Mintlify `<Note>` or `<Warning>`,
  whose children it was printing as JSX.
- **`printWidth` is a free choice, so it matches `ui/.prettierrc`.** With
  `proseWrap: preserve` and embedded formatting off, nothing in these files is
  wrapped at all: the diff is byte-identical at 80, 100 and 120 columns. 100 is
  set so there is one number to remember for the repository, not because the
  measurement preferred it.

## What is excluded, and why

`.prettierignore` carries the reasoning per entry. In summary: `ui/` (it has its
own config, ignore file and check step), `charts/rolter/templates/` (Go
templates that end in `.yaml` and no YAML parser can read), `crates/*/CHANGELOG.md`
(release-plz regenerates them), `CLAUDE.md` (a symlink to `AGENTS.md`),
`docs/dev-docs/book/` and the other build outputs.

`.github/workflows/` is excluded for a less obvious reason.
`scripts/check-release-handoff.sh` asserts the release pipeline is still wired by
grepping the workflow files as text, and several of its patterns require a
`needs: [a, b, c]` list to be on one line. Prettier reflows a long one into a
block flow sequence, which changes nothing the runner sees and fails the check
immediately. Until that script parses YAML instead of grepping it (#1723), the
workflows stay unformatted; `actionlint` and `zizmor` already own them.

## The one-time reformat

The reformat is a single commit of its own, separate from the commit that added
the tooling, and its SHA is listed in `.git-blame-ignore-revs`. Configure git
once and `git blame` skips it:

```
git config blame.ignoreRevsFile .git-blame-ignore-revs
```

Gating only _changed_ files was the alternative and was rejected: a
per-file-changed gate leaves the tree permanently half-formatted, so the
"reformatted paragraph" diffs keep arriving one file at a time forever, and the
hook has to grow a notion of which files are in the club.

Nothing in that commit changes what a page means. It was verified by parsing all
164 files before and after with `remark-parse` + `remark-gfm` + `remark-mdx` and
comparing the resulting document trees: headings, link targets, code-block
contents, list nesting and every MDX element name and attribute are identical.
That is the check that matters for the two navs — `docs/dev-docs/SUMMARY.md` for
mdBook and `docs/user-docs/docs.json` for Mintlify — since a reflow that moved a
link target would break the book build or the docs site silently. The
`llms.txt tracks the docs nav` job was confirmed green on the reformatted tree
for the same reason.

[#1695]: https://github.com/rolter-ai/rolter/issues/1695
