//! `rolter check` — pre-boot validation for a production deployment (#854).
//!
//! The local path (`rolter easy-up`) is deliberately frictionless and may stay
//! loose: it runs with no database, no provider keys and no secrets at all.
//! That is a feature, and this command must never make it prompt for anything.
//!
//! The production path had none of the guardrails that made the local looseness
//! safe. A misconfigured deployment did not fail — it *started degraded*. The
//! sharpest example, and the reason this exists: when the KEK is missing, the
//! control plane's default-provider seed logs a warning and carries on with an
//! **unsealed** inline `api_key`. Nothing fails, nothing is red, and a
//! credential the operator believed was encrypted at rest is not.
//!
//! So this is a gate, not a linter: it exits non-zero on anything that would
//! produce a silently-degraded production process, and it is meant to run
//! before the process starts — as a container entrypoint step, a Helm hook, or
//! by hand.

use std::fmt::Write as _;

use clap::Args;

/// Environment variable holding the key-encryption key.
///
/// Duplicated from `rolter_store::postgres::crypto::KEK_ENV` rather than
/// imported: that crate is optional here (behind the `postgres` feature) and
/// `rolter check` has to work without it. A `#[cfg(feature = "postgres")]` test
/// below pins the two together — given that this whole command exists because
/// the *name* of this variable drifted between code and docs, letting it drift
/// again inside the codebase would be a poor joke.
const KEK_ENV: &str = "ROLTER_KEK";

/// Variable name several docs pages wrongly told operators to set. Nothing in
/// the codebase has ever read it, so a deployment that sets it has no KEK at
/// all — the single most consequential way to end up silently degraded.
const WRONG_KEK_ENV: &str = "ROLTER_MASTER_KEY";

/// Credentials that ship in the example files. Finding one in a production
/// environment means a placeholder was carried through rather than replaced.
const EXAMPLE_VALUES: &[(&str, &str)] = &[
    (
        "postgres://rolter:rolter@localhost:5432/rolter",
        "the example database URL, credentials and all",
    ),
    (
        "cm9sdGVyLWUyZS10ZXN0LWtlay1ub3Qtc2VjcmV0ISE=",
        "the throwaway KEK from the e2e test harness",
    ),
];

/// Shortest KEK we accept. The KEK is stretched through SHA-256, so any string
/// *works*; that is exactly why a length floor is worth enforcing here rather
/// than trusting the derivation to disguise a weak secret.
const MIN_KEK_LEN: usize = 16;

#[derive(Args, Debug, Default)]
pub struct CheckArgs {
    /// path to the gateway config file to validate
    #[arg(short, long, env = "ROLTER_CONFIG")]
    pub config: Option<String>,

    /// treat warnings as failures too
    #[arg(long)]
    pub strict: bool,

    /// also probe that the configured datastores accept a TCP connection.
    /// off by default so `rolter check` stays offline and side-effect free
    #[arg(long)]
    pub connect: bool,
}

/// One finding. `fatal` decides the exit code; a warning is printed and the
/// run still succeeds unless `--strict`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Finding {
    fatal: bool,
    pub(crate) title: String,
    detail: String,
}

impl Finding {
    fn error(title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            fatal: true,
            title: title.into(),
            detail: detail.into(),
        }
    }

    fn warn(title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            fatal: false,
            title: title.into(),
            detail: detail.into(),
        }
    }
}

/// How the checks read the environment. Taking this as a trait keeps every
/// rule unit-testable without mutating real process env, which is global state
/// and races across parallel tests.
pub(crate) trait Env {
    fn get(&self, key: &str) -> Option<String>;
}

struct ProcessEnv;

impl Env for ProcessEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|v| !v.trim().is_empty())
    }
}

/// The KEK rules. This is the check that motivated the command.
fn check_kek(env: &dyn Env, out: &mut Vec<Finding>) {
    match env.get(KEK_ENV) {
        None => {
            // name the likely cause rather than only the symptom: several docs
            // pages told operators to set the wrong variable
            let detail = if env.get(WRONG_KEK_ENV).is_some() {
                format!(
                    "{WRONG_KEK_ENV} is set but nothing reads it — the variable rolter reads is \
                     {KEK_ENV}. Rename it. Until you do, provider credentials are not encrypted \
                     at rest: the control plane logs a warning and stores an inline api_key \
                     unsealed rather than failing."
                )
            } else {
                format!(
                    "{KEK_ENV} is unset. Provider credentials cannot be encrypted at rest, and \
                     the default-provider seed will store an inline api_key unsealed with only a \
                     warning. Generate one with `openssl rand -hex 32`."
                )
            };
            out.push(Finding::error(format!("{KEK_ENV} is missing"), detail));
        }
        Some(kek) if kek.trim().len() < MIN_KEK_LEN => out.push(Finding::error(
            format!("{KEK_ENV} is too short"),
            format!(
                "the KEK is {} characters; use at least {MIN_KEK_LEN}. It is stretched through \
                 SHA-256, so a short secret still produces a valid key — and a brute-forceable \
                 one. Generate with `openssl rand -hex 32`.",
                kek.trim().len()
            ),
        )),
        Some(_) => {}
    }
}

