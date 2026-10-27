# Configuration export and the import round trip

`rolter-seed --import` and `rolter config export` are the write and read halves
of one round trip. `crates/rolter-control/src/seed.rs` consumes a
`rolter.toml`; `crates/rolter-control/src/config_export.rs` produces one. They
are coupled on purpose, and the coupling is the thing to keep intact.

## The invariant

> Import a file, export it, re-import the export, export again: the two exports
> are byte-identical.

`config_export::tests::round_trip` asserts exactly that against a scratch
postgres schema. It is the acceptance test for the feature, and it fails the
moment the two halves disagree — a section the exporter writes that the importer
drops, or a value that renders differently on the second pass.

## Adding a section

A new section belongs in **both** modules or in neither:

1. `seed.rs` — an upsert keyed by the section's slug or natural key, following
   the desired-state rules the module doc states (never delete a row the file
   does not mention, never rewrite a slug, never touch a sealed credential).
2. `config_export.rs` — a `render_*` function emitting the same keys, sorted.
3. `config_export::tests::round_trip::FIXTURE` — a row of the new kind, so the
   round trip actually covers it.
4. The `HEADER` constant, which tells the operator what round-trips and what
   does not. A section added to the exporter and not to the header is a silent
   promise.
5. `docs/user-docs/configuration/config-export.mdx`, whose table mirrors the header.

Exporting something the importer ignores is worse than not exporting it: the
file then looks like a complete description of the deployment while a re-import
quietly drops part of it.

## The stamp and the shape move together

The export writes `schema_version = <CURRENT_SCHEMA_VERSION>` and ADR-0022's tiered `[[providers.readonly]]` / `[[providers.default]]` spelling. Neither may be changed without the other, and the reason is not stylistic (#1513).

The migration chain selects steps with `MIGRATIONS.iter().filter(|m| m.from >= from)`. A file stamped `2` that still contained the deprecated flat `[[providers]]` arrays would therefore skip the v1 -> v2 step **forever**. It would load correctly today, because `split_section` accepts both spellings, and then misbehave under the first v2 -> v3 migration written against the tiered shape — a failure that arrives one release after the mistake, in a step whose author did nothing wrong.

`an_exported_config_has_no_pending_migrations` and `the_export_uses_the_tiered_provider_spelling` pin the two halves. Source the stamp from `rolter_core::config_migrate::CURRENT_SCHEMA_VERSION`, never as a literal.

Emitting both tiers is also what keeps the export lossless. The flat array _is_ the readonly tier — it cannot express `providers.default` at all — so exporting a deployment with seeded defaults used to drop them silently. `the_default_tier_round_trips` covers that direction.

## Budgets and rate limits (#1312)

`BudgetConfig::id` is a database id, which the one identity rule of the format
forbids, so the export rewrites it: `config_export::load_scope_refs` reads the
names (`ScopeRefs`) and `render_caps` writes `id = "acme/platform"`-style
references, with `export_document` as the single entry both the endpoint and the
CLI call. `seed::resolve_cap_scope` is the reverse lookup. The file reuses the
existing `scope` / `id` keys, so it is still a valid `GatewayConfig`.

The importer matches a file entry to a row by scope (plus window for a budget)
and, when several rows share it, by position oldest-first, so two caps on one
scope re-import as two. Period spelling is left as the row has it (`30d` and
`monthly` are one window), which is also why an unchanged import bumps nothing.
A reference that does not resolve, or is a UUID, is skipped with a warning; a
value no column or gateway could hold fails the whole import before any write.
`ScopeRefs::insert` drops a name containing `/` rather than export an ambiguous
path.

## Why some tables are out

- **Caps on a virtual key or a person** (`BudgetScope::Key`, `User`) have no
  identity that survives a move: a key is a digest and its name is not unique, a
  person's email would put accounts in a committed file. They are counted in a
  trailing comment rather than written (#1312).
- **Virtual keys** exist in the store only as a one-way digest.
- **Prompt-template activation scopes** are database ids too. Only the
  route-model bindings survive; an org-wide template exports with no `routes`
  list, which is what the importer already reads as "all routes".

## Secrets

`ConfigStore::load` returns providers with `api_key` **decrypted** — the gateway
needs the plaintext, so the snapshot path has to carry it. The exporter
therefore renders provider credentials from `api_key_env_name()` only, as an
`api_keys = [{ env = "..." }]` entry (#1720), and never reads `api_key` at all. `config_export::tests::a_decrypted_provider_key_never_reaches_the_document`
pins that against the emitted bytes rather than by review.

A provider with no `api_key_env` gets a comment where the credential would be,
so the omission is visible in the artifact.

### Both spellings, one column

`ProviderConfig` accepts the credential as either `api_key`/`api_key_env` or the
plural `api_keys` array. Every persistence path — the exporter, `rolter-seed
--import` and the `[[providers.default]]` seed — goes through
`ProviderConfig::api_key_env_name()` for the variable name and
`inline_api_key()` for the literal, so the spelling a document happens to use
makes no difference to what is stored or exported. Reading `api_key_env`
directly is what made a plural-form provider import with no credential at all
(#1514).

The split between the two accessors is the safety property: `api_key_env_name()`
returns a variable _name_ and is the only one an exported document can reach,
while `inline_api_key()` returns a secret and has exactly one caller, the seal
into `provider_keys`.

`providers.api_key_env` is a single column and `provider_keys` holds one sealed
row per provider, so a multi-key provider is not representable in the store. The
importer keeps the first entry naming an `env` and warns with the count it
dropped rather than failing the import. Normalising the spellings (#1720) does not
change that: the export writes the plural form, but the array itself does not
round-trip.

The export also writes the proxy pool as `egress_proxies` (`egress_proxy_pool()`),
so a provider stored with the legacy single `egress_proxy` is rewritten into the
plural column the first time its export is re-imported; both resolve to the same
pool.
