# What 1.0.0 guarantees, surface by surface

**Status:** Accepted · **Date:** 9 Sep 2026 · **Issues:** [#922](https://github.com/rolter-ai/rolter/issues/922)
**Relates:** ADR-0011 (API surface v1), ADR-0012 (Conventional Commits), ADR-0017 (provider/model addressing), ADR-0030 (control-plane OpenAPI document)

## Context

`1.0.0` is not a quality claim. It is a compatibility claim, and semver assigns
it a meaning whether or not anyone writes one down. Today the workspace is
`0.1.0`, where semver says nothing is stable, so the question has never had to
be answered. Cutting 1.0 answers it by default — badly — unless it is chosen.

rolter has four externally visible surfaces plus one that is visible only
because of how it is distributed, and they do not have the same answer:

| Surface | Who consumes it | Can rolter control its shape? |
|---|---|---|
| `/v1/*` gateway | OpenAI and Anthropic SDKs, any HTTP client | No — it mirrors two vendors |
| `/api/v1/*` control API | the dashboard, and SDKs after #851/#421/#422 | Yes, entirely |
| `rolter.toml` and env vars | operators, Helm values, compose files | Yes |
| the Postgres schema | rolter itself; operators' backups | Yes |
| eight crates on crates.io | anyone who runs `cargo add` | Yes |

Two things already exist and constrain the answer rather than being free
choices:

- [`docs/development/api-stability.md`](../development/api-stability.md) (#1217)
  already scoped `cargo-semver-checks` to four crates against the previous
  release tag, and it ends by *planning* to make that job blocking at 1.0 and to
  revisit the guarded list. That plan is a promise made in advance of this
  decision, and this ADR is what supersedes it.
- [Stability markers](../development/stability-markers.md) (#1385) ship a
  per-subsystem `experimental` marker whose entire purpose is to be the
  documented exemption from whatever 1.0 promises. This ADR uses it and does not
  invent a second exemption route.

The crates are the surface most likely to be answered by accident. Publishing
`rolter-core 1.0.0` states that its Rust API is stable, because that is what the
number means on crates.io, and two mechanisms would then start enforcing a
promise nobody made: the `semver-checks` job, which
[`api-stability.md`](../development/api-stability.md) plans to promote at 1.0,
and release-plz, whose `semver_check` defaults to `true` for library packages
and would propose a **major bump of the whole workspace** — every binary, the
wheel, the Docker tag — because a shared helper changed shape. #1041 and #1042
are already filed, explicitly behaviour-preserving refactors of exactly that
kind.

## Decision

### 1. The Rust crates are internal. There is no stable Rust API at 1.0

rolter's product is two binaries and a dashboard. The crates exist so those
binaries can share code; they are on crates.io because `cargo install rolter`
requires every transitive dependency to be published, not because anyone asked
to build against them.

At 1.0 and for the whole of 1.x, **no rolter crate offers a stable Rust API.**
Any public item in any crate may change or disappear in any release. The version
number on crates.io is the product's version, and it says nothing about the
Rust surface.

Because a version number cannot carry that caveat, three places say it in words:

- the `description` of every published library crate ends with
  `— internal crate, no stable rust api`, which is what crates.io search,
  `cargo add` and the docs.rs sidebar show;
- the crate-level `//!` docs say it in the first screen a docs.rs visitor reads;
- the README says it under **Repository layout**, next to the crate list.

And two mechanisms are made to agree with it rather than contradict it:

- `release-plz` gets `semver_check = false` at the workspace level, so a Rust
  API diff can never drive rolter's product version.
- The `semver-checks` job stays **advisory permanently** — `continue-on-error`,
  never in `ci-ok`'s `needs:`. It is retained as a review signal on the four
  crates in `GUARDED`, not as a gate, and
  [`api-stability.md`](../development/api-stability.md) is rewritten to say so
  rather than to plan a promotion that this decision withdraws.

Rejected: **freeze the Rust API at 1.0.** It buys nothing for the users rolter
has and costs every internal refactor, starting with two already filed. A
promise whose first act is to block behaviour-preserving cleanups is a promise
made to nobody.

Rejected: **stop publishing the library crates.** Cargo requires the full
dependency graph to be on crates.io for `cargo install rolter` to resolve, so
`publish = false` on `rolter-core` removes the installation path that ADR-0010
picked. The crates have to be published; the question is only what publishing
them says.

Rejected: **version the libraries separately, keeping them at `0.x` while the
binaries go `1.x`.** This is the semver-correct way to say "no stable Rust API",
and it is the closest call here. It loses on operational cost: the shared
workspace version is what lets one package cut the single `v{version}` tag, what
`pyproject.toml` reads for the wheel, and what the `/api/v1/version` endpoint
reports. Splitting it means per-crate bump decisions, diverging changelogs, and
an operator reading `rolter-core 0.9.3` inside `rolter 1.4.0` and reasonably
concluding the product ships a pre-release component. The disclaimer belongs
where a reader actually looks — the description and the docs.rs landing page —
and nushell publishes forty `nu-*` crates on exactly this model.

### 2. `/v1/*` guarantees the dialect, not a schema

rolter cannot promise a frozen `/v1/*`. The schema is OpenAI's and Anthropic's,
they change it without asking, and the `v1` in the path is theirs too — client
SDKs hardcode it, so rolter has no `/v2/` escape hatch on this surface at all.
A promise to freeze it would be broken by someone else's release notes.

What 1.0 promises instead:

- **Fidelity.** A request that the upstream provider's own API accepts is
  accepted by rolter and forwarded, and the response is shaped the way that
  provider shapes it. Unknown body fields travel; rolter is a pipe, and that is
  what makes this promise keepable.
- **Tracking, not freezing.** When OpenAI or Anthropic changes a dialect, rolter
  follows. A change that exists only to restore fidelity with an upstream is
  **not** a rolter breaking change, does not carry `BREAKING CHANGE:`, and is
  called out in the release notes as an upstream dialect change. Refusing to
  follow would break every client that also talks to the real provider, which is
  the opposite of the compatibility rolter sells.
- **The rolter-owned parts of `/v1/*` are promised like the control API.** These
  are the parts no vendor owns: the virtual-key auth scheme
  (`Authorization: Bearer` and `x-api-key`), the model-addressing grammar —
  route names and `provider-slug/model` from ADR-0017 — the shape of rolter's
  own error envelope, rolter-specific request and response headers, and the
  shape of `GET /v1/models`. These follow the `/api/v1/*` rules below, including
  the deprecation window.

The practical read for an operator: if the field you depend on is in OpenAI's or
Anthropic's documentation, your guarantee is that rolter keeps up with them. If
it is in rolter's documentation and nowhere else, your guarantee is rolter's.

### 3. `/api/v1/*` is additive within `v1`, with a two-minor-release, 90-day removal window

This is the surface rolter fully controls, and after #851/#421/#422 it is the
one SDKs pin against. It gets the real promise.

Within the `v1` prefix, for the whole of 1.x:

- **Additive changes may ship in any release.** New endpoints, new optional
  request fields, new response fields, new values in an enum the OpenAPI
  document marks as open. A client that ignores unknown fields is never broken
  by these, and clients are expected to ignore unknown fields.
- **Removing or incompatibly changing anything published in
  [the control-plane OpenAPI document](2026-09-08-control-plane-openapi-document.md)
  requires a deprecation period of at least two minor releases *and* at least 90
  days, whichever ends later.** The clock starts at the release that publishes
  the deprecation. The deprecation is announced in the release notes, marked
  `deprecated: true` on the operation or schema in the control-plane OpenAPI
  document, and served with `Deprecation` and `Sunset` response headers
  (RFC 9745, RFC 8594) so a client can find out without reading a changelog.
  After the window, removal ships in a normal minor release.
- **`/api/v2/` is reserved for a wholesale reshaping**, served alongside `v1`,
  not spent on a single removed field.

Why two minors *and* 90 days rather than one number. A pure release count is
worthless at an unfixed cadence — two minors can be a fortnight. A pure time
period can pass with no release at all, so nobody ever sees the notice. Both
bounds together mean the deprecation appears in at least two sets of release
notes *and* survives a quarterly upgrade cycle, so a team that upgrades once a
quarter meets the warning at least once before it bites. Longer windows (180
days, a year) sound generous and in practice freeze the API, which pushes work
toward a premature `v2` — the outcome the window exists to avoid.

One escape: a change required to fix a security vulnerability may shorten the
window. It is still announced, and the advisory says why.

That the dashboard ships in lockstep with the control plane is not a reason to
skip the window. The dashboard is not the only client any more.

### 4. Config files: a 1.0 `rolter.toml` is read by every 1.x

A configuration file and an environment that a 1.0.0 binary accepts are accepted
by every later 1.x binary, with the same meaning. New keys are optional and
default to the previous behaviour.

Removing a key, or changing what an existing key means, follows the same window
as `/api/v1/*`: two minor releases and 90 days, announced in the release notes,
with the deprecated key still honoured and `rolter check` warning on it for the
whole window. ADR-0022's deprecated top-level `[[providers]]` / `[[routes]]` /
`[[provider_groups]]` arrays are already on this footing.

The mechanical basis for the forward half is that unknown keys are ignored
rather than rejected, so an older 1.x binary starts against a newer file. It
starts having silently ignored the newer keys, which is a real trap;
`rolter check` is where that is surfaced, and widening it to report unrecognised
keys is worth doing on its own.

### 5. The database schema moves forward only. There is no downgrade

Migrations are append-only (#724) and there are no `down` scripts. 1.0 does not
add any. The supported upgrade is: take a backup, start the newer binary, let it
migrate. The supported rollback is **restore the backup** — see
[Backup and restore](../deployment/backup-and-restore.md). Pointing an older
binary at a database a newer one has migrated is not a supported configuration.

Two things soften that in practice and neither is a promise:

- The data plane never reads Postgres. The gateway consumes
  `/internal/snapshot`, so schema compatibility is a question between the
  control plane and its own database, and the two ship together.
- Because migrations only add, an older 1.x control plane usually keeps running
  against a schema a newer 1.x has migrated, which is what makes a rolling
  upgrade survivable. A migration that changes the meaning of an existing column
  breaks this, and such a migration is a breaking change under rule 6 and is
  announced as one.

### 6. What `BREAKING CHANGE:` means after 1.0

Before 1.0 the trailer meant "something moved", including a Rust API. That
reading dies here, because release-plz turns `!` or a `BREAKING CHANGE:` footer
into a **major bump of the shared workspace version** — one trailer takes the
whole product to `2.0.0`.

After 1.0:

- **Use it for a change to a promised surface that ships without a completed
  deprecation window.** That is the definition. It requires a written migration
  note in the release, and it takes rolter to the next major.
- **Do not use it for a removal that completed its window.** The announcement
  and the window *are* the compatibility mechanism; that is the whole value of
  promising a window instead of promising permanence. The removal ships as a
  normal `feat`/`refactor` whose release notes name what went.
- **Do not use it for a Rust API change** in any crate. They are internal
  (rule 1).
- **Do not use it for an upstream dialect change** rolter is tracking (rule 2).
- **Do not use it for a subsystem carrying the `experimental` marker.** That
  marker is the documented exemption, and it already says the subsystem may
  change shape or be removed in a minor release. Which subsystems those are
  lives in `SUBSYSTEMS` and in
  [Stability markers](../development/stability-markers.md); this ADR neither
  restates that table nor adds a second exemption mechanism beside it.

The rule of thumb the trailer encodes: if the change *could* have been
deprecated first, it must be, and then it is not breaking. `BREAKING CHANGE:` is
for the changes that could not.

## Consequences

- Somebody who runs `cargo add rolter-core` at 1.0 is told, in the place cargo
  shows them, that they are on their own. Nothing else changes for them, and
  #1041 and #1042 proceed.
- The `semver-checks` job never becomes a merge gate, and
  [`api-stability.md`](../development/api-stability.md)'s "At 1.0" section is
  rewritten to match. A red advisory run means a shared crate's API moved —
  useful review information, not a failure.
- release-plz stops running `cargo-semver-checks` at all, so the product version
  is driven only by commit types, which is what ADR-0012 always intended.
- `/api/v1/*` acquires real obligations that do not exist today: `deprecated:
  true` in the OpenAPI document, `Deprecation`/`Sunset` headers, and a
  deprecations section in the release notes. None of that is implemented yet;
  it is filed as follow-up work and is a prerequisite for the first deprecation,
  not for 1.0 itself.
- The `experimental` marker becomes load-bearing. A subsystem left marked is
  exempt from all of the above, so *removing* a marker is now the expensive act
  and belongs in the PR that closes the gap the note describes — exactly as
  [Stability markers](../development/stability-markers.md) already requires.
- The honest cost of rule 2 is that an operator's `/v1/*` compatibility is
  partly held by OpenAI and Anthropic. rolter states that plainly rather than
  implying a guarantee it would have to break the first time a vendor ships a
  change.
