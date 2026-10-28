//! The wire format of `/internal/snapshot`, and how the two binaries agree on
//! one (#1512, ADR-0038).
//!
//! The snapshot's `version` is a change counter: it says *when* the config
//! last moved, not what shape the payload has. During a rolling upgrade the
//! control plane and the gateways are different builds, so the payload carries
//! a `format` integer of its own, and the gateway states the formats it reads
//! in a `formats` query parameter. The control plane serves the highest format
//! both sides know, converts down to it when that is older than its own, and
//! answers `406 Not Acceptable` when there is none, so a mismatch is a loud,
//! early refusal rather than config that propagates and is applied in part.
//!
//! This module is what both binaries share:
//!
//! - [`SNAPSHOT_FORMAT`] and [`MIN_SERVED_FORMAT`], the format this build
//!   writes and reads and the oldest one a control plane of this build serves;
//! - [`FormatRange`], the `formats` statement, with [`negotiate`] choosing the
//!   format to serve;
//! - [`DownStep`] and [`DOWN_STEPS`], the chain that serves an older gateway,
//!   applied by [`down_convert`] (before the snapshot is sanitized) and
//!   [`down_rewrite`] (after it is serialized).
//!
//! What counts as a format bump is section 2 of ADR-0038; the `Changed the
//! snapshot payload` row of the AGENTS.md maintenance matrix is the checklist.
//! The shape guard in this module's tests catches the mechanical half of it: a
//! path removed from the serialized config, or a JSON type changed, fails until
//! [`SNAPSHOT_FORMAT`] is bumped and the fixture regenerated with
//! `ROLTER_TEST_UPDATE_SNAPSHOT_SHAPE=1`.

use std::fmt;

use crate::GatewayConfig;

/// The snapshot format this build writes, and the only one its gateway reads.
pub const SNAPSHOT_FORMAT: u32 = 1;

/// The oldest snapshot format a control plane of this build still serves.
///
/// Raising it retires a format, which ADR-0032 treats as a removal: the
/// format's entry in the control plane's `DEPRECATIONS` must be past its sunset
/// and its removable-from release, and a `rolter-control` test holds this
/// constant to that registry.
pub const MIN_SERVED_FORMAT: u32 = 1;

// a control plane that did not serve its own format could serve nobody
const _: () = assert!(MIN_SERVED_FORMAT >= 1 && MIN_SERVED_FORMAT <= SNAPSHOT_FORMAT);

/// The format of a reply with no `format` field, and the only format a gateway
/// that sends no `formats` reads: every build that predates negotiation.
pub const LEGACY_FORMAT: u32 = 1;

/// The query parameter a gateway states its readable formats in.
pub const FORMATS_PARAM: &str = "formats";

/// An inclusive range of snapshot formats, as `formats=2` or `formats=1-2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub struct FormatRange {
    pub min: u32,
    pub max: u32,
}

/// A `formats` value that does not parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("formats must be a snapshot format number or an inclusive range such as 1-2, not {0:?}")]
pub struct InvalidFormats(pub String);

impl FormatRange {
    /// A range holding exactly one format.
    pub const fn single(format: u32) -> Self {
        Self {
            min: format,
            max: format,
        }
    }

    /// What a control plane of this build serves.
    pub const fn served() -> Self {
        Self {
            min: MIN_SERVED_FORMAT,
            max: SNAPSHOT_FORMAT,
        }
    }

    /// What a gateway of this build reads.
    pub const fn readable() -> Self {
        Self::single(SNAPSHOT_FORMAT)
    }

    /// What a gateway that states nothing reads.
    pub const fn legacy() -> Self {
        Self::single(LEGACY_FORMAT)
    }

    pub fn contains(&self, format: u32) -> bool {
        (self.min..=self.max).contains(&format)
    }

    /// The formats both ranges hold, or `None` when they are disjoint.
    pub fn intersect(&self, other: &FormatRange) -> Option<FormatRange> {
        let min = self.min.max(other.min);
        let max = self.max.min(other.max);
        (min <= max).then_some(FormatRange { min, max })
    }

