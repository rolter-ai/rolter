//! Carrying a `rolter.toml` across a version boundary (#326).
//!
//! An operator upgrading rolter should not have to hand-edit their config, and
//! should not be forced through intermediate releases to skip from `N` to
//! `N+k`. This module is the mechanism for both: the file carries a
//! [`schema_version`](crate::GatewayConfig::schema_version), and [`MIGRATIONS`]
//! is a chain of steps keyed by that version. Upgrading is a fold over the
//! chain, so skipping releases costs nothing.
//!
//! # Migrating in memory, never in place
//!
//! [`migrate`] rewrites the parsed document, not the file on disk.
//! [`GatewayConfig::from_toml_str`](crate::GatewayConfig::from_toml_str) calls
//! it on every load, so an old file simply works against a new binary with no
//! manual step — which is what "seamless" has to mean for a process that may be
//! started by a container entrypoint with a read-only config mount.
//!
//! Writing the migrated file back is deliberately not done here. `rolter config
//! export` already owns emitting a canonical `rolter.toml`, and a loader that
//! edits its own input is a surprise in exactly the situation — a crash-looping
//! container — where an operator can least afford one.
//!
//! # A newer file is not an error
//!
//! [`crate::config_lint`] explains why the config types carry no
//! `deny_unknown_fields`: a file written for `X.y.z` has to stay loadable by
//! every other `X.*` build, *including an older one it gets rolled back onto*.
//! A stamp is bound by the same promise. Meeting a `schema_version` from the
//! future therefore reports [`Report::ahead`] and loads the document unchanged,
//! rather than refusing to start. Refusing would make the stamp the one thing
//! that turns a rollback — the move an operator makes when everything else has
//! already gone wrong — into an outage.
//!
//! # The singular credential and proxy spellings
//!
//! A provider carries two spellings for the same thing: `api_key` /
//! `api_key_env` beside the weighted `api_keys` list, and `egress_proxy` beside
//! `egress_proxies`. Normalising them used to be unsafe, and the reason is worth
//! keeping on the record: `rolter-control`'s seed and config-export paths read
//! the singular fields **directly**, so rewriting a document into the plural
//! form would have left them `None` and silently changed what `rolter-seed
//! --import` stored.
//!
//! That was fixed (#1514). Persistence now goes through
//! [`ProviderConfig::api_key_env_name`](crate::ProviderConfig::api_key_env_name)
//! and [`inline_api_key`](crate::ProviderConfig::inline_api_key), and the proxy
//! paths always carried both spellings, so either form imports and exports
//! identically. The step from version 2 to 3 (#1720) is the normalisation: it
//! rewrites a provider's `api_key` / `api_key_env` into one `api_keys` entry and
//! its `egress_proxy` into a one-element `egress_proxies`.
//!
//! The decision recorded on #1720: the singular fields are **never** removed
//! from the config types. They stay accepted as deserialize-time shims for as
//! long as the format lives, because a file written by this build has to load on
//! the older build it may be rolled back onto, and a file written by an older
//! build has to load here. [`crate::config_lint`] warns that they are deprecated
//! and names the plural spelling, and this step is what rewrites them. One
//! caveat the step respects: the control-plane store holds a single credential
//! per provider, so the migration never merges a singular credential into an
//! `api_keys` list that already has entries (the singular fields were ignored
//! beside a non-empty list, and stay where they are for the lint to report).
//!
//! A migration is safe here only when it is **behaviour-preserving**: the
//! `GatewayConfig` parsed from the migrated document must resolve to the same
//! credentials and proxies as the one parsed from the original. `migration_is_behaviour_preserving` pins that for every
//! fixture.
//!
//! # Renaming a key will need [`crate::config_lint`] to change with it
//!
//! The version 2 to 3 step *consumes* keys: `api_key`, `api_key_env` and
//! `egress_proxy` become `api_keys` and `egress_proxies`. That did not break the
//! lint, because the singular fields are still on the config types and so are
//! still recognised as keys; the lint parses the file as written and says they
//! are deprecated instead. A migration that removed a key from the types would
//! break it: the lint would report `old_key` as unrecognised and tell the
//! operator to fix something rolter had already handled.
//!
//! Resist the obvious fix of linting the migrated document. The paths the lint
//! prints are how an operator finds the line in their own file, and migrating
//! first rewrites them into paths the file does not contain — `providers[0].x`
//! becomes `providers.readonly[0].x`, which greps to nothing. The shape to
//! reach for instead is a migration declaring the key paths it consumes, with
//! the lint skipping exactly those and leaving every other path as written.

