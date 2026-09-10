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
| `rolter-balancer` | yes | the `LoadBalancer` trait is the documented extension point |
| `rolter-gateway` | yes | data-plane surface; changes here are behavioural, not structural |
| `rolter-proxy` | yes | provider dialect adapters |
| `rolter-core` | no | config types churn with every new provider kind and strategy |
| `rolter-store` | no | repo signatures and row structs track the schema |
| `rolter-control` | no | CRUD payload structs track the dashboard |
| `rolter` | no | launcher binary, no library surface |

The guarded crates are checked at `--release-type patch` — the strict reading.
On those crates a breaking change has to be a deliberate act: it fails the job,
and the fix is either to keep the API or to move the crate off the list on
purpose, in the same pull request, with a line in this table saying why.

The unguarded three are unguarded because their Rust API is not a promise
anyone has made. They are the crates whose public items exist so the two
binaries can share code, and they change shape whenever the schema, the config
or the dashboard payloads change. Pretending otherwise is what produced the
permanent red.

## What this does *not* cover

The surfaces rolter's users actually depend on are not Rust APIs:

- the OpenAI- and Anthropic-compatible `/v1/*` gateway surface,
- the control-plane REST API under `/api/v1/*`,
- the configuration file keys and environment variables,
- the database schema and its migrations.

None of those are checked here — `cargo-semver-checks` cannot see them.
Deciding what each of them guarantees at 1.0 is #922, and this page is the
Rust-crate half of the answer it will need.

The configuration-file half already has one property that a 1.0 promise will
have to keep: rolter's config types carry no `deny_unknown_fields`, so a
`rolter.toml` written for any build loads on any other build of the same major,
including an older one it is rolled back onto. That is why an unknown key can
only ever be a warning, and why the warning lives in `rolter check` rather than
in the deserializer —
[Unrecognised keys in `rolter.toml`](../deployment/preflight-validation.md#unrecognised-keys-in-roltertoml-1424)
covers what it reports and how `--strict` turns it into a CI gate.

The other half of what #922 needs is the list of subsystems the promise does
*not* cover. That is a separate axis, set per subsystem rather than per crate,
and it lives in [Stability markers](stability-markers.md): an `experimental`
marker is a documented exemption saying the subsystem may change shape or be
removed in a minor release.

## At 1.0

Two things change, both in #922's scope:

1. The job stops being informational. `continue-on-error` comes off and
   `semver-checks` joins `ci-ok`'s `needs:` in `.github/workflows/ci.yml`.
2. The guarded list is revisited against whatever 1.0 declares stable. A crate
   that ships a 1.0 API belongs on the list; one that stays an implementation
   detail should say so in its `Cargo.toml` description rather than sit silently
   in the "no" column here.

Until then the job is green on a clean tree, and a red run means a guarded
crate's API moved.
