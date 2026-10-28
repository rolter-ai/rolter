# Config schema migrations

How a `rolter.toml` written for one release is carried across a version boundary, and why the mechanism is shaped the way it is (#326).

The operator-facing side of this is [Upgrading Rolter](../../user-docs/deployment/upgrading.mdx). This page is the design.

## The shape

`crates/rolter-core/src/config_migrate.rs` holds a chain of steps keyed by schema version:

```rust
pub static MIGRATIONS: &[Migration] = &[
    Migration { from: 1, to: 2, summary: "...", apply: tier_provider_sections },
    Migration { from: 2, to: 3, summary: "...", apply: plural_provider_spellings },
];
```

`migrate()` reads the file's `schema_version`, runs every step whose `from` is at or above it, and stamps the result. `N -> N+k` is therefore the same code path as `N -> N+1` — a fold over the chain — which is what makes skipping releases free rather than a separate feature.

`GatewayConfig::from_toml_str` calls it on every load, ahead of the tiered-section extraction, so a migration can still see `providers` and `provider_groups` as part of the document.

## Three rules, each of which cost something to learn

### A migration is behaviour-preserving or it is not a migration

The `GatewayConfig` parsed from the migrated document must resolve to the same providers, credentials and proxies as the one parsed from the original. `migration_is_behaviour_preserving` pins the tiering step and `the_plural_rewrite_resolves_to_the_same_credentials_and_proxies` the credential and proxy step.

This is what makes it safe to migrate on _every_ load rather than as a separate operator-invoked step. The moment a migration can change behaviour, running it implicitly at boot becomes the wrong design and the whole thing needs an apply/confirm cycle.

### A file from the future loads, it does not fail

`config_lint`'s module docs already state the promise the config types make: no `deny_unknown_fields`, because a file written for `X.y.z` has to stay loadable by every other `X.*` build, **including an older one it gets rolled back onto**.

A version stamp is the obvious place to break that promise, and breaking it would be worst exactly when it hurts most — a rollback is what an operator reaches for when something has already gone wrong. So a `schema_version` newer than the build understands sets `Report::ahead`, logs a warning, and loads the document unchanged.

### Migrate in memory, never in place

`migrate()` rewrites the parsed document. Nothing writes the file back.

A loader that edits its own input is a surprise in a crash-looping container, where the operator is least able to absorb one, and the config mount may be read-only anyway. Emitting a canonical `rolter.toml` is `rolter config export`'s job (and #1513 is making its output current).

## The singular credential and proxy spellings (#1720)

`api_key` / `api_key_env` have the plural successor `api_keys`, and `egress_proxy` has `egress_proxies`. The step from version 2 to 3 (`plural_provider_spellings`) rewrites a provider's single credential into one `api_keys` entry and its single proxy into a one-element `egress_proxies`, in the flat `[[providers]]` array and in both tiers.

This was blocked until #1514: `rolter-control`'s seed, snapshot and export paths read the singular fields directly, so normalising the document would have silently changed what `rolter-seed --import` stored. Persistence now goes through `ProviderConfig::api_key_env_name`, `inline_api_key` and `egress_proxy_pool`, so either spelling imports and exports identically.

Decisions that bind this step:

- **The singular fields stay accepted forever** as deserialize-time shims. They are never removed from `ProviderConfig`: a file written by this build has to load on the older build it may be rolled back onto, and an older file has to load here. The migration rewrites the document; it does not retire the keys.
- **`config_lint` warns that they are deprecated** (`deprecated_keys`, logged by the same once-per-file load path as the unknown-key warning). They are not "unrecognised": the model reads them and the setting takes effect.
- **A provider that already has a non-empty plural list is left alone.** The singular field beside it was ignored, so rewriting would invent a second key, and deleting it would delete the one thing the lint can point at.
- **The control-plane store still holds one credential per provider**, so a multi-key `api_keys` list does not round-trip through the database; `surplus_api_key_count` warns about the surplus on import.
- The kinds that must source their key from one environment variable (`requires_env_api_key`) are validated through the accessors, so the one `api_keys` entry the migration produces passes the rule the singular `api_key_env` always passed.

## Adding a step

1. Append a `Migration` to `MIGRATIONS` with `from` equal to the current `CURRENT_SCHEMA_VERSION` and `to` one higher.
2. Bump `CURRENT_SCHEMA_VERSION`. `the_chain_is_contiguous_and_reaches_the_current_version` fails if you forget, rather than the step silently never running.
3. Add a fixture in the old spelling and assert it parses identically to the new one.
4. Keep the step idempotent — `migrating_twice_changes_nothing_the_second_time` runs it against an already-migrated document.

### If your step renames a key, `config_lint` needs to change with it

The version 3 step consumes the singular provider keys, but the config types still accept them, so the lint still recognises them and reports them as deprecated rather than unknown. A step that removes a key from the types is different: the lint parses the file as written and would report the old spelling as an unrecognised key, telling an operator to fix something rolter already handled.

Do not fix this by linting the migrated document. The paths the lint prints are how an operator finds the line in their own file, and migrating first rewrites them into paths the file does not contain — `providers[0].x` becomes `providers.readonly[0].x`, which greps to nothing. Have the migration declare the key paths it consumes, and have the lint skip exactly those.

## Previewing

`rolter check --migrations` renders the plan without writing anything, and adds a non-fatal finding so a CI job running `--strict` can notice a config drifting behind. It is opt-in because an operator who has never stamped their file would otherwise meet the warning on every run of a perfectly working deployment, and a warning everyone learns to skip past is worse than no warning.

## Not covered here

The control-plane-to-gateway snapshot is the other half of #326, and it made the opposite choice. Its `version` is a change counter, so the payload also carries a `format` integer, and the gateway states the formats it reads in `formats=N` on every poll ([ADR-0038](../adr/2026-10-27-snapshot-format-version.md), #1512). A mismatch is refused with `406` rather than loaded with a warning, because a refused snapshot costs staleness (the gateway keeps the config it last applied) where a refused file would cost the process. Where `config_migrate` steps a file _up_ to the running build, the snapshot's `DownStep` chain in `rolter_core::snapshot_format` steps the control plane's config _down_ to what an older gateway reads, so conversion runs where the database is and never in a gateway. See [Configuration & hot reload](config-and-hot-reload.md#snapshot-format).
