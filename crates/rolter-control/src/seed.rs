//! idempotent database bootstrap shared by the `rolter-seed` binary, the unified
//! launcher's `seed` subcommand (what the published image and wheels ship,
//! #1897) and its `easy-up` subcommand.
//!
//! [`seed`] creates an org (+ a `default`/`default` team/project), an optional
//! admin user, and imports providers, provider groups, routes, model prices
//! and prompt templates from a bootstrap `rolter.toml`. The org, team, project
//! and admin steps check for an existing row first, so re-running creates
//! nothing twice.
//!
//! Budgets and rate limits import by the portable name of their scope, not by
//! database id (#1312); see `import_caps`.
//!
//! What this consumes is what [`crate::config_export::render`] emits: the two
//! are the read and write halves of one round trip, so a section added to one
//! belongs in the other (#1082).
//!
//! The bootstrap-toml import is an **upsert**: the file is the desired state,
//! so a re-import of an edited file applies the edits rather than skipping the
//! rows it already created (#927). "Idempotent" used to mean only that
//! re-running was safe — an operator who added a missing `api_key_env` and
//! re-imported got `imported provider` in the log and no change in the
//! database, and found out through 401s from the upstream that read as a bad
//! key rather than as config that was never applied.
//!
//! Two things are deliberately left alone by a re-import:
//!
//! - the **sealed credential** in `provider_keys`. A key rotated through the
//!   dashboard outranks `api_key_env` in the snapshot's precedence, and a
//!   stale value in a file must not clobber it.
//! - a provider's **slug**, its stable identity once assigned.
//!
//! Rows present in the database but absent from the file are not deleted: the
//! file is the desired state for what it names, not an exhaustive inventory.
//! The one exception is a route's weighted variants (#391): a route whose file
//! entry declares `[[routes.variants]]` has its stored set replaced by the
//! file's, since an ordered, weighted set cannot be merged, and the variants
//! that removes are named in a warning. A route that declares none keeps its
//! stored variants. See `import_route_variants`.
//!
//! That last rule is why `[logging.payload_capture]` is imported on *presence*
//! rather than on value (#954). Every other section of the file is a list, so
//! "absent" and "empty" look the same and mean the same. `logging` is a single
//! row with `serde` defaults, so a parsed config always carries a
//! `payload_capture` block whether or not the file wrote one — importing it
//! unconditionally would turn every re-import into a silent "capture off",
//! undoing a decision an operator made in the dashboard. The raw TOML is
//! therefore re-parsed to ask whether the key was actually written. The same
//! goes for `logging.ui_events` (#1748), whose default is *on*: a file that
//! never mentions it must not re-enable a stream the dashboard switched off.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use argon2::password_hash::PasswordHasher;
use argon2::Argon2;
use rolter_core::{
    BalancingStrategy, BudgetPeriod, BudgetScope, GatewayConfig, PromptTemplate, ProviderKind,
};
use rolter_store::postgres::repo::{
    BudgetRepo, LoggingSettingsRepo, LoggingSettingsUpdate, ModelPriceRepo, NewModelPrice,
    NewProvider, OrgRepo, ProjectRepo, ProviderGroupRepo, ProviderRepo, ProviderUpdate,
    RateLimitRepo, RouteRepo, RouteTargetRepo, RouteVariantRepo, RouteVariantSpec,
    RouteVariantTargetSpec, TeamRepo, UnitRates, Within,
};
use sqlx::PgPool;
use uuid::Uuid;

/// Command-line arguments of `rolter seed` and the standalone `rolter-seed`
/// binary, so the two cannot drift apart. [`run`] is the whole command.
#[derive(clap::Args, Debug)]
pub struct SeedArgs {
    #[arg(long, env = "ROLTER_DATABASE_URL", hide_env_values = true)]
    pub database_url: String,
    /// org display name; created if no org with the derived slug exists yet
    #[arg(long, default_value = "default")]
    pub org: String,
    /// org slug; defaults to a slugified `--org`
    #[arg(long)]
    pub org_slug: Option<String>,
    /// admin user email; combined with --admin-password to create an admin user
    #[arg(long)]
    pub admin_email: Option<String>,
    #[arg(long)]
    pub admin_password: Option<String>,
    /// bootstrap rolter.toml to import providers/routes from. The file is the
    /// desired state: re-importing an edited file updates the rows it already
    /// created. Never overwrites a credential sealed through the dashboard,
    /// and never deletes rows the file no longer mentions
    #[arg(long)]
    pub import: Option<PathBuf>,
}

/// Connect, migrate and [`seed`] from command-line arguments, then print which
/// org was seeded. Shared by `rolter seed` and `rolter-seed`.
pub async fn run(args: SeedArgs) -> anyhow::Result<()> {
    let pool = rolter_store::postgres::connect(&args.database_url).await?;
    rolter_store::postgres::run_migrations(&pool).await?;

    let summary = seed(
        &pool,
        &SeedOptions {
            org: args.org,
            org_slug: args.org_slug,
            admin_email: args.admin_email,
            admin_password: args.admin_password,
            import: args.import,
        },
    )
    .await?;

    println!(
        "seeded org '{}' (slug '{}')",
        summary.org_name, summary.org_slug
    );
    Ok(())
}

/// Options controlling what [`seed`] creates. `org` defaults to `default` when
/// left empty; `org_slug` defaults to a slugified `org`.
#[derive(Debug, Clone, Default)]
pub struct SeedOptions {
    pub org: String,
    pub org_slug: Option<String>,
    pub admin_email: Option<String>,
    pub admin_password: Option<String>,
    pub import: Option<PathBuf>,
}

/// What [`seed`] found or created — enough for a caller to print a summary
/// without re-querying (never carries the admin password).
#[derive(Debug, Clone)]
pub struct SeedSummary {
    pub org_name: String,
    pub org_slug: String,
    pub admin_email: Option<String>,
    pub admin_created: bool,
}

/// Idempotently bootstrap `pool`: org, default team/project, optional admin,
/// optional bootstrap-toml import. Assumes migrations have already run.
pub async fn seed(pool: &PgPool, opts: &SeedOptions) -> anyhow::Result<SeedSummary> {
    // before the first insert, not with the import at the end of it: an
    // operator who sees `api_key_evn` named while the org is still being
    // created can stop and fix the file, rather than finding out from a
    // half-seeded database that the provider it named has no credential (#1434)
    if let Some(path) = &opts.import {
        lint_import_file(path);
    }

    let org_name = if opts.org.trim().is_empty() {
        "default"
    } else {
        opts.org.trim()
    };
    let org_slug = opts.org_slug.clone().unwrap_or_else(|| slugify(org_name));

    let orgs = OrgRepo(pool);
    let org = match orgs.list().await?.into_iter().find(|o| o.slug == org_slug) {
        Some(existing) => {
            tracing::info!(org = %existing.name, "org already exists, reusing");
            existing
        }
        None => {
            let created = orgs.create(org_name, &org_slug).await?;
            tracing::info!(org = %created.name, "created org");
            created
        }
    };

    let teams = TeamRepo(pool);
    let team = match teams
        .list(org.id)
        .await?
        .into_iter()
        .find(|t| t.name == "default")
    {
        Some(t) => t,
        None => teams.create(org.id, "default").await?,
    };

    let projects = ProjectRepo(pool);
    let project = match projects
        .list(team.id)
        .await?
        .into_iter()
        .find(|p| p.name == "default")
    {
        Some(p) => p,
        None => projects.create(team.id, "default").await?,
    };

    let mut admin_created = false;
    if let (Some(email), Some(password)) = (&opts.admin_email, &opts.admin_password) {
        admin_created = create_admin(pool, email, password).await?;
    }

    if let Some(path) = &opts.import {
        import_bootstrap_toml(pool, org.id, project.id, path).await?;
    }

    Ok(SeedSummary {
        org_name: org.name,
        org_slug: org.slug,
        admin_email: opts.admin_email.clone(),
        admin_created,
    })
}

/// Warn about unknown keys and recognised sections the bootstrap importer skips.
///
/// Returns how many were reported, so the emission is assertable without a
/// database. Nothing here can fail the import: an unknown key never stopped the
/// file from loading and must not start now, and a file that cannot be read at
/// all is reported with the error it deserves by the import itself.
fn lint_import_file(path: &Path) -> usize {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let unknown = rolter_core::config_lint::warn_unknown_keys_once(path, &raw);
            unknown + warn_skipped_sections(path, &raw)
        }
        Err(_) => 0,
    }
}

/// Top-level sections the importer writes to the store: one entry per field of
/// the parsed config that `import_config` / `import_bootstrap_toml` reads. The
/// skipped-section lint treats these as imported, and a source-scanning test
/// fails when the importer starts reading a field that is not listed here.
const IMPORTED_SECTIONS: &[&str] = &[
    "providers",
    "provider_groups",
    "routes",
    "model_prices",
    "budgets",
    "rate_limits",
    "prompt_templates",
];

/// Sections that configure the gateway process itself, or stamp the file, so
/// the bootstrap import has nothing to write and says nothing about them.
const GATEWAY_LOCAL_SECTIONS: &[&str] = &["schema_version", "server", "tls"];

/// The only `[logging]` keys the importer writes; the rest of the table is
/// reported as skipped.
const IMPORTED_LOGGING_KEYS: &[&str] = &["ui_events", "payload_capture"];

/// Where an operator manages a section the importer skips: a dashboard screen,
/// and where one exists the control-plane endpoint that does the same. The
/// endpoint is checked against the served OpenAPI document by a test, so a
/// renamed route fails the build instead of leaving the hint pointing nowhere.
struct ManagedIn {
    sections: &'static [&'static str],
    screen: &'static str,
    endpoint: Option<(&'static str, &'static str)>,
}

const MANAGED_IN: &[ManagedIn] = &[
    ManagedIn {
        sections: &["adaptive_routing"],
        screen: "dashboard Adaptive Routing",
        endpoint: Some(("PUT", "/api/v1/adaptive-routing-policy")),
    },
    ManagedIn {
        sections: &["virtual_keys", "db_virtual_keys"],
        screen: "dashboard Virtual Keys",
        endpoint: None,
    },
    ManagedIn {
        sections: &["mcp_servers", "mcp_oauth_sessions"],
        screen: "dashboard MCP Servers and MCP OAuth",
        endpoint: None,
    },
    ManagedIn {
        sections: &["logging"],
        screen: "dashboard Logs Settings; only ui_events and payload_capture are imported",
        endpoint: None,
    },
    ManagedIn {
        sections: &["plugins"],
        screen: "dashboard Plugins",
        endpoint: None,
    },
    ManagedIn {
        sections: &["models"],
        screen: "dashboard Models",
        endpoint: None,
    },
    ManagedIn {
        sections: &[
            "guardrails",
            "guardrail_webhook",
            "semantic_guards",
            "pii_sanitizer",
        ],
        screen: "dashboard Guardrails",
        endpoint: None,
    },
    ManagedIn {
        sections: &["feature_flags"],
        screen: "dashboard Feature Flags",
        endpoint: None,
    },
    ManagedIn {
        sections: &["security", "egress"],
        screen: "dashboard Security",
        endpoint: None,
    },
];

const MANAGED_IN_FALLBACK: &str =
    "the matching dashboard settings screen, or the gateway configuration for a file-backed deployment";