use serde::Serialize;

use crate::error::Result;

/// Schema version the current build writes and understands.
pub const CURRENT_SCHEMA_VERSION: u32 = 3;

/// Version assumed for a file with no `schema_version` key.
///
/// Every `rolter.toml` written before #326 is unstamped, and there are far more
/// of those in the world than stamped ones. Treating absence as version 1 makes
/// the whole existing corpus the chain's starting point instead of a special
/// case each migration has to test for.
pub const PRE_STAMP_SCHEMA_VERSION: u32 = 1;

/// The config key holding the stamp.
pub const SCHEMA_VERSION_KEY: &str = "schema_version";

/// One edit a migration made to the document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    /// dotted path to what changed, in the same shape `config_lint` reports
    pub path: String,
    /// what happened, as a sentence an operator can act on
    pub detail: String,
}

/// One step in the chain: everything needed to take a document from one schema
/// version to the next.
pub struct Migration {
    /// version this step reads
    pub from: u32,
    /// version this step produces; always `from + 1`, pinned by a test
    pub to: u32,
    /// one line describing the step, shown by `rolter check --migrations`
    pub summary: &'static str,
    /// rewrites the document in place, returning what it touched. Must be a
    /// no-op on a document that has already been migrated, and must preserve
    /// the parsed `GatewayConfig`.
    pub apply: fn(&mut toml::Table) -> Vec<Change>,
}

/// What [`migrate`] did, or what it would do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    /// version the document carried on the way in
    pub from: u32,
    /// version it carries on the way out
    pub to: u32,
    /// the steps applied, in order
    pub steps: Vec<Step>,
    /// the file is stamped newer than this build understands. The document is
    /// left untouched and loading continues; see the module docs.
    pub ahead: bool,
}

impl Report {
    /// Whether anything actually changed.
    pub fn is_empty(&self) -> bool {
        self.steps.iter().all(|s| s.changes.is_empty())
    }

    /// Every change across every step, flattened.
    pub fn changes(&self) -> impl Iterator<Item = &Change> {
        self.steps.iter().flat_map(|s| s.changes.iter())
    }
}

/// One applied step and what it touched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    pub from: u32,
    pub to: u32,
    pub summary: String,
    pub changes: Vec<Change>,
}

/// The chain. Append-only and contiguous: each entry's `from` is the previous
/// entry's `to`, and the last entry's `to` is [`CURRENT_SCHEMA_VERSION`].
/// `the_chain_is_contiguous` enforces both, so a new step that forgets to bump
/// the constant fails the build rather than silently never running.
pub static MIGRATIONS: &[Migration] = &[
    Migration {
        from: 1,
        to: 2,
        summary: "rewrite the deprecated [[providers]] / [[provider_groups]] arrays \
                  as the tiered { readonly, default } tables (ADR-0022)",
        apply: tier_provider_sections,
    },
    Migration {
        from: 2,
        to: 3,
        summary: "rewrite a provider's api_key / api_key_env as an api_keys entry and its \
                  egress_proxy as a one-element egress_proxies (#1720)",
        apply: plural_provider_spellings,
    },
];

