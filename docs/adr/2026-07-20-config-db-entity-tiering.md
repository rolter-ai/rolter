# Uniform config-vs-DB tiering for models, providers, and provider groups

## Metadata

| Field | Value |
| --- | --- |
| Product | rolter |
| Date | 20 Jul 2026 |
| Status | ACCEPTED |
| Issues | [#306](https://github.com/rolter-ai/rolter/issues/306) |
| Relates | ADR-0017 (provider/model addressing), ADR-0018 (granular CRUD) |

## Context

rolter bootstraps from a TOML config and (optionally) a Postgres store, merged at
startup via `MergedConfigStore`. Today the three routable entity types treat the
config/DB boundary inconsistently:

| Entity | readonly (config-owned) | default (seed→DB, editable) | API/UI (DB) |
| --- | --- | --- | --- |
| **models** | `[[routes]]` + `[models.readonly]` | `[models.default]` (seeded once) | CRUD ✓ |
| **providers** | `[[providers]]` | — (missing) | CRUD ✓ |
| **provider groups** | `[[provider_groups]]` (ADR-0017 addendum) | — | — (no store) |

Three problems:

1. **Providers cannot be seeded-then-edited.** A `[[providers]]` entry is
   permanently config-owned and rejected by CRUD (`require_not_config_owned`), so an
   operator who wants to bootstrap a provider from config but later edit its
   credentials/base_url through the UI has no path.
2. **Provider groups have no DB tier at all** — they exist only as read-only config
   (ADR-0017 addendum / #571), so they cannot be created via the API or UI.
3. **The model shape is idiosyncratic** — top-level `[[routes]]` *and* a parallel
   `[models.readonly]` both mean "immutable config route", which is redundant and
   does not generalize to providers/groups.

The desired behaviour is uniform across all three: an operator can declare an entry
as **readonly** (immutable, served straight from config), as a **default** (seeded
into the DB once at startup, then owned and editable via API/UI), or create it
purely through the **API/UI** (a DB row with no config presence).

## Decision

Adopt one uniform two-tier config wrapper for every routable entity type — models,
providers, and provider groups — layered over the existing DB (CRUD) tier:

```toml
[models]
readonly = [ /* immutable, config-owned, rejected by CRUD */ ]
default  = [ /* seeded into the default project once, then DB-owned */ ]

[providers]
readonly = [ ... ]
default  = [ ... ]

[provider_groups]
readonly = [ ... ]
default  = [ ... ]
```

Semantics, identical for each entity type:

- **readonly** — merged into the effective running config and into the gateway
  snapshot directly; tracked in `ConfigOwned`; CRUD create/update/delete against a
  name/slug in this set is rejected. Immutable for the process lifetime.
- **default** — seeded into the bootstrap `default/default/default` tenancy exactly
  once (idempotent: an existing row by natural key is never overwritten on restart),
  then it is an ordinary DB row — editable and deletable via API/UI, and **not**
  config-owned. Mirrors today's `seed_default_models`.
- **DB (API/UI)** — pure store rows with no config presence, already the CRUD path.

Resolution precedence when the same natural key (`routes.model`, `providers.slug`,
group slug) appears in more than one tier: **readonly wins** (it is immutable and
must never be shadowed by a DB row), then the DB row, then a `default` that has not
yet been claimed as a DB row. Seeding a `default` whose key already exists as
readonly is a config error, surfaced at load time (as `models.default` already does
against readonly routes).

### Migration & back-compat

- The current top-level `[[routes]]` and `[[providers]]` arrays are kept as
  **deprecated aliases** for their `readonly` tier, so existing configs and the
  bootstrap `rolter.example.toml` keep working unchanged. New docs steer operators
  to `[models.readonly]` / `[providers.readonly]`.
- `[[provider_groups]]` (introduced in the ADR-0017 addendum / #572, before this
  ADR) becomes the `provider_groups.readonly` alias; the config-only groups slice is
  forward-compatible with this decision.
- Slugs remain the stable identity across tiers (ADR-0017): a `default` provider or
  group keeps its slug when it becomes a DB row, so its `provider-slug/model` /
  `group-slug/model` address is stable across the config→DB transition.

## Consequences

- One mental model and one code path shape (`readonly` merge + `default` seed +
  `ConfigOwned` guard) for all three entity types; the provider-groups and provider
  work reuses the model tiering rather than inventing per-entity rules.
- Providers gain a seed-then-edit story; provider groups gain a full DB lifecycle.
- New store tables/repos are required for provider groups (`provider_groups` +
  membership), plus seed functions for `providers.default` and
  `provider_groups.default`, plus group CRUD — tracked as separate PRs (see below).
- The deprecated top-level arrays add a small amount of parse-time aliasing to
  maintain until a future breaking release removes them.

## Follow-up implementation issues

1. **core**: uniform tier wrapper for providers + provider groups (readonly/default),
   with top-level `[[routes]]`/`[[providers]]`/`[[provider_groups]]` kept as
   deprecated readonly aliases; load-time validation that a `default` never
   collides with a readonly key.
2. **store/control**: `providers.default` seed tier — seed function mirroring
   `seed_default_models`; providers seeded this way are DB-owned, not config-owned.
3. **store**: `provider_groups` + membership tables, repo, and `MergedConfigStore`
   wiring so DB-defined groups reach the gateway snapshot.
4. **control**: provider-group CRUD (create/update/delete, slug validation shared
   with providers, 409+suggestion on slug conflict) + `provider_groups.default`
   seed.
5. **ui**: provider-group management screens (create/edit/membership/strategy),
   surfaced alongside providers and models. *(separate issue — not in the initial
   backend PRs)*