fn managed_in(section: &str) -> String {
    match MANAGED_IN
        .iter()
        .find(|entry| entry.sections.contains(&section))
    {
        Some(ManagedIn {
            screen,
            endpoint: Some((method, path)),
            ..
        }) => format!("{screen} or {method} {path}"),
        Some(ManagedIn { screen, .. }) => (*screen).to_string(),
        None => MANAGED_IN_FALLBACK.to_string(),
    }
}

/// Warn once per declared section that the bootstrap importer does not write.
/// Gateway-local settings and the schema stamp are intentionally quiet.
fn warn_skipped_sections(path: &Path, raw: &str) -> usize {
    let Ok(toml::Value::Table(declared)) = toml::from_str(raw) else {
        return 0;
    };
    let Ok(serde_json::Value::Object(known)) = serde_json::to_value(GatewayConfig::default())
    else {
        return 0;
    };
    let logging_fields = known.get("logging").and_then(serde_json::Value::as_object);
    let mut count = 0;
    for (section, value) in declared {
        if GATEWAY_LOCAL_SECTIONS.contains(&section.as_str())
            || IMPORTED_SECTIONS.contains(&section.as_str())
        {
            continue;
        }
        if section == "logging" {
            if value.as_table().is_some_and(|fields| {
                fields.keys().all(|key| {
                    IMPORTED_LOGGING_KEYS.contains(&key.as_str())
                        || logging_fields.is_none_or(|known| !known.contains_key(key))
                })
            }) {
                continue;
            }
        } else if !known.contains_key(&section) {
            // unknown keys belong to config_lint, not the skipped-section warning
            continue;
        }
        let managed_in = managed_in(&section);
        tracing::warn!(file = %path.display(), section = %section, managed_in = %managed_in, "bootstrap import does not apply this section");
        count += 1;
    }
    count
}

fn slugify(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .fold(String::new(), |mut acc, s| {
            if !acc.is_empty() {
                acc.push('-');
            }
            acc.push_str(s);
            acc
        })
}

/// The `providers.kind` column value for a parsed [`ProviderKind`]. Shared by
/// the provider import and anything else that has to name the stored enum.
fn provider_kind_column(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Openai => "openai",
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::OpenaiCompatible => "openai_compatible",
        ProviderKind::Ollama => "ollama",
        ProviderKind::OllamaCloud => "ollama_cloud",
        ProviderKind::LlamaCpp => "llama_cpp",
        ProviderKind::Openrouter => "openrouter",
        ProviderKind::Tei => "tei",
        ProviderKind::AzureOpenai => "azure_openai",
        ProviderKind::Bedrock => "bedrock",
        ProviderKind::Vertex => "vertex",
        ProviderKind::Gemini => "gemini",
        ProviderKind::GeminiNative => "gemini_native",
        ProviderKind::GeminiInteractions => "gemini_interactions",
        ProviderKind::Mistral => "mistral",
        ProviderKind::Groq => "groq",
        ProviderKind::Xai => "xai",
        ProviderKind::MetaLlamaApi => "meta_llama_api",
        ProviderKind::Cohere => "cohere",
        ProviderKind::Perplexity => "perplexity",
        ProviderKind::Together => "together",
        ProviderKind::Fireworks => "fireworks",
        ProviderKind::Databricks => "databricks",
        ProviderKind::AlephAlpha => "aleph_alpha",
        ProviderKind::Nebius => "nebius",
        ProviderKind::Ovhcloud => "ovhcloud",
        ProviderKind::Scaleway => "scaleway",
        ProviderKind::Deepseek => "deepseek",
        ProviderKind::Qwen => "qwen",
        ProviderKind::Zhipu => "zhipu",
        ProviderKind::Kimi => "kimi",
        ProviderKind::Ernie => "ernie",
        ProviderKind::Doubao => "doubao",
        ProviderKind::Hunyuan => "hunyuan",
        ProviderKind::Yi => "yi",
        ProviderKind::Minimax => "minimax",
        ProviderKind::Baichuan => "baichuan",
        ProviderKind::Gigachat => "gigachat",
        ProviderKind::YandexGpt => "yandex_gpt",
        ProviderKind::CloudRu => "cloud_ru",
        ProviderKind::MtsAi => "mts_ai",
        ProviderKind::Naver => "naver",
        ProviderKind::Upstage => "upstage",
        ProviderKind::Rinna => "rinna",
        ProviderKind::Rakuten => "rakuten",
        ProviderKind::Sarvam => "sarvam",
        ProviderKind::Krutrim => "krutrim",
        ProviderKind::Falcon => "falcon",
        ProviderKind::SystemOne => "system_one",
        ProviderKind::Infinity => "infinity",
    }
}

/// The import stops rather than write a name another org already holds: the
/// gateway keeps route, provider and group names in one namespace across every
/// org, so a second row would make the snapshot refuse for the whole fleet
/// (#1845). The message does not say whose it is.
fn taken_elsewhere(kind: &str, name: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{kind} '{name}' is already in use elsewhere in this deployment; rename it in the file"
    )
}

/// The stored strategy value for a parsed [`BalancingStrategy`]. Routes and
/// provider groups share the column vocabulary, so they share this.
fn strategy_column(strategy: BalancingStrategy) -> &'static str {
    match strategy {
        BalancingStrategy::RoundRobin => "round_robin",
        BalancingStrategy::Random => "random",
        BalancingStrategy::PowerOfTwo => "power_of_two",
        BalancingStrategy::ConsistentHash => "consistent_hash",
        BalancingStrategy::CacheAware => "cache_aware",
        BalancingStrategy::Weighted => "weighted",
        BalancingStrategy::Pipeline => "pipeline",
        BalancingStrategy::Cheapest => "cheapest",
        BalancingStrategy::Fastest => "fastest",
        BalancingStrategy::PreciseCacheAware => "precise_cache_aware",
        BalancingStrategy::LmcacheAware => "lmcache_aware",
        BalancingStrategy::Adaptive => "adaptive",
        BalancingStrategy::LoraAware => "lora_aware",
        BalancingStrategy::PredictedLatency => "predicted_latency",
    }
}

/// Create a superadmin user; returns `true` when a new row was inserted and
/// `false` when one already existed for `email`.
async fn create_admin(pool: &PgPool, email: &str, password: &str) -> anyhow::Result<bool> {
    let existing: Option<Uuid> = sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_optional(pool)
        .await?;
    if existing.is_some() {
        tracing::info!(%email, "admin user already exists, skipping");
        return Ok(false);
    }

    // hash_password generates its own 16-byte salt from getrandom
    let hash = Argon2::default()
        .hash_password(password.as_bytes())
        .map_err(|e| anyhow::anyhow!("hash admin password: {e}"))?
        .to_string();

    sqlx::query("insert into users (email, password_hash, is_superadmin) values ($1, $2, true)")
        .bind(email)
        .bind(hash)
        .execute(pool)
        .await?;
    tracing::info!(%email, "created admin user");
    Ok(true)
}

pub(crate) async fn import_bootstrap_toml(
    pool: &PgPool,
    org_id: Uuid,
    project_id: Uuid,
    path: &Path,
) -> anyhow::Result<()> {
    let config = GatewayConfig::load(path)?;
    import_config(pool, org_id, project_id, &config).await?;
    if declares_payload_capture(path)? {
        import_payload_capture(pool, &config.logging.payload_capture).await?;
    }
    if declares_ui_events(path)? {
        import_ui_events(pool, config.logging.ui_events).await?;
    }
    Ok(())
}

/// Whether the file at `path` writes `logging.ui_events` itself, for the same
/// reason as [`declares_payload_capture`].
fn declares_ui_events(path: &Path) -> anyhow::Result<bool> {
    let text = std::fs::read_to_string(path)?;
    let value: toml::Value = toml::from_str(&text)?;
    Ok(value
        .get("logging")
        .and_then(|logging| logging.get("ui_events"))
        .is_some())
}

/// Apply the file's UX event opt-out to the `logging_settings` row, leaving
/// every other column as it is.
async fn import_ui_events(pool: &PgPool, enabled: bool) -> anyhow::Result<()> {
    let repo = LoggingSettingsRepo(pool);
    let current = repo.get().await?;
    repo.update(LoggingSettingsUpdate {
        sample_rate: current.sample_rate,
        payload_capture_enabled: current.payload_capture_enabled,
        payload_capture_max_bytes: current.payload_capture_max_bytes,
        payload_capture_redact_fields: &current.payload_capture_redact_fields,
        payload_capture_redact_detectors: None,
        payload_capture_redact_patterns: None,
        payload_capture_models: &current.payload_capture_models,
        payload_capture_virtual_key_ids: &current.payload_capture_virtual_key_ids,
        retention_days: current.retention_days,
        payload_retention_hours: current.payload_retention_hours,
        ui_events: Some(enabled),
    })
    .await?;
    tracing::info!(enabled, "imported ui events policy");
    Ok(())
}

/// Whether the file at `path` writes a `[logging.payload_capture]` section of
/// its own. Parsed from the raw text rather than from [`GatewayConfig`],
/// which cannot tell a written default from an unwritten one.
fn declares_payload_capture(path: &Path) -> anyhow::Result<bool> {
    let text = std::fs::read_to_string(path)?;
    let value: toml::Value = toml::from_str(&text)?;
    Ok(value
        .get("logging")
        .and_then(|logging| logging.get("payload_capture"))
        .is_some())
}

/// Apply the file's payload-capture policy to the single `logging_settings`
/// row, leaving sampling and retention as they are — those are operational
/// dials the file has no opinion about.
async fn import_payload_capture(
    pool: &PgPool,
    capture: &rolter_core::PayloadCaptureConfig,
) -> anyhow::Result<()> {
    let repo = LoggingSettingsRepo(pool);
    let current = repo.get().await?;
    let max_bytes = i32::try_from(capture.max_bytes).unwrap_or(i32::MAX);
    repo.update(LoggingSettingsUpdate {
        sample_rate: current.sample_rate,
        payload_capture_enabled: capture.enabled,
        payload_capture_max_bytes: max_bytes,
        payload_capture_redact_fields: &capture.redact_fields,
        payload_capture_redact_detectors: Some(&capture.redact_detectors),
        payload_capture_redact_patterns: Some(&capture.redact_patterns),
        payload_capture_models: &capture.models,
        payload_capture_virtual_key_ids: &capture.virtual_key_ids,
        retention_days: current.retention_days,
        payload_retention_hours: current.payload_retention_hours,
        ui_events: None,
    })
    .await?;
    tracing::info!(
        enabled = capture.enabled,
        max_bytes,
        "imported payload capture policy"
    );
    Ok(())
}

/// Warn when a provider configures more keys than the store can hold.
///
/// `providers.api_key_env` is a single column and `provider_keys` a single
/// sealed row per provider, so everything past the first `api_keys` entry is
/// dropped on import. That is a real loss of configured capacity — providers
/// cap throughput per key — so it is said out loud rather than swallowed. It is
/// not fatal: refusing the import would take a deployment down over a weight.
fn warn_on_surplus_api_keys(p: &rolter_core::ProviderConfig) {
    let surplus = p.surplus_api_key_count();
    if surplus > 0 {
        tracing::warn!(
            provider = %p.name,
            dropped = surplus,
            "provider configures multiple api_keys; the store holds one per provider, \
             so only the first is imported"
        );
    }
}

/// Whether a stored json setting equals the file's, compared as json so key
/// order and defaults spelled out on one side do not read as a change.
fn same_setting<T: serde::Serialize>(stored: &Option<serde_json::Value>, file: &Option<T>) -> bool {
    let file = file.as_ref().and_then(|v| serde_json::to_value(v).ok());
    *stored == file
}

