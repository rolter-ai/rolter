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

## One organization per file (#3440)

Names are per org (#1857, ADR-0039) and the file format carries no org, so two
orgs that share a route or provider name cannot be one valid file. The decision
was one org per file rather than an `org` key on every row, which would need an
importer spanning orgs that nothing asks for. `ExportOptions::org` (`rolter
config export --org <slug>`, `?org=<slug>`) runs `scope_to_org` over the loaded
config before the clash check: providers, groups and routes owned by that org
or by none, the org's prompt templates (the `{org_id}:` half of their ids), the
caps `ScopeRefs::in_org` places in it (owner slugs are recorded for every
entity by `insert_owned`; a person with no `within` by `OrgView::members`; a key
by `OrgView::key_orgs`; an unknown scope in no file). Prices, currency and
logging are deployment-wide and stay, with one exception: the importer applies
a `[logging.payload_capture]` table to the whole deployment, so a capture
limited to keys (which spans orgs) is left out of a per-org file with a
comment (`scope_to_org` returns that, `render_document` omits it), and the
re-import then leaves capture alone (`declares_payload_capture`). Anything read by name must be keyed by the org too:
`with_stored_variants` keys by (project, model), because a model name alone
hands one org the other's variants. The clash check then runs on what is
left, so an unscoped export of such a deployment still refuses and names the
option. Test: `config_export::tests::round_trip::two_orgs_sharing_names_export_one_file_each`.

## An org that shadows a deployment-wide row (#3481)

ADR-0040 lets an org's route, provider or provider group shadow a row no org
owns (the gateway's own config file) of the same name. A flat file cannot hold
both, so `scope_to_org` drops the deployment-wide row whenever the org has one
of that name and returns the dropped names in `OrgScope::shadowed`;
`render_shadowed` writes them as a comment, so a reader sees what the file
leaves out. There is no flag. A provider shadowed by _slug_ only (a different name) is
not dropped, and the clash check still refuses them. Groups are keyed by slug, as `clashing_names` and the importer do. A kept no-org route or group that uses a shadowed provider is refused (`ExportError::ShadowedBinding`, code `config_export_binding_shadowed`), since the import's org-owned copy would bind to the org's provider. The unscoped refusal is `ExportError::NeedsOrganization` (code `config_export_needs_org`) whenever `an_org_settles_every_clash`. The import turns the
deployment-wide rows that stay in the file into the org's own, which then shadow
the originals, so a second export of the same database drops them too; the
round-trip test therefore re-adds only the shadowed rows, and also asserts the real case where the originals are all still served. Test:
`config_export::tests::round_trip::an_org_shadowing_deployment_wide_rows_exports_without_them`.

## Leaving person caps out (#3396)

A cap on a person names the account by email (#3311), and #1312 kept addresses
out of committed files on purpose. `ExportOptions::without_person_caps`
(`--without-person-caps`, `?without_person_caps=true`) makes `load_scope_refs`
skip the `users` query, so no address is read, and `render_caps` then counts
those caps in its trailing comment because they have no portable name, as it
counts key caps. The default stays as decided on #3305. Test:
`config_export::tests::round_trip::no_address_is_exported_when_person_caps_are_left_out`.

## The MCP manifest marker (#3398)

`orgs.mcp_enforce_manifests` decides for an org with no `mcp_gateway_settings`
row (migration `0152`), and no saved setting carries it, so the export reads it
straight from `orgs` (`load_mcp_manifest_enforcement`) into
`GatewayConfig::mcp_manifest_enforcement` and writes
`[[mcp_manifest_enforcement]]` (`org` slug, `enforce`). The field is bootstrap
only: the store never fills it, so the snapshot does not carry it, and the
representative snapshot fixture holds one entry only so the shape guard sees it.
`import_mcp_manifest_enforcement` writes only `enforce = false`, only to the org
the import targets and only when it is still strict, because an import never
makes an org stricter; entries naming another org are skipped with a warning.
`MergedConfigStore::load` clears the field, so a control plane started with an
exported file does not ship it. It
looks before it writes, since the `0153` trigger is a statement trigger that
bumps `config_version` even for an update that matches no row. Tests:
`config_export::tests::round_trip::the_mcp_manifest_marker_is_exported_for_an_org_without_settings`
and `seed::tests::the_mcp_manifest_marker_never_makes_an_org_stricter`.

## Saved MCP gateway settings (#3482)

An org's saved `mcp_gateway_settings` row is exported as `[[mcp_gateway_settings]]`
(`load_mcp_gateway_settings`, `render_mcp_gateway_settings`) from
`GatewayConfig::mcp_gateway_settings`, a bootstrap-only field like the marker
above: `MergedConfigStore::load` clears it and the snapshot carries the same
values as `mcp_settings`. Unlike the marker it is the desired state.
`import_mcp_gateway_settings` sets the targeted org to exactly the entry,
stricter or laxer, only when the stored row differs (the `0114` trigger is a
statement trigger), logs the previous and new values, validates the ranges the
table's check constraints hold, and skips entries for other orgs with a warning.
No migration was needed. Test:
`config_export::tests::round_trip::saved_mcp_gateway_settings_round_trip_for_one_org`.

## MCP tool groups (#3514)

An org's MCP tool groups are exported as `[[mcp_gateway_tool_groups]]`
(`load_mcp_gateway_tool_groups`, `render_mcp_gateway_tool_groups`) from
`GatewayConfig::mcp_gateway_tool_groups`, bootstrap-only like the two tables
above: `MergedConfigStore::load` clears it, and the snapshot carries the store's
groups as `mcp_tool_groups`, by id. The file names nothing by id. A tool's
server is its slug in the org; a team or project assignment is the `org/team` or
`org/team/project` path the caps use (`ScopeRefs`); a key assignment is its
project path plus its `name`, or, for a key with none, the `key_prefix` the
dashboard shows. Never the key's secret or digest. An assignment with no such
name (a key of a project whose name holds `/`) is left out with a warning in the
control plane's log.

`import_mcp_gateway_tool_groups` is the desired state for the groups the file
names, in the targeted org only. `validate_mcp_gateway_tool_groups` refuses a
malformed entry before anything is written. A group is matched on
`(org, slug)`, created with its assignments in one transaction
(`McpToolGroupRepo::create`), or updated only where its name, description,
state or tools (compared as a set) differ, and its assignments replaced only
where they differ, since both tables bump `config_version` per statement. Each
write is logged. `resolve_tool_group_subjects` resolves the references: one that
names another org or nothing here is skipped with a warning, and while one is
missing the import only adds assignments, never removes them, as the missing
one may be one the group has. A key reference that matches more than one key of
the project (a prefix matches unnamed keys only) refuses the import before any
write (#3575). Assigned groups are unioned (`mcp_policy.rs`), so assigning a
group to an unintended key can grant tools even when that key already has a
restriction. `preflight_mcp_tool_groups` resolves every targeted group's subjects
before any import write and retains those ids for the write phase, rather than
resolving a reference again after the file starts applying. Give colliding keys
distinct names before importing. A tool on a server the org lacks also fails
preflight, since dropping it would leave a server later created under
that slug ungoverned. An assignment the export cannot name (a team or project
whose name holds `/` or is empty, or a key of such a project) is in no file, so
a replace keeps it (`unportable_subjects`) with a warning, rather than handing
its subject the org-wide set back. A group that cannot be written fails the import.
The export deduplicates public key references, but an import still requires each
to identify at most one key. No migration was needed. Tests in
`config_export::tests::round_trip`:
`mcp_tool_groups_round_trip_with_their_assignments`,
`an_mcp_tool_group_keeps_its_group_when_a_key_is_missing`,
`an_ambiguous_key_prefix_cannot_grant_another_keys_tools` and
`an_ambiguous_key_name_cannot_grant_another_keys_tools`. The ambiguity regressions
check the snapshot's tool union for the unintended key, unchanged config version
on repeated refusals, and no-op repeated imports after the collision is resolved.

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

### Per-person caps (#3305, #3311)

A `user` cap travels by the account's email, decided on #3305 and built in #3311.
`load_scope_refs` reads the email of the accounts that hold a cap, and only
those, so the file carries no address that is not a cap's own; `ScopeRefs::insert`
records one only when it is a single token. `ScopeRefs::within` turns a narrowed
cap's `within` into the same slug paths the other caps use, `{ org = "acme" }` or
`{ project = "acme/platform/api" }`, and a cap whose place has no name stays out
of the file and is counted in the trailing comment.

`resolve_cap_scope` finds the account by `users.email` and `resolve_within` the
place. Neither creates anything: an account or place that does not exist on the
receiving deployment skips the cap with a warning, the rule every unresolved
scope already follows. A cap is matched to its row by scope, window **and**
place, so one person's un-narrowed cap and their cap in a project are two rows
that each re-import as themselves, and an unchanged import still writes nothing.
`within` on any other scope is refused before the first write, as the store
refuses it. Tests: `config_export::tests::round_trip`, whose fixture carries a
person's caps and one for an account that does not exist.

## Why some tables are out

- **Caps on a virtual key** (`BudgetScope::Key`) have no identity that survives a
  move, and the keys they would attach to are never exported: a key is a digest
  and its name is not unique. They are counted in a trailing comment rather than
  written (#1312, #3305).
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