/// Management API and snapshot endpoint must not be open in production.
fn check_admin_token(env: &dyn Env, out: &mut Vec<Finding>) {
    if env.get("ROLTER_ADMIN_TOKEN").is_none() {
        out.push(Finding::error(
            "ROLTER_ADMIN_TOKEN is missing",
            "the management API and /internal/snapshot are unauthenticated without it. Anyone \
             who can reach the control plane can read the whole config snapshot and mutate \
             providers, routes and virtual keys.",
        ));
    }
}

/// A production control plane without a database is not a production control
/// plane: config, RBAC, keys and pricing all live in Postgres.
fn check_datastores(env: &dyn Env, out: &mut Vec<Finding>) {
    match env.get("ROLTER_DATABASE_URL") {
        None => out.push(Finding::error(
            "ROLTER_DATABASE_URL is missing",
            "the control plane falls back to an in-memory store, so every provider, route, \
             virtual key and budget is lost on restart.",
        )),
        Some(url) if !url.starts_with("postgres://") && !url.starts_with("postgresql://") => out
            .push(Finding::error(
                "ROLTER_DATABASE_URL is not a postgres URL",
                format!(
                    "expected a postgres:// or postgresql:// URL, got `{}`",
                    redact(&url)
                ),
            )),
        Some(_) => {}
    }

    if env.get("ROLTER_REDIS_URL").is_none() {
        out.push(Finding::warn(
            "ROLTER_REDIS_URL is unset",
            "rate-limit counters and spend budgets fall back to per-process state, so limits are \
             enforced per replica rather than per deployment. Fine for a single replica, wrong \
             for more than one.",
        ));
    }
}

/// The dashboard reads usage rows the gateway writes. If nothing tells the two
/// sides where ClickHouse is, the analytics screens are empty forever and look
/// exactly like a quiet deployment (#929).
fn check_analytics_destination(env: &dyn Env, out: &mut Vec<Finding>) {
    let configured = env
        .get("CLICKHOUSE_URL")
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty());
    if configured.is_some() {
        return;
    }
    // no database means this is a toy or a smoke test, and an empty analytics
    // screen is not a surprise there
    if env.get("ROLTER_DATABASE_URL").is_none() {
        return;
    }
    out.push(Finding::warn(
        "CLICKHOUSE_URL is unset, so the analytics screens will stay empty",
        "the gateway writes request, usage and cost rows to ClickHouse and the dashboard reads \
         them back from it. With no destination the rows are never written, and Usage, Costs \
         and Logs render as an empty deployment rather than an unconfigured one. Both \
         processes read this one variable; the gateway also inherits it from the control \
         plane's snapshot when only the control plane has it.",
    ));
}

/// Placeholder values that were meant to be replaced.
fn check_example_values(env: &dyn Env, out: &mut Vec<Finding>) {
    for var in [
        KEK_ENV,
        "ROLTER_DATABASE_URL",
        "ROLTER_ADMIN_TOKEN",
        "ROLTER_INTERNAL_TOKEN",
    ] {
        let Some(value) = env.get(var) else { continue };
        for (example, what) in EXAMPLE_VALUES {
            if value.contains(example) {
                out.push(Finding::error(
                    format!("{var} still holds an example value"),
                    format!(
                        "this is {what}. It is published in the repository, so treat it as \
                             public knowledge and replace it."
                    ),
                ));
            }
        }
    }
}

/// Binding the management plane to every interface is the one network default
/// that is right for local and wrong for production.
fn check_exposure(env: &dyn Env, out: &mut Vec<Finding>) {
    if env
        .get("ROLTER_CONTROL_HOST")
        .is_none_or(|h| h == "0.0.0.0")
    {
        out.push(Finding::warn(
            "control plane binds 0.0.0.0",
            "the management API is reachable on every interface. Bind it to a private address, \
             or keep it behind an ingress that terminates TLS and restricts access.",
        ));
    }
}

/// Redact anything that looks like credentials in a URL before printing it —
/// this output goes to CI logs.
fn redact(url: &str) -> String {
    match (url.find("://"), url.find('@')) {
        (Some(scheme), Some(at)) if at > scheme => {
            format!("{}://***@{}", &url[..scheme], &url[at + 1..])
        }
        _ => url.to_string(),
    }
}

/// The virtual-key pepper. Without it a leaked digest is directly usable, and
/// unlike the KEK nothing warns about its absence at startup.
fn check_key_pepper(env: &dyn Env, out: &mut Vec<Finding>) {
    if env.get("ROLTER_KEY_PEPPER").is_none() {
        out.push(Finding::warn(
            "ROLTER_KEY_PEPPER is unset",
            "virtual-key digests are unpeppered, so a leaked digest is usable as-is against \
             this deployment. Set the same value on every replica — changing it after keys \
             have been issued invalidates all of them. `rolter init` generates one.",
        ));
    }
}