/// Refuse a file whose provider names one of the deployment's own secrets as
/// its credential variable. The import stores that name as `api_key_env`, and
/// the gateway would read it and send it to the provider's `api_base`; the
/// dashboard refuses the same names at save time. Checked before any row is
/// written, so a bad file imports nothing.
fn refuse_reserved_env_names(config: &GatewayConfig) -> anyhow::Result<()> {
    for p in &config.providers {
        let envs = p
            .api_key_env
            .iter()
            .chain(p.api_keys.iter().filter_map(|k| k.env.as_ref()));
        for env in envs {
            if rolter_core::is_control_plane_secret(env) {
                anyhow::bail!(
                    "provider '{}' names {env} as its credential, which holds a deployment \
                     secret and cannot be used as a provider credential",
                    p.name
                );
            }
        }
    }
    Ok(())
}

/// Upsert an already-parsed bootstrap config. Split out from
/// [`import_bootstrap_toml`] so the desired-state behaviour can be tested
/// without going through a file on disk.
async fn import_config(
    pool: &PgPool,
    org_id: Uuid,
    project_id: Uuid,
    config: &GatewayConfig,
) -> anyhow::Result<()> {
    refuse_reserved_env_names(config)?;
    let providers = ProviderRepo(pool);
    let routes = RouteRepo(pool);
    let targets = RouteTargetRepo(pool);

    let mut provider_ids = HashMap::new();
    for p in &config.providers {
        let kind = provider_kind_column(p.kind);
        // both spellings of the credential converge here: the store holds one
        // plaintext variable name per provider, and reading `api_key_env`
        // directly made a provider written as `api_keys = [{ env = "..." }]`
        // import with no credential at all (#1514)
        let api_key_env = p.api_key_env_name();
        warn_on_surplus_api_keys(p);
        // `project_scoped` means the project the file is imported into: a file
        // names no projects of its own (#1919)
        let scope = p.project_scoped.then_some(project_id);
        let existing = providers
            .list(org_id)
            .await?
            .into_iter()
            .find(|row| row.name == p.name);
        let row = match existing {
            // the file is the desired state, so an existing row is brought in
            // line with it rather than left alone (#927). the sealed credential
            // in `provider_keys` is never touched: a key rotated through the
            // dashboard outranks `api_key_env` in the snapshot's precedence and
            // must not be clobbered by a stale value from a file
            Some(row) => {
                let unchanged = row.kind == kind
                    && row.api_base == p.api_base
                    && row.api_key_env.as_deref() == api_key_env
                    && row.egress_proxy == p.egress_proxy
                    && row.egress_proxies.0 == p.egress_proxies
                    && same_setting(&row.kv_events, &p.kv_events)
                    && same_setting(&row.lmcache, &p.lmcache)
                    && row.allow_custom_api_base == p.allow_custom_api_base
                    && row.project_id == scope;
                if unchanged {
                    tracing::info!(provider = %p.name, "provider unchanged");
                    row
                } else {
                    // narrowing a provider to one project must not strand a
                    // route or group of another project that already uses it
                    if row.project_id != scope {
                        let dependents = providers.dependents_outside(row.id, scope).await?;
                        if !dependents.is_empty() {
                            return Err(anyhow::anyhow!(
                                "provider '{}' is marked project_scoped but {} use it from \
                                 outside this project; scope or remove those first",
                                p.name,
                                dependents.join(", ")
                            ));
                        }
                    }
                    let updated = providers
                        .update(
                            row.id,
                            ProviderUpdate {
                                // slug is immutable once assigned; renaming the
                                // stable identity of a provider is not something a
                                // re-import should do behind the operator's back
                                slug: None,
                                kind: Some(kind),
                                api_base: Some(&p.api_base),
                                api_key_env: Some(api_key_env),
                                egress_proxy: Some(p.egress_proxy.as_deref()),
                                egress_proxies: Some(&p.egress_proxies),
                                kv_events: Some(p.kv_events.as_ref()),
                                lmcache: Some(p.lmcache.as_ref()),
                                project_id: Some(scope),
                                allow_custom_api_base: Some(p.allow_custom_api_base),
                            },
                        )
                        .await?;
                    tracing::info!(provider = %p.name, "updated provider from file");
                    updated
                }
            }
            None => {
                let slug = p.slug.clone().unwrap_or_else(|| slugify(&p.name));
                // another org already holds the name; writing it would make the
                // snapshot refuse for the whole fleet (#1845)
                if providers.name_in_use(&p.name).await?
                    || providers.address_slug_in_use(&slug, None).await?
                {
                    return Err(taken_elsewhere("provider", &p.name));
                }
                let created = providers
                    .create(
                        org_id,
                        NewProvider {
                            name: &p.name,
                            slug: &slug,
                            kind,
                            api_base: &p.api_base,
                            api_key_env,
                            egress_proxy: p.egress_proxy.as_deref(),
                            egress_proxies: &p.egress_proxies,
                            kv_events: p.kv_events.as_ref(),
                            lmcache: p.lmcache.as_ref(),
                            project_id: scope,
                            allow_custom_api_base: p.allow_custom_api_base,
                        },
                    )
                    .await?;
                tracing::info!(provider = %p.name, "created provider");
                created
            }
        };
        provider_ids.insert(p.name.clone(), row.id);
    }

    for r in &config.routes {
        let strategy = strategy_column(r.strategy);
        let existing = routes
            .list(project_id)
            .await?
            .into_iter()
            .find(|row| row.model == r.model);
        let route_row = match existing {
            Some(row) if row.strategy == strategy => {
                tracing::info!(model = %r.model, "route unchanged");
                row
            }
            Some(row) => {
                let updated = routes.set_strategy(row.id, strategy).await?;
                tracing::info!(
                    model = %r.model,
                    from = %row.strategy,
                    to = strategy,
                    "updated route strategy from file"
                );
                updated
            }
            None => {
                if routes.model_in_use(&r.model).await? {
                    return Err(taken_elsewhere("route", &r.model));
                }
                // `slug/model` is another org's provider or group address; the
                // route would take it from the operator's own keys
                if routes
                    .name_takes_address_outside_org(&r.model, Some(org_id))
                    .await?
                {
                    return Err(anyhow::anyhow!(
                        "route '{}' is a provider or provider group address another \
                         organization answers; rename it in the file",
                        r.model
                    ));
                }
                let created = routes.create(project_id, &r.model, strategy).await?;
                tracing::info!(model = %r.model, "created route");
                created
            }
        };

        // round-trip the admin param defaults + override policy (idempotent:
        // re-importing overwrites with the toml's current values)
        if !r.params.is_empty()
            || r.param_policy.mode != rolter_core::OverrideMode::Allow
            || !r.param_policy.allow.is_empty()
            || !r.param_policy.deny.is_empty()
        {
            let params = serde_json::to_value(&r.params)?;
            let param_policy = serde_json::to_value(&r.param_policy)?;
            routes
                .set_params(route_row.id, &params, &param_policy)
                .await?;
        }

        let existing_targets = targets.list(route_row.id).await?;
        for t in &r.targets {
            let Some(&provider_id) = provider_ids.get(&t.provider) else {
                tracing::warn!(
                    target_provider = %t.provider,
                    model = %r.model,
                    "skipping target: provider not imported"
                );
                continue;
            };
            // provider + upstream model is the natural key; the weight is the
            // part an operator edits and re-imports, so a match updates it
            // rather than counting as "already there" (#927)
            let existing_target = existing_targets.iter().find(|row| {
                row.provider_id == provider_id
                    && row.upstream_model.as_deref() == t.model.as_deref()
            });
            match existing_target {
                Some(row) if row.weight == t.weight as i32 => continue,
                Some(row) => {
                    targets.set_weight(row.id, t.weight as i32).await?;
                    tracing::info!(
                        model = %r.model,
                        target_provider = %t.provider,
                        from = row.weight,
                        to = t.weight,
                        "updated target weight from file"
                    );
                }
                None => {
                    targets
                        .create(
                            route_row.id,
                            provider_id,
                            t.model.as_deref(),
                            t.weight as i32,
                        )
                        .await?;
                    tracing::info!(
                        model = %r.model,
                        target_provider = %t.provider,
                        "created route target"
                    );
                }
            }
        }
        import_route_variants(pool, route_row.id, r, &provider_ids).await?;
    }
    import_provider_groups(pool, org_id, project_id, config, &provider_ids).await?;
    import_model_prices(pool, config).await?;
    import_caps(pool, config).await?;
    import_prompt_templates(pool, org_id, project_id, config).await?;

    Ok(())
}

/// Make a route's `[[routes.variants]]` its stored variant set (#391), so a
/// config route moves into the database without losing its split.
///
/// A route whose file entry declares no variants keeps whatever the store
/// holds: a file written before variants existed would otherwise wipe a split
/// built in the dashboard. Those stored variants still decide the route's
/// routing, so a warning names them when the file gives the route targets of
/// its own, which they shadow.
///
/// A route that declares some has them replaced whole, in the file's order,
/// which is the routing order. That is the one place an import deletes rows
/// the file does not name (see the module doc): a variant set is ordered and
/// weighted as a whole, so merging two of them has no meaning. The variants it
/// removes are named in a warning. When a variant target names a provider the
/// file does not import, the route's variants are left alone, for the reason a
/// provider group with an unresolved member is: a partial set would drop the
/// variants the file could not name.
async fn import_route_variants(
    pool: &PgPool,
    route_id: Uuid,
    route: &rolter_core::ModelRoute,
    provider_ids: &HashMap<String, Uuid>,
) -> anyhow::Result<()> {
    let repo = RouteVariantRepo(pool);
    if route.variants.is_empty() {
        if !route.targets.is_empty() {
            let stored = repo.list(route_id).await?;
            if !stored.is_empty() {
                let names: Vec<&str> = stored.iter().map(|v| v.name.as_str()).collect();
                tracing::warn!(
                    model = %route.model,
                    variants = ?names,
                    "the file gives this route targets but no variants, and the stored \
                     variants are kept; while a route has variants its targets are not used, \
                     so the file's targets do not route until the variants are removed"
                );
            }
        }
        return Ok(());
    }
    let mut specs = Vec::with_capacity(route.variants.len());
    for variant in &route.variants {
        let mut targets = Vec::with_capacity(variant.targets.len());
        for target in &variant.targets {
            let Some(&provider_id) = provider_ids.get(&target.provider) else {
                tracing::warn!(
                    model = %route.model,
                    variant = %variant.name,
                    target_provider = %target.provider,
                    "skipping the route's variants: a variant target's provider was not imported"
                );
                return Ok(());
            };
            targets.push(RouteVariantTargetSpec {
                provider_id,
                upstream_model: target.model.clone(),
                weight: i32::try_from(target.weight)?,
            });
        }
        specs.push(RouteVariantSpec {
            name: variant.name.clone(),
            weight: i32::try_from(variant.weight)?,
            params: serde_json::to_value(&variant.params)?,
            targets,
        });
    }
    let stored = repo.list(route_id).await?;
    let removed: Vec<&str> = stored
        .iter()
        .map(|v| v.name.as_str())
        .filter(|name| !specs.iter().any(|spec| spec.name == *name))
        .collect();
    if !removed.is_empty() {
        tracing::warn!(
            model = %route.model,
            removed = ?removed,
            "the file's variants replace the route's whole set, so stored variants it \
             does not declare are deleted"
        );
    }
    let (_, changed) = repo.replace(route_id, &specs).await?;
    if changed {
        tracing::info!(model = %route.model, variants = specs.len(), "set route variants from file");
    } else {
        tracing::info!(model = %route.model, "route variants unchanged");
    }
    Ok(())
}