/// ADR-0022 replaced the flat `[[providers]]` array with a tiered table that
/// distinguishes config-owned read-only entries from seeded defaults. Both
/// spellings still parse — `split_section` treats a bare array as all-readonly
/// — so this rewrite changes nothing about the resulting config. What it
/// changes is the file: an operator who runs the dry-run is told, once, that
/// their config uses a deprecated shape and what the current one looks like.
fn tier_provider_sections(doc: &mut toml::Table) -> Vec<Change> {
    let mut changes = Vec::new();
    for section in ["providers", "provider_groups"] {
        let Some(toml::Value::Array(entries)) = doc.get(section) else {
            // absent, or already the tiered table
            continue;
        };
        let count = entries.len();
        let entries = entries.clone();
        let mut tiered = toml::Table::new();
        tiered.insert("readonly".to_string(), toml::Value::Array(entries));
        doc.insert(section.to_string(), toml::Value::Table(tiered));
        changes.push(Change {
            path: section.to_string(),
            detail: format!(
                "the deprecated [[{section}]] array becomes [{section}] readonly = [...]; \
                 {count} entr{} moved to the readonly tier",
                if count == 1 { "y" } else { "ies" }
            ),
        });
    }
    changes
}

/// The provider tables of a document, in either accepted shape: the flat
/// `[[providers]]` array, or the tiered `{ readonly, default }` table.
fn provider_entries(doc: &mut toml::Table) -> Vec<(String, &mut toml::Table)> {
    let mut out = Vec::new();
    match doc.get_mut("providers") {
        Some(toml::Value::Array(entries)) => {
            for (index, entry) in entries.iter_mut().enumerate() {
                if let toml::Value::Table(provider) = entry {
                    out.push((format!("providers[{index}]"), provider));
                }
            }
        }
        Some(toml::Value::Table(tiers)) => {
            for (tier, value) in tiers.iter_mut() {
                if tier != "readonly" && tier != "default" {
                    continue;
                }
                let toml::Value::Array(entries) = value else {
                    continue;
                };
                for (index, entry) in entries.iter_mut().enumerate() {
                    if let toml::Value::Table(provider) = entry {
                        out.push((format!("providers.{tier}[{index}]"), provider));
                    }
                }
            }
        }
        _ => {}
    }
    out
}

/// #1720: write each provider's single credential and single proxy in the
/// plural spelling. Neither rewrite changes what the provider resolves to:
/// [`ProviderConfig::resolve_api_key`](crate::ProviderConfig::resolve_api_key)
/// prefers an inline key to its variable, as an `api_keys` entry does, and a
/// one-element `api_keys` list is a single key of weight 1 exactly as the pair
/// was. A provider that already has a non-empty plural list is left alone: the
/// singular field beside it was ignored, and deleting it would also delete the
/// one thing the lint can point the operator at.
fn plural_provider_spellings(doc: &mut toml::Table) -> Vec<Change> {
    let mut changes = Vec::new();
    for (path, provider) in provider_entries(doc) {
        let has_keys = provider
            .get("api_keys")
            .and_then(toml::Value::as_array)
            .is_some_and(|keys| !keys.is_empty());
        if !has_keys {
            let key = provider.get("api_key").cloned();
            let env = provider.get("api_key_env").cloned();
            if key.is_some() || env.is_some() {
                let mut entry = toml::Table::new();
                if let Some(key) = &key {
                    entry.insert("key".to_string(), key.clone());
                }
                if let Some(env) = &env {
                    entry.insert("env".to_string(), env.clone());
                }
                provider.remove("api_key");
                provider.remove("api_key_env");
                provider.insert(
                    "api_keys".to_string(),
                    toml::Value::Array(vec![toml::Value::Table(entry)]),
                );
                let moved = match (key.is_some(), env.is_some()) {
                    (true, true) => "api_key and api_key_env",
                    (true, false) => "api_key",
                    _ => "api_key_env",
                };
                changes.push(Change {
                    path: format!("{path}.{moved}"),
                    detail: format!(
                        "{moved} becomes a single entry of api_keys; the provider resolves the \
                         same credential"
                    ),
                });
            }
        }
        let has_proxies = provider
            .get("egress_proxies")
            .and_then(toml::Value::as_array)
            .is_some_and(|proxies| !proxies.is_empty());
        if !has_proxies {
            if let Some(proxy @ toml::Value::String(_)) = provider.get("egress_proxy").cloned() {
                provider.remove("egress_proxy");
                provider.insert(
                    "egress_proxies".to_string(),
                    toml::Value::Array(vec![proxy]),
                );
                changes.push(Change {
                    path: format!("{path}.egress_proxy"),
                    detail: "egress_proxy becomes a one-element egress_proxies; the provider \
                             uses the same proxy"
                        .to_string(),
                });
            }
        }
    }
    changes
}