/// CORS. The dashboard is a same-origin SPA served by the control plane, so a
/// wildcard origin buys nothing and hands any page on the internet the
/// authenticated management API through a logged-in operator's browser.
fn check_cors(env: &dyn Env, out: &mut Vec<Finding>) {
    if env
        .get("ROLTER_CORS_ALLOW_ORIGINS")
        .is_some_and(|v| v.split(',').any(|o| o.trim() == "*"))
    {
        out.push(Finding::error(
            "CORS allows every origin",
            "ROLTER_CORS_ALLOW_ORIGINS contains `*`. The dashboard is served same-origin by \
             the control plane and needs no cross-origin grant; a wildcard lets any page a \
             logged-in operator visits drive the management API as them. List the exact \
             origins that need it, or remove the variable.",
        ));
    }
}

/// Reachability of the datastores, behind `--connect` because a check that
/// opens sockets cannot be the default for an offline `rolter check`.
///
/// A TCP connect, not a protocol handshake: it needs no driver, works without
/// the `postgres` feature, and answers the question that actually goes wrong in
/// a production rollout — the name does not resolve, or nothing is listening.
/// A database that accepts TCP but rejects the credentials is a different
/// failure, and one the process reports loudly on its own.
async fn check_reachability(env: &dyn Env, out: &mut Vec<Finding>) {
    for (var, fatal) in [("ROLTER_DATABASE_URL", true), ("ROLTER_REDIS_URL", false)] {
        let Some(url) = env.get(var) else { continue };
        let Some(target) = host_port(&url) else {
            continue;
        };
        let reachable = tokio::time::timeout(
            std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS),
            tokio::net::TcpStream::connect(&target),
        )
        .await;
        let detail = match reachable {
            Ok(Ok(_)) => continue,
            Ok(Err(error)) => format!("connecting to {target} failed: {error}"),
            Err(_) => format!("connecting to {target} timed out after {CONNECT_TIMEOUT_SECS}s"),
        };
        let title = format!("{var} is not reachable");
        out.push(if fatal {
            Finding::error(title, detail)
        } else {
            Finding::warn(title, detail)
        });
    }
}

/// Does the KEK in the environment open the secrets this store already holds?
/// (#923)
///
/// Every other KEK rule above reads the environment alone, so all of them pass
/// on a restored database sealed under a *different* KEK. That failure has no
/// startup symptom at all: the process boots, the dashboard renders, and the
/// first sign is an upstream 401 hours later, blamed on the provider. This is
/// the only check that can catch it, because only the data knows.
///
/// Behind `--connect` for the same reason the reachability probe is: it opens a
/// socket. Behind the `postgres` feature because it needs the driver and the
/// cipher, and `rolter check` must keep working in a build that has neither.
#[cfg(feature = "postgres")]
async fn check_kek_opens_the_store(env: &dyn Env, out: &mut Vec<Finding>) {
    use rolter_store::postgres::crypto::Kek;
    use rolter_store::postgres::kek_audit;

    let (Some(database_url), Some(secret)) = (env.get("ROLTER_DATABASE_URL"), env.get(KEK_ENV))
    else {
        return;
    };
    let timeout = std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS);
    let audit = match kek_audit::audit_kek_at(&database_url, &Kek::from_secret(&secret), timeout)
        .await
    {
        Ok(audit) => audit,
        // not fatal on its own: an unusable database is already an error from
        // the reachability probe, and a schema this binary predates is a
        // migration away rather than a KEK problem. Saying which check did not
        // run beats reporting a KEK as verified when it was never tried
        Err(error) => {
            out.push(Finding::warn(
                "could not verify the KEK against the store",
                format!(
                    "reading the sealed columns failed, so a KEK mismatch would not be caught                      here: {error}"
                ),
            ));
            return;
        }
    };

    if let Some(finding) = kek_mismatch_finding(&audit) {
        out.push(finding);
    }
}

/// Turn an audit into a finding, or `None` when the KEK opened everything.
///
/// Split out from the probe above so the message — the part an operator reads
/// at 3am with a restored database and no idea why the gateway is returning
/// 401s — is testable without a database.
#[cfg(feature = "postgres")]
fn kek_mismatch_finding(audit: &rolter_store::postgres::kek_audit::KekAudit) -> Option<Finding> {
    if audit.is_intact() {
        return None;
    }
    let mut detail = format!(
        "{KEK_ENV} does not open secrets already in this database. This is what a restore onto          the wrong KEK looks like: the dump carried the ciphertext, the KEK was not in the dump,          and nothing else fails until an upstream call does. Restore the KEK that sealed this          database — the ciphertext cannot be recovered without it. See          docs/deployment/backup-and-restore.md."
    );
    for line in audit.damage_report() {
        detail.push_str("\n       - ");
        detail.push_str(&line);
    }
    Some(Finding::error(
        format!("{KEK_ENV} does not match the one this store was sealed with"),
        detail,
    ))
}