    /// Parse a `formats` value: one format, or `min-max` with `min <= max`.
    /// Format `0` does not exist.
    pub fn parse(value: &str) -> Result<Self, InvalidFormats> {
        let invalid = || InvalidFormats(value.to_string());
        let number = |part: &str| -> Result<u32, InvalidFormats> {
            let part = part.trim();
            // `u32::from_str` takes a leading `+`, which is not this grammar
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid());
            }
            match part.parse::<u32>() {
                Ok(0) | Err(_) => Err(invalid()),
                Ok(n) => Ok(n),
            }
        };
        let range = match value.split_once('-') {
            Some((min, max)) => FormatRange {
                min: number(min)?,
                max: number(max)?,
            },
            None => FormatRange::single(number(value)?),
        };
        if range.min > range.max {
            return Err(invalid());
        }
        Ok(range)
    }
}

impl fmt::Display for FormatRange {
    /// The `formats` spelling: `2` for one format, `1-2` for a range.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.min == self.max {
            write!(f, "{}", self.min)
        } else {
            write!(f, "{}-{}", self.min, self.max)
        }
    }
}

impl std::str::FromStr for FormatRange {
    type Err = InvalidFormats;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// The served and the requested range had no format in common.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unsupported {
    pub served: FormatRange,
    pub requested: FormatRange,
}

impl Unsupported {
    /// The gateway reads only formats newer than the control plane serves:
    /// the gateway leads, which is the unsupported upgrade order.
    pub fn gateway_is_newer(&self) -> bool {
        self.requested.min > self.served.max
    }

    /// One sentence on what to do about it.
    pub fn action(&self) -> &'static str {
        if self.gateway_is_newer() {
            "upgrade the control plane first, or roll the gateway back"
        } else {
            "upgrade the gateway, or roll the control plane back"
        }
    }
}

impl fmt::Display for Unsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the control plane serves snapshot {}; the gateway reads {}",
            describe(self.served),
            self.requested
        )
    }
}

/// `format 1` or `formats 1-2`, for messages.
pub fn describe(range: FormatRange) -> String {
    if range.min == range.max {
        format!("format {}", range.min)
    } else {
        format!("formats {range}")
    }
}

/// Why a `formats` statement could not be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NegotiationError {
    /// the value did not parse: `400 invalid_query`
    Invalid(InvalidFormats),
    /// no format in common: `406 snapshot_format_unsupported`
    Unsupported(Unsupported),
}

/// The format to serve a gateway that stated `requested` (the raw `formats`
/// value, `None` when it sent none) from a control plane that serves `served`.
///
/// An absent statement is a gateway that predates negotiation, which reads
/// [`LEGACY_FORMAT`] and nothing else. Otherwise it is the highest format both
/// ranges hold.
pub fn negotiate(
    requested: Option<&str>,
    served: FormatRange,
) -> Result<(u32, FormatRange), NegotiationError> {
    let requested = match requested {
        None => FormatRange::legacy(),
        Some(value) => FormatRange::parse(value).map_err(NegotiationError::Invalid)?,
    };
    match served.intersect(&requested) {
        Some(common) => Ok((common.max, requested)),
        None => Err(NegotiationError::Unsupported(Unsupported {
            served,
            requested,
        })),
    }
}

/// One step down the format chain: takes a snapshot from format `from` to
/// format `to`, the mirror image of
/// [`config_migrate::Migration`](crate::config_migrate::Migration).
///
/// A step does the first of these that is enough (ADR-0038 section 4):
/// rewrite the serialized JSON losslessly ([`rewrite`](Self::rewrite)); omit
/// the entities the older format cannot express ([`omit`](Self::omit), which
/// runs before the snapshot is sanitized so the cascade removes their
/// references); or refuse, when what the older format cannot carry is
/// deployment-wide.
#[derive(Clone, Copy)]
pub struct DownStep {
    /// the format this step reads; always `to + 1`
    pub from: u32,
    /// the format it produces
    pub to: u32,
    /// one line describing the step
    pub summary: &'static str,
    /// drops from the typed config what format `to` cannot express, returning
    /// one problem line per entity left out, or `Err` with the reason when the
    /// config cannot be expressed in format `to` at all. Must cost nothing
    /// while nothing the bump added is in use
    pub omit: fn(&mut GatewayConfig) -> Result<Vec<String>, String>,
    /// rewrites the serialized `config` object from format `from` to `to`
    pub rewrite: fn(&mut serde_json::Value),
}

