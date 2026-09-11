//! Finding keys in a `rolter.toml` that no config struct claims (#1424).
//!
//! rolter's config types deliberately do not use `deny_unknown_fields`, and
//! that is load-bearing: a file written for `X.y.z` has to stay loadable by
//! every other `X.*` build, including an older one it gets rolled back onto.
//! Rejecting a key the running binary has not heard of would turn a downgrade
//! into an outage.
//!
//! The cost of that promise is that `connect_timout_ms` is not a typo, it is
//! silence — the default applies, nothing is logged, and the operator is left
//! debugging a timeout they believe they configured.
//!
//! So the detection lives here rather than in the deserialization path: the
//! document is parsed a second time through [`serde_ignored`], which reports
//! every key the derived `Deserialize` impls dropped. Nothing in this module
//! changes what the gateway or the control plane accept at runtime — it only
//! gives the loader something to say.
//!
//! Using the real `Deserialize` impls is the point. A hand-maintained list of
//! valid keys would be wrong within a release, and wrong in the direction that
//! reports a brand-new key as a typo.
//!
//! [`warn_unknown_keys_once`] is the reporting half, and it hangs off
//! [`GatewayConfig::load`] rather than off any one binary (#1434). An operator
//! who mistypes a key does not run `rolter check` first — they restart the
//! gateway and watch the log, so that is where the sentence has to appear.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::config::{GatewayConfig, ProviderConfig, ProviderGroupConfig};
use crate::error::Result;

/// The two config sections that are pre-extracted from the raw document by
/// [`GatewayConfig::from_toml_str`] before it deserializes the rest, because a
/// single key accepts both the deprecated `[[providers]]` array and the tiered
/// `[providers] readonly/default` table (ADR-0022). They never reach the
/// `GatewayConfig` deserializer, so the lint has to walk them itself.
const TIERED_SECTIONS: [&str; 2] = ["providers", "provider_groups"];

/// The tier keys a `[providers]` / `[provider_groups]` table may carry.
const TIER_KEYS: [&str; 2] = ["readonly", "default"];

/// Longest edit distance that still counts as "you probably meant this".
///
/// Three would start suggesting `model` for `weight`; one alone would miss a
/// transposition plus a dropped letter, which is what a real typo looks like.
const MAX_SUGGESTION_DISTANCE: usize = 2;

/// One key in the file that no config struct claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownKey {
    /// dotted path to the key, with array elements indexed:
    /// `providers[0].connect_timout_ms`
    pub path: String,
    /// nearest recognised key at the same level, when one is close enough to
    /// be a plausible typo rather than a coincidence
    pub suggestion: Option<String>,
}

impl std::fmt::Display for UnknownKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path)?;
        if let Some(suggestion) = &self.suggestion {
            write!(f, " (did you mean `{suggestion}`?)")?;
        }
        Ok(())
    }
}

/// One step of a path through the document.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Key(String),
    Index(usize),
}

/// Render a path the way an operator would write it back into the file.
fn render(segments: &[Segment]) -> String {
    let mut out = String::new();
    for segment in segments {
        match segment {
            Segment::Key(key) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(key);
            }
            Segment::Index(index) => {
                out.push('[');
                out.push_str(&index.to_string());
                out.push(']');
            }
        }
    }
    out
}

/// Report every key in `toml_src` that the config model ignores.
///
/// Returns an error only when the document is not valid TOML at all; a file
/// that parses but carries nonsense keys is a `Ok(vec![...])`, because the file
/// *is* still valid — that is exactly the guarantee this module exists to
/// preserve.
pub fn unknown_keys(toml_src: &str) -> Result<Vec<UnknownKey>> {
    let mut doc: toml::Value = toml::from_str(toml_src)?;
    let mut findings = Vec::new();

    if let Some(table) = doc.as_table_mut() {
        for section in TIERED_SECTIONS {
            let Some(value) = table.remove(section) else {
                continue;
            };
            if section == "providers" {
                lint_tiered_section::<ProviderConfig>(value, section, &mut findings);
            } else {
                lint_tiered_section::<ProviderGroupConfig>(value, section, &mut findings);
            }
        }
    }

    lint_value::<GatewayConfig>(doc, &[], &mut findings);
    findings.sort_by(|a, b| a.path.cmp(&b.path));
    findings.dedup();
    Ok(findings)
}