/// Seconds allowed for a `--connect` probe. Short: this runs before a process
/// starts, and a check that hangs is worse than one that reports unreachable.
const CONNECT_TIMEOUT_SECS: u64 = 5;

/// `host:port` from a URL, with the scheme's default port when none is given.
/// Deliberately string-level — pulling a URL parser in for this would be the
/// only reason the crate needed one.
fn host_port(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?']).next()?;
    // strip any credentials
    let authority = authority.rsplit('@').next()?;
    if authority.is_empty() {
        return None;
    }
    // ipv6 literals carry their own colons, so only treat a colon after the
    // closing bracket (or in a bracket-free authority) as the port separator
    let has_port = match authority.rfind(']') {
        Some(bracket) => authority[bracket..].contains(':'),
        None => authority.contains(':'),
    };
    if has_port {
        return Some(authority.to_string());
    }
    let port = match scheme {
        "postgres" | "postgresql" => 5432,
        "redis" | "rediss" => 6379,
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    Some(format!("{authority}:{port}"))
}

pub(crate) fn run_checks(env: &dyn Env) -> Vec<Finding> {
    let mut out = Vec::new();
    check_kek(env, &mut out);
    check_admin_token(env, &mut out);
    check_datastores(env, &mut out);
    check_example_values(env, &mut out);
    check_exposure(env, &mut out);
    check_key_pepper(env, &mut out);
    check_cors(env, &mut out);
    check_analytics_destination(env, &mut out);
    out
}

/// Render findings into a report. Returns `(report, should_fail)`.
fn report(findings: &[Finding], strict: bool) -> (String, bool) {
    let errors = findings.iter().filter(|f| f.fatal).count();
    let warnings = findings.len() - errors;

    let mut out = String::new();
    for finding in findings {
        let tag = if finding.fatal { "error" } else { "warn " };
        let _ = writeln!(out, "{tag}  {}", finding.title);
        let _ = writeln!(out, "       {}\n", finding.detail);
    }

    if findings.is_empty() {
        out.push_str("all pre-boot checks passed\n");
        return (out, false);
    }

    let _ = writeln!(out, "{errors} error(s), {warnings} warning(s)");
    (out, errors > 0 || (strict && warnings > 0))
}

/// Report every provider that opted out of its kind's host pin.
///
/// A hosted provider kind names one operator, so pinning its `api_base` is what
/// stops a config edit from pointing a vendor credential somewhere else. The
/// opt-out is legitimate — an egress proxy, a compatible endpoint, a local fake
/// for testing the dialect — but it should never be invisible, so it is a
/// warning here: `--strict` fails on it, a normal run reports it and continues.
fn custom_api_base_findings(config: &rolter_core::GatewayConfig) -> Vec<Finding> {
    config
        .custom_api_base_advisories()
        .into_iter()
        .map(|detail| Finding::warn("provider opts out of its host pin", detail))
        .collect()
}

/// Report every key in the file the config model does not recognise (#1424).
///
/// rolter's config types have no `deny_unknown_fields` on purpose: a file
/// written for one build must stay loadable by an older one, so an unknown key
/// is ignored rather than rejected. The price is that `connect_sesc` produces
/// no error, no log line and the default value, which is indistinguishable from
/// a setting that did not take effect for some other reason.
///
/// A warning is the whole point: the file is still valid and the process will
/// still start, so nothing here may be fatal. `--strict` promotes it, which is
/// the setting a CI job that lints a config wants.
///
/// The same keys are logged by [`rolter_core::GatewayConfig::load`] at startup,
/// through the same [`rolter_core::config_lint::describe`] sentence, so the
/// report here and the gateway's log say one thing rather than two (#1434).
fn unknown_key_findings(raw: &str) -> Vec<Finding> {
    let Ok(unknown) = rolter_core::config_lint::unknown_keys(raw) else {
        // not parseable as TOML at all — already reported by the load below
        return Vec::new();
    };
    unknown
        .into_iter()
        .map(|key| {
            Finding::warn(
                format!("unrecognised config key {}", key.path),
                rolter_core::config_lint::describe(&key),
            )
        })
        .collect()
}

pub async fn run(args: CheckArgs) -> anyhow::Result<()> {
    let mut findings = run_checks(&ProcessEnv);

    if args.connect {
        check_reachability(&ProcessEnv, &mut findings).await;
        #[cfg(feature = "postgres")]
        check_kek_opens_the_store(&ProcessEnv, &mut findings).await;
    }

    // the config file is optional: a fully DB-backed deployment has none
    if let Some(path) = args.config.as_deref() {
        if let Ok(raw) = std::fs::read_to_string(path) {
            findings.extend(unknown_key_findings(&raw));
        }
        match rolter_core::GatewayConfig::load(std::path::Path::new(path)) {
            Ok(config) => findings.extend(custom_api_base_findings(&config)),
            Err(error) => findings.push(Finding::error(
                format!("config file {path} is not usable"),
                error.to_string(),
            )),
        }
    }

    let (text, failed) = report(&findings, args.strict);
    print!("{text}");
    if failed {
        anyhow::bail!("pre-boot validation failed; refusing to report this deployment as ready");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeEnv(HashMap<String, String>);

    impl FakeEnv {
        fn with(mut self, key: &str, value: &str) -> Self {
            self.0.insert(key.into(), value.into());
            self
        }

        /// a deployment that passes every check, as the baseline to perturb
        fn healthy() -> Self {
            Self::default()
                .with(KEK_ENV, "b6f1c0d2e3a4957886b1c0d2e3a49578")
                .with("ROLTER_ADMIN_TOKEN", "an-admin-token")
                .with("ROLTER_DATABASE_URL", "postgres://user:pw@db:5432/rolter")
                .with("ROLTER_REDIS_URL", "redis://redis:6379")
                .with("ROLTER_CONTROL_HOST", "127.0.0.1")
                .with("ROLTER_KEY_PEPPER", "a-deployment-wide-pepper")
                .with("CLICKHOUSE_URL", "http://clickhouse:8123")
        }
    }

    impl Env for FakeEnv {
        fn get(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned().filter(|v| !v.trim().is_empty())
        }
    }

    fn titles(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|f| f.title.as_str()).collect()
    }

    #[test]
    fn a_correctly_configured_deployment_is_clean() {
        let findings = run_checks(&FakeEnv::healthy());
        assert!(
            findings.is_empty(),
            "unexpected findings: {:?}",
            titles(&findings)
        );
        let (text, failed) = report(&findings, true);
        assert!(!failed);
        assert!(text.contains("all pre-boot checks passed"));
    }

    #[test]
    fn a_host_pin_opt_out_warns_without_blocking_the_boot() {
        let config = rolter_core::GatewayConfig::from_toml_str(
            r#"
            [[providers]]
            name = "openrouter-edge"
            kind = "openrouter"
            api_base = "http://127.0.0.1:18002"
            api_key_env = "OPENROUTER_API_KEY"
            allow_custom_api_base = true

            [[routes]]
            model = "router-chat"
            [[routes.targets]]
            provider = "openrouter-edge"
            "#,
        )
        .unwrap();

        let findings = custom_api_base_findings(&config);
        assert_eq!(titles(&findings), vec!["provider opts out of its host pin"]);
        assert!(findings.iter().all(|finding| !finding.fatal));

        // reported, and survivable — until --strict, which is the point of it
        let (text, failed) = report(&findings, false);
        assert!(!failed, "{text}");
        assert!(text.contains("openrouter-edge"), "{text}");
        assert!(report(&findings, true).1, "--strict has to fail on it");
    }

    /// a config exercising all three shapes at once: a top-level table, a key
    /// nested inside one, and a key inside an array of tables
    const TYPO_CONFIG: &str = r#"
        [server]
        port = 8080
        prot = 9090

        [timeouts]
        connect_sesc = 3

        [[providers]]
        name = "local"
        kind = "openai_compatible"
        api_base = "http://127.0.0.1:8000/v1"
        api_key_evn = "LOCAL_API_KEY"

        [[routes]]
        model = "local-chat"
        [[routes.targets]]
        provider = "local"
        wieght = 2
        "#;

    #[test]
    fn unrecognised_keys_are_reported_with_their_path_and_a_suggestion() {
        let findings = unknown_key_findings(TYPO_CONFIG);
        assert_eq!(
            titles(&findings),
            [
                "unrecognised config key providers[0].api_key_evn",
                "unrecognised config key routes[0].targets[0].wieght",
                "unrecognised config key server.prot",
                "unrecognised config key timeouts.connect_sesc",
            ]
        );
        assert!(
            findings[3].detail.contains("Did you mean `connect_secs`?"),
            "{}",
            findings[3].detail
        );
    }

    #[test]
    fn an_unrecognised_key_warns_but_strict_makes_it_a_failure() {
        let findings = unknown_key_findings(TYPO_CONFIG);
        assert!(
            findings.iter().all(|finding| !finding.fatal),
            "the file is still valid toml and still loads; nothing here may block a boot"
        );
        assert!(!report(&findings, false).1, "a warning must not fail a run");
        assert!(
            report(&findings, true).1,
            "--strict is what a CI config lint runs"
        );

        // and the config it warned about still parses, unchanged
        let config = rolter_core::GatewayConfig::from_toml_str(TYPO_CONFIG).expect("still valid");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.providers.len(), 1);
    }

    #[test]
    fn a_clean_config_produces_no_unknown_key_findings() {
        let findings = unknown_key_findings(
            r#"
            [server]
            port = 8080

            [[providers]]
            name = "local"
            kind = "openai_compatible"
            api_base = "http://127.0.0.1:8000/v1"
            api_key_env = "LOCAL_API_KEY"
            "#,
        );
        assert!(findings.is_empty(), "{:?}", titles(&findings));
    }

    #[test]
    fn a_file_that_is_not_toml_is_left_to_the_loader_to_report() {
        // one clear "this file is not usable" error beats that error plus a
        // pile of speculative key warnings
        assert!(unknown_key_findings("{ not toml at all").is_empty());
    }

    #[test]
    fn a_pinned_provider_leaves_the_check_silent() {
        let config = rolter_core::GatewayConfig::from_toml_str(
            r#"
            [[providers]]
            name = "openrouter"
            kind = "openrouter"
            api_base = "https://openrouter.ai/api/v1"
            api_key_env = "OPENROUTER_API_KEY"

            [[routes]]
            model = "router-chat"
            [[routes.targets]]
            provider = "openrouter"
            "#,
        )
        .unwrap();
        assert!(custom_api_base_findings(&config).is_empty());
    }

    #[test]
    fn a_missing_kek_is_fatal() {
        let mut env = FakeEnv::healthy();
        env.0.remove(KEK_ENV);
        let findings = run_checks(&env);
        let kek = findings
            .iter()
            .find(|f| f.title.contains(KEK_ENV))
            .expect("missing KEK not reported");
        assert!(kek.fatal);
        // the consequence, not just the absence
        assert!(kek.detail.contains("unsealed"));
    }

    #[test]
    fn the_wrong_kek_variable_is_named_explicitly() {
        // the exact trap the docs led operators into: ROLTER_MASTER_KEY is set,
        // looks configured, and is read by nothing
        let mut env = FakeEnv::healthy();
        env.0.remove(KEK_ENV);
        let env = env.with(WRONG_KEK_ENV, "a-perfectly-good-secret-value");
        let findings = run_checks(&env);
        let kek = findings
            .iter()
            .find(|f| f.title.contains(KEK_ENV))
            .expect("missing KEK not reported");
        assert!(
            kek.detail.contains(WRONG_KEK_ENV) && kek.detail.contains("nothing reads it"),
            "the report must name the wrong variable: {}",
            kek.detail
        );
    }

    #[test]
    fn a_short_kek_is_rejected_despite_hashing() {
        let env = FakeEnv::healthy().with(KEK_ENV, "hunter2");
        let findings = run_checks(&env);
        assert!(findings
            .iter()
            .any(|f| f.fatal && f.title.contains("too short")));
    }

    #[test]
    fn example_values_are_caught() {
        let env = FakeEnv::healthy().with(
            "ROLTER_DATABASE_URL",
            "postgres://rolter:rolter@localhost:5432/rolter",
        );
        let findings = run_checks(&env);
        assert!(findings
            .iter()
            .any(|f| f.fatal && f.title.contains("example value")));
    }

    #[test]
    fn the_e2e_throwaway_kek_is_caught() {
        let env = FakeEnv::healthy().with(KEK_ENV, "cm9sdGVyLWUyZS10ZXN0LWtlay1ub3Qtc2VjcmV0ISE=");
        let findings = run_checks(&env);
        assert!(findings
            .iter()
            .any(|f| f.fatal && f.title.contains("example value")));
    }

    #[test]
    fn a_missing_admin_token_is_fatal() {
        let mut env = FakeEnv::healthy();
        env.0.remove("ROLTER_ADMIN_TOKEN");
        let findings = run_checks(&env);
        assert!(findings
            .iter()
            .any(|f| f.fatal && f.title.contains("ROLTER_ADMIN_TOKEN")));
    }

    #[test]
    fn a_missing_database_is_fatal_and_a_missing_redis_is_only_a_warning() {
        let mut env = FakeEnv::healthy();
        env.0.remove("ROLTER_DATABASE_URL");
        env.0.remove("ROLTER_REDIS_URL");
        let findings = run_checks(&env);

        let db = findings
            .iter()
            .find(|f| f.title.contains("ROLTER_DATABASE_URL"))
            .expect("missing database not reported");
        assert!(db.fatal);

        let redis = findings
            .iter()
            .find(|f| f.title.contains("ROLTER_REDIS_URL"))
            .expect("missing redis not reported");
        // a single-replica deployment is legitimately fine without redis
        assert!(!redis.fatal);
    }

    #[cfg(feature = "postgres")]
    mod kek_against_the_store {
        use super::*;
        use rolter_store::postgres::kek_audit::{ColumnAudit, KekAudit, SEALED_COLUMNS};

        fn audit(sampled: usize, opened: usize) -> KekAudit {
            KekAudit {
                columns: vec![ColumnAudit {
                    column: SEALED_COLUMNS[0],
                    sampled,
                    opened,
                }],
            }
        }

        #[test]
        fn a_kek_that_opens_everything_produces_no_finding() {
            assert!(kek_mismatch_finding(&audit(10, 10)).is_none());
        }

        #[test]
        fn an_empty_store_produces_no_finding_either() {
            // vacuously intact. `rolter check` must not invent a failure for a
            // deployment that simply has not stored a credential yet
            assert!(kek_mismatch_finding(&KekAudit::default()).is_none());
        }

        #[test]
        fn a_mismatch_is_fatal_and_explains_the_restore_that_caused_it() {
            let finding =
                kek_mismatch_finding(&audit(10, 0)).expect("a wrong KEK must be reported");
            assert!(
                finding.fatal,
                "starting into an unreadable store is worse than not starting"
            );
            assert!(finding.title.contains(KEK_ENV));
            assert!(
                finding.detail.contains("restore"),
                "the detail must name the cause, not only the symptom: {}",
                finding.detail
            );
            assert!(
                finding.detail.contains("provider_keys"),
                "the detail must name what cannot be read: {}",
                finding.detail
            );
            assert!(
                finding.detail.contains("backup-and-restore"),
                "the detail must point at the runbook: {}",
                finding.detail
            );
        }

        #[test]
        fn a_partial_mismatch_is_reported_too() {
            // one unreadable row is already a split store; nothing about it
            // gets better by being rare
            let finding = kek_mismatch_finding(&audit(10, 9)).expect("reported");
            assert!(finding.detail.contains("1 of 10"));
        }
    }

    #[test]
    fn a_non_postgres_database_url_is_rejected() {
        let env = FakeEnv::healthy().with("ROLTER_DATABASE_URL", "mysql://user:pw@db/rolter");
        let findings = run_checks(&env);
        assert!(findings
            .iter()
            .any(|f| f.fatal && f.title.contains("not a postgres URL")));
    }

    #[test]
    fn a_rejected_database_url_is_redacted_in_the_report() {
        let env = FakeEnv::healthy().with("ROLTER_DATABASE_URL", "mysql://user:hunter2@db/rolter");
        let findings = run_checks(&env);
        let (text, _) = report(&findings, false);
        assert!(
            !text.contains("hunter2"),
            "credentials leaked into the report:\n{text}"
        );
        assert!(text.contains("***@db/rolter"));
    }

    #[test]
    fn binding_the_control_plane_to_all_interfaces_warns() {
        let env = FakeEnv::healthy().with("ROLTER_CONTROL_HOST", "0.0.0.0");
        let findings = run_checks(&env);
        let exposure = findings
            .iter()
            .find(|f| f.title.contains("0.0.0.0"))
            .expect("exposure not reported");
        assert!(!exposure.fatal);
    }

    #[test]
    fn an_unset_control_host_warns_because_the_default_is_all_interfaces() {
        let mut env = FakeEnv::healthy();
        env.0.remove("ROLTER_CONTROL_HOST");
        let findings = run_checks(&env);
        assert!(findings.iter().any(|f| f.title.contains("0.0.0.0")));
    }

    #[test]
    fn strict_promotes_warnings_to_failure() {
        let env = FakeEnv::healthy().with("ROLTER_CONTROL_HOST", "0.0.0.0");
        let findings = run_checks(&env);
        assert!(findings.iter().all(|f| !f.fatal), "expected warnings only");
        assert!(!report(&findings, false).1, "warnings alone must not fail");
        assert!(report(&findings, true).1, "--strict must fail on warnings");
    }

    #[test]
    fn blank_values_count_as_unset() {
        // `ROLTER_KEK=` in a compose file is absence, not configuration
        let env = FakeEnv::healthy().with(KEK_ENV, "   ");
        let findings = run_checks(&env);
        assert!(findings
            .iter()
            .any(|f| f.fatal && f.title.contains("is missing")));
    }

    /// The command exists because this variable's name drifted between code and
    /// docs. Pin the local copy to the store's definition so it cannot drift
    /// again inside the codebase.
    #[cfg(feature = "postgres")]
    #[test]
    fn the_kek_variable_name_matches_the_store() {
        assert_eq!(KEK_ENV, rolter_store::postgres::crypto::KEK_ENV);
    }

    #[test]
    fn redact_leaves_a_credential_free_url_alone() {
        assert_eq!(
            redact("postgres://db:5432/rolter"),
            "postgres://db:5432/rolter"
        );
        assert_eq!(redact("not-a-url"), "not-a-url");
    }
    #[test]
    fn a_missing_key_pepper_warns() {
        let mut env = FakeEnv::healthy();
        env.0.remove("ROLTER_KEY_PEPPER");
        let findings = run_checks(&env);
        assert_eq!(titles(&findings), ["ROLTER_KEY_PEPPER is unset"]);
        assert!(!findings[0].fatal, "an unpeppered deployment still works");
    }

    #[test]
    fn a_wildcard_cors_origin_is_fatal() {
        let findings = run_checks(&FakeEnv::healthy().with("ROLTER_CORS_ALLOW_ORIGINS", "*"));
        assert_eq!(titles(&findings), ["CORS allows every origin"]);
        assert!(findings[0].fatal);
        // a wildcard hidden in a list is the same hole
        let findings = run_checks(
            &FakeEnv::healthy().with("ROLTER_CORS_ALLOW_ORIGINS", "https://ops.example.com, *"),
        );
        assert_eq!(titles(&findings), ["CORS allows every origin"]);
    }

    #[test]
    fn named_cors_origins_are_accepted() {
        let findings = run_checks(
            &FakeEnv::healthy().with("ROLTER_CORS_ALLOW_ORIGINS", "https://ops.example.com"),
        );
        assert!(findings.is_empty(), "{:?}", titles(&findings));
    }

    #[test]
    fn host_port_fills_in_the_scheme_default_and_strips_credentials() {
        assert_eq!(
            host_port("postgres://user:pw@db.internal/rolter").as_deref(),
            Some("db.internal:5432")
        );
        assert_eq!(
            host_port("postgres://user:pw@db.internal:6000/rolter").as_deref(),
            Some("db.internal:6000")
        );
        assert_eq!(host_port("redis://cache").as_deref(), Some("cache:6379"));
        // an ipv6 literal's own colons are not a port
        assert_eq!(
            host_port("redis://[2001:db8::1]").as_deref(),
            Some("[2001:db8::1]:6379")
        );
        assert_eq!(
            host_port("redis://[2001:db8::1]:6380").as_deref(),
            Some("[2001:db8::1]:6380")
        );
        // an unknown scheme has no default port to guess at
        assert_eq!(host_port("mysql://db"), None);
        assert_eq!(host_port("not-a-url"), None);
    }

    #[tokio::test]
    async fn an_unreachable_database_is_fatal_and_an_unreachable_redis_is_not() {
        // a loopback port that was just released: the connect is refused
        // immediately and deterministically. an off-host address would be at the
        // mercy of whatever the test environment does to outbound traffic
        let closed = {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            drop(listener);
            addr
        };
        let env = FakeEnv::healthy()
            .with(
                "ROLTER_DATABASE_URL",
                &format!("postgres://user:pw@{closed}/rolter"),
            )
            .with("ROLTER_REDIS_URL", &format!("redis://{closed}"));
        let mut findings = Vec::new();
        check_reachability(&env, &mut findings).await;
        assert_eq!(
            titles(&findings),
            [
                "ROLTER_DATABASE_URL is not reachable",
                "ROLTER_REDIS_URL is not reachable"
            ]
        );
        assert!(findings[0].fatal, "no database is not a degraded start");
        assert!(!findings[1].fatal, "redis has a documented fallback");
    }

    #[tokio::test]
    async fn a_reachable_datastore_produces_no_finding() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let env = FakeEnv::healthy().with(
            "ROLTER_DATABASE_URL",
            &format!("postgres://user:pw@{addr}/rolter"),
        );
        let mut findings = Vec::new();
        check_reachability(&env, &mut findings).await;
        assert!(
            findings.iter().all(|f| !f.title.contains("DATABASE")),
            "{:?}",
            titles(&findings)
        );
    }

    #[test]
    fn an_unset_clickhouse_url_is_called_out_rather_than_left_to_an_empty_chart() {
        // #929: the dashboard reads rows the gateway writes. With no
        // destination, Usage/Costs/Logs render as a quiet deployment rather
        // than an unconfigured one, and nothing anywhere says which it is
        let env = FakeEnv::healthy();
        let mut findings = Vec::new();
        check_analytics_destination(&env, &mut findings);
        assert!(findings.is_empty(), "{:?}", titles(&findings));

        let mut env = FakeEnv::healthy();
        env.0.remove("CLICKHOUSE_URL");
        let mut findings = Vec::new();
        check_analytics_destination(&env, &mut findings);
        assert_eq!(findings.len(), 1, "{:?}", titles(&findings));
        assert!(findings[0].title.contains("CLICKHOUSE_URL"));
        // a warning, not an error: a deployment can legitimately run without
        // analytics, it just should not be surprised by an empty screen
        assert!(!findings[0].fatal);
    }

    #[test]
    fn a_deployment_with_no_database_is_not_nagged_about_analytics() {
        // no store means a smoke test or a local toy, where an empty analytics
        // screen surprises nobody
        let mut env = FakeEnv::healthy();
        env.0.remove("CLICKHOUSE_URL");
        env.0.remove("ROLTER_DATABASE_URL");
        let mut findings = Vec::new();
        check_analytics_destination(&env, &mut findings);
        assert!(findings.is_empty(), "{:?}", titles(&findings));
    }

    #[test]
    fn an_empty_clickhouse_url_counts_as_unset() {
        // a compose file that declares the variable without a value is the
        // same deployment as one that never mentioned it
        let env = FakeEnv::healthy().with("CLICKHOUSE_URL", "   ");
        let mut findings = Vec::new();
        check_analytics_destination(&env, &mut findings);
        assert_eq!(findings.len(), 1, "{:?}", titles(&findings));
    }
}
