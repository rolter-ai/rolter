//! Drift guard for the `rolter.toml` reference (#2752, #2920).
//!
//! `docs/user-docs/configuration/config-file.mdx` is what an operator reads to
//! learn which keys the file takes. A key the page does not mention is a key
//! nobody sets, and a typo in one is silent, so the page has to track the
//! config structs in `crates/rolter-core/src/config.rs`.
//!
//! Three rules, each failing on the PR that breaks it:
//!
//! 1. **Every field has an entry in its own section.** Each [`CHECKS`] row names
//!    a heading of the page and the struct that heading documents. A field
//!    counts as documented only by a `<ParamField path="…">` between that
//!    heading and the next heading of any level. An earlier version searched the
//!    whole page, so a common name such as `enabled` was satisfied by whatever
//!    other section happened to document one (#2752).
//! 2. **Every top-level table has a section.** [`tables`] must list exactly the
//!    fields of `GatewayConfig`, so a new table fails here until the page and
//!    `rolter.example.toml` say something about it. A table the control plane
//!    owns is allowed to be a paragraph that links its page, but it is still
//!    named.
//! 3. **The example file agrees.** `rolter.example.toml` shows every table a
//!    file can set, and every field a section documents, commented out with its
//!    default where that is the point of the example. The closing note of the
//!    page names the tables the example leaves out.
//!
//! One struct parser serves all of it. It reads the source rather than the
//! compiled types, as `env_var_names.rs` does for the environment variables: a
//! scan over the workspace, so the drift fails on the PR that writes it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const REFERENCE: &str = "docs/user-docs/configuration/config-file.mdx";
const EXAMPLE: &str = "rolter.example.toml";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the workspace root is readable")
}

fn read(relative: &str) -> String {
    std::fs::read_to_string(workspace_root().join(relative))
        .unwrap_or_else(|err| panic!("{relative} is readable: {err}"))
}

// ---------------------------------------------------------------------------
// the struct parser
// ---------------------------------------------------------------------------

/// The keys `pub struct <name> { … }` writes in a config file, parsed out of
/// `source`, or `None` when the source declares no such struct.
///
/// A field is the key its TOML spelling gives it, so
/// `#[serde(rename = "default")] pub defaults` is `default`. The struct ends at
/// the first unindented brace: a bare `}` would stop at one inside a doc
/// comment and silently drop every field after it.
fn fields_in(source: &str, name: &str) -> Option<BTreeSet<String>> {
    let start = source.find(&format!("pub struct {name} {{"))?;
    let rest = &source[start..];
    let body = &rest[..rest.find("\n}")?];

    let mut fields = BTreeSet::new();
    // attributes seen since the previous field, which is where a rename lives
    let mut attributes = String::new();
    for line in body.lines().skip(1) {
        let line = line.trim();
        if line.starts_with("#[") {
            attributes.push_str(line);
        } else if let Some(field) = line.strip_prefix("pub ") {
            let Some((field, _)) = field.split_once(':') else {
                continue;
            };
            fields.insert(rename_in(&attributes).unwrap_or_else(|| field.trim().to_string()));
            attributes.clear();
        }
    }
    Some(fields)
}

/// The `rename = "…"` of a `#[serde(…)]` attribute, if it carries one.
fn rename_in(attributes: &str) -> Option<String> {
    let after = attributes.split_once("rename = \"")?.1;
    Some(after.split_once('"')?.0.to_string())
}

/// The fields of the config struct `name`, wherever in `rolter-core` it lives.
///
/// This is the one parser every check shares. Config types sit in `config.rs`
/// and in the modules that own a feature (`guardrails.rs`, `currency.rs`, …).
fn struct_fields(name: &str) -> BTreeSet<String> {
    let dir = workspace_root().join("crates/rolter-core/src");
    for entry in std::fs::read_dir(&dir)
        .expect("rolter-core/src is readable")
        .flatten()
    {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap_or_default();
        if let Some(fields) = fields_in(&source, name) {
            assert!(!fields.is_empty(), "`pub struct {name}` has no fields");
            return fields;
        }
    }
    panic!("no `pub struct {name} {{` under crates/rolter-core/src");
}

