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
5. `user-docs/configuration/config-export.mdx`, whose table mirrors the header.

Exporting something the importer ignores is worse than not exporting it: the
file then looks like a complete description of the deployment while a re-import
quietly drops part of it.

## Why some tables are out

- **Budgets and rate limits** are keyed by the database id of the org, team,
  project or virtual key they cap (`BudgetConfig::id`). A file carrying those
  ids is not portable, which contradicts the one identity rule the format has:
  slug or public model name, never a database id. Exporting them needs a
  scope-slug resolution layer on both sides first.
- **Virtual keys** exist in the store only as a one-way digest.
- **Prompt-template activation scopes** are database ids too. Only the
  route-model bindings survive; an org-wide template exports with no `routes`
  list, which is what the importer already reads as "all routes".

## Secrets

`ConfigStore::load` returns providers with `api_key` **decrypted** — the gateway
needs the plaintext, so the snapshot path has to carry it. The exporter
therefore renders provider credentials from `api_key_env` only and never reads
`api_key` at all. `config_export::tests::a_decrypted_provider_key_never_reaches_the_document`
pins that against the emitted bytes rather than by review.

A provider with no `api_key_env` gets a comment where the credential would be,
so the omission is visible in the artifact.