/// Read the stamp, defaulting to [`PRE_STAMP_SCHEMA_VERSION`].
///
/// A stamp that is not a positive integer is treated as absent rather than as
/// an error: the document still has to load, and `config_lint` is the surface
/// that reports a malformed key.
pub fn schema_version_of(doc: &toml::Table) -> u32 {
    doc.get(SCHEMA_VERSION_KEY)
        .and_then(toml::Value::as_integer)
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v > 0)
        .unwrap_or(PRE_STAMP_SCHEMA_VERSION)
}

/// Fold the document forward to [`CURRENT_SCHEMA_VERSION`].
///
/// `N -> N+k` is the same code path as `N -> N+1`: every step whose `from` is
/// at or above the document's version runs, in order. A document already at the
/// current version applies no steps and reports no changes.
pub fn migrate(doc: &mut toml::Table) -> Report {
    let from = schema_version_of(doc);
    if from > CURRENT_SCHEMA_VERSION {
        return Report {
            from,
            to: from,
            steps: Vec::new(),
            ahead: true,
        };
    }

    let mut steps = Vec::new();
    for migration in MIGRATIONS.iter().filter(|m| m.from >= from) {
        let changes = (migration.apply)(doc);
        steps.push(Step {
            from: migration.from,
            to: migration.to,
            summary: migration.summary.to_string(),
            changes,
        });
    }
    doc.insert(
        SCHEMA_VERSION_KEY.to_string(),
        toml::Value::Integer(i64::from(CURRENT_SCHEMA_VERSION)),
    );
    Report {
        from,
        to: CURRENT_SCHEMA_VERSION,
        steps,
        ahead: false,
    }
}