// ---------------------------------------------------------------------------
// the page, one section at a time
// ---------------------------------------------------------------------------

/// Whether `line` opens a markdown heading of any level.
fn is_heading(line: &str) -> bool {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    (1..=6).contains(&hashes) && line[hashes..].starts_with(' ')
}

/// The text under `heading` (written as it is on the page, `### \[cache\]`),
/// up to the next heading of **any** level, or `None` when no such heading
/// exists.
///
/// Any level, not the next one of the same rank: `### [[routes]]` and its
/// `#### [[routes.targets]]` both have a `model`, and each must document its
/// own. A `#` line inside a code fence is a comment, not a heading.
fn section<'a>(page: &'a str, heading: &str) -> Option<&'a str> {
    let mut in_fence = false;
    let mut start = None;
    let mut offset = 0;
    for line in page.split_inclusive('\n') {
        let text = line.trim_end();
        if text.starts_with("```") {
            in_fence = !in_fence;
        } else if !in_fence && is_heading(text) {
            if let Some(start) = start {
                return Some(&page[start..offset]);
            }
            if text == heading {
                start = Some(offset + line.len());
            }
        }
        offset += line.len();
    }
    start.map(|start| &page[start..])
}

/// The `path` of every `<ParamField path="…">` in `text`.
fn documented_fields(text: &str) -> BTreeSet<String> {
    text.split("<ParamField")
        .skip(1)
        .filter_map(|tag| {
            let tag = &tag[..tag.find('>')?];
            let after = tag.split_once("path=\"")?.1;
            Some(after.split_once('"')?.0.to_string())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// rule 1: every field has an entry in its own section
// ---------------------------------------------------------------------------

/// Where the keys a section must document come from.
enum Keys {
    /// every field of this struct
    Struct(&'static str),
    /// a top-level key that is a scalar, not a table
    Named(&'static [&'static str]),
}

/// One section of the page and what it must document.
struct Check {
    /// the heading as written on the page
    heading: &'static str,
    keys: Keys,
    /// fields the section deliberately has no entry for, each with the reason.
    /// A reason that stops being true is a failing test, see
    /// `no_exemption_outlives_its_reason`
    exempt: &'static [(&'static str, &'static str)],
}

/// A row that expects every field of `strukt` under `heading`.
const fn plain(heading: &'static str, strukt: &'static str) -> Check {
    with_exempt(heading, strukt, &[])
}

/// [`plain`], except for the fields in `exempt`.
const fn with_exempt(
    heading: &'static str,
    strukt: &'static str,
    exempt: &'static [(&'static str, &'static str)],
) -> Check {
    Check {
        heading,
        keys: Keys::Struct(strukt),
        exempt,
    }
}

const BY_THE_STORE: (&str, &str) = (
    "tenancy",
    "filled in by the store, never written in a config file",
);
const OWN_HEADING: &str = "a table with a heading of its own below";

const CHECKS: &[Check] = &[
    plain("### \\[server\\]", "ServerConfig"),
    plain("### \\[tls\\]", "TlsConfig"),
    with_exempt(
        "### \\[\\[providers\\]\\]",
        "ProviderConfig",
        &[BY_THE_STORE],
    ),
    with_exempt(
        "### \\[\\[provider_groups\\]\\]",
        "ProviderGroupConfig",
        &[BY_THE_STORE],
    ),
    plain("#### \\[\\[provider_groups.members\\]\\]", "GroupMember"),
    with_exempt(
        "### \\[\\[routes\\]\\]",
        "ModelRoute",
        &[
            BY_THE_STORE,
            ("targets", OWN_HEADING),
            ("params", OWN_HEADING),
            ("param_policy", OWN_HEADING),
            ("variants", OWN_HEADING),
            ("advanced", OWN_HEADING),
            ("cache", OWN_HEADING),
        ],
    ),
    plain("#### \\[\\[routes.targets\\]\\]", "Target"),
    plain("#### \\[routes.param_policy\\]", "ParamPolicy"),
    plain("#### \\[\\[routes.variants\\]\\]", "Variant"),
    plain("#### \\[routes.advanced\\]", "AdvancedModelConfig"),
    plain("##### \\[routes.advanced.guardrails\\]", "RouteGuardrails"),
    plain("#### \\[routes.cache\\]", "RouteCache"),
    plain("##### \\[routes.cache.semantic\\]", "SemanticCacheConfig"),
    plain("### \\[models\\]", "ModelTiersConfig"),
    plain("### \\[\\[virtual_keys\\]\\]", "VirtualKeyConfig"),
    plain("### \\[\\[model_prices\\]\\]", "ModelPriceConfig"),
    plain("### \\[currency\\]", "CurrencyConfig"),
    plain("### \\[\\[budgets\\]\\]", "BudgetConfig"),
    Check {
        heading: "### unpriced_policy",
        keys: Keys::Named(&["unpriced_policy"]),
        exempt: &[],
    },
    plain("### \\[\\[rate_limits\\]\\]", "RateLimitConfig"),
    plain("### \\[retry\\]", "RetryConfig"),
    plain("### \\[cooldown\\]", "CooldownConfig"),
    plain("### \\[cache\\]", "CacheConfig"),
    plain("### \\[responses\\]", "ResponsesConfig"),
    plain("### \\[timeouts\\]", "TimeoutConfig"),
    plain("### \\[compatibility\\]", "CompatibilityConfig"),
    plain("### \\[client\\]", "ClientConfig"),
    plain("### \\[model_defaults\\]", "ModelDefaultsConfig"),
    plain("### \\[adaptive_routing\\]", "AdaptiveRoutingConfig"),
    plain("### \\[queue\\]", "QueueConfig"),
    plain("### \\[health\\]", "HealthConfig"),
    plain("### \\[breaker\\]", "BreakerConfig"),
    plain("### \\[metrics_scrape\\]", "MetricsScrapeConfig"),
    plain("### \\[realtime\\]", "RealtimeConfig"),
    plain("### \\[usage_recording\\]", "UsageRecordingConfig"),
    plain("### \\[security\\]", "SecurityPolicyConfig"),
    plain("### \\[logging\\]", "LoggingConfig"),
    plain("#### \\[logging.payload_capture\\]", "PayloadCaptureConfig"),
    plain("### \\[feature_flags\\]", "FeatureFlagsConfig"),
    plain("### \\[guardrails\\]", "GuardrailsConfig"),
    plain("#### \\[\\[guardrails.rules\\]\\]", "GuardrailRule"),
    plain("### \\[egress\\]", "EgressPolicy"),
    plain("### \\[guardrail_webhook\\]", "GuardrailWebhookConfig"),
    plain("### \\[pii_sanitizer\\]", "PiiSanitizerConfig"),
    plain("### \\[prompt_templates\\]", "PromptTemplatesConfig"),
    with_exempt(
        "#### \\[\\[prompt_templates.templates\\]\\]",
        "PromptTemplate",
        &[(
            "scopes",
            "tenant scopes of a database-backed template, set through the control plane",
        )],
    ),
    plain(
        "##### \\[\\[prompt_templates.templates.variables\\]\\]",
        "TemplateVariable",
    ),
    plain(
        "##### \\[\\[prompt_templates.templates.decorators\\]\\]",
        "Decorator",
    ),
];

impl Keys {
    fn names(&self) -> BTreeSet<String> {
        match self {
            Keys::Struct(name) => struct_fields(name),
            Keys::Named(names) => names.iter().map(|n| n.to_string()).collect(),
        }
    }
}

/// What `check` requires of `page` and does not find, as `(field, reason)`
/// lines for a failure message.
fn missing_from(page: &str, check: &Check) -> Vec<String> {
    let Some(body) = section(page, check.heading) else {
        return vec![format!("the page has no `{}` heading", check.heading)];
    };
    let documented = documented_fields(body);
    check
        .keys
        .names()
        .into_iter()
        .filter(|field| !documented.contains(field))
        .filter(|field| !check.exempt.iter().any(|(name, _)| name == field))
        .map(|field| format!("`{field}` has no <ParamField> under `{}`", check.heading))
        .collect()
}

#[test]
fn every_config_field_is_documented_in_its_own_section() {
    let page = read(REFERENCE);
    let missing: Vec<String> = CHECKS
        .iter()
        .flat_map(|check| missing_from(&page, check))
        .collect();
    assert!(
        missing.is_empty(),
        "these config fields are defined in crates/rolter-core but not documented in the \
         section of {REFERENCE} that owns them. A <ParamField> under another heading does \
         not count (#2752):\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn no_exemption_outlives_its_reason() {
    let page = read(REFERENCE);
    let mut stale = Vec::new();
    for check in CHECKS {
        let fields = check.keys.names();
        let documented = section(&page, check.heading)
            .map(documented_fields)
            .unwrap_or_default();
        for (field, _) in check.exempt {
            if !fields.contains(*field) {
                stale.push(format!(
                    "`{field}` is exempt under `{}` but is not a field of it",
                    check.heading
                ));
            } else if documented.contains(*field) {
                stale.push(format!(
                    "`{field}` is exempt under `{}` yet documented there; drop the exemption",
                    check.heading
                ));
            }
        }
    }
    assert!(
        stale.is_empty(),
        "stale exemptions:\n  {}",
        stale.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// rule 2 and 3: every top-level table, on the page and in the example
// ---------------------------------------------------------------------------

/// Who writes a top-level table.
enum Owner {
    /// a `rolter.toml` sets it, so `rolter.example.toml` must show it
    File,
    /// the control plane builds it from its database and ships it in the
    /// snapshot; a file does not set it. Its section links this page instead
    ControlPlane { page: &'static str },
}

/// One field of `GatewayConfig` and the section that answers for it.
///
/// The page and the example both follow the TOML spelling, so a table's heading
/// (`### \[tls\]`) and example marker (`[tls]`) come from its name; `custom`
/// is for the few that share a section or are spelled differently.
struct Table {
    field: &'static str,
    heading: String,
    /// the text `rolter.example.toml` must contain, for a table a file sets
    example: Option<String>,
    owner: Owner,
}

impl Table {
    fn new(field: &'static str, heading: String, example: String) -> Self {
        Table {
            field,
            heading,
            example: Some(example),
            owner: Owner::File,
        }
    }

    /// `schema_version = …`
    fn scalar(field: &'static str) -> Self {
        Self::new(field, format!("### {field}"), format!("{field} ="))
    }

    /// `[field]`
    fn single(field: &'static str) -> Self {
        Self::new(field, format!("### \\[{field}\\]"), format!("[{field}]"))
    }

    /// `[[field]]`
    fn repeated(field: &'static str) -> Self {
        let heading = format!("### \\[\\[{field}\\]\\]");
        Self::new(field, heading, format!("[[{field}]]"))
    }

    /// A table whose section or spelling does not follow its name.
    fn custom(field: &'static str, heading: &str, example: &str) -> Self {
        Self::new(field, heading.to_string(), example.to_string())
    }

    /// The same section as [`Table::single`] or [`Table::repeated`], for a table
    /// the control plane owns and `page` documents.
    fn owned(mut self, page: &'static str) -> Self {
        self.example = None;
        self.owner = Owner::ControlPlane { page };
        self
    }
}

fn tables() -> Vec<Table> {
    vec![
        Table::scalar("schema_version"),
        Table::single("server"),
        Table::single("tls"),
        Table::repeated("providers"),
        // the two tiers share one section with their `readonly` siblings
        Table::custom(
            "provider_defaults",
            "### Config tiers: readonly and default",
            "[[providers.default]]",
        ),
        Table::repeated("routes"),
        Table::repeated("provider_groups"),
        Table::custom(
            "provider_group_defaults",
            "### Config tiers: readonly and default",
            "[[provider_groups.default]]",
        ),
        Table::custom("models", "### \\[models\\]", "[[models.readonly]]"),
        Table::repeated("virtual_keys"),
        Table::repeated("db_virtual_keys").owned("/concepts/virtual-keys"),
        Table::repeated("mcp_servers").owned("/configuration/mcp-servers"),
        Table::repeated("mcp_oauth_sessions").owned("/security/mcp-oauth"),
        Table::repeated("model_prices"),
        Table::single("currency"),
        Table::repeated("budgets"),
        Table::scalar("unpriced_policy"),
        Table::repeated("rate_limits"),
        Table::single("retry"),
        Table::single("cooldown"),
        Table::single("cache"),
        Table::single("responses"),
        Table::single("timeouts"),
        Table::single("compatibility"),
        Table::single("client"),
        Table::single("security"),
        Table::single("model_defaults"),
        Table::single("adaptive_routing"),
        Table::single("queue"),
        Table::single("health"),
        Table::single("breaker"),
        Table::single("metrics_scrape"),
        Table::single("realtime"),
        Table::single("logging"),
        Table::single("usage_recording"),
        // a gateway reading a file never applies it: only the control plane's
        // store does, from Settings -> Feature Flags
        Table::single("feature_flags").owned("/configuration/feature-flags"),
        Table::single("guardrails"),
        Table::single("egress"),
        Table::single("guardrail_webhook"),
        Table::single("pii_sanitizer"),
        Table::single("prompt_templates"),
        Table::single("plugins").owned("/configuration/plugins"),
    ]
}

#[test]
fn every_top_level_table_has_a_section() {
    let declared = struct_fields("GatewayConfig");
    let tables = tables();
    let listed: BTreeSet<String> = tables.iter().map(|t| t.field.to_string()).collect();
    let unlisted: Vec<&String> = declared.difference(&listed).collect();
    let unknown: Vec<&String> = listed.difference(&declared).collect();
    assert!(
        unlisted.is_empty() && unknown.is_empty(),
        "tables() must name exactly the fields of GatewayConfig. Not listed (add a section to \
         {REFERENCE} and an entry to tables()): {unlisted:?}. Listed but not a field: {unknown:?}"
    );

    let page = read(REFERENCE);
    let mut problems = Vec::new();
    for table in &tables {
        let Some(body) = section(&page, &table.heading) else {
            problems.push(format!(
                "`{}` has no `{}` heading on the page",
                table.field, table.heading
            ));
            continue;
        };
        if let Owner::ControlPlane { page: link } = table.owner {
            if !body.contains(&format!("]({link}")) {
                problems.push(format!(
                    "`{}` is owned by the control plane, so its section has to link {link}",
                    table.field
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn the_example_shows_every_table_a_file_can_set() {
    let example = read(EXAMPLE);
    let missing: Vec<String> = tables()
        .iter()
        .filter_map(|table| {
            let marker = table.example.as_deref()?;
            (!example.contains(marker)).then(|| format!("`{marker}` (the `{}` table)", table.field))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "{EXAMPLE} is missing these tables; a commented-out block with the defaults is enough:\n  {}",
        missing.join("\n  ")
    );
}

/// The closing note of the page, which says what the example file holds.
fn closing_note(page: &str) -> &str {
    let start = page.rfind("<Note>").expect("the page ends with a <Note>");
    &page[start..]
}

#[test]
fn the_closing_note_names_the_tables_a_file_cannot_set() {
    let page = read(REFERENCE);
    let note = closing_note(&page);
    let tables = tables();
    let missing: Vec<&str> = tables
        .iter()
        .filter(|table| matches!(table.owner, Owner::ControlPlane { .. }))
        .map(|table| table.field)
        .filter(|field| !note.contains(&format!("`{field}`")))
        .collect();
    assert!(
        missing.is_empty(),
        "the closing note of {REFERENCE} says rolter.example.toml has every table a file can \
         set; it has to say which ones it leaves out, and does not name {missing:?}"
    );
}

/// Whether `example` sets or opens `field` somewhere: a `field = …` line or a
/// table header ending in `.field`, commented out or not.
///
/// A bare substring search would let `enabled` or `model` be satisfied by any
/// prose that happens to contain the word.
fn example_mentions(example: &str, field: &str) -> bool {
    example.lines().any(|line| {
        let line = line.trim_start_matches('#').trim();
        if let Some(header) = line.strip_prefix('[') {
            let header = header.trim_start_matches('[');
            let header = header.split(']').next().unwrap_or_default();
            return header.rsplit('.').next() == Some(field);
        }
        line.strip_prefix(field)
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    })
}

#[test]
fn every_field_a_file_section_documents_is_in_the_example() {
    // a commented-out block counts: the example shows the defaults without
    // applying them. The control plane's tables are not file settings, so they
    // have no block to find
    let example = read(EXAMPLE);
    let tables = tables();
    let mut missing = Vec::new();
    for check in CHECKS {
        let Keys::Struct(name) = check.keys else {
            continue;
        };
        let control_plane_owned = tables.iter().any(|table| {
            table.heading == check.heading && matches!(table.owner, Owner::ControlPlane { .. })
        });
        if control_plane_owned {
            continue;
        }
        for field in check.keys.names() {
            let exempt = check.exempt.iter().any(|(exempt, _)| *exempt == field);
            if !exempt && !example_mentions(&example, &field) {
                missing.push(format!("{name}::{field}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "these config fields are missing from {EXAMPLE}; a commented-out line with the default \
         is enough:\n  {}",
        missing.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// the guard's own tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = r#"
/// A doc comment with a stray brace } in it.
#[derive(Debug)]
pub struct Sample {
    /// a plain field
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, rename = "default")]
    pub defaults: Vec<String>,
    pub name: String,
}

pub struct Other {
    pub unrelated: u8,
}
"#;

    #[test]
    fn the_parser_reads_each_field_by_its_toml_spelling() {
        let fields = fields_in(SOURCE, "Sample").expect("Sample is declared");
        let want: BTreeSet<String> = ["default", "enabled", "name"].map(String::from).into();
        assert_eq!(fields, want);
        assert!(fields_in(SOURCE, "Missing").is_none());
    }

    #[test]
    fn the_parser_finds_a_struct_in_the_real_sources() {
        let fields = struct_fields("CacheConfig");
        assert!(fields.contains("default_ttl_secs"), "{fields:?}");
        // a struct that lives outside config.rs, and one with a renamed field
        assert!(struct_fields("GuardrailsConfig").contains("streaming_post_call"));
        assert!(struct_fields("ModelTiersConfig").contains("default"));
    }

    const PAGE: &str = "\
### \\[first\\]

<ParamField path=\"enabled\" type=\"boolean\">
  switch
</ParamField>

```toml
### a comment in a fence is not a heading
```

#### \\[first.nested\\]

<ParamField path=\"only_nested\" type=\"string\">
  nested
</ParamField>

### \\[second\\]

prose, and no entries
";

    #[test]
    fn a_section_runs_to_the_next_heading_of_any_level() {
        let first = section(PAGE, "### \\[first\\]").expect("first exists");
        assert!(documented_fields(first).contains("enabled"));
        assert!(
            !documented_fields(first).contains("only_nested"),
            "a nested table's entry must not count for its parent"
        );
        assert_eq!(
            documented_fields(section(PAGE, "#### \\[first.nested\\]").unwrap()),
            BTreeSet::from(["only_nested".to_string()])
        );
        assert!(section(PAGE, "### \\[third\\]").is_none());
    }

    #[test]
    fn a_field_documented_only_in_another_section_is_reported_as_missing() {
        // `enabled` is documented under [first] and nowhere under [second], the
        // case the whole-page search of #2752 let through
        let second = Check {
            heading: "### \\[second\\]",
            keys: Keys::Named(&["enabled"]),
            exempt: &[],
        };
        assert_eq!(
            missing_from(PAGE, &second),
            vec!["`enabled` has no <ParamField> under `### \\[second\\]`".to_string()]
        );

        let first = Check {
            heading: "### \\[first\\]",
            keys: Keys::Named(&["enabled"]),
            exempt: &[],
        };
        assert!(missing_from(PAGE, &first).is_empty());

        let absent = Check {
            heading: "### \\[third\\]",
            keys: Keys::Named(&["enabled"]),
            exempt: &[],
        };
        assert_eq!(missing_from(PAGE, &absent).len(), 1);
    }

    #[test]
    fn an_exempt_field_is_not_reported() {
        let second = Check {
            heading: "### \\[second\\]",
            keys: Keys::Named(&["enabled"]),
            exempt: &[("enabled", "documented elsewhere")],
        };
        assert!(missing_from(PAGE, &second).is_empty());
    }
}
