# API stability and the semver gate

`quality / semver-checks` was red on every run, including on `master` and on
pull requests that touched no Rust at all (#1213). The failure was real but it
said nothing anyone could act on:

```
--- failure struct_pub_field_missing: pub struct field removed or renamed ---
  field allow_direct_provider_keys of struct SecuritySettings, previously in
  rolter-store-0.1.0/src/postgres/models.rs:726
```

A check that is red on a clean tree is worse than no check. Everybody learns to
scroll past it, and the one time it catches something real it is indistinguishable
from the noise.

## Why it was always red

Two causes stacked.

**The baseline was the published crate.** `cargo-semver-checks` diffed the
working tree against `rolter-store-0.1.0` on crates.io, so every pre-1.0 change
to a public item in a published crate was reported.

**The version on `master` is the released version.** release-plz bumps versions
*at release time*, in the release PR — so between two releases the tree always
claims to be the version it is being compared against. `cargo-semver-checks`
reads that as "no change; assume minor", and under 0.x semantics a minor bump
does not license a breaking change. Any breaking change is therefore red from
the moment it merges until the next release, no matter how deliberate it was.

That is not a bug in the tool. It is what happens when a gate is pointed at a
surface that has not been declared stable yet.

## What the job checks now

The gate is scoped instead of silenced. It compares against the previous
release **tag** (`--baseline-rev`, resolved with `git describe`), not the
published crate, and it checks only the crates listed in `GUARDED` in
`.github/workflows/quality.yml`:

| Crate | Guarded | Why |
|---|---|---|
| `rolter-auth` | yes | virtual keys, roles and access checks; consumed by both planes |
| `rolter-balancer` | yes | in-tree strategies all implement one `LoadBalancer` trait, so a change to it reaches every strategy at once |
| `rolter-gateway` | yes | data-plane surface; changes here are behavioural, not structural |
| `rolter-proxy` | yes | provider dialect adapters |
| `rolter-core` | no | config types churn with every new provider kind and strategy |
| `rolter-store` | no | repo signatures and row structs track the schema |
| `rolter-control` | no | CRUD payload structs track the dashboard |
| `rolter` | no | launcher binary, no library surface |

The guarded crates are checked at `--release-type patch` — the strict reading.
On those crates a change to a public item is surfaced rather than passing
unremarked: the job goes red, and the pull request either keeps the API or says
in its description that moving it was the point.

The unguarded three are unguarded because their public items exist purely so the
two binaries can share code, and they change shape whenever the schema, the
config or the dashboard payloads change. Checking them produced the permanent
red this page opens with.

**The list is about review value, not about a promise.** No rolter crate has a
stable Rust API — [ADR-0032](../adr/2026-09-09-one-point-oh-compatibility-guarantees.md)
decides that explicitly, and 1.0 does not change it. A crate is on the list
because an unintended change to its API is likely enough to be an unintended
change in *behaviour* that a second look is worth the noise, and off it when it
is not. Adding or removing one is that judgement, made in the same pull request,
with a line in the table saying why.

## What this does *not* cover

The surfaces rolter's users actually depend on are not Rust APIs:

- the OpenAI- and Anthropic-compatible `/v1/*` gateway surface,
- the control-plane REST API under `/api/v1/*`,
- the configuration file keys and environment variables,
- the database schema and its migrations.

None of those are checked here — `cargo-semver-checks` cannot see them.
What each of them guarantees is decided in
[ADR-0032](../adr/2026-09-09-one-point-oh-compatibility-guarantees.md) (#922),
and the user-facing statement of it is
[Versioning & compatibility](../../user-docs/community/versioning.mdx).

The other half of what #922 needs is the list of subsystems the promise does
*not* cover. That is a separate axis, set per subsystem rather than per crate,
and it lives in [Stability markers](stability-markers.md): an `experimental`
marker is a documented exemption saying the subsystem may change shape or be
removed in a minor release.

## At 1.0 — the job stays advisory

An earlier version of this page planned to promote the job at 1.0: drop
`continue-on-error`, add it to `ci-ok`'s `needs:`, and revisit `GUARDED`
against whatever 1.0 declared stable.
[ADR-0032](../adr/2026-09-09-one-point-oh-compatibility-guarantees.md) withdraws
that plan, because 1.0 declares **no** Rust API stable. Every published crate is
internal implementation detail, said so in its `Cargo.toml` description, its
`//!` docs and the README, and a gate enforcing a promise the ADR disclaims
would be worse than either choice alone.

So the job is permanently advisory:

- `continue-on-error: true` stays, and `semver-checks` never joins `ci-ok`'s
  `needs:`. A red run is a *review signal* — "this pull request moved a public
  item in a crate both binaries share" — and is often the correct outcome, as it
  is for the behaviour-preserving refactors in #1041 and #1042.
- `GUARDED` keeps its four crates for the same reason: those are where an
  unintended API change is most likely to be an unintended *behaviour* change
  worth a second look. Adding or removing one is a judgement about review value,
  not about a promise, and still wants a line in the table above saying why.
- release-plz has `semver_check = false` at the workspace level in
  `.github/config/release-plz.toml`. Left at its default it would run
  `cargo-semver-checks` itself and propose a **major bump of the shared
  workspace version** — every binary, the wheel, the Docker tag — because a
  shared helper changed shape. rolter's version is driven by commit types and
  nothing else.

The job is green on a clean tree, and a red run means a guarded crate's API
moved. That is information, not a verdict.