impl fmt::Debug for DownStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownStep")
            .field("from", &self.from)
            .field("to", &self.to)
            .field("summary", &self.summary)
            .finish_non_exhaustive()
    }
}

/// The production chain, newest step first. Empty: this release writes format
/// 1 and serves nothing older.
pub const DOWN_STEPS: &[DownStep] = &[];

/// The config cannot be expressed in the format the gateway reads:
/// `406 snapshot_format_inexpressible`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inexpressible {
    /// the format the gateway reads
    pub format: u32,
    /// what that format cannot carry, as one sentence
    pub reason: String,
}

impl fmt::Display for Inexpressible {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the config cannot be expressed in snapshot format {}: {}",
            self.format, self.reason
        )
    }
}

/// The steps that take a snapshot from `from` down to `to`, in the order they
/// apply, or the first format with no step.
fn steps(chain: &[DownStep], from: u32, to: u32) -> Result<Vec<&DownStep>, u32> {
    ((to + 1)..=from)
        .rev()
        .map(|k| chain.iter().find(|s| s.from == k && s.to + 1 == k).ok_or(k))
        .collect()
}

/// The typed half of a down-conversion from `from` to `to`: every step's
/// [`omit`](DownStep::omit), newest first. Runs before the snapshot is
/// sanitized. Returns the problem lines for what was left out.
pub fn down_convert(
    chain: &[DownStep],
    config: &mut GatewayConfig,
    from: u32,
    to: u32,
) -> Result<Vec<String>, Inexpressible> {
    let steps = steps(chain, from, to).map_err(|missing| Inexpressible {
        format: to,
        reason: format!("this control plane has no down-step from format {missing}"),
    })?;
    let mut problems = Vec::new();
    for step in steps {
        let omitted = (step.omit)(config).map_err(|reason| Inexpressible { format: to, reason })?;
        problems.extend(omitted);
    }
    Ok(problems)
}

/// The serialized half of a down-conversion from `from` to `to`: every step's
/// [`rewrite`](DownStep::rewrite) over the `config` object, newest first. Runs
/// after [`down_convert`] succeeded, which is what proved the chain complete.
pub fn down_rewrite(chain: &[DownStep], config: &mut serde_json::Value, from: u32, to: u32) {
    if let Ok(steps) = steps(chain, from, to) {
        for step in steps {
            (step.rewrite)(config);
        }
    }
}