/// What one unrecognised key means, in the words every surface uses for it.
///
/// `rolter check` prints this as a finding's detail and every load path logs it
/// as a warning, so an operator who meets the same typo twice reads the same
/// sentence twice rather than two descriptions they have to reconcile.
pub fn describe(key: &UnknownKey) -> String {
    let mut detail = format!(
        "`{}` is not a key rolter reads. It is ignored rather than rejected, so the setting it \
         looks like it configures is silently at its default.",
        key.path
    );
    if let Some(suggestion) = &key.suggestion {
        detail.push_str(&format!(" Did you mean `{suggestion}`?"));
    }
    detail
}

/// Log one warning per key in `toml_src` that the config model ignores, naming
/// `path` so a deployment with several config files says which one.
///
/// Returns how many keys were reported, which is what makes the emission
/// testable without capturing a subscriber.
///
/// Never fatal and never fallible: a file that is not TOML at all reports
/// nothing here and is left to the loader, which fails with one clear error
/// rather than a pile of speculative key warnings on top of it.
pub fn warn_unknown_keys(path: &Path, toml_src: &str) -> usize {
    let Ok(findings) = unknown_keys(toml_src) else {
        return 0;
    };
    for key in &findings {
        tracing::warn!(
            config = %path.display(),
            key = %key.path,
            "unrecognised config key: {}",
            describe(key)
        );
    }
    findings.len()
}

/// [`warn_unknown_keys`], but at most once per file for the life of the
/// process.
///
/// `rolter easy-up` runs the seed importer, the control plane and the gateway
/// in one process, and all three read the same `rolter.toml`. Reporting one
/// typo three times reads like three problems, and the third copy is the one
/// an operator scrolls past.
pub fn warn_unknown_keys_once(path: &Path, toml_src: &str) -> usize {
    static SEEN: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    // canonicalize so `./rolter.toml` and `rolter.toml` are one file; a path
    // that cannot be resolved is still worth deduplicating as written
    let identity = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let first_time = SEEN
        .get_or_init(Mutex::default)
        .lock()
        // a poisoned lock here would silence the warning for the rest of the
        // process, which is the failure this whole module exists to prevent
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(identity);
    if !first_time {
        return 0;
    }
    warn_unknown_keys(path, toml_src)
}

/// Walk a section that accepts either a flat array of entries or a
/// `{ readonly = [...], default = [...] }` table.
fn lint_tiered_section<T>(value: toml::Value, section: &str, out: &mut Vec<UnknownKey>)
where
    T: DeserializeOwned + Serialize,
{
    match value {
        toml::Value::Array(entries) => {
            for (index, entry) in entries.into_iter().enumerate() {
                let prefix = [Segment::Key(section.to_string()), Segment::Index(index)];
                lint_value::<T>(entry, &prefix, out);
            }
        }
        toml::Value::Table(table) => {
            for (key, tier) in table {
                if !TIER_KEYS.contains(&key.as_str()) {
                    // `split_section` drops anything that is not a tier, so a
                    // misspelled tier name silently seeds nothing at all
                    out.push(UnknownKey {
                        path: render(&[
                            Segment::Key(section.to_string()),
                            Segment::Key(key.clone()),
                        ]),
                        suggestion: nearest(&key, TIER_KEYS.iter().copied()),
                    });
                    continue;
                }
                let toml::Value::Array(entries) = tier else {
                    continue;
                };
                for (index, entry) in entries.into_iter().enumerate() {
                    let prefix = [
                        Segment::Key(section.to_string()),
                        Segment::Key(key.clone()),
                        Segment::Index(index),
                    ];
                    lint_value::<T>(entry, &prefix, out);
                }
            }
        }
        // neither shape: `from_toml_str` rejects this outright, and an error
        // beats a pile of warnings about a section that will not load
        _ => {}
    }
}