/// What [`migrate`] would do to this file, without touching anything.
///
/// The dry-run behind `rolter check --migrations`. Parses, migrates a throwaway
/// copy and returns the report.
pub fn plan(raw: &str) -> Result<Report> {
    let mut doc: toml::Table = toml::from_str(raw)?;
    Ok(migrate(&mut doc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GatewayConfig;

    /// A file in the deprecated flat-array shape, as an operator on an older
    /// release actually has it on disk.
    const LEGACY: &str = r#"
        [[providers]]
        name = "openai-main"
        kind = "openai"
        api_base = "https://api.openai.com"
        api_key_env = "OPENAI_API_KEY"

        [[providers]]
        name = "anthropic-main"
        kind = "anthropic"
        api_base = "https://api.anthropic.com"
        api_key_env = "ANTHROPIC_API_KEY"

        [[provider_groups]]
        name = "frontier"
        [[provider_groups.members]]
        provider = "openai-main"

        [[routes]]
        model = "chat"
        [[routes.targets]]
        provider = "openai-main"
    "#;

    /// The same deployment written in the current shape.
    const CURRENT: &str = r#"
        schema_version = 3

        [providers]
        readonly = [
            { name = "openai-main", kind = "openai", api_base = "https://api.openai.com", api_keys = [{ env = "OPENAI_API_KEY" }] },
            { name = "anthropic-main", kind = "anthropic", api_base = "https://api.anthropic.com", api_keys = [{ env = "ANTHROPIC_API_KEY" }] },
        ]

        [provider_groups]
        readonly = [
            { name = "frontier", members = [{ provider = "openai-main" }] },
        ]

        [[routes]]
        model = "chat"
        [[routes.targets]]
        provider = "openai-main"
    "#;

    fn table(raw: &str) -> toml::Table {
        toml::from_str(raw).expect("fixture parses")
    }

    #[test]
    fn the_chain_is_contiguous_and_reaches_the_current_version() {
        // a step that skips a version would never run for a document sitting on
        // the version it skipped, and the failure would be silent
        for pair in MIGRATIONS.windows(2) {
            assert_eq!(
                pair[0].to, pair[1].from,
                "gap in the migration chain between {} and {}",
                pair[0].to, pair[1].from
            );
        }
        for m in MIGRATIONS {
            assert_eq!(m.to, m.from + 1, "a step must advance exactly one version");
        }
        let last = MIGRATIONS.last().expect("at least one migration");
        assert_eq!(
            last.to, CURRENT_SCHEMA_VERSION,
            "the chain must end at CURRENT_SCHEMA_VERSION; bump the constant when adding a step"
        );
        assert_eq!(
            MIGRATIONS.first().expect("at least one").from,
            PRE_STAMP_SCHEMA_VERSION,
            "the chain must start where an unstamped file starts"
        );
    }

    #[test]
    fn an_unstamped_file_is_migrated_and_stamped() {
        let mut doc = table(LEGACY);
        let report = migrate(&mut doc);

        assert_eq!(report.from, PRE_STAMP_SCHEMA_VERSION);
        assert_eq!(report.to, CURRENT_SCHEMA_VERSION);
        assert!(!report.ahead);
        assert!(
            !report.is_empty(),
            "the legacy fixture has something to migrate"
        );

        assert_eq!(
            doc.get(SCHEMA_VERSION_KEY)
                .and_then(toml::Value::as_integer),
            Some(i64::from(CURRENT_SCHEMA_VERSION))
        );
        // the flat arrays became tiered tables
        assert!(doc["providers"].is_table());
        assert!(doc["providers"]["readonly"].is_array());
        assert!(doc["provider_groups"]["readonly"].is_array());
    }

    /// The property that makes a migration safe to run on every load.
    #[test]
    fn migration_is_behaviour_preserving() {
        let before = GatewayConfig::from_toml_str(LEGACY).expect("legacy parses");

        let mut doc = table(LEGACY);
        migrate(&mut doc);
        let migrated_raw = toml::to_string(&doc).expect("re-serialises");
        let after = GatewayConfig::from_toml_str(&migrated_raw).expect("migrated parses");

        assert_eq!(
            before.providers.len(),
            after.providers.len(),
            "provider count changed"
        );
        assert_eq!(
            before.providers.iter().map(|p| &p.name).collect::<Vec<_>>(),
            after.providers.iter().map(|p| &p.name).collect::<Vec<_>>(),
        );
        assert_eq!(
            before.provider_defaults.len(),
            after.provider_defaults.len()
        );
        assert_eq!(
            before
                .provider_groups
                .iter()
                .map(|g| &g.name)
                .collect::<Vec<_>>(),
            after
                .provider_groups
                .iter()
                .map(|g| &g.name)
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            before.routes.iter().map(|r| &r.model).collect::<Vec<_>>(),
            after.routes.iter().map(|r| &r.model).collect::<Vec<_>>(),
        );
    }

    /// Acceptance criterion: old config in, current config out.
    #[test]
    fn a_legacy_file_loads_identically_to_its_current_spelling() {
        let legacy = GatewayConfig::from_toml_str(LEGACY).expect("legacy parses");
        let current = GatewayConfig::from_toml_str(CURRENT).expect("current parses");

        assert_eq!(
            legacy.providers.iter().map(|p| &p.name).collect::<Vec<_>>(),
            current
                .providers
                .iter()
                .map(|p| &p.name)
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            legacy
                .provider_groups
                .iter()
                .map(|g| &g.name)
                .collect::<Vec<_>>(),
            current
                .provider_groups
                .iter()
                .map(|g| &g.name)
                .collect::<Vec<_>>(),
        );
        // both report the version they were understood as, not what was on disk
        assert_eq!(legacy.schema_version, Some(CURRENT_SCHEMA_VERSION));
        assert_eq!(current.schema_version, Some(CURRENT_SCHEMA_VERSION));
    }

    /// Every spelling of a provider's credential and proxy, as an operator on a
    /// version 2 build has them on disk.
    const SINGULAR: &str = r#"
        schema_version = 2

        [providers]
        readonly = [
            { name = "env-only", kind = "openai", api_base = "https://a.example", api_key_env = "ROLTER_TEST_ENV" },
            { name = "inline-only", kind = "openai", api_base = "https://b.example", api_key = "sk-inline" },
            { name = "both", kind = "openai", api_base = "https://c.example", api_key = "sk-inline", api_key_env = "ROLTER_TEST_ENV" },
            { name = "proxied", kind = "openai", api_base = "https://d.example", api_key_env = "ROLTER_TEST_ENV", egress_proxy = "http://proxy:3128" },
            { name = "already-plural", kind = "openai", api_base = "https://e.example", api_keys = [{ env = "ROLTER_TEST_ENV", weight = 3 }], egress_proxies = ["http://one:1", "http://two:2"] },
            { name = "plural-beside-singular", kind = "openai", api_base = "https://f.example", api_key_env = "IGNORED_ENV", api_keys = [{ env = "ROLTER_TEST_ENV" }], egress_proxy = "http://ignored:1", egress_proxies = ["http://used:2"] },
            { name = "bare", kind = "ollama", api_base = "http://localhost:11434" },
        ]
        default = [
            { name = "seeded", kind = "openai", api_base = "https://g.example", api_key_env = "ROLTER_TEST_ENV" },
        ]
    "#;

    #[test]
    fn the_singular_spellings_become_the_plural_ones() {
        let mut doc = table(SINGULAR);
        let report = migrate(&mut doc);
        assert_eq!((report.from, report.to), (2, CURRENT_SCHEMA_VERSION));

        let readonly = doc["providers"]["readonly"].as_array().expect("readonly");
        let by_name = |name: &str| {
            readonly
                .iter()
                .chain(doc["providers"]["default"].as_array().expect("default"))
                .find(|p| p["name"].as_str() == Some(name))
                .and_then(toml::Value::as_table)
                .unwrap_or_else(|| panic!("no provider {name}"))
        };

        let env_only = by_name("env-only");
        assert!(!env_only.contains_key("api_key_env"));
        assert_eq!(
            env_only["api_keys"].to_string(),
            r#"[{ env = "ROLTER_TEST_ENV" }]"#
        );
        let inline = by_name("inline-only");
        assert!(!inline.contains_key("api_key"));
        assert_eq!(inline["api_keys"].to_string(), r#"[{ key = "sk-inline" }]"#);
        let both = by_name("both");
        assert!(!both.contains_key("api_key") && !both.contains_key("api_key_env"));
        assert_eq!(both["api_keys"].as_array().expect("one entry").len(), 1);
        let proxied = by_name("proxied");
        assert!(!proxied.contains_key("egress_proxy"));
        assert_eq!(
            proxied["egress_proxies"].to_string(),
            r#"["http://proxy:3128"]"#
        );
        // the default tier is rewritten too
        assert!(by_name("seeded").contains_key("api_keys"));
        // a provider with a plural list keeps its weights, and the singular
        // fields that were ignored beside it stay for the lint to report
        let plural = by_name("already-plural");
        assert_eq!(plural["api_keys"][0]["weight"].as_integer(), Some(3));
        assert_eq!(plural["egress_proxies"].as_array().map(Vec::len), Some(2));
        let beside = by_name("plural-beside-singular");
        assert_eq!(beside["api_key_env"].as_str(), Some("IGNORED_ENV"));
        assert_eq!(beside["egress_proxy"].as_str(), Some("http://ignored:1"));
        assert_eq!(beside["api_keys"].as_array().map(Vec::len), Some(1));
        // a provider with neither is untouched
        assert!(!by_name("bare").contains_key("api_keys"));
    }

    /// The property the issue asks for: the migrated document resolves to the
    /// same credentials and proxies as the document it came from.
    #[test]
    fn the_plural_rewrite_resolves_to_the_same_credentials_and_proxies() {
        let before = GatewayConfig::from_toml_str(SINGULAR).expect("version 2 parses");
        let mut doc = table(SINGULAR);
        migrate(&mut doc);
        let migrated_raw = toml::to_string(&doc).expect("re-serialises");
        let after = GatewayConfig::from_toml_str(&migrated_raw).expect("migrated parses");

        // from_toml_str migrates, so `before` is already the plural form; the
        // raw table is what keeps the singular spelling for the comparison
        let mut singular: toml::Table = table(SINGULAR);
        singular.remove(SCHEMA_VERSION_KEY);
        let mut original = Vec::new();
        for tier in ["readonly", "default"] {
            for entry in singular["providers"][tier].as_array().expect("tier") {
                original.push(
                    entry
                        .clone()
                        .try_into::<crate::ProviderConfig>()
                        .expect("provider parses as written"),
                );
            }
        }
        let migrated: Vec<_> = after
            .providers
            .iter()
            .chain(&after.provider_defaults)
            .collect();
        assert_eq!(original.len(), migrated.len());
        for (was, now) in original.iter().zip(&migrated) {
            assert_eq!(was.name, now.name);
            assert_eq!(
                was.api_key_env_name(),
                now.api_key_env_name(),
                "{}",
                was.name
            );
            assert_eq!(was.inline_api_key(), now.inline_api_key(), "{}", was.name);
            assert_eq!(
                was.egress_proxy_pool(),
                now.egress_proxy_pool(),
                "{}",
                was.name
            );
            assert_eq!(
                was.resolve_api_keys(),
                now.resolve_api_keys(),
                "{}",
                was.name
            );
            assert_eq!(
                was.surplus_api_key_count(),
                now.surplus_api_key_count(),
                "{}",
                was.name
            );
        }
        // and the loader hands back the migrated form for the version 2 file
        assert_eq!(before.providers.len(), after.providers.len());
        assert!(before
            .providers
            .iter()
            .all(|p| p.api_key_env.is_none() || p.name == "plural-beside-singular"));
    }

    #[test]
    fn a_migrated_singular_file_is_up_to_date() {
        let mut doc = table(SINGULAR);
        migrate(&mut doc);
        let migrated_raw = toml::to_string(&doc).expect("re-serialises");

        let report = plan(&migrated_raw).expect("plans");
        assert_eq!(report.from, CURRENT_SCHEMA_VERSION);
        assert!(
            report.is_empty(),
            "a migrated document must be a fixed point: {:?}",
            report.changes().collect::<Vec<_>>()
        );
        // and a version 2 file with singular spellings is not
        let pending = plan(SINGULAR).expect("plans");
        assert!(pending.changes().any(|c| c.path.ends_with(".api_key_env")));
        assert!(pending.changes().any(|c| c.path.ends_with(".egress_proxy")));
    }

    #[test]
    fn the_flat_provider_array_is_rewritten_too() {
        // a hand-written version 2 file may still carry the deprecated array
        let raw = r#"
            schema_version = 2
            [[providers]]
            name = "flat"
            kind = "openai"
            api_base = "https://a.example"
            api_key_env = "FLAT_KEY"
        "#;
        let mut doc = table(raw);
        let report = migrate(&mut doc);
        assert!(report
            .changes()
            .any(|c| c.path == "providers[0].api_key_env"));
        assert!(doc["providers"][0]["api_keys"].is_array());
    }

    #[test]
    fn migrating_twice_changes_nothing_the_second_time() {
        let mut doc = table(LEGACY);
        migrate(&mut doc);
        let once = doc.clone();

        let second = migrate(&mut doc);
        assert!(
            second.is_empty(),
            "a migrated document must be a fixed point: {:?}",
            second.changes().collect::<Vec<_>>()
        );
        assert_eq!(once, doc, "the second pass rewrote the document");
    }

    #[test]
    fn a_file_from_the_future_loads_unchanged_instead_of_failing() {
        let raw = format!("schema_version = {}\n", CURRENT_SCHEMA_VERSION + 7);
        let mut doc = table(&raw);
        let before = doc.clone();

        let report = migrate(&mut doc);

        assert!(report.ahead, "a newer stamp must be reported as ahead");
        assert_eq!(report.from, CURRENT_SCHEMA_VERSION + 7);
        assert_eq!(
            report.to,
            CURRENT_SCHEMA_VERSION + 7,
            "the document keeps its own version rather than being downgraded"
        );
        assert_eq!(
            before, doc,
            "a document from the future must not be rewritten"
        );
        // and the loader still accepts it: a rollback is not an outage
        assert!(GatewayConfig::from_toml_str(&raw).is_ok());
    }

    #[test]
    fn a_malformed_stamp_is_treated_as_absent() {
        for raw in [
            "schema_version = \"two\"\n",
            "schema_version = 0\n",
            "schema_version = -3\n",
        ] {
            let doc = table(raw);
            assert_eq!(
                schema_version_of(&doc),
                PRE_STAMP_SCHEMA_VERSION,
                "malformed stamp in {raw:?} should fall back to the pre-stamp version"
            );
        }
    }

    #[test]
    fn plan_reports_without_touching_the_input() {
        let report = plan(LEGACY).expect("plans");
        assert_eq!(report.from, PRE_STAMP_SCHEMA_VERSION);
        assert!(!report.is_empty());
        assert!(report.changes().any(|c| c.path == "providers"));
        assert!(report.changes().any(|c| c.path == "provider_groups"));

        // an already-current file has nothing pending
        assert!(plan(CURRENT).expect("plans").is_empty());
    }

    /// `N -> N+k` must be the same fold as a sequence of single hops. Proven
    /// against synthetic steps rather than the real chain, which is one entry
    /// long today and so cannot exercise skipping on its own.
    #[test]
    fn a_multi_version_bump_folds_the_whole_chain() {
        fn stamp(doc: &mut toml::Table, key: &str) -> Vec<Change> {
            doc.insert(key.to_string(), toml::Value::Boolean(true));
            vec![Change {
                path: key.to_string(),
                detail: format!("set {key}"),
            }]
        }
        fn step_a(doc: &mut toml::Table) -> Vec<Change> {
            stamp(doc, "a")
        }
        fn step_b(doc: &mut toml::Table) -> Vec<Change> {
            stamp(doc, "b")
        }
        fn step_c(doc: &mut toml::Table) -> Vec<Change> {
            stamp(doc, "c")
        }

        let chain = [
            Migration {
                from: 1,
                to: 2,
                summary: "a",
                apply: step_a,
            },
            Migration {
                from: 2,
                to: 3,
                summary: "b",
                apply: step_b,
            },
            Migration {
                from: 3,
                to: 4,
                summary: "c",
                apply: step_c,
            },
        ];

        // a document three versions behind runs every step, in order
        let mut doc = toml::Table::new();
        let from = schema_version_of(&doc);
        let applied: Vec<_> = chain
            .iter()
            .filter(|m| m.from >= from)
            .map(|m| {
                (m.apply)(&mut doc);
                m.summary
            })
            .collect();
        assert_eq!(
            applied,
            vec!["a", "b", "c"],
            "skipping releases must not skip steps"
        );
        assert!(doc.contains_key("a") && doc.contains_key("b") && doc.contains_key("c"));

        // a document one version behind runs only the last step
        let mut doc = toml::Table::new();
        doc.insert(SCHEMA_VERSION_KEY.to_string(), toml::Value::Integer(3));
        let from = schema_version_of(&doc);
        let applied: Vec<_> = chain
            .iter()
            .filter(|m| m.from >= from)
            .map(|m| {
                (m.apply)(&mut doc);
                m.summary
            })
            .collect();
        assert_eq!(applied, vec!["c"]);
        assert!(
            !doc.contains_key("a"),
            "an already-applied step must not re-run"
        );
    }

    /// The lint reads the file as written. The version 3 step consumes the
    /// singular provider keys, which the config types still accept, so neither
    /// the written file nor its migrated form reports an unrecognised key. A step
    /// that removed a key from the types would break this — see the module docs.
    #[test]
    fn no_migration_consumes_a_key_the_lint_would_then_call_unrecognised() {
        let migrated = {
            let mut doc = table(LEGACY);
            migrate(&mut doc);
            toml::to_string(&doc).expect("re-serialises")
        };
        for fixture in [LEGACY, CURRENT, migrated.as_str()] {
            let unknown = crate::config_lint::unknown_keys(fixture).expect("lints");
            assert!(
                unknown.is_empty(),
                "fixture reports unrecognised keys, so a migration and the lint now disagree: \
                 {unknown:?}"
            );
        }
    }
}