/// Upsert `[[provider_groups]]` keyed by slug, then replace each group's
/// membership with what the file names — the same desired-state rule the rest
/// of the import follows (#1082, so a configuration export re-imports whole).
///
/// A group whose members do not all resolve to providers the file also
/// declares is left entirely alone. Applying a partial membership would delete
/// the members the file could not name, which is the one thing an import is
/// never allowed to do.
async fn import_provider_groups(
    pool: &PgPool,
    org_id: Uuid,
    project_id: Uuid,
    config: &GatewayConfig,
    provider_ids: &HashMap<String, Uuid>,
) -> anyhow::Result<()> {
    if config.provider_groups.is_empty() {
        return Ok(());
    }
    let groups = ProviderGroupRepo(pool);
    for g in &config.provider_groups {
        let slug = g.slug.clone().unwrap_or_else(|| slugify(&g.name));
        let strategy = strategy_column(g.strategy);
        let scope = g.project_scoped.then_some(project_id);

        let mut members = Vec::with_capacity(g.members.len());
        let mut unresolved = false;
        for member in &g.members {
            match provider_ids.get(&member.provider) {
                Some(&provider_id) => {
                    members.push((provider_id, member.model.clone(), member.weight as i32))
                }
                None => {
                    tracing::warn!(
                        group = %g.name,
                        member_provider = %member.provider,
                        "leaving group membership alone: provider not imported"
                    );
                    unresolved = true;
                }
            }
        }
        // a project-scoped group may hold its own project's providers and
        // org-wide ones; an org-wide group only org-wide ones (#1919)
        let member_ids: Vec<Uuid> = members.iter().map(|(id, _, _)| *id).collect();
        let outside = ProviderRepo(pool)
            .scoped_outside(&member_ids, scope)
            .await?;
        if !outside.is_empty() {
            return Err(anyhow::anyhow!(
                "provider group '{}' holds project-scoped provider(s) {} its own scope does \
                 not cover; mark the group project_scoped too or use org-wide providers",
                g.name,
                outside.join(", ")
            ));
        }

        let existing = groups
            .list(org_id)
            .await?
            .into_iter()
            .find(|row| row.slug == slug);
        let row = match existing {
            Some(row)
                if row.name == g.name && row.strategy == strategy && row.project_id == scope =>
            {
                row
            }
            // the slug is the stable identity, so it is never rewritten — the
            // same rule providers follow
            Some(row) => {
                // a scope change the file leaves the membership alone for must
                // still suit the members the group already has
                if unresolved && row.project_id != scope {
                    let held: Vec<Uuid> = groups
                        .members(row.id)
                        .await?
                        .into_iter()
                        .map(|m| m.provider_id)
                        .collect();
                    let outside = ProviderRepo(pool).scoped_outside(&held, scope).await?;
                    if !outside.is_empty() {
                        return Err(anyhow::anyhow!(
                            "provider group '{}' holds project-scoped provider(s) {} its new \
                             scope does not cover",
                            g.name,
                            outside.join(", ")
                        ));
                    }
                }
                let updated = groups
                    .update(row.id, Some(&g.name), None, Some(strategy), Some(scope))
                    .await?;
                tracing::info!(group = %g.name, "updated provider group from file");
                updated
            }
            None => {
                // groups share the provider slug namespace at the gateway
                if ProviderRepo(pool).address_slug_in_use(&slug, None).await? {
                    return Err(taken_elsewhere("provider group", &g.name));
                }
                let created = groups
                    .create(org_id, &g.name, &slug, strategy, scope)
                    .await?;
                tracing::info!(group = %g.name, "created provider group");
                created
            }
        };

        if !unresolved {
            groups.set_members(row.id, &members).await?;
        }
    }
    Ok(())
}

/// The row a portable `[[budgets]]` / `[[rate_limits]]` reference names on this
/// deployment, as `(scope_type, scope_id)`; `None` when it names nothing here
/// (#1312).
///
/// The reference is the file's `id`, spelled the way the export writes it:
/// `acme` for an org slug, `acme/platform` for a team (org slug, team name),
/// `acme/platform/gateway` for a project, `acme/payments` for a business unit
/// or customer (org slug, its own slug). A database id is refused rather than
/// resolved, since it is the thing that does not survive a move; a virtual key
/// or a person has no name that does, so those are never resolved.
async fn resolve_cap_scope(
    pool: &PgPool,
    scope: BudgetScope,
    reference: &str,
) -> anyhow::Result<Option<(&'static str, Uuid)>> {
    if Uuid::parse_str(reference).is_ok() {
        return Ok(None);
    }
    let parts: Vec<&str> = reference.split('/').collect();
    let (scope_type, found): (&'static str, Option<Uuid>) = match (scope, parts.as_slice()) {
        (BudgetScope::Org, [org]) => (
            "org",
            sqlx::query_scalar("select id from orgs where slug = $1")
                .bind(org)
                .fetch_optional(pool)
                .await?,
        ),
        (BudgetScope::Team, [org, team]) => (
            "team",
            sqlx::query_scalar(
                "select t.id from teams t join orgs o on o.id = t.org_id
                 where o.slug = $1 and t.name = $2",
            )
            .bind(org)
            .bind(team)
            .fetch_optional(pool)
            .await?,
        ),
        (BudgetScope::Project, [org, team, project]) => (
            "project",
            sqlx::query_scalar(
                "select p.id from projects p join teams t on t.id = p.team_id
                 join orgs o on o.id = t.org_id
                 where o.slug = $1 and t.name = $2 and p.name = $3",
            )
            .bind(org)
            .bind(team)
            .bind(project)
            .fetch_optional(pool)
            .await?,
        ),
        (BudgetScope::BusinessUnit, [org, slug]) => (
            "business_unit",
            sqlx::query_scalar(
                "select x.id from business_units x join orgs o on o.id = x.org_id
                 where o.slug = $1 and x.slug = $2",
            )
            .bind(org)
            .bind(slug)
            .fetch_optional(pool)
            .await?,
        ),
        (BudgetScope::Customer, [org, slug]) => (
            "customer",
            sqlx::query_scalar(
                "select x.id from customers x join orgs o on o.id = x.org_id
                 where o.slug = $1 and x.slug = $2",
            )
            .bind(org)
            .bind(slug)
            .fetch_optional(pool)
            .await?,
        ),
        _ => return Ok(None),
    };
    Ok(found.map(|id| (scope_type, id)))
}

