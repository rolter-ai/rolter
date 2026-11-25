# Module structure lint (deslop-please)

[deslop-please] checks how the workspace's modules depend on each other, after
Ousterhout's _A Philosophy of Software Design_: deep modules behind narrow
interfaces. It reads the SCIP index that `rust-analyzer scip` writes, so it sees
the code the way `cargo check` builds it, macros and build-script output
included. clippy looks at one function at a time; this looks at what one module
knows about another.

It runs as two steps of `quality.yml`'s `rust lint` job, so it gates `ci-ok`
([#3717]).

## The lints

| Lint             | Fires when                                                           | Question to answer before fixing                                                     |
| ---------------- | -------------------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| `module-cycle`   | modules of one crate use each other in a circle                      | which layer of abstraction is each module at? the lower one stops knowing the higher |
| `tangle`         | items (types, functions) know each other in a circle across modules  | same as `module-cycle`, one level down                                               |
| `field-leak`     | a type hides some fields, and code outside it reads or writes others | why do these fields exist, and what do the places that touch them have in common?    |
| `shallow-module` | a module exposes most of its functions                               | can the callers' patterns become a few deeper operations?                            |
| `wide-directory` | a directory holds more than 15 Rust files and directories            | what are the parts about? group them by that                                         |
| `long-file`      | a file has more than 2,000 lines                                     | same as `wide-directory`                                                             |

A `field-leak` only fires on a type that keeps at least one field to itself. A
plain record whose every field is read outside, such as most of the
configuration structs, is not a leak.

The fix is a change in what modules know about each other, never a rename. The
tool follows getters, `pub` fields and code pasted into the caller, so turning
`state.metrics.foo.fetch_add(1)` into `state.metrics.foo().fetch_add(1)` keeps
the error under the getter's name. For a leaked type, give it operations shaped
like what its callers do and fix all of that type's leaks together; the report's
`also known outside` line lists them. For a cycle, move the shared item down
into the lower module, or lift the call that needs both into their parent.

## Running it

```
just deslop          # check, the way CI does
just deslop-update   # rewrite both allowlists after fixing listed debt
```

It needs the binary and rust-analyzer with the standard library's sources:

```
cargo install deslop-please --locked   # or the release tarball CI pins
rustup component add rust-analyzer rust-src
```

rustup leaves a `rust-analyzer` proxy on `PATH` even without the component, so
check with `rust-analyzer --version`, not `command -v`.

The first run of a configuration builds the workspace's build scripts and proc
macros, as `cargo check` does, and caches the index under
`target/deslop-please/`. Measured on an Apple-silicon Mac with a warm `target/`:
about 100 seconds for default features and 165 for `--all-features`. After
that, a run re-indexes only when a source file changed. Exit code 0 is clean,
1 is findings, 2 means the tool itself failed (stderr names cargo or
rust-analyzer).

`deslop-please <crate_or_mod>` narrows the report to one crate or module, which
is the useful form while working on a fix.

## The two allowlists

The debt master had when the lint was adopted is listed, one `<lint> <key>` per
line, in two files:

- `.deslop-please.allow`: default features, what clippy builds
- `.deslop-please.all-features.allow`: `--all-features`, what `cargo doc` builds

Keys are symbol paths (`field-leak rolter_gateway::metrics::Metrics::cache_hits_total`),
not line numbers, so unrelated edits never touch them.

A run with `--allow` fails only on a finding its list does not cover. A cycle
that shrinks stays covered, since its members are a subset of the listed one; a
cycle that gains a module is a new finding. Fixed entries never fail: the run
prints how many listed entries are gone.

There are two lists because the configurations report different things.
`cfg`-gated code changes which fields a type keeps to itself, so a type can leak
under one configuration and be a plain record under the other, and the modules
behind `postgres` and `onnx` add long files, cycles and the three shallow
`rolter_store` modules of their own. One merged list would
make each run report the other configuration's entries as fixed, and the
`--update-allow` command the tool then suggests would delete them.

After fixing listed debt, run `just deslop-update` and check that `git diff` on
both files only removes lines. A line it adds is a new finding: fix it rather
than commit it. Do not add an entry by hand. If a finding still looks wrong
under the right configuration, the tool is young (0.1.x): bring the report
excerpt to the PR and the upstream issue tracker before restructuring code
around it.

## The baseline at adoption

Master at `d7a67c09`:

| Lint             | Default features | All features |
| ---------------- | ---------------- | ------------ |
| `field-leak`     | 211              | 232          |
| `long-file`      | 18               | 34           |
| `module-cycle`   | 5                | 9            |
| `wide-directory` | 4                | 7            |
| `tangle`         | 1                | 2            |
| `shallow-module` | 0                | 3            |
| total            | 239              | 287          |

`rolter-gateway` carries 168 of the default-feature findings, most of them
fields of `Metrics`, `Snapshot` and `AppState` that the request handlers reach
into, plus one module cycle through `handlers`, `state` and nine other modules.
Under `--all-features`, 21 `rolter-control` modules around `crud`, `auth` and
`rbac` form one more cycle.

Paying this down is tracked in epic [#3719], one child issue per cycle, per
leaking type or per crate's layout. A PR that fixes listed debt names the child
it closes and shrinks both lists with `just deslop-update`.

## Bumping the tool

CI pins the version and the sha256 of the `x86_64-unknown-linux-musl` release
tarball in the `install deslop-please` step, the same way it pins actionlint: a
release that adds a lint must not fail every open PR with no change in this
repository. To bump, change both values (the digest is the release's
`<tarball>.sha256`), run `just deslop-update` with the new version installed
locally, and commit the workflow and both lists in one PR.

[deslop-please]: https://github.com/deterministic-machines/deslop-please
[#3717]: https://github.com/rolter-ai/rolter/issues/3717
[#3719]: https://github.com/rolter-ai/rolter/issues/3719
