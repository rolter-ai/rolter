# Config schema migrations

How a `rolter.toml` written for one release is carried across a version boundary, and why the mechanism is shaped the way it is (#326).

The operator-facing side of this is [Upgrading Rolter](../../user-docs/deployment/upgrading.mdx). This page is the design.

## The shape

`crates/rolter-core/src/config_migrate.rs` holds a chain of steps keyed by schema version:

```rust
pub static MIGRATIONS: &[Migration] = &[Migration {
    from: 1,
    to: 2,
    summary: "...",
    apply: tier_provider_sections,
}];
```

`migrate()` reads the file's `schema_version`, runs every step whose `from` is at or above it, and stamps the result. `N -> N+k` is therefore the same code path as `N -> N+1` — a fold over the chain — which is what makes skipping releases free rather than a separate feature.

`GatewayConfig::from_toml_str` calls it on every load, ahead of the tiered-section extraction, so a migration can still see `providers` and `provider_groups` as part of the document.

## Three rules, each of which cost something to learn

### A migration is behaviour-preserving or it is not a migration

The `GatewayConfig` parsed from the migrated document must equal the one parsed from the original. `migration_is_behaviour_preserving` pins this.

This is what makes it safe to migrate on _every_ load rather than as a separate operator-invoked step. The moment a migration can change behaviour, running it implicitly at boot becomes the wrong design and the whole thing needs an apply/confirm cycle.

### A file from the future loads, it does not fail

`config_lint`'s module docs already state the promise the config types make: no `deny_unknown_fields`, because a file written for `X.y.z` has to stay loadable by every other `X.*` build, **including an older one it gets rolled back onto**.

A version stamp is the obvious place to break that promise, and breaking it would be worst exactly when it hurts most — a rollback is what an operator reaches for when something has already gone wrong. So a `schema_version` newer than the build understands sets `Report::ahead`, logs a warning, and loads the document unchanged.

### Migrate in memory, never in place

`migrate()` rewrites the parsed document. Nothing writes the file back.

A loader that edits its own input is a surprise in a crash-looping container, where the operator is least able to absorb one, and the config mount may be read-only anyway. Emitting a canonical `rolter.toml` is `rolter config export`'s job (and #1513 is making its output current).

## What is deliberately not a migration

`api_key` / `api_key_env` / `egress_proxy` have plural successors and look like the obvious first migration. They are not one: `rolter-control`'s seed, snapshot and export paths read the singular fields **directly** rather than through `resolve_api_key()` and `egress_proxy_pool()`, so normalising the document would silently change what `rolter-seed --import` stores.

That is #1514. Until every reader goes through the accessor, these stay deserialize-time shims — which is how this repository has handled config evolution so far, and the reason the migration chain starts as short as it does.

## Adding a step

1. Append a `Migration` to `MIGRATIONS` with `from` equal to the current `CURRENT_SCHEMA_VERSION` and `to` one higher.
2. Bump `CURRENT_SCHEMA_VERSION`. `the_chain_is_contiguous_and_reaches_the_current_version` fails if you forget, rather than the step silently never running.
3. Add a fixture in the old spelling and assert it parses identically to the new one.
4. Keep the step idempotent — `migrating_twice_changes_nothing_the_second_time` runs it against an already-migrated document.

### If your step renames a key, `config_lint` needs to change with it

No current step _consumes_ a key, so the lint and the chain do not interact. The first rename breaks that: the lint parses the file as written and would report the old spelling as an unrecognised key, telling an operator to fix something rolter already handled.

Do not fix this by linting the migrated document. The paths the lint prints are how an operator finds the line in their own file, and migrating first rewrites them into paths the file does not contain — `providers[0].x` becomes `providers.readonly[0].x`, which greps to nothing. Have the migration declare the key paths it consumes, and have the lint skip exactly those.

## Previewing

`rolter check --migrations` renders the plan without writing anything, and adds a non-fatal finding so a CI job running `--strict` can notice a config drifting behind. It is opt-in because an operator who has never stamped their file would otherwise meet the warning on every run of a perfectly working deployment, and a warning everyone learns to skip past is worse than no warning.

## Not covered here

The control-plane-to-gateway snapshot carries `config_version`, a change counter, not a format version — so a mixed-version fleet during a rolling upgrade has no negotiation. That is #1512, with its own ADR.