/// Upsert `[[budgets]]` and `[[rate_limits]]` (#1312).
///
/// A cap's identity is its scope plus, for a budget, its window; two caps that
/// share one are matched to the file's entries in order, oldest first, so the
/// export of a deployment that has two re-imports as two. An entry whose scope
/// does not resolve here is left alone and reported, never half applied: the
/// same rule as a provider group with a member the file cannot name. Rows the
/// file does not mention are not deleted. A value outside what the column or
/// the gateway can hold fails the import before it writes anything, as a
/// negative price does.
async fn import_caps(pool: &PgPool, config: &GatewayConfig) -> anyhow::Result<()> {
    use rust_decimal::Decimal;
    let ceiling: Decimal = "99999999.9999".parse()?;
    for b in &config.budgets {
        if b.limit_usd.is_sign_negative() || b.limit_usd > ceiling {
            anyhow::bail!(
                "budget on {:?} '{}': limit_usd must be from 0 to 99999999.9999",
                b.scope,
                b.id
            );
        }
    }
    for l in &config.rate_limits {
        if l.rpm == Some(0) || l.tpm == Some(0) || (l.rpm.is_none() && l.tpm.is_none()) {
            anyhow::bail!(
                "rate limit on {:?} '{}': needs an rpm cap, a tpm cap or both, each at least 1",
                l.scope,
                l.id
            );
        }
    }

    let budgets = BudgetRepo(pool);
    let mut taken: HashMap<(&'static str, Uuid, BudgetPeriod), usize> = HashMap::new();
    for b in &config.budgets {
        let Some((scope_type, scope_id)) = resolve_cap_scope(pool, b.scope, &b.id).await? else {
            tracing::warn!(scope = ?b.scope, id = %b.id, "budget scope does not resolve on this deployment; left alone");
            continue;
        };
        let nth = taken.entry((scope_type, scope_id, b.period)).or_default();
        let same_window: Vec<_> = budgets
            .list_for_scope(scope_type, scope_id)
            .await?
            .into_iter()
            .filter(|row| BudgetPeriod::parse(&row.period) == Some(b.period))
            .collect();
        let limit = b.limit_usd.to_string();
        let policy = b.unpriced_policy.map(|p| match p {
            rolter_core::UnpricedPolicy::Ignore => "ignore",
            rolter_core::UnpricedPolicy::Warn => "warn",
            rolter_core::UnpricedPolicy::Block => "block",
        });
        match same_window.get(*nth) {
            Some(row) => {
                // the period is left in whatever spelling the row has: `30d`
                // and `monthly` are one window, and rewriting one into the
                // other would bump the version for nothing
                let edit = budgets
                    .update(row.id, Some(&limit), None, Some(policy))
                    .await?;
                if edit.changed {
                    tracing::info!(scope = ?b.scope, id = %b.id, "updated budget from file");
                }
            }
            None => {
                let period = match b.period {
                    BudgetPeriod::Daily => "daily",
                    BudgetPeriod::Monthly => "monthly",
                    BudgetPeriod::Total => "total",
                };
                budgets
                    .create(
                        scope_type,
                        scope_id,
                        &limit,
                        period,
                        policy,
                        Within::default(),
                    )
                    .await?;
                tracing::info!(scope = ?b.scope, id = %b.id, "created budget");
            }
        }
        *nth += 1;
    }

    let limits = RateLimitRepo(pool);
    let mut taken: HashMap<(&'static str, Uuid), usize> = HashMap::new();
    for l in &config.rate_limits {
        let Some((scope_type, scope_id)) = resolve_cap_scope(pool, l.scope, &l.id).await? else {
            tracing::warn!(scope = ?l.scope, id = %l.id, "rate limit scope does not resolve on this deployment; left alone");
            continue;
        };
        let nth = taken.entry((scope_type, scope_id)).or_default();
        let existing = limits.list_for_scope(scope_type, scope_id).await?;
        let rpm = l.rpm.map(i32::try_from).transpose()?;
        let tpm = l.tpm.map(i32::try_from).transpose()?;
        match existing.get(*nth) {
            Some(row) => {
                let edit = limits.update(row.id, Some(rpm), Some(tpm)).await?;
                if edit.changed {
                    tracing::info!(scope = ?l.scope, id = %l.id, "updated rate limit from file");
                }
            }
            None => {
                limits
                    .create(scope_type, scope_id, rpm, tpm, Within::default())
                    .await?;
                tracing::info!(scope = ?l.scope, id = %l.id, "created rate limit");
            }
        }
        *nth += 1;
    }
    Ok(())
}

/// Upsert `[[model_prices]]` keyed by public model name. Prices are what a
/// budget counts against, so an export that carried routes without them would
/// promote a deployment that silently charges nothing (#1082).
async fn import_model_prices(pool: &PgPool, config: &GatewayConfig) -> anyhow::Result<()> {
    if config.model_prices.is_empty() {
        return Ok(());
    }
    let prices = ModelPriceRepo(pool);
    // checked before the first write, so a file with one bad price imports
    // none of them rather than half. the database would refuse the row anyway
    // (0084), but with a constraint name where this says which price and why
    let problems: Vec<String> = config
        .model_prices
        .iter()
        .flat_map(|price| {
            price
                .rate_problems()
                .into_iter()
                .map(|problem| format!("[[model_prices]] '{}': {problem}", price.model))
        })
        .collect();
    if !problems.is_empty() {
        anyhow::bail!("refusing to import: {}", problems.join("; "));
    }
    for price in &config.model_prices {
        // the column is `numeric`, so the rate crosses as text rather than
        // through a float bind that would round it on the way in
        prices
            .upsert(NewModelPrice {
                model: &price.model,
                input_per_mtok: &price.input_per_mtok.to_string(),
                output_per_mtok: &price.output_per_mtok.to_string(),
                cached_input_per_mtok: price
                    .cached_input_per_mtok
                    .map(|v| v.to_string())
                    .as_deref(),
                // the file is the desired state, so a price without the key
                // is a price without a write rate: `Some(None)` clears one
                // rather than leaving a stale rate behind
                cache_write_per_mtok: Some(
                    price.cache_write_per_mtok.map(|v| v.to_string()).as_deref(),
                ),
                // and the same for the 1 hour rate (#2891)
                cache_write_1h_per_mtok: Some(
                    price
                        .cache_write_1h_per_mtok
                        .map(|v| v.to_string())
                        .as_deref(),
                ),
                // and the unit rates (#2937)
                // and the audio token rates (#1882)
                units: UnitRates {
                    image_per_unit: Some(price.image_per_unit.map(|v| v.to_string()).as_deref()),
                    audio_input_per_minute: Some(
                        price
                            .audio_input_per_minute
                            .map(|v| v.to_string())
                            .as_deref(),
                    ),
                    speech_per_mchar: Some(
                        price.speech_per_mchar.map(|v| v.to_string()).as_deref(),
                    ),

                    audio_input_per_mtok: Some(
                        price.audio_input_per_mtok.map(|v| v.to_string()).as_deref(),
                    ),
                    audio_output_per_mtok: Some(
                        price
                            .audio_output_per_mtok
                            .map(|v| v.to_string())
                            .as_deref(),
                    ),
                    cached_audio_input_per_mtok: Some(
                        price
                            .cached_audio_input_per_mtok
                            .map(|v| v.to_string())
                            .as_deref(),
                    ),
                },
                currency: &price.currency,
            })
            .await?;
        tracing::info!(model = %price.model, "imported model price");
    }
    Ok(())
}

async fn import_prompt_templates(
    pool: &PgPool,
    org_id: Uuid,
    project_id: Uuid,
    config: &GatewayConfig,
) -> anyhow::Result<()> {
    if config.prompt_templates.templates.is_empty() {
        return Ok(());
    }

    let routes = RouteRepo(pool);
    let route_rows = routes.list(project_id).await?;
    let mut route_ids_by_model: HashMap<String, Vec<Uuid>> = HashMap::new();
    for route in route_rows {
        route_ids_by_model
            .entry(route.model.clone())
            .or_default()
            .push(route.id);
    }

    for template in &config.prompt_templates.templates {
        import_prompt_template(pool, org_id, template, &route_ids_by_model).await?;
    }
    Ok(())
}

async fn import_prompt_template(
    pool: &PgPool,
    org_id: Uuid,
    template: &PromptTemplate,
    route_ids_by_model: &HashMap<String, Vec<Uuid>>,
) -> anyhow::Result<()> {
    let variables = serde_json::to_value(&template.variables)?;
    let decorators = serde_json::to_value(&template.decorators)?;
    let version = i32::try_from(template.version)
        .map_err(|_| anyhow::anyhow!("prompt template version out of range"))?;

    let mut tx = pool.begin().await?;
    sqlx::query(
        "insert into prompt_templates (org_id, name, slug, description)
         values ($1, $2, $3, '')
         on conflict (org_id, slug) do nothing",
    )
    .bind(org_id)
    .bind(&template.id)
    .bind(&template.id)
    .execute(&mut *tx)
    .await?;
    let template_id: Uuid =
        sqlx::query_scalar("select id from prompt_templates where org_id = $1 and slug = $2")
            .bind(org_id)
            .bind(&template.id)
            .fetch_one(&mut *tx)
            .await?;

    let existing_version: Option<(serde_json::Value, serde_json::Value)> = sqlx::query_as(
        "select variables, decorators
         from prompt_template_versions
         where template_id = $1 and version = $2",
    )
    .bind(template_id)
    .bind(version)
    .fetch_optional(&mut *tx)
    .await?;
    match existing_version {
        Some((stored_variables, stored_decorators))
            if stored_variables != variables || stored_decorators != decorators =>
        {
            anyhow::bail!(
                "prompt template '{}' version {} differs from the immutable stored version",
                template.id,
                template.version
            );
        }
        Some(_) => {}
        None => {
            sqlx::query(
                "insert into prompt_template_versions (
                     template_id, version, variables, decorators
                 )
                 values ($1, $2, $3, $4)",
            )
            .bind(template_id)
            .bind(version)
            .bind(&variables)
            .bind(&decorators)
            .execute(&mut *tx)
            .await?;
        }
    }

    let published_version: Option<i32> =
        sqlx::query_scalar("select published_version from prompt_templates where id = $1")
            .bind(template_id)
            .fetch_one(&mut *tx)
            .await?;
    if published_version.is_none() {
        if template.routes.is_empty() {
            sqlx::query(
                "insert into prompt_template_scopes (
                     template_id, version, scope_type, scope_id, org_id
                 )
                 values ($1, $2, 'org', $3, $3)
                 on conflict (template_id, version, scope_type, scope_id) do nothing",
            )
            .bind(template_id)
            .bind(version)
            .bind(org_id)
            .execute(&mut *tx)
            .await?;
        } else {
            for model in &template.routes {
                let Some(route_ids) = route_ids_by_model.get(model) else {
                    tracing::warn!(template = %template.id, model, "skipping prompt-template route scope: route model not imported");
                    continue;
                };
                for route_id in route_ids {
                    sqlx::query(
                        "insert into prompt_template_scopes (
                             template_id, version, scope_type, scope_id, route_id
                         )
                         values ($1, $2, 'route', $3, $3)
                         on conflict (template_id, version, scope_type, scope_id) do nothing",
                    )
                    .bind(template_id)
                    .bind(version)
                    .bind(route_id)
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }

        sqlx::query(
            "update prompt_templates
             set published_version = $2
             where id = $1",
        )
        .bind(template_id)
        .bind(version)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    tracing::info!(template = %template.id, version = template.version, "imported prompt template");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        declares_payload_capture, declares_ui_events, import_bootstrap_toml,
        import_prompt_template, lint_import_file, slugify,
    };
    use rolter_core::{
        Decorator, DecoratorPosition, DecoratorRole, PromptTemplate, TemplateVariable,
    };
    use rolter_store::postgres::repo::{ProviderRepo, RouteRepo, RouteTargetRepo};
    use rolter_store::postgres::test_schema::TestSchema;
    use std::collections::HashMap;
    use uuid::Uuid;

    /// #3097: an import file naming a deployment secret as a credential imports
    /// nothing, so the name never reaches `providers.api_key_env`
    #[test]
    fn an_import_naming_a_deployment_secret_as_a_credential_is_refused() {
        let toml = |env: &str| {
            format!(
                "[[providers]]\nname = \"p\"\nkind = \"openai\"\n\
                 api_base = \"http://127.0.0.1:1\"\napi_key_env = \"{env}\"\n"
            )
        };
        let bad = rolter_core::GatewayConfig::from_toml_str(&toml("ROLTER_KEK")).unwrap();
        let err = super::refuse_reserved_env_names(&bad)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("provider 'p'") && err.contains("ROLTER_KEK"),
            "{err}"
        );
        let ok = rolter_core::GatewayConfig::from_toml_str(&toml("OPENAI_API_KEY")).unwrap();
        assert!(super::refuse_reserved_env_names(&ok).is_ok());
    }

    #[test]
    fn slugify_normalizes() {
        assert_eq!(slugify("Default"), "default");
        assert_eq!(slugify("Acme Corp!"), "acme-corp");
        assert_eq!(slugify("  multi  space "), "multi-space");
    }

    /// #1434: a mistyped key in the imported file seeds nothing, and the
    /// database ends up quietly missing the setting the operator wrote.
    #[test]
    fn an_unrecognised_key_in_the_import_file_is_reported_without_stopping_the_import() {
        let dir = tempdir("lint-import");
        let path = dir.join("rolter.toml");
        std::fs::write(
            &path,
            r#"
[[providers]]
name = "local"
kind = "openai_compatible"
api_base = "http://127.0.0.1:8000/v1"
api_key_evn = "LOCAL_API_KEY"
"#,
        )
        .unwrap();

        assert_eq!(lint_import_file(&path), 1);
        // and the file the lint complained about is still perfectly importable:
        // nothing about an unknown key may abort a seed
        let config =
            rolter_core::GatewayConfig::load(&path).expect("an unknown key never blocks the load");
        assert_eq!(config.providers.len(), 1);
    }

    #[test]
    fn a_recognised_but_skipped_import_section_is_reported() {
        let dir = tempdir("skipped-import");
        let path = dir.join("rolter.toml");
        std::fs::write(&path, "[adaptive_routing]\nenabled = true\n").unwrap();
        let output = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        #[derive(Clone)]
        struct Capture(std::sync::Arc<parking_lot::Mutex<Vec<u8>>>);
        impl std::io::Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let captured = output.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || Capture(captured.clone()))
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            assert_eq!(lint_import_file(&path), 1);
        });
        let log = String::from_utf8(output.lock().clone()).unwrap();
        assert_eq!(
            log.matches("bootstrap import does not apply this section")
                .count(),
            1
        );
        assert!(log.contains("adaptive_routing"), "{log}");
        assert!(log.contains("PUT /api/v1/adaptive-routing-policy"), "{log}");
    }

    #[test]
    fn skipped_sections_warn_once_and_unknown_logging_keys_are_not_double_reported() {
        let path = std::path::Path::new("bootstrap.toml");
        assert_eq!(super::warn_skipped_sections(path, "[retry]\nmax_retries = 3\n[queue]\nenabled = true\n[logging]\nui_events = false\nsample_rate = 0.1\n"), 3);
        assert_eq!(
            super::warn_skipped_sections(path, "[logging]\nui_evnts = false\n"),
            0
        );
        assert_eq!(
            super::warn_skipped_sections(path, "[invented]\nvalue = 1\n"),
            0
        );
        assert_eq!(super::warn_skipped_sections(path, "not valid toml"), 0);
    }

    /// The lint's list of imported sections is the importer's own: every
    /// `config.<field>` the non-test code reads is one the lint stays quiet
    /// about, and a field the importer stopped reading is no longer claimed.
    /// Scans the source, so teaching the importer a section without listing it
    /// in `IMPORTED_SECTIONS` fails here instead of leaving a stale warning.
    #[test]
    fn the_imported_sections_are_the_config_fields_the_importer_reads() {
        let source = include_str!("seed.rs");
        let production = &source[..source.find("mod tests {").expect("tests module")];
        let mut read = std::collections::BTreeSet::new();
        for (at, _) in production.match_indices("config.") {
            let before = production[..at].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.') {
                continue;
            }
            let field: String = production[at + "config.".len()..]
                .chars()
                .take_while(|c| c.is_ascii_lowercase() || *c == '_')
                .collect();
            if !field.is_empty() {
                read.insert(field);
            }
        }
        // `logging` is imported key by key, which IMPORTED_LOGGING_KEYS names
        read.remove("logging");
        let listed: std::collections::BTreeSet<String> = super::IMPORTED_SECTIONS
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            read, listed,
            "IMPORTED_SECTIONS must name exactly the config fields the importer reads"
        );
        for key in super::IMPORTED_LOGGING_KEYS {
            assert!(
                production.contains(&format!("config.logging.{key}")),
                "the importer no longer reads logging.{key}"
            );
        }
    }

    /// Every endpoint a skipped-section hint points an operator at is one the
    /// served OpenAPI document describes, so renaming a route fails here.
    #[test]
    fn every_managed_in_endpoint_is_a_documented_control_plane_operation() {
        for entry in super::MANAGED_IN {
            if let Some((method, path)) = entry.endpoint {
                assert!(
                    crate::openapi::documents_operation(method, path),
                    "{method} {path} is not in the served OpenAPI document"
                );
            }
        }
    }

    /// A section named in the hint table is one the lint would report, never
    /// one it treats as imported or gateway-local.
    #[test]
    fn no_hint_is_written_for_a_section_the_lint_never_reports() {
        for entry in super::MANAGED_IN {
            for section in entry.sections {
                assert!(!super::IMPORTED_SECTIONS.contains(section), "{section}");
                assert!(
                    !super::GATEWAY_LOCAL_SECTIONS.contains(section),
                    "{section}"
                );
            }
        }
    }

    #[test]
    fn imported_and_gateway_local_sections_do_not_warn() {
        let dir = tempdir("imported-sections");
        let path = dir.join("rolter.toml");
        std::fs::write(&path, "schema_version = 2\n[server]\n[tls]\n[logging]\nui_events = false\n[logging.payload_capture]\nenabled = false\n").unwrap();
        assert_eq!(lint_import_file(&path), 0);
    }

    #[test]
    fn a_missing_import_file_is_not_this_lint_to_report() {
        // the import itself fails with a readable error; a second complaint
        // about a file nobody can read adds nothing
        assert_eq!(
            lint_import_file(&tempdir("lint-absent").join("nope.toml")),
            0
        );
    }

    /// A provider written in the current plural spelling imported with no
    /// credential at all: the upsert read `api_key_env` directly, saw `None`,
    /// and the key was gone (#1514).
    #[tokio::test]
    async fn a_plural_form_credential_survives_the_import() {
        let Some(db) = scratch_db().await else {
            return;
        };
        let pool = db.pool().clone();
        let (org_id, project_id) = bootstrap_org(&pool).await;

        let dir = tempdir("plural-keys");
        let path = dir.join("rolter.toml");
        std::fs::write(
            &path,
            r#"
[[providers]]
name = "openai-plural"
kind = "openai"
api_base = "https://api.openai.com"
[[providers.api_keys]]
env = "OPENAI_KEY_A"
weight = 3
[[providers.api_keys]]
env = "OPENAI_KEY_B"
weight = 1
"#,
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();

        let provider = ProviderRepo(&pool)
            .list(org_id)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.name == "openai-plural")
            .expect("the provider must have been created");
        assert_eq!(
            provider.api_key_env.as_deref(),
            Some("OPENAI_KEY_A"),
            "the plural-form credential was dropped on import"
        );

        // and the change-detection compares the same spelling, so re-importing
        // the identical file is a no-op rather than a perpetual rewrite
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        let rows = ProviderRepo(&pool).list(org_id).await.unwrap();
        assert_eq!(rows.iter().filter(|r| r.name == "openai-plural").count(), 1);
        assert_eq!(
            rows.iter()
                .find(|r| r.name == "openai-plural")
                .and_then(|r| r.api_key_env.as_deref()),
            Some("OPENAI_KEY_A")
        );
    }

    /// #2876: the file is the desired state, so the cache-write rate follows it
    /// both ways. A price with the key sets the rate, and the same price
    /// without it clears one rather than leaving a rate nobody wrote down.
    #[tokio::test]
    async fn the_cache_write_rate_follows_the_import_file() {
        let Some(db) = scratch_db().await else {
            return;
        };
        let pool = db.pool().clone();
        let (org_id, project_id) = bootstrap_org(&pool).await;
        let dir = tempdir("cache-write-rate");
        let path = dir.join("rolter.toml");
        let prices = rolter_store::postgres::repo::ModelPriceRepo(&pool);
        let stored = || async {
            prices
                .list()
                .await
                .unwrap()
                .into_iter()
                .find(|price| price.model == "claude")
                .expect("the price must have been imported")
                .cache_write_per_mtok
        };

        std::fs::write(
            &path,
            "[[model_prices]]\nmodel = \"claude\"\ninput_per_mtok = 3.0\n\
             output_per_mtok = 15.0\ncache_write_per_mtok = 3.75\ncurrency = \"USD\"\n",
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        assert_eq!(stored().await.as_deref(), Some("3.750000"));

        std::fs::write(
            &path,
            "[[model_prices]]\nmodel = \"claude\"\ninput_per_mtok = 3.0\n\
             output_per_mtok = 15.0\ncurrency = \"USD\"\n",
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        assert_eq!(stored().await, None);
    }

    /// #2889: a negative rate is refused before anything is written, and says
    /// which price it is, instead of surfacing as a constraint violation on
    /// whichever row the database reached first.
    #[tokio::test]
    async fn a_negative_rate_in_the_import_file_imports_nothing() {
        let Some(db) = scratch_db().await else {
            return;
        };
        let pool = db.pool().clone();
        let (org_id, project_id) = bootstrap_org(&pool).await;
        let dir = tempdir("negative-rate");
        let path = dir.join("rolter.toml");
        std::fs::write(
            &path,
            "[[model_prices]]\nmodel = \"fine\"\ninput_per_mtok = 3.0\noutput_per_mtok = 15.0\n\n\
             [[model_prices]]\nmodel = \"refunds\"\ninput_per_mtok = 3.0\n\
             output_per_mtok = 15.0\ncached_input_per_mtok = -0.3\n",
        )
        .unwrap();

        let error = import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .expect_err("a negative rate is refused");
        let message = error.to_string();
        assert!(message.contains("'refunds'"), "{message}");
        assert!(message.contains("cached_input_per_mtok"), "{message}");
        let prices = rolter_store::postgres::repo::ModelPriceRepo(&pool);
        assert!(prices.list().await.unwrap().is_empty());

        // and the same for the 1 hour rate (#2891)
        std::fs::write(
            &path,
            "[[model_prices]]\nmodel = \"long\"\ninput_per_mtok = 3.0\n\
             output_per_mtok = 15.0\ncache_write_1h_per_mtok = -6.0\n",
        )
        .unwrap();
        let error = import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .expect_err("a negative 1 hour rate is refused");
        let message = error.to_string();
        assert!(message.contains("'long'"), "{message}");
        assert!(message.contains("cache_write_1h_per_mtok"), "{message}");
        assert!(prices.list().await.unwrap().is_empty());
    }

    /// #2891: the 1 hour rate follows the file the same way, independently of
    /// the 5 minute one.
    #[tokio::test]
    async fn the_one_hour_cache_write_rate_follows_the_import_file() {
        let Some(db) = scratch_db().await else {
            return;
        };
        let pool = db.pool().clone();
        let (org_id, project_id) = bootstrap_org(&pool).await;
        let dir = tempdir("cache-write-1h-rate");
        let path = dir.join("rolter.toml");
        let prices = rolter_store::postgres::repo::ModelPriceRepo(&pool);
        let stored = || async {
            let price = prices
                .list()
                .await
                .unwrap()
                .into_iter()
                .find(|price| price.model == "claude")
                .expect("the price must have been imported");
            (price.cache_write_per_mtok, price.cache_write_1h_per_mtok)
        };

        std::fs::write(
            &path,
            "[[model_prices]]\nmodel = \"claude\"\ninput_per_mtok = 3.0\n\
             output_per_mtok = 15.0\ncache_write_per_mtok = 3.75\n\
             cache_write_1h_per_mtok = 6.0\ncurrency = \"USD\"\n",
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        let (five_minute, one_hour) = stored().await;
        assert_eq!(five_minute.as_deref(), Some("3.750000"));
        assert_eq!(one_hour.as_deref(), Some("6.000000"));

        // the file drops the 1 hour key only: that rate is cleared and the
        // other stays
        std::fs::write(
            &path,
            "[[model_prices]]\nmodel = \"claude\"\ninput_per_mtok = 3.0\n\
             output_per_mtok = 15.0\ncache_write_per_mtok = 3.75\ncurrency = \"USD\"\n",
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        let (five_minute, one_hour) = stored().await;
        assert_eq!(five_minute.as_deref(), Some("3.750000"));
        assert_eq!(one_hour, None);
    }

    /// #2937: the unit rates follow the file as the cache write rates do, and a
    /// negative one refuses the whole import.
    #[tokio::test]
    async fn the_unit_rates_follow_the_import_file() {
        let Some(db) = scratch_db().await else {
            return;
        };
        let pool = db.pool().clone();
        let (org_id, project_id) = bootstrap_org(&pool).await;
        let dir = tempdir("unit-rates");
        let path = dir.join("rolter.toml");
        let prices = rolter_store::postgres::repo::ModelPriceRepo(&pool);
        let stored = || async {
            let price = prices
                .list()
                .await
                .unwrap()
                .into_iter()
                .find(|price| price.model == "media")
                .expect("the price must have been imported");
            (
                price.image_per_unit,
                price.audio_input_per_minute,
                price.speech_per_mchar,
            )
        };

        std::fs::write(
            &path,
            "[[model_prices]]\nmodel = \"media\"\ninput_per_mtok = 0\noutput_per_mtok = 0\n\
             image_per_unit = 0.04\naudio_input_per_minute = 0.006\nspeech_per_mchar = 15\n",
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        let (image, audio, speech) = stored().await;
        assert_eq!(image.as_deref(), Some("0.040000"));
        assert_eq!(audio.as_deref(), Some("0.006000"));
        assert_eq!(speech.as_deref(), Some("15.000000"));

        // the file is the desired state: a key it drops is cleared
        std::fs::write(
            &path,
            "[[model_prices]]\nmodel = \"media\"\ninput_per_mtok = 0\noutput_per_mtok = 0\n\
             image_per_unit = 0.04\n",
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        let (image, audio, speech) = stored().await;
        assert_eq!(image.as_deref(), Some("0.040000"));
        assert_eq!((audio, speech), (None, None));

        std::fs::write(
            &path,
            "[[model_prices]]\nmodel = \"media\"\ninput_per_mtok = 0\noutput_per_mtok = 0\n\
             speech_per_mchar = -1\n",
        )
        .unwrap();
        let refused = import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap_err();
        assert!(
            refused.to_string().contains("speech_per_mchar"),
            "{refused}"
        );
    }

    /// #1882: the audio token rates follow the import file like the unit rates,
    /// and a negative one refuses the whole import.
    #[tokio::test]
    async fn the_audio_rates_follow_the_import_file() {
        let Some(db) = scratch_db().await else {
            return;
        };
        let pool = db.pool().clone();
        let (org_id, project_id) = bootstrap_org(&pool).await;
        let dir = tempdir("audio-rates");
        let path = dir.join("rolter.toml");
        let prices = rolter_store::postgres::repo::ModelPriceRepo(&pool);
        let stored = || async {
            let price = prices
                .list()
                .await
                .unwrap()
                .into_iter()
                .find(|price| price.model == "voice")
                .expect("the price must have been imported");
            (
                price.audio_input_per_mtok,
                price.audio_output_per_mtok,
                price.cached_audio_input_per_mtok,
            )
        };

        std::fs::write(
            &path,
            "[[model_prices]]\nmodel = \"voice\"\ninput_per_mtok = 4\noutput_per_mtok = 16\n\
             audio_input_per_mtok = 32\naudio_output_per_mtok = 64\n\
             cached_audio_input_per_mtok = 0.4\n",
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        let (input, output, cached) = stored().await;
        assert_eq!(input.as_deref(), Some("32.000000"));
        assert_eq!(output.as_deref(), Some("64.000000"));
        assert_eq!(cached.as_deref(), Some("0.400000"));

        // the file is the desired state: a key it drops is cleared
        std::fs::write(
            &path,
            "[[model_prices]]\nmodel = \"voice\"\ninput_per_mtok = 4\noutput_per_mtok = 16\n\
             audio_input_per_mtok = 32\n",
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        let (input, output, cached) = stored().await;
        assert_eq!(input.as_deref(), Some("32.000000"));
        assert_eq!((output, cached), (None, None));

        std::fs::write(
            &path,
            "[[model_prices]]\nmodel = \"voice\"\ninput_per_mtok = 4\noutput_per_mtok = 16\n\
             audio_output_per_mtok = -1\n",
        )
        .unwrap();
        let refused = import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap_err();
        assert!(
            refused.to_string().contains("audio_output_per_mtok"),
            "{refused}"
        );
    }

    /// #927: a re-import of an edited file used to log `imported provider` and
    /// change nothing. The operator's next signal was a 401 from the upstream,
    /// which reads as a bad key rather than as config that was never applied.
    #[tokio::test]
    async fn reimporting_an_edited_file_applies_the_edits() {
        let Some(db) = scratch_db().await else {
            return;
        };
        let pool = db.pool().clone();
        let (org_id, project_id) = bootstrap_org(&pool).await;

        let dir = tempdir("reimport");
        let path = dir.join("rolter.toml");
        std::fs::write(
            &path,
            r#"
[[providers]]
name = "tei-embed-02"
kind = "tei"
api_base = "http://tei:80"

[[routes]]
model = "embed"
strategy = "round_robin"
[[routes.targets]]
provider = "tei-embed-02"
weight = 1
"#,
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();

        // the exact edit from the issue: an api_key_env that was missing, plus
        // a corrected base, a different strategy and a re-weighted target
        std::fs::write(
            &path,
            r#"
[[providers]]
name = "tei-embed-02"
kind = "tei"
api_base = "http://tei:8080"
api_key_env = "TEI_KEY"

[[routes]]
model = "embed"
strategy = "power_of_two"
[[routes.targets]]
provider = "tei-embed-02"
weight = 7
"#,
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();

        let provider = ProviderRepo(&pool)
            .list(org_id)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.name == "tei-embed-02")
            .expect("the provider must still be there");
        assert_eq!(
            provider.api_key_env.as_deref(),
            Some("TEI_KEY"),
            "the added api_key_env was not applied"
        );
        assert_eq!(provider.api_base, "http://tei:8080");

        let route = RouteRepo(&pool)
            .list(project_id)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.model == "embed")
            .expect("the route must still be there");
        assert_eq!(route.strategy, "power_of_two");

        let targets = RouteTargetRepo(&pool).list(route.id).await.unwrap();
        assert_eq!(targets.len(), 1, "the target was duplicated, not updated");
        assert_eq!(targets[0].weight, 7);

        // and nothing was duplicated along the way
        let providers = ProviderRepo(&pool).list(org_id).await.unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(RouteRepo(&pool).list(project_id).await.unwrap().len(), 1);
    }

    /// #954: presence, not value, is what makes the import touch capture. A
    /// parsed [`GatewayConfig`] always carries a `payload_capture` block, so
    /// only the raw text can say whether the operator wrote one.
    #[test]
    fn only_a_written_capture_section_counts_as_declared() {
        let dir = tempdir("declares");
        let path = dir.join("rolter.toml");

        std::fs::write(&path, "[logging]\nclickhouse_url = \"http://ch:8123\"\n").unwrap();
        assert!(
            !declares_payload_capture(&path).unwrap(),
            "a logging section without capture must not count as declaring one"
        );

        std::fs::write(&path, "[[providers]]\nname = \"a\"\nkind = \"openai\"\n").unwrap();
        assert!(
            !declares_payload_capture(&path).unwrap(),
            "a file with no logging section at all must not count"
        );

        // written and off is still written: turning capture *off* through the
        // file has to be applicable too
        std::fs::write(&path, "[logging.payload_capture]\nenabled = false\n").unwrap();
        assert!(declares_payload_capture(&path).unwrap());

        std::fs::write(&path, "[logging.payload_capture]\nenabled = true\n").unwrap();
        assert!(declares_payload_capture(&path).unwrap());
    }

    /// The dogfood profile turns capture on through the import file, and the
    /// gateway only sees that once it reaches `logging_settings` — the row the
    /// snapshot is built from. A file that says nothing must leave the row
    /// exactly as the dashboard left it.
    #[tokio::test]
    async fn a_declared_capture_policy_lands_in_logging_settings() {
        let Some(db) = scratch_db().await else {
            return;
        };
        let pool = db.pool().clone();
        let (org_id, project_id) = bootstrap_org(&pool).await;
        let settings = rolter_store::postgres::repo::LoggingSettingsRepo(&pool);

        let dir = tempdir("capture");
        let path = dir.join("rolter.toml");
        std::fs::write(
            &path,
            r#"
[logging.payload_capture]
enabled = true
max_bytes = 32768
redact_fields = ["authorization", "api_key"]
models = ["gpt-4o"]
"#,
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();

        let row = settings.get().await.unwrap();
        assert!(row.payload_capture_enabled, "capture never reached the row");
        assert_eq!(row.payload_capture_max_bytes, 32768);
        assert_eq!(
            row.payload_capture_redact_fields,
            vec!["authorization".to_string(), "api_key".to_string()]
        );
        assert_eq!(row.payload_capture_models, vec!["gpt-4o".to_string()]);
        // sampling and retention are operational dials the file has no opinion
        // about, so the import leaves them where they were
        let sample_rate = row.sample_rate;
        let retention_days = row.retention_days;

        // a file that never mentions capture must not silently switch it off
        std::fs::write(
            &path,
            "[[providers]]\nname = \"openai-primary\"\nkind = \"openai\"\napi_base = \"https://api.openai.com\"\n",
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();

        let row = settings.get().await.unwrap();
        assert!(
            row.payload_capture_enabled,
            "a file silent about capture turned it off"
        );
        assert_eq!(row.sample_rate, sample_rate);
        assert_eq!(row.retention_days, retention_days);
    }

    /// #1748: `logging.ui_events` is imported on presence, like capture. The
    /// default is on, so a file silent about it must not undo a dashboard
    /// opt-out, and one that writes `false` must reach the row the control
    /// plane reads.
    #[tokio::test]
    async fn a_declared_ui_events_opt_out_lands_in_logging_settings() {
        let Some(db) = scratch_db().await else {
            return;
        };
        let pool = db.pool().clone();
        let (org_id, project_id) = bootstrap_org(&pool).await;
        let settings = rolter_store::postgres::repo::LoggingSettingsRepo(&pool);

        let dir = tempdir("ui-events");
        let path = dir.join("rolter.toml");
        std::fs::write(
            &path,
            "[logging]
ui_events = false
",
        )
        .unwrap();
        assert!(declares_ui_events(&path).unwrap());
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        assert!(!settings.get().await.unwrap().ui_events);

        std::fs::write(
            &path,
            "[logging]
sample_rate = 1.0
",
        )
        .unwrap();
        assert!(!declares_ui_events(&path).unwrap());
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        assert!(
            !settings.get().await.unwrap().ui_events,
            "a file silent about ui_events switched the stream back on"
        );
    }

    /// #1919: `project_scoped` scopes a provider or group to the project the
    /// file is imported into, the file is the desired state for it in both
    /// directions, and an export names it again.
    #[tokio::test]
    async fn project_scope_round_trips_through_import_and_export() {
        use rolter_store::ConfigStore;
        let Some(db) = scratch_db().await else {
            return;
        };
        let pool = db.pool().clone();
        let (org_id, project_id) = bootstrap_org(&pool).await;
        let dir = tempdir("scope");
        let path = dir.join("rolter.toml");

        std::fs::write(
            &path,
            r#"
[[providers]]
name = "private"
kind = "openai_compatible"
api_base = "http://private:8000"
project_scoped = true

[[providers]]
name = "shared"
kind = "openai_compatible"
api_base = "http://shared:8000"

[[provider_groups]]
name = "pool"
project_scoped = true
[[provider_groups.members]]
provider = "private"
[[provider_groups.members]]
provider = "shared"

[[routes]]
model = "chat"
[[routes.targets]]
provider = "private"
"#,
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        let scope_of = |name: &'static str| {
            let pool = pool.clone();
            async move {
                ProviderRepo(&pool)
                    .list(org_id)
                    .await
                    .unwrap()
                    .into_iter()
                    .find(|row| row.name == name)
                    .unwrap()
                    .project_id
            }
        };
        assert_eq!(scope_of("private").await, Some(project_id));
        assert_eq!(scope_of("shared").await, None);
        let group = rolter_store::postgres::repo::ProviderGroupRepo(&pool)
            .list(org_id)
            .await
            .unwrap();
        assert_eq!(group[0].project_id, Some(project_id));

        // the export says the same, which is what makes it re-importable
        let store = rolter_store::PostgresConfigStore::with_kek(pool.clone(), None);
        let exported = crate::config_export::render(&store.load().await.unwrap());
        assert_eq!(
            exported.matches("project_scoped = true").count(),
            2,
            "the scoped provider and group should export as scoped:\n{exported}"
        );

        // an org-wide group may not hold a scoped provider
        std::fs::write(
            &path,
            r#"
[[providers]]
name = "private"
kind = "openai_compatible"
api_base = "http://private:8000"
project_scoped = true

[[provider_groups]]
name = "leaky"
[[provider_groups.members]]
provider = "private"
"#,
        )
        .unwrap();
        let err = import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("leaky"), "{err}");

        // the file is the desired state: dropping the flag makes it org-wide
        std::fs::write(
            &path,
            r#"
[[providers]]
name = "private"
kind = "openai_compatible"
api_base = "http://private:8000"
"#,
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        assert_eq!(scope_of("private").await, None);
    }

    /// The upsert must not reach into `provider_keys`: a key rotated through
    /// the dashboard outranks `api_key_env`, and a stale file must not
    /// clobber it.
    #[tokio::test]
    async fn a_reimport_leaves_a_dashboard_sealed_credential_alone() {
        let Some(db) = scratch_db().await else {
            return;
        };
        let pool = db.pool().clone();
        let (org_id, project_id) = bootstrap_org(&pool).await;

        let dir = tempdir("sealed");
        let path = dir.join("rolter.toml");
        let without_env = r#"
[[providers]]
name = "openai-primary"
kind = "openai"
api_base = "https://api.openai.com"
"#;
        std::fs::write(&path, without_env).unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();

        let provider_id = ProviderRepo(&pool).list(org_id).await.unwrap()[0].id;
        // stand in for a rotation through the dashboard
        sqlx::query(
            "insert into provider_keys (provider_id, ciphertext, nonce) values ($1, $2, $3)",
        )
        .bind(provider_id)
        .bind(b"sealed-by-the-dashboard".to_vec())
        .bind(b"nonce".to_vec())
        .execute(&pool)
        .await
        .unwrap();

        std::fs::write(
            &path,
            r#"
[[providers]]
name = "openai-primary"
kind = "openai"
api_base = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"
"#,
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();

        let sealed: Vec<u8> =
            sqlx::query_scalar("select ciphertext from provider_keys where provider_id = $1")
                .bind(provider_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            sealed, b"sealed-by-the-dashboard",
            "the re-import overwrote a rotated credential"
        );
    }

    /// The variants half of `rolter.toml` (#391), as one file the tests below
    /// share: a weighted split with variant params and an ordered target list.
    const VARIANT_FILE: &str = r#"
[[providers]]
name = "primary"
kind = "openai"
api_base = "https://api.openai.com"

[[providers]]
name = "secondary"
kind = "openai_compatible"
api_base = "http://vllm:8000"

[[routes]]
model = "split"
strategy = "weighted"
[[routes.variants]]
name = "stable"
weight = 7
[[routes.variants.targets]]
provider = "primary"
model = "gpt-4o"
weight = 2
[[routes.variants.targets]]
provider = "secondary"
weight = 1
[[routes.variants]]
name = "canary"
weight = 3
[routes.variants.params]
temperature = 0.7
[[routes.variants.targets]]
provider = "secondary"
model = "llama"
weight = 1
[[routes.variants]]
name = "fallback"
weight = 0
[[routes.variants.targets]]
provider = "primary"
weight = 1
"#;

    /// The acceptance bar of #391: a route defined in `rolter.toml` and the
    /// same route after `rolter-seed --import` has moved it into the database
    /// route every request the same way. Both pick the same primary for every
    /// draw, fall back in the same order, try the same targets in the same
    /// order and apply the same params.
    #[tokio::test]
    async fn a_config_route_and_its_imported_db_route_route_identically() {
        let Some(db) = scratch_db().await else {
            return;
        };
        let pool = db.pool().clone();
        let (org_id, project_id) = bootstrap_org(&pool).await;
        let dir = tempdir("variants-identical");
        let path = dir.join("rolter.toml");
        std::fs::write(&path, VARIANT_FILE).unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();

        use rolter_store::ConfigStore;
        let file = rolter_core::GatewayConfig::from_toml_str(VARIANT_FILE).unwrap();
        let store = rolter_store::PostgresConfigStore::with_kek(pool.clone(), None);
        let stored = store.load().await.unwrap();
        let from_file = file.routes.iter().find(|r| r.model == "split").unwrap();
        let from_db = stored.routes.iter().find(|r| r.model == "split").unwrap();

        assert_eq!(from_db.strategy, from_file.strategy);
        assert!(from_db.has_variants());
        let shape = |route: &rolter_core::ModelRoute| {
            route
                .variants
                .iter()
                .map(|v| {
                    let targets: Vec<_> = v
                        .targets
                        .iter()
                        .map(|t| (t.provider.clone(), t.model.clone(), t.weight))
                        .collect();
                    (v.name.clone(), v.weight, targets)
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(shape(from_db), shape(from_file));
        for step in 0..1000 {
            let r = f64::from(step) / 1000.0;
            let primary = from_file.sample_variant(r);
            assert_eq!(from_db.sample_variant(r), primary, "draw {r}");
            let primary = primary.unwrap();
            assert_eq!(
                from_db.fallback_order(primary),
                from_file.fallback_order(primary)
            );
        }
        for (db_variant, file_variant) in from_db.variants.iter().zip(&from_file.variants) {
            let mut db_body = serde_json::json!({"temperature": 0.1});
            let mut file_body = db_body.clone();
            from_db.apply_variant_params(db_variant, &mut db_body);
            from_file.apply_variant_params(file_variant, &mut file_body);
            assert_eq!(db_body, file_body, "variant '{}'", file_variant.name);
        }
    }

    /// An import never deletes what the file does not mention, so a file with
    /// no variants keeps a stored split. A file whose variant names a provider
    /// it does not import leaves the stored split whole rather than half
    /// applied, and an unchanged file writes nothing.
    #[tokio::test]
    async fn a_reimport_replaces_variants_only_when_the_file_names_them_all() {
        let Some(db) = scratch_db().await else {
            return;
        };
        let pool = db.pool().clone();
        let (org_id, project_id) = bootstrap_org(&pool).await;
        let dir = tempdir("variants-reimport");
        let path = dir.join("rolter.toml");
        std::fs::write(&path, VARIANT_FILE).unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        let route = RouteRepo(&pool)
            .list(project_id)
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.model == "split")
            .unwrap();
        let variants = || async {
            rolter_store::postgres::repo::RouteVariantRepo(&pool)
                .list(route.id)
                .await
                .unwrap()
        };
        let first = variants().await;
        let names: Vec<_> = first.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["stable", "canary", "fallback"]);
        assert_eq!(first[1].params, serde_json::json!({"temperature": 0.7}));
        assert_eq!(first[0].targets.len(), 2);

        // unchanged: the rows keep their ids
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        let again = variants().await;
        assert_eq!(
            again.iter().map(|v| v.id).collect::<Vec<_>>(),
            first.iter().map(|v| v.id).collect::<Vec<_>>(),
            "an unchanged file rewrote the variants"
        );

        // a file with the route but no variants keeps them
        std::fs::write(
            &path,
            "[[routes]]\nmodel = \"split\"\nstrategy = \"weighted\"\n",
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        assert_eq!(variants().await.len(), 3);

        // a variant naming a provider the file does not import changes nothing
        std::fs::write(
            &path,
            "[[routes]]\nmodel = \"split\"\nstrategy = \"weighted\"\n\
             [[routes.variants]]\nname = \"only\"\n\
             [[routes.variants.targets]]\nprovider = \"elsewhere\"\n",
        )
        .unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        assert_eq!(variants().await.len(), 3);

        // an edited split is applied, in the file's order
        std::fs::write(&path, VARIANT_FILE.replace("weight = 7", "weight = 5")).unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        let edited = variants().await;
        assert_eq!(edited[0].name, "stable");
        assert_eq!(edited[0].weight, 5);
    }

    /// The served config drops a variant target whose provider the snapshot
    /// left out, and an import replaces a route's whole variant set, so an
    /// export of the served set would delete that target on the next import.
    /// The export writes the stored set instead, marking the target, and the
    /// importer then leaves the route's variants as they are.
    #[tokio::test]
    async fn an_export_keeps_a_variant_target_on_a_left_out_provider() {
        let Some(db) = scratch_db().await else {
            return;
        };
        let pool = db.pool().clone();
        let (org_id, project_id) = bootstrap_org(&pool).await;
        let dir = tempdir("variants-left-out");
        let path = dir.join("rolter.toml");
        std::fs::write(&path, VARIANT_FILE).unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        // a proxy pool that does not parse is one way a provider is left out of
        // what the store loads; the constraint that stops the api writing one
        // is dropped so the row can be made
        sqlx::query("alter table providers drop constraint providers_egress_proxies_strings")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "update providers set egress_proxies = '[\"http://proxy.example.com:3128\", 7]'::jsonb
             where name = 'secondary'",
        )
        .execute(&pool)
        .await
        .unwrap();

        use rolter_store::ConfigStore;
        let store = rolter_store::PostgresConfigStore::with_kek(pool.clone(), None);
        let served = store.load().await.unwrap();
        let route = served.routes.iter().find(|r| r.model == "split").unwrap();
        assert!(
            route.variants.iter().all(|v| v.name != "canary"),
            "the canary's only provider is left out, so it is not served"
        );

        let exported = crate::config_export::export_document(&pool, &served)
            .await
            .unwrap();
        assert!(
            exported.contains("# provider 'secondary' is not in this export"),
            "{exported}"
        );
        assert!(exported.contains("name = \"canary\""), "{exported}");

        let before = rolter_store::postgres::repo::RouteVariantRepo(&pool)
            .list(
                RouteRepo(&pool)
                    .list(project_id)
                    .await
                    .unwrap()
                    .into_iter()
                    .find(|r| r.model == "split")
                    .unwrap()
                    .id,
            )
            .await
            .unwrap();
        std::fs::write(&path, &exported).unwrap();
        import_bootstrap_toml(&pool, org_id, project_id, &path)
            .await
            .unwrap();
        let after = rolter_store::postgres::repo::RouteVariantRepo(&pool)
            .list(before[0].route_id)
            .await
            .unwrap();
        assert_eq!(
            after.iter().map(|v| v.id).collect::<Vec<_>>(),
            before.iter().map(|v| v.id).collect::<Vec<_>>(),
            "re-importing the export rewrote the variants"
        );
        assert_eq!(after.len(), 3);
    }

    /// A scratch directory for the bootstrap toml a test writes.
    fn tempdir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rolter-seed-{label}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A schema of its own per test: the coverage job runs plain `cargo test`
    /// against a shared database and would otherwise race. The guard drops the
    /// schema when the test finishes, panic included (#1364).
    async fn scratch_db() -> Option<TestSchema> {
        let url = rolter_store::postgres::test_database::url()
            .await
            .or_else(|| {
                eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
                None
            })?;
        Some(TestSchema::migrated(&url).await)
    }

    async fn bootstrap_org(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let team = super::TeamRepo(pool)
            .create(org_id, "default")
            .await
            .unwrap();
        let project = super::ProjectRepo(pool)
            .create(team.id, "default")
            .await
            .unwrap();
        (org_id, project.id)
    }

    #[tokio::test]
    async fn prompt_template_seed_is_idempotent_and_rejects_version_drift() {
        let Some(db) = scratch_db().await else {
            return;
        };
        let pool = db.pool().clone();
        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let template = PromptTemplate {
            id: "support-baseline".to_string(),
            version: 1,
            routes: Vec::new(),
            scopes: Vec::new(),
            variables: vec![TemplateVariable {
                name: "tone".to_string(),
                required: false,
                default: Some("calm".to_string()),
            }],
            decorators: vec![Decorator {
                role: DecoratorRole::System,
                position: DecoratorPosition::Prepend,
                content: "tone={{ tone }}".to_string(),
            }],
        };
        let route_ids = HashMap::new();
        import_prompt_template(&pool, org_id, &template, &route_ids)
            .await
            .unwrap();
        import_prompt_template(&pool, org_id, &template, &route_ids)
            .await
            .unwrap();

        let template_count: i64 = sqlx::query_scalar("select count(*) from prompt_templates")
            .fetch_one(&pool)
            .await
            .unwrap();
        let version_count: i64 =
            sqlx::query_scalar("select count(*) from prompt_template_versions")
                .fetch_one(&pool)
                .await
                .unwrap();
        let scope_count: i64 = sqlx::query_scalar("select count(*) from prompt_template_scopes")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!((template_count, version_count, scope_count), (1, 1, 1));

        let mut drifted = template;
        drifted.decorators[0].content = "different".to_string();
        assert!(import_prompt_template(&pool, org_id, &drifted, &route_ids)
            .await
            .is_err());
    }
}