/// Deserialize `value` as `T`, recording every key `T` ignored.
///
/// `prefix` is prepended to each reported path so a caller that split the
/// document up still reports paths relative to the whole file.
fn lint_value<T>(value: toml::Value, prefix: &[Segment], out: &mut Vec<UnknownKey>)
where
    T: DeserializeOwned + Serialize,
{
    let mut ignored: Vec<Vec<Segment>> = Vec::new();
    let parsed: std::result::Result<T, _> =
        serde_ignored::deserialize(value, |path| ignored.push(segments(&path)));
    if ignored.is_empty() {
        return;
    }

    // the parsed value serialized back out is the only description of "what
    // this type accepts" that cannot drift from the type itself. it under-reports
    // where a field is `skip_serializing_if` and absent, which costs a
    // suggestion but never invents a wrong one.
    //
    // json rather than toml: a `None` field has no TOML representation, so the
    // toml serializer drops the whole value, and the fields most worth
    // suggesting (`api_key_env`, `slug`) are exactly the optional ones
    let known = parsed
        .ok()
        .and_then(|parsed| serde_json::to_value(parsed).ok());

    for path in ignored {
        let Some((Segment::Key(key), parent)) = path.split_last() else {
            // an ignored array element rather than a key; nothing to name
            continue;
        };
        let suggestion = known
            .as_ref()
            .and_then(|known| lookup(known, parent))
            .and_then(|object| nearest(key, object.keys().map(String::as_str)));
        let mut full = prefix.to_vec();
        full.extend(path.iter().cloned());
        out.push(UnknownKey {
            path: render(&full),
            suggestion,
        });
    }
}

/// Follow `path` into `value`, returning the object it lands on.
fn lookup<'v>(
    value: &'v serde_json::Value,
    path: &[Segment],
) -> Option<&'v serde_json::Map<String, serde_json::Value>> {
    let mut current = value;
    for segment in path {
        current = match segment {
            Segment::Key(key) => current.as_object()?.get(key)?,
            Segment::Index(index) => current.as_array()?.get(*index)?,
        };
    }
    current.as_object()
}

/// Flatten a [`serde_ignored::Path`] into segments, dropping the wrapper
/// variants that describe how a value was reached rather than where it lives.
fn segments(path: &serde_ignored::Path<'_>) -> Vec<Segment> {
    match path {
        serde_ignored::Path::Root => Vec::new(),
        serde_ignored::Path::Seq { parent, index } => {
            let mut out = segments(parent);
            out.push(Segment::Index(*index));
            out
        }
        serde_ignored::Path::Map { parent, key } => {
            let mut out = segments(parent);
            out.push(Segment::Key(key.clone()));
            out
        }
        serde_ignored::Path::Some { parent }
        | serde_ignored::Path::NewtypeStruct { parent }
        | serde_ignored::Path::NewtypeVariant { parent } => segments(parent),
    }
}

/// Closest candidate to `key`, or `None` when nothing is close enough.
fn nearest<'a>(key: &str, candidates: impl Iterator<Item = &'a str>) -> Option<String> {
    candidates
        .filter(|candidate| *candidate != key)
        .map(|candidate| (edit_distance(key, candidate), candidate))
        // a short key is close to everything, so scale the budget down with it
        .filter(|(distance, candidate)| {
            *distance <= MAX_SUGGESTION_DISTANCE.min(key.len().max(candidate.len()) / 2)
        })
        .min_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)))
        .map(|(_, candidate)| candidate.to_string())
}