/// The JSON shape of a serialized value: every path to a leaf or a container,
/// with the JSON types seen there. A list's items share the path segment `[]`,
/// and `null` is not recorded, since an `Option` that happens to be unset says
/// nothing about the type it carries.
pub fn shape(
    value: &serde_json::Value,
) -> std::collections::BTreeMap<String, std::collections::BTreeSet<&'static str>> {
    fn walk(
        path: &str,
        value: &serde_json::Value,
        out: &mut std::collections::BTreeMap<String, std::collections::BTreeSet<&'static str>>,
    ) {
        use serde_json::Value;
        let kind = match value {
            Value::Null => return,
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        out.entry(path.to_string()).or_default().insert(kind);
        match value {
            Value::Array(items) => {
                let child = format!("{path}[]");
                for item in items {
                    walk(&child, item, out);
                }
            }
            Value::Object(fields) => {
                for (key, field) in fields {
                    let child = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    walk(&child, field, out);
                }
            }
            _ => {}
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk("", value, &mut out);
    out.remove("");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderKind;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn a_statement_is_one_format_or_an_inclusive_range() {
        assert_eq!(FormatRange::parse("2"), Ok(FormatRange::single(2)));
        assert_eq!(
            FormatRange::parse("1-3"),
            Ok(FormatRange { min: 1, max: 3 })
        );
        assert_eq!(
            FormatRange::parse(" 1 - 2 "),
            Ok(FormatRange { min: 1, max: 2 })
        );
        for bad in [
            "",
            "0",
            "0-1",
            "2-1",
            "x",
            "1-",
            "-1",
            "1-2-3",
            "+1",
            "1.5",
            "99999999999",
        ] {
            assert!(FormatRange::parse(bad).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn a_range_displays_as_the_statement_it_parses_from() {
        for range in [FormatRange::single(1), FormatRange { min: 1, max: 2 }] {
            assert_eq!(FormatRange::parse(&range.to_string()), Ok(range));
        }
        assert_eq!(FormatRange::single(4).to_string(), "4");
        assert_eq!(FormatRange { min: 2, max: 3 }.to_string(), "2-3");
        assert_eq!(describe(FormatRange::single(1)), "format 1");
        assert_eq!(describe(FormatRange { min: 2, max: 3 }), "formats 2-3");
    }

    #[test]
    fn this_build_serves_and_reads_its_own_format() {
        assert!(FormatRange::served().contains(SNAPSHOT_FORMAT));
        assert_eq!(
            FormatRange::readable(),
            FormatRange::single(SNAPSHOT_FORMAT)
        );
        assert_eq!(
            FormatRange::readable().to_string(),
            SNAPSHOT_FORMAT.to_string()
        );
    }

    #[test]
    fn an_absent_statement_reads_the_legacy_format_only() {
        assert_eq!(
            negotiate(None, FormatRange::single(1)),
            Ok((1, FormatRange::single(1)))
        );
        // a silent gateway is never served a later format, even when it is served
        assert_eq!(
            negotiate(None, FormatRange { min: 1, max: 2 }),
            Ok((1, FormatRange::single(1)))
        );
        assert_eq!(
            negotiate(None, FormatRange::single(2)),
            Err(NegotiationError::Unsupported(Unsupported {
                served: FormatRange::single(2),
                requested: FormatRange::single(1),
            }))
        );
    }

    #[test]
    fn an_overlapping_statement_gets_the_highest_common_format() {
        let served = FormatRange { min: 1, max: 3 };
        assert_eq!(negotiate(Some("2"), served).map(|n| n.0), Ok(2));
        assert_eq!(negotiate(Some("2-5"), served).map(|n| n.0), Ok(3));
        assert_eq!(negotiate(Some("1-2"), served).map(|n| n.0), Ok(2));
    }

    #[test]
    fn a_disjoint_statement_is_unsupported_and_says_which_side_to_move() {
        let Err(NegotiationError::Unsupported(older)) =
            negotiate(Some("1"), FormatRange { min: 2, max: 3 })
        else {
            panic!("an older gateway was served");
        };
        assert!(!older.gateway_is_newer());
        assert_eq!(
            older.to_string(),
            "the control plane serves snapshot formats 2-3; the gateway reads 1"
        );
        assert!(older.action().starts_with("upgrade the gateway"));

        let Err(NegotiationError::Unsupported(newer)) =
            negotiate(Some("2"), FormatRange::single(1))
        else {
            panic!("a newer gateway was served");
        };
        assert!(newer.gateway_is_newer());
        assert!(newer.action().starts_with("upgrade the control plane"));
    }

    #[test]
    fn a_malformed_statement_is_invalid_not_unsupported() {
        assert!(matches!(
            negotiate(Some("two"), FormatRange::served()),
            Err(NegotiationError::Invalid(_))
        ));
    }

    // a synthetic chain: format 3 renamed `server` to `listener` (a rewrite),
    // format 2 added a provider kind format 1 cannot carry (an omission), and a
    // provider named `deployment-wide` stands for a setting format 1 cannot carry
    // at all (a refusal)
    fn rename_back(config: &mut serde_json::Value) {
        if let Some(object) = config.as_object_mut() {
            if let Some(listener) = object.remove("listener") {
                object.insert("server".to_string(), listener);
            }
        }
    }

    fn keep(_: &mut GatewayConfig) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }

    fn untouched(_: &mut serde_json::Value) {}

    fn omit_anthropic(config: &mut GatewayConfig) -> Result<Vec<String>, String> {
        if config.providers.iter().any(|p| p.name == "deployment-wide") {
            return Err("a deployment-wide setting has no format 1 spelling".to_string());
        }
        let mut problems = Vec::new();
        config.providers.retain(|p| {
            let keep = p.kind != ProviderKind::Anthropic;
            if !keep {
                problems.push(format!(
                    "provider '{}' omitted: format 1 cannot carry its kind",
                    p.name
                ));
            }
            keep
        });
        Ok(problems)
    }

    const CHAIN: &[DownStep] = &[
        DownStep {
            from: 3,
            to: 2,
            summary: "server is spelled listener",
            omit: keep,
            rewrite: rename_back,
        },
        DownStep {
            from: 2,
            to: 1,
            summary: "format 1 has no anthropic providers",
            omit: omit_anthropic,
            rewrite: untouched,
        },
    ];

    fn config() -> GatewayConfig {
        GatewayConfig {
            providers: vec![
                crate::ProviderConfig {
                    name: "openai".to_string(),
                    kind: ProviderKind::Openai,
                    ..Default::default()
                },
                crate::ProviderConfig {
                    name: "claude".to_string(),
                    kind: ProviderKind::Anthropic,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn the_production_chain_covers_every_served_format() {
        assert!(steps(DOWN_STEPS, SNAPSHOT_FORMAT, MIN_SERVED_FORMAT).is_ok());
        for step in DOWN_STEPS {
            assert_eq!(step.from, step.to + 1, "{step:?}");
        }
    }

    #[test]
    fn serving_the_own_format_converts_nothing() {
        let mut config = config();
        assert_eq!(down_convert(CHAIN, &mut config, 3, 3), Ok(Vec::new()));
        assert_eq!(config.providers.len(), 2);
    }

    #[test]
    fn a_rewrite_runs_only_for_the_steps_it_crosses() {
        let mut json = serde_json::json!({"listener": {"port": 1}});
        down_rewrite(CHAIN, &mut json, 3, 2);
        assert_eq!(json, serde_json::json!({"server": {"port": 1}}));

        let mut json = serde_json::json!({"listener": {"port": 1}});
        down_rewrite(CHAIN, &mut json, 2, 1);
        assert_eq!(json, serde_json::json!({"listener": {"port": 1}}));
    }

    #[test]
    fn the_chain_omits_what_the_older_format_cannot_carry() {
        let mut config = config();
        let problems = down_convert(CHAIN, &mut config, 3, 1).expect("expressible");
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.providers[0].name, "openai");
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("'claude'"), "{problems:?}");

        // and a step that is not crossed does not run
        let mut config = self::config();
        assert_eq!(down_convert(CHAIN, &mut config, 3, 2), Ok(Vec::new()));
        assert_eq!(config.providers.len(), 2);
    }

    #[test]
    fn the_chain_refuses_what_it_cannot_omit() {
        let mut config = config();
        config.providers.push(crate::ProviderConfig {
            name: "deployment-wide".to_string(),
            ..Default::default()
        });
        let refused = down_convert(CHAIN, &mut config, 3, 1).unwrap_err();
        assert_eq!(refused.format, 1);
        assert!(
            refused.to_string().contains("no format 1 spelling"),
            "{refused}"
        );
    }

    #[test]
    fn a_gap_in_the_chain_is_inexpressible_rather_than_a_skipped_step() {
        let refused = down_convert(&CHAIN[..1], &mut config(), 3, 1).unwrap_err();
        assert!(refused.reason.contains("from format 2"), "{refused}");
    }

    #[test]
    fn a_shape_records_paths_and_types_and_ignores_null() {
        let found = shape(&serde_json::json!({
            "a": {"b": 1, "c": null},
            "list": [{"x": "s"}, {"x": 2}],
        }));
        let expected: BTreeMap<String, BTreeSet<&str>> = [
            ("a", vec!["object"]),
            ("a.b", vec!["number"]),
            ("list", vec!["array"]),
            ("list[]", vec!["object"]),
            ("list[].x", vec!["number", "string"]),
        ]
        .into_iter()
        .map(|(p, t)| (p.to_string(), t.into_iter().collect()))
        .collect();
        assert_eq!(found, expected);
    }

    /// The fixture's shape, keyed by path, with the format it was taken at.
    #[derive(serde::Serialize, serde::Deserialize)]
    struct ShapeFixture {
        format: u32,
        paths: BTreeMap<String, BTreeSet<String>>,
    }

    fn fixture_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/snapshot_shape.json")
    }

    /// The representative config: a snapshot with at least one element in
    /// every list, so the guard sees the shape of what a list holds and not
    /// only that it is a list. Decoded through `GatewayConfig` and serialized
    /// again, so what is checked is what the control plane writes.
    fn representative_shape() -> BTreeMap<String, BTreeSet<String>> {
        let config: GatewayConfig = serde_json::from_str(include_str!(
            "../tests/fixtures/snapshot_representative.json"
        ))
        .expect("snapshot_representative.json decodes as a GatewayConfig");
        let json = serde_json::to_value(&config).expect("the config serializes");
        shape(&json)
            .into_iter()
            .map(|(path, kinds)| (path, kinds.into_iter().map(str::to_string).collect()))
            .collect()
    }

    /// Every list the representative config serializes empty. An empty list
    /// hides the shape of its items from the guard, so a new `Vec` field fails
    /// here until the representative config gives it an element.
    #[test]
    fn the_representative_config_populates_every_list() {
        let current = representative_shape();
        let empty: Vec<&String> = current
            .iter()
            .filter(|(path, kinds)| {
                kinds.contains("array") && !current.contains_key(&format!("{path}[]"))
            })
            .map(|(path, _)| path)
            .collect();
        assert!(
            empty.is_empty(),
            "tests/fixtures/snapshot_representative.json leaves these lists empty, so the \
             shape guard cannot see what they hold; give each one an element: {empty:#?}"
        );
    }

    /// Every `path: type` in `fixture` that `current` no longer serializes.
    fn lost(
        fixture: &BTreeMap<String, BTreeSet<String>>,
        current: &BTreeMap<String, BTreeSet<String>>,
    ) -> Vec<String> {
        let mut lost = Vec::new();
        for (path, kinds) in fixture {
            for kind in kinds {
                if !current.get(path).is_some_and(|now| now.contains(kind)) {
                    lost.push(format!("{path}: {kind}"));
                }
            }
        }
        lost
    }

    /// The shape guard (ADR-0038 section 2). An older gateway decodes exactly
    /// the paths the fixture lists, with those types, so the serialized config
    /// must stay a superset of it within one format. A new path is additive
    /// and passes; a removed path or a changed type is a format bump.
    #[test]
    fn the_serialized_config_keeps_the_shape_of_its_format() {
        let current = representative_shape();
        let path = fixture_path();
        let on_disk: Option<ShapeFixture> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());

        if std::env::var_os("ROLTER_TEST_UPDATE_SNAPSHOT_SHAPE").is_some() {
            // regenerating cannot launder a breaking change: a lost path needs
            // the bump first
            if let Some(fixture) = &on_disk {
                let lost = lost(&fixture.paths, &current);
                assert!(
                    fixture.format != SNAPSHOT_FORMAT || lost.is_empty(),
                    "the serialized config lost {lost:#?} within snapshot format \
                     {SNAPSHOT_FORMAT}; bump SNAPSHOT_FORMAT and add a down-step \
                     before regenerating (ADR-0038 section 2)"
                );
            }
            let fixture = ShapeFixture {
                format: SNAPSHOT_FORMAT,
                paths: current.clone(),
            };
            let mut text = serde_json::to_string_pretty(&fixture).expect("serializes");
            text.push('\n');
            std::fs::create_dir_all(path.parent().expect("has a parent")).expect("mkdir");
            std::fs::write(&path, text).expect("writes the fixture");
            return;
        }

        let fixture = on_disk.unwrap_or_else(|| {
            panic!(
                "{} is missing or unreadable; regenerate it with \
                 ROLTER_TEST_UPDATE_SNAPSHOT_SHAPE=1",
                path.display()
            )
        });
        assert_eq!(
            fixture.format, SNAPSHOT_FORMAT,
            "SNAPSHOT_FORMAT moved; regenerate the shape fixture with \
             ROLTER_TEST_UPDATE_SNAPSHOT_SHAPE=1"
        );
        let lost = lost(&fixture.paths, &current);
        assert!(
            lost.is_empty(),
            "the serialized config no longer carries {lost:#?}. An older gateway \
             decodes these paths with these types, so this is a snapshot format \
             bump: increment SNAPSHOT_FORMAT, add a down-step and a DEPRECATIONS \
             entry for the old format, then regenerate with \
             ROLTER_TEST_UPDATE_SNAPSHOT_SHAPE=1 (ADR-0038 section 2)"
        );
    }
}