/// Levenshtein distance, two rows rather than a full matrix.
///
/// Hand-rolled because the alternative is a dependency for forty lines used in
/// exactly one place, on strings that are never longer than a config key.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, ac) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, bc) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ac != bc);
            let insertion = current[j] + 1;
            let deletion = previous[j + 1] + 1;
            current[j + 1] = substitution.min(insertion).min(deletion);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(src: &str) -> Vec<String> {
        unknown_keys(src)
            .expect("valid toml")
            .into_iter()
            .map(|finding| finding.path)
            .collect()
    }

    #[test]
    fn a_clean_config_reports_nothing() {
        let src = r#"
            [server]
            host = "127.0.0.1"
            port = 8080

            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com/v1"
            api_key_env = "OPENAI_API_KEY"

            [[routes]]
            model = "gpt-4o"
            strategy = "power_of_two"
            [[routes.targets]]
            provider = "openai"
            model = "gpt-4o"
            weight = 3
        "#;
        assert_eq!(paths(src), Vec::<String>::new());
    }

    #[test]
    fn the_shipped_example_config_is_clean() {
        // if the file every operator copies from carries a key rolter ignores,
        // this lint would be teaching the typo rather than catching it
        let src = include_str!("../../../rolter.example.toml");
        let findings = unknown_keys(src).expect("the example config must parse");
        assert!(
            findings.is_empty(),
            "rolter.example.toml has unrecognised keys: {:?}",
            findings.iter().map(ToString::to_string).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_top_level_typo_is_reported_with_a_suggestion() {
        let findings = unknown_keys(
            r#"
            [server]
            host = "0.0.0.0"
            prot = 8080
        "#,
        )
        .expect("valid toml");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].path, "server.prot");
        assert_eq!(findings[0].suggestion.as_deref(), Some("port"));
    }

    #[test]
    fn an_unknown_top_level_table_is_reported_once() {
        // one finding for the table, not one per key inside it: the operator
        // has a section rolter has never heard of, and listing its contents
        // buries that
        let findings = unknown_keys(
            r#"
            [balancing]
            strategy = "round_robin"
            sticky = true
        "#,
        )
        .expect("valid toml");
        assert_eq!(
            findings.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["balancing"]
        );
    }

    #[test]
    fn a_typo_inside_an_array_of_tables_carries_its_index() {
        // a misspelled `api_key_env` is the sharpest version of the #1424
        // complaint: the provider loads, has no credential, and every call to
        // it fails upstream with nothing pointing at the config
        let findings = unknown_keys(
            r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com/v1"
            api_key_env = "OPENAI_API_KEY"

            [[providers]]
            name = "local"
            kind = "openai_compatible"
            api_base = "http://127.0.0.1:8000/v1"
            api_key_evn = "LOCAL_API_KEY"
        "#,
        )
        .expect("valid toml");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].path, "providers[1].api_key_evn");
        assert_eq!(findings[0].suggestion.as_deref(), Some("api_key_env"));
    }

    #[test]
    fn the_motivating_typo_from_the_issue_is_reported() {
        // `connect_timout_ms` from #1424, adjusted to the key rolter actually
        // has: the timeout section is global, and a typo there silently leaves
        // the default in place
        let findings = unknown_keys(
            r#"
            [timeouts]
            connect_sesc = 3
        "#,
        )
        .expect("valid toml");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(
            findings[0].to_string(),
            "timeouts.connect_sesc (did you mean `connect_secs`?)"
        );
    }

    #[test]
    fn the_tiered_provider_form_is_walked_too() {
        let findings = unknown_keys(
            r#"
            [providers]
            default = [
              { name = "openai", kind = "openai", api_base = "https://api.openai.com/v1", api_key_env = "OPENAI_API_KEY", weght = 2 },
            ]
        "#,
        )
        .expect("valid toml");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].path, "providers.default[0].weght");
    }

    #[test]
    fn a_misspelled_tier_is_reported_rather_than_seeding_nothing() {
        let findings = unknown_keys(
            r#"
            [providers]
            defualt = [
              { name = "openai", kind = "openai", api_base = "https://api.openai.com/v1", api_key_env = "OPENAI_API_KEY" },
            ]
        "#,
        )
        .expect("valid toml");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].path, "providers.defualt");
        assert_eq!(findings[0].suggestion.as_deref(), Some("default"));
    }

    #[test]
    fn a_nested_table_typo_keeps_its_full_path() {
        let findings = unknown_keys(
            r#"
            [[routes]]
            model = "gpt-4o"
            [routes.cache]
            enabled = true
            ttl_secs = 60
            tll_secs = 60
        "#,
        )
        .expect("valid toml");
        assert_eq!(
            findings.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["routes[0].cache.tll_secs"]
        );
    }

    #[test]
    fn deeply_nested_targets_are_covered() {
        let findings = unknown_keys(
            r#"
            [[routes]]
            model = "gpt-4o"
            [[routes.targets]]
            provider = "openai"
            wieght = 2
        "#,
        )
        .expect("valid toml");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].path, "routes[0].targets[0].wieght");
        assert_eq!(findings[0].suggestion.as_deref(), Some("weight"));
    }

    #[test]
    fn free_form_maps_never_produce_a_finding() {
        // route `params` is an open map of provider parameters; every key in it
        // is by definition recognised, and flagging one would be a false alarm
        // on a correct config
        let findings = unknown_keys(
            r#"
            [[routes]]
            model = "gpt-4o"
            [routes.params]
            top_p = 0.9
            anything_at_all = "fine"
        "#,
        )
        .expect("valid toml");
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn an_unrelated_key_gets_no_suggestion() {
        let findings = unknown_keys(
            r#"
            [server]
            kubernetes_namespace = "prod"
        "#,
        )
        .expect("valid toml");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].suggestion, None);
    }

    #[test]
    fn findings_are_ordered_by_path_so_the_report_is_stable() {
        let findings = unknown_keys(
            r#"
            zzz_unknown = 1
            aaa_unknown = 2

            [server]
            prot = 1
        "#,
        )
        .expect("valid toml");
        assert_eq!(
            findings.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["aaa_unknown", "server.prot", "zzz_unknown"]
        );
    }

    #[test]
    fn malformed_toml_is_an_error_not_a_finding() {
        assert!(unknown_keys("this is not toml").is_err());
    }

    #[test]
    fn a_config_with_unknown_keys_still_loads() {
        // the guarantee this whole module is built around: reporting the key
        // must not change whether the file is accepted
        let src = r#"
            [server]
            port = 8080
            prot = 9090
        "#;
        let config = GatewayConfig::from_toml_str(src).expect("still valid");
        assert_eq!(config.server.port, 8080);
        assert_eq!(paths(src), ["server.prot"]);
    }

    /// Collects whatever a `tracing` subscriber formats, so the warnings can be
    /// read back as text rather than trusted from a return value alone.
    #[derive(Clone, Default)]
    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Capture {
        fn text(&self) -> String {
            String::from_utf8(
                self.0
                    .lock()
                    .expect("no test panicked holding this")
                    .clone(),
            )
            .expect("tracing writes utf-8")
        }
    }

    impl std::io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("no test panicked holding this")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
        type Writer = Capture;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Run `f` with a warn-level subscriber and hand back what it wrote.
    fn captured(f: impl FnOnce()) -> String {
        let capture = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(capture.clone())
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        capture.text()
    }

    /// A path of its own per test: [`warn_unknown_keys_once`] keeps
    /// process-wide state keyed by path, so a shared name would make one test's
    /// result depend on whether another ran first.
    fn scratch_path(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rolter-config-lint-{label}-{}-{:?}.toml",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    const TYPO_CONFIG: &str = r#"
        [server]
        port = 8080
        prot = 9090

        [[providers]]
        name = "local"
        kind = "openai_compatible"
        api_base = "http://127.0.0.1:8000/v1"
        api_key_evn = "LOCAL_API_KEY"
        "#;

    #[test]
    fn each_unrecognised_key_is_logged_with_its_file_and_a_suggestion() {
        let path = scratch_path("logged");
        let text = captured(|| {
            assert_eq!(warn_unknown_keys(&path, TYPO_CONFIG), 2);
        });

        assert!(text.contains("WARN"), "must be a warning, not info: {text}");
        assert!(text.contains("server.prot"), "{text}");
        assert!(text.contains("providers[0].api_key_evn"), "{text}");
        assert!(text.contains("Did you mean `api_key_env`?"), "{text}");
        // the file is named: a deployment can have more than one
        assert!(text.contains(&path.display().to_string()), "{text}");
    }

    #[test]
    fn a_clean_config_logs_nothing_at_all() {
        let path = scratch_path("clean");
        let text = captured(|| {
            assert_eq!(warn_unknown_keys(&path, "[server]\nport = 8080\n"), 0);
        });
        assert!(text.is_empty(), "{text}");
    }

    #[test]
    fn a_file_that_is_not_toml_is_left_to_the_loader() {
        // the load that follows fails with one clear error; burying it under
        // speculative key warnings helps nobody
        let path = scratch_path("not-toml");
        let text = captured(|| {
            assert_eq!(warn_unknown_keys(&path, "{ not toml at all"), 0);
        });
        assert!(text.is_empty(), "{text}");
    }

    #[test]
    fn the_same_file_is_only_reported_once_per_process() {
        // easy-up seeds, then starts the control plane, then starts the gateway,
        // all from one rolter.toml in one process
        let path = scratch_path("once");
        std::fs::write(&path, TYPO_CONFIG).expect("temp dir is writable");

        let first = captured(|| assert_eq!(warn_unknown_keys_once(&path, TYPO_CONFIG), 2));
        assert!(first.contains("server.prot"), "{first}");

        let second = captured(|| assert_eq!(warn_unknown_keys_once(&path, TYPO_CONFIG), 0));
        assert!(second.is_empty(), "{second}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn loading_a_file_with_unknown_keys_warns_and_still_starts() {
        // the whole guarantee in one test: the warning is emitted by the real
        // load path every binary uses, and the load still succeeds
        let path = scratch_path("load");
        std::fs::write(&path, TYPO_CONFIG).expect("temp dir is writable");

        let mut config = None;
        let text = captured(|| config = Some(GatewayConfig::load(&path).expect("still loads")));
        let _ = std::fs::remove_file(&path);

        let config = config.expect("load ran");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.providers.len(), 1);
        assert!(text.contains("server.prot"), "{text}");
        assert!(text.contains("providers[0].api_key_evn"), "{text}");
    }

    #[test]
    fn the_description_is_the_one_rolter_check_prints() {
        assert_eq!(
            describe(&UnknownKey {
                path: "timeouts.connect_sesc".to_string(),
                suggestion: Some("connect_secs".to_string()),
            }),
            "`timeouts.connect_sesc` is not a key rolter reads. It is ignored rather than \
             rejected, so the setting it looks like it configures is silently at its default. Did \
             you mean `connect_secs`?"
        );
        // no suggestion, no dangling question
        assert!(!describe(&UnknownKey {
            path: "server.kubernetes_namespace".to_string(),
            suggestion: None,
        })
        .contains("Did you mean"));
    }

    #[test]
    fn edit_distance_is_symmetric_and_zero_on_equality() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("port", "port"), 0);
        assert_eq!(edit_distance("prot", "port"), 2);
        assert_eq!(edit_distance("port", "prot"), 2);
        assert_eq!(edit_distance("", "port"), 4);
        assert_eq!(edit_distance("port", ""), 4);
    }

    #[test]
    fn a_short_key_does_not_attract_a_suggestion_from_every_neighbour() {
        // distance 2 between three-letter keys is most of the word
        assert_eq!(nearest("ttl", ["url", "name"].into_iter()), None);
        assert_eq!(
            nearest("nam", ["name", "kind"].into_iter()).as_deref(),
            Some("name")
        );
    }
}
