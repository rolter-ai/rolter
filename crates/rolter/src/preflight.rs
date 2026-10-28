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

use clap::{Args, ValueEnum};

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

/// The process a `rolter check` run validates the environment of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Role {
    /// the data plane: only the rules that apply to a gateway
    Gateway,
    /// the control plane and dashboard: every rule
    #[default]
    Control,
}

#[derive(Args, Debug, Default)]
pub struct CheckArgs {
    /// path to the gateway config file to validate
    #[arg(short, long, env = "ROLTER_CONFIG")]
    pub config: Option<String>,

    /// treat warnings as failures too
    #[arg(long)]
    pub strict: bool,

    /// which process this environment is for. a gateway pod has no database,
    /// admin token or dashboard, so the control-plane rules do not apply to it
    #[arg(long, value_enum, env = "ROLTER_CHECK_ROLE", default_value = "control")]
    pub role: Role,

    /// also probe that the configured datastores accept a TCP connection.
    /// off by default so `rolter check` stays offline and side-effect free
    #[arg(long)]
    pub connect: bool,

    /// report the config-schema migrations this build would apply to the file,
    /// without writing anything. the pre-flight preview for an upgrade (#326)
    #[arg(long)]
    pub migrations: bool,
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
    match env.get("ROLTER_ADMIN_TOKEN") {
        None => out.push(Finding::error(
            "ROLTER_ADMIN_TOKEN is missing",
            "the management API and /internal/snapshot are unauthenticated without it. Anyone \
             who can reach the control plane can read the whole config snapshot and mutate \
             providers, routes and virtual keys.",
        )),
        // the control plane refuses to start on the same condition (#1964)
        Some(token) => {
            if let Err(short) = rolter_core::admin_token::check(&token) {
                out.push(Finding::error(
                    "ROLTER_ADMIN_TOKEN is too short",
                    format!("{short}; the control plane will refuse to start with it."),
                ));
            }
        }
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
                    rolter_core::redact::redact_url(&url)
                ),
            )),
        Some(_) => {}
    }
    check_redis(env, out);
}

fn check_redis(env: &dyn Env, out: &mut Vec<Finding>) {
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
///
/// Only an open bind is dangerous: with `ROLTER_ADMIN_TOKEN` set every
/// management route is authenticated, and a pod has to listen on every
/// interface for its Service to reach it, so the chart's `0.0.0.0` is intended.
///
/// An unset `ROLTER_CONTROL_HOST` is not a finding: the control plane's own
/// default bind is `127.0.0.1`, so nothing is exposed by omission.
fn check_exposure(env: &dyn Env, out: &mut Vec<Finding>) {
    if env.get("ROLTER_ADMIN_TOKEN").is_some() {
        return;
    }
    if env
        .get("ROLTER_CONTROL_HOST")
        .is_some_and(|h| h == "0.0.0.0")
    {
        out.push(Finding::warn(
            "control plane binds 0.0.0.0",
            "the management API is reachable on every interface. Bind it to a private address, \
             or keep it behind an ingress that terminates TLS and restricts access.",
        ));
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

/// The session pepper, the control plane's counterpart to the key pepper.
///
/// A warning like the key pepper's rather than an error. Session, invitation
/// and SCIM tokens are 256 bits from the CSPRNG and recovery codes 80, all far
/// past brute force, so an unpeppered digest still cannot be inverted and the
/// deployment works. What goes is the defence in depth.
/// Without a pepper, someone who can write to the database can plant a session
/// for a token they chose, and the failed-login throttle's Redis keys carry no
/// deployment salt, so two unpeppered deployments sharing one Redis share
/// each other's lockouts.
fn check_session_pepper(env: &dyn Env, out: &mut Vec<Finding>) {
    if env.get("ROLTER_SESSION_PEPPER").is_none() {
        out.push(Finding::warn(
            "ROLTER_SESSION_PEPPER is unset",
            "sign-in sessions, invitation links, SCIM tokens and MFA recovery codes are stored \
             as unpeppered digests, so anyone who can write to the database can plant a session \
             for a token of their choosing. Set the same value on every control-plane replica. \
             Setting or changing it signs everyone out and voids open invitations, SCIM tokens \
             and recovery codes, so do it before the deployment has users or in a maintenance \
             window. `rolter init` generates one.",
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

/// The finding for a relay check that failed (#3304): a warning, since mail is
/// optional and a failing relay only costs the convenience of emailed links.
/// The stage and a fixed sentence; never the relay's address or credential.
#[cfg(feature = "postgres")]
fn mail_relay_finding(failure: &rolter_control::mail::RelayFailure) -> Finding {
    Finding::warn(
        format!("the mail relay failed its {} check", failure.stage.as_str()),
        format!(
            "{} (ROLTER_SMTP_URL). Invitations are still created and their links returned; \
             they are just not emailed until this is fixed.",
            failure.reason
        ),
    )
}

/// Connect, set up TLS and authenticate against the configured mail relay,
/// without sending a message (#3304). Behind `--connect` because it opens a
/// socket, and behind the `postgres` feature because the mailer is part of the
/// control plane's database-backed build. The relay is held to the egress policy
/// of the file given with `--config`, the one the control plane runs with, so an
/// address the operator allowed is not reported as denied.
#[cfg(feature = "postgres")]
async fn check_mail_relay(egress: &rolter_core::EgressPolicy, out: &mut Vec<Finding>) {
    if let Some(Err(failure)) = rolter_control::mail::check_env_relay(egress).await {
        out.push(mail_relay_finding(&failure));
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
    let audit =
        match kek_audit::audit_kek_at(&database_url, &Kek::from_secret(&secret), timeout).await {
            Ok(audit) => audit,
            // not fatal on its own: an unusable database is already an error from
            // the reachability probe, and a schema this binary predates is a
            // migration away rather than a KEK problem. Saying which check did not
            // run beats reporting a KEK as verified when it was never tried
            Err(error) => {
                out.push(kek_unverified_finding(&error));
                return;
            }
        };

    if let Some(finding) = kek_mismatch_finding(&audit) {
        out.push(finding);
    }
}

/// The warning for an audit that could not be read at all, so the KEK was never
/// tried against the store.
#[cfg(feature = "postgres")]
fn kek_unverified_finding(error: &dyn std::fmt::Display) -> Finding {
    Finding::warn(
        "could not verify the KEK against the store",
        format!(
            "reading the sealed columns failed, so a KEK mismatch would not be caught \
             here: {error}"
        ),
    )
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
        "{KEK_ENV} does not open secrets already in this database. This is what a restore onto \
         the wrong KEK looks like: the dump carried the ciphertext, the KEK was not in the dump, \
         and nothing else fails until an upstream call does. Restore the KEK that sealed this \
         database — the ciphertext cannot be recovered without it. See \
         docs/dev-docs/deployment/backup-and-restore.md."
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

pub(crate) fn run_checks(env: &dyn Env, role: Role) -> Vec<Finding> {
    let mut out = Vec::new();
    check_example_values(env, &mut out);
    check_key_pepper(env, &mut out);
    check_analytics_destination(env, &mut out);
    match role {
        Role::Control => {
            check_kek(env, &mut out);
            check_admin_token(env, &mut out);
            check_datastores(env, &mut out);
            check_exposure(env, &mut out);
            check_session_pepper(env, &mut out);
            check_cors(env, &mut out);
        }
        // the gateway reads its config from the control plane and keeps counters
        // in redis, so the redis warning is the one datastore rule that is its own
        Role::Gateway => check_redis(env, &mut out),
    }
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

/// A route header the gateway will not send (#2924): a name that is not a valid
/// header name, a value the wire cannot carry, or a credential or framing header
/// such as `authorization`. The gateway leaves it out and logs the same sentence
/// at startup, so the route serves without it; fatal here because the file is
/// not doing what its author wrote, and a deploy check is the place to say so.
fn route_header_findings(config: &rolter_core::GatewayConfig) -> Vec<Finding> {
    config
        .routes
        .iter()
        .flat_map(|route| {
            route.advanced.header_problems().into_iter().map(|problem| {
                Finding::error(
                    format!(
                        "route '{}' sets a header the gateway will not send",
                        route.model
                    ),
                    problem,
                )
            })
        })
        .collect()
}

/// The public example key in a config file is a credential the whole internet
/// knows, with access to every model. Fatal rather than a warning: a production
/// config that carries it serves it to every gateway polling the control plane
/// (#2408).
fn example_key_findings(config: &rolter_core::GatewayConfig) -> Vec<Finding> {
    if !config
        .virtual_keys
        .iter()
        .any(|k| k.is_public_example_key())
    {
        return Vec::new();
    }
    vec![Finding::error(
        format!(
            "config declares the public virtual key {}",
            rolter_core::PUBLIC_EXAMPLE_KEY
        ),
        "it ships in rolter.example.toml and the image's baked config, is published in the \
         repository, and allows every model. Delete its [[virtual_keys]] entry (`rolter init \
         --profile production` writes a config without it) and mint keys from the dashboard \
         or API instead.",
    )]
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

/// `project_scoped = true` in a file the gateway serves directly is inert (#2468).
///
/// A warning, not an error: the same file is valid input to `rolter-seed
/// --import`, where the flag is the point.
fn project_scoped_findings(config: &rolter_core::GatewayConfig) -> Vec<Finding> {
    config
        .project_scoped_warnings()
        .into_iter()
        .map(|detail| {
            Finding::warn(
                "project_scoped has no effect without the control plane",
                detail,
            )
        })
        .collect()
}

/// A route that balances across decision-only and generative targets (#3154).
///
/// A warning, not an error: the gateway serves the route, it just sends some
/// requests to a target that cannot answer them.
fn mixed_decision_route_findings(config: &rolter_core::GatewayConfig) -> Vec<Finding> {
    config
        .mixed_decision_route_warnings()
        .into_iter()
        .map(|detail| Finding::warn("route mixes decision-only and generative targets", detail))
        .collect()
}

/// Every problem the gateway's own validation finds in the file (#3320).
///
/// `GatewayConfig::load` parses the file and checks only its CA bundles; the
/// gateway then runs the strict [`rolter_core::GatewayConfig::validate`] at
/// startup, and every route, provider, budget or cache setting it names fails
/// to serve. Running it here is what makes a file that passes `rolter check`
/// one the gateway accepts. A route header problem is left out: the finding
/// above already reports it, by the route's name.
fn validation_findings(config: &rolter_core::GatewayConfig) -> Vec<Finding> {
    let Err(problems) = config.validate() else {
        return Vec::new();
    };
    let reported: std::collections::HashSet<String> = config
        .routes
        .iter()
        .flat_map(|route| {
            route
                .advanced
                .header_problems()
                .into_iter()
                .map(move |problem| format!("route '{}': {problem}", route.model))
        })
        .collect();
    problems
        .into_iter()
        .filter(|problem| !reported.contains(problem))
        .map(|detail| Finding::error("config fails the gateway's validation", detail))
        .collect()
}

/// Render the migration plan for a config file, and the finding that goes with it.
///
/// This is a preview, not an action: nothing is written, here or at load time.
/// The gateway migrates the parsed document in memory on every boot, so a file
/// that is behind still starts — which is the whole point of #326, and why a
/// pending migration is a warning rather than an error.
///
/// Printed only under `--migrations`. An operator who has never stamped their
/// file would otherwise meet this on every `rolter check` of a perfectly
/// working deployment, and a warning everyone learns to skip past is worse than
/// no warning.
fn migration_findings(raw: &str, out: &mut String) -> Vec<Finding> {
    let report = match rolter_core::plan_config_migration(raw) {
        Ok(report) => report,
        // unparsable as TOML; the load below reports it properly
        Err(_) => return Vec::new(),
    };

    if report.ahead {
        let _ = writeln!(
            out,
            "config schema: file is version {}, this build understands {}\n",
            report.from,
            rolter_core::CURRENT_SCHEMA_VERSION
        );
        return vec![Finding::warn(
            "config is newer than this build",
            format!(
                "The file is stamped schema_version {}, and this build understands {}. It loads \
                 unchanged and keys this build does not know are ignored, so a rollback still \
                 starts — but settings written for the newer version are silently inactive.",
                report.from,
                rolter_core::CURRENT_SCHEMA_VERSION
            ),
        )];
    }

    if report.is_empty() {
        let _ = writeln!(
            out,
            "config schema: version {}, up to date; no migrations pending\n",
            report.to
        );
        return Vec::new();
    }

    let _ = writeln!(
        out,
        "config schema: version {} -> {}, {} migration(s) pending",
        report.from,
        report.to,
        report
            .steps
            .iter()
            .filter(|s| !s.changes.is_empty())
            .count()
    );
    for step in report.steps.iter().filter(|s| !s.changes.is_empty()) {
        let _ = writeln!(out, "  {} -> {}  {}", step.from, step.to, step.summary);
        for change in &step.changes {
            let _ = writeln!(out, "      {}: {}", change.path, change.detail);
        }
    }
    out.push('\n');

    vec![Finding::warn(
        "config is written for an older schema version",
        format!(
            "The file is schema_version {} and this build writes {}. rolter migrates it in \
             memory on every load, so nothing here is broken and no action is required to \
             start — this is a preview of what the upgrade does, not a fault. The listing \
             above is every change; nothing else in the file is touched.",
            report.from, report.to
        ),
    )]
}

pub async fn run(args: CheckArgs) -> anyhow::Result<()> {
    let mut findings = run_checks(&ProcessEnv, args.role);

    if args.connect {
        check_reachability(&ProcessEnv, &mut findings).await;
        #[cfg(feature = "postgres")]
        if args.role == Role::Control {
            check_kek_opens_the_store(&ProcessEnv, &mut findings).await;
            let egress = args
                .config
                .as_deref()
                .and_then(|path| rolter_core::GatewayConfig::load(std::path::Path::new(path)).ok())
                .map(|config| config.egress)
                .unwrap_or_default();
            check_mail_relay(&egress, &mut findings).await;
        }
    }

    // the config file is optional: a fully DB-backed deployment has none
    let mut preamble = String::new();
    if let Some(path) = args.config.as_deref() {
        if let Ok(raw) = std::fs::read_to_string(path) {
            findings.extend(unknown_key_findings(&raw));
            if args.migrations {
                findings.extend(migration_findings(&raw, &mut preamble));
            }
        }
        match rolter_core::GatewayConfig::load(std::path::Path::new(path)) {
            Ok(config) => {
                findings.extend(custom_api_base_findings(&config));
                findings.extend(route_header_findings(&config));
                findings.extend(example_key_findings(&config));
                findings.extend(project_scoped_findings(&config));
                findings.extend(mixed_decision_route_findings(&config));
                findings.extend(validation_findings(&config));
            }
            Err(error) => findings.push(Finding::error(
                format!("config file {path} is not usable"),
                error.to_string(),
            )),
        }
    }

    let (text, failed) = report(&findings, args.strict);
    print!("{preamble}{text}");
    if failed {
        anyhow::bail!("pre-boot validation failed; refusing to report this deployment as ready");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// #1584: `rolter check` refuses a route whose every target is a
    /// restricted fallback, by the name the gateway's own validation uses.
    #[test]
    fn a_route_of_only_restricted_targets_is_an_error() {
        let config = rolter_core::GatewayConfig::from_toml_str(
            r#"
[[providers]]
name = "premium"
kind = "openai"
api_base = "https://api.openai.com"

[[routes]]
model = "stranded"
[[routes.targets]]
provider = "premium"
takes_over = ["refusal"]
"#,
        )
        .expect("parses");
        let findings = validation_findings(&config);
        assert_eq!(findings.len(), 1);
        let (text, failed) = report(&findings, false);
        assert!(failed, "{text}");
        assert!(text.contains("all_targets_restricted"), "{text}");
    }

    /// #3320: a file that parses but that the gateway's validation refuses
    /// fails `rolter check`, one error finding per problem.
    #[test]
    fn a_file_that_parses_but_fails_validation_is_an_error() {
        let config = rolter_core::GatewayConfig::from_toml_str(
            r#"
[[providers]]
name = "openai"
kind = "openai"
api_base = "https://api.openai.com"

[[routes]]
model = "chat"
[[routes.targets]]
provider = "missing"

[[routes]]
model = "chat"
[[routes.targets]]
provider = "openai"
weight = 0
"#,
        )
        .expect("parses");
        let findings = validation_findings(&config);
        let (text, failed) = report(&findings, false);
        assert!(failed, "{text}");
        for problem in [
            "targets unknown provider 'missing'",
            "duplicate route model 'chat'",
            "weight",
        ] {
            assert!(text.contains(problem), "{problem} missing from {text}");
        }
        assert!(findings.len() >= 3, "one finding per problem: {findings:?}");
    }

    /// A valid file adds no finding, and a route header problem is reported
    /// once, by its own finding, not again as a validation problem.
    #[test]
    fn validation_adds_nothing_to_a_valid_file_or_a_header_problem() {
        let valid = rolter_core::GatewayConfig::from_toml_str(
            r#"
[[providers]]
name = "openai"
kind = "openai"
api_base = "https://api.openai.com"

[[routes]]
model = "chat"
[[routes.targets]]
provider = "openai"
"#,
        )
        .expect("parses");
        assert!(validation_findings(&valid).is_empty());

        let mut header = valid.clone();
        header.routes[0]
            .advanced
            .headers
            .insert("authorization".into(), "Bearer x".into());
        assert_eq!(route_header_findings(&header).len(), 1);
        assert!(
            validation_findings(&header).is_empty(),
            "{:?}",
            validation_findings(&header)
        );
    }

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
                .with(
                    "ROLTER_ADMIN_TOKEN",
                    "an-admin-token-of-a-sound-length-0123",
                )
                .with("ROLTER_DATABASE_URL", "postgres://user:pw@db:5432/rolter")
                .with("ROLTER_REDIS_URL", "redis://redis:6379")
                .with("ROLTER_CONTROL_HOST", "127.0.0.1")
                .with("ROLTER_KEY_PEPPER", "a-deployment-wide-pepper")
                .with("ROLTER_SESSION_PEPPER", "a-control-plane-session-pepper")
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

    /// A deployment in the deprecated flat-array shape, as an operator on an
    /// older release has it on disk.
    const LEGACY_CONFIG: &str = r#"
        [[providers]]
        name = "openai-main"
        kind = "openai"
        api_base = "https://api.openai.com"
        api_key_env = "OPENAI_API_KEY"

        [[routes]]
        model = "chat"
        [[routes.targets]]
        provider = "openai-main"
    "#;

    #[test]
    fn the_migration_preview_names_every_pending_step_and_warns_without_failing() {
        let mut out = String::new();
        let findings = migration_findings(LEGACY_CONFIG, &mut out);

        assert!(
            out.contains("version 1 -> 3"),
            "the preview must name both versions: {out}"
        );
        assert!(
            out.contains("readonly"),
            "the preview must say what the step does: {out}"
        );
        assert!(
            out.contains("api_keys"),
            "the preview must name the credential step too: {out}"
        );
        assert_eq!(
            titles(&findings),
            ["config is written for an older schema version"]
        );
        // a file that is merely behind must never block a boot
        assert!(findings.iter().all(|f| !f.fatal));
        let (_, failed) = report(&findings, false);
        assert!(!failed, "a pending migration is not a pre-boot failure");
    }

    #[test]
    fn an_up_to_date_config_reports_nothing_pending() {
        let raw = format!("schema_version = {}\n", rolter_core::CURRENT_SCHEMA_VERSION);
        let mut out = String::new();
        let findings = migration_findings(&raw, &mut out);

        assert!(out.contains("up to date"), "{out}");
        assert!(findings.is_empty(), "{:?}", titles(&findings));
    }

    /// The rollback promise: a file from a newer build warns, and still loads.
    #[test]
    fn a_config_from_a_newer_build_warns_but_is_not_fatal() {
        let raw = format!(
            "schema_version = {}\n",
            rolter_core::CURRENT_SCHEMA_VERSION + 1
        );
        let mut out = String::new();
        let findings = migration_findings(&raw, &mut out);

        assert!(out.contains("this build understands"), "{out}");
        assert_eq!(titles(&findings), ["config is newer than this build"]);
        assert!(findings.iter().all(|f| !f.fatal));
        let (_, failed) = report(&findings, false);
        assert!(
            !failed,
            "a rollback must not be reported as a pre-boot failure"
        );
    }

    #[test]
    fn an_unparsable_config_is_left_to_the_loader_to_report() {
        let mut out = String::new();
        let findings = migration_findings("this is not = = toml", &mut out);
        assert!(findings.is_empty());
        assert!(
            out.is_empty(),
            "no preview for a file that does not parse: {out}"
        );
    }

    #[test]
    fn the_public_example_key_is_a_fatal_finding() {
        let config = rolter_core::GatewayConfig::from_toml_str(
            "[[virtual_keys]]\nkey = \"sk-rolter-dev\"\n",
        )
        .expect("parses");
        let findings = example_key_findings(&config);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].fatal, "must be an error, not a warning");
        let (_, failed) = report(&findings, false);
        assert!(failed, "a plain `rolter check` must fail on it");

        let other = rolter_core::GatewayConfig::from_toml_str(
            "[[virtual_keys]]\nkey = \"sk-something-else\"\n",
        )
        .expect("parses");
        assert!(example_key_findings(&other).is_empty());
    }

    #[test]
    fn a_correctly_configured_deployment_is_clean() {
        let findings = run_checks(&FakeEnv::healthy(), Role::Control);
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
    fn a_route_header_that_replaces_the_credential_fails_the_check() {
        let config = rolter_core::GatewayConfig::from_toml_str(
            r#"
            [[providers]]
            name = "up"
            kind = "openai_compatible"
            api_base = "http://127.0.0.1:1"

            [[routes]]
            model = "m"
            [routes.advanced]
            headers = { "Authorization" = "Bearer sk-route", "x-team" = "platform" }
            [[routes.targets]]
            provider = "up"
            "#,
        )
        .unwrap();

        let findings = route_header_findings(&config);
        assert_eq!(
            titles(&findings),
            vec!["route 'm' sets a header the gateway will not send"]
        );
        let (text, failed) = report(&findings, false);
        assert!(failed, "{text}");
        assert!(
            text.contains("header 'Authorization' cannot be set"),
            "{text}"
        );
        assert!(
            !text.contains("x-team"),
            "a sendable header is not a finding: {text}"
        );
    }

    #[test]
    fn a_missing_kek_is_fatal() {
        let mut env = FakeEnv::healthy();
        env.0.remove(KEK_ENV);
        let findings = run_checks(&env, Role::Control);
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
        let findings = run_checks(&env, Role::Control);
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
        // under MIN_KEK_LEN on purpose, so it is a slice of a generated value
        let short = uuid::Uuid::new_v4().simple().to_string()[..7].to_string();
        let env = FakeEnv::healthy().with(KEK_ENV, &short);
        let findings = run_checks(&env, Role::Control);
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
        let findings = run_checks(&env, Role::Control);
        assert!(findings
            .iter()
            .any(|f| f.fatal && f.title.contains("example value")));
    }

    #[test]
    fn the_e2e_throwaway_kek_is_caught() {
        let env = FakeEnv::healthy().with(KEK_ENV, "cm9sdGVyLWUyZS10ZXN0LWtlay1ub3Qtc2VjcmV0ISE=");
        let findings = run_checks(&env, Role::Control);
        assert!(findings
            .iter()
            .any(|f| f.fatal && f.title.contains("example value")));
    }

    #[test]
    fn a_short_admin_token_is_fatal_and_a_generated_one_is_not() {
        let short = FakeEnv::healthy().with("ROLTER_ADMIN_TOKEN", "changeme");
        assert!(run_checks(&short, Role::Control)
            .iter()
            .any(|f| f.fatal && f.title.contains("ROLTER_ADMIN_TOKEN is too short")));

        let generated =
            FakeEnv::healthy().with("ROLTER_ADMIN_TOKEN", &"0123456789abcdef".repeat(2));
        assert!(!run_checks(&generated, Role::Control)
            .iter()
            .any(|f| f.title.contains("ROLTER_ADMIN_TOKEN")));
    }

    #[test]
    fn a_missing_admin_token_is_fatal() {
        let mut env = FakeEnv::healthy();
        env.0.remove("ROLTER_ADMIN_TOKEN");
        let findings = run_checks(&env, Role::Control);
        assert!(findings
            .iter()
            .any(|f| f.fatal && f.title.contains("ROLTER_ADMIN_TOKEN")));
    }

    #[test]
    fn a_missing_database_is_fatal_and_a_missing_redis_is_only_a_warning() {
        let mut env = FakeEnv::healthy();
        env.0.remove("ROLTER_DATABASE_URL");
        env.0.remove("ROLTER_REDIS_URL");
        let findings = run_checks(&env, Role::Control);

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
    #[test]
    fn a_failing_mail_relay_is_a_warning_that_names_the_stage_only() {
        use rolter_control::mail::{RelayFailure, RelayStage};
        let failure = RelayFailure {
            stage: RelayStage::Auth,
            reason: "the mail server refused the credential in the relay url".to_string(),
        };
        let finding = mail_relay_finding(&failure);
        assert!(!finding.fatal, "mail is optional");
        assert_eq!(finding.title, "the mail relay failed its auth check");
        assert!(finding.detail.contains("refused the credential"));
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

        /// Both messages were once wrapped across source lines without a `\`
        /// continuation, so the next line's indentation became a run of spaces
        /// in the middle of the sentence (#1946). Only the first line of the
        /// mismatch detail is prose; the damage lines below it are indented on
        /// purpose.
        #[test]
        fn the_kek_messages_read_as_single_spaced_sentences() {
            let mismatch = kek_mismatch_finding(&audit(10, 0)).expect("reported");
            let prose = mismatch.detail.lines().next().expect("a first line");
            assert!(!prose.contains("  "), "run of spaces in: {prose}");

            let unverified = kek_unverified_finding(&"connection reset");
            assert!(
                !unverified.detail.contains("  "),
                "run of spaces in: {}",
                unverified.detail
            );
            assert!(unverified.detail.ends_with("here: connection reset"));
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
        let findings = run_checks(&env, Role::Control);
        assert!(findings
            .iter()
            .any(|f| f.fatal && f.title.contains("not a postgres URL")));
    }

    #[test]
    fn a_rejected_database_url_is_redacted_in_the_report() {
        let password = format!("pw-{}", uuid::Uuid::new_v4());
        let url = format!("mysql://user:{password}@db/rolter");
        let env = FakeEnv::healthy().with("ROLTER_DATABASE_URL", &url);
        let findings = run_checks(&env, Role::Control);
        let (text, _) = report(&findings, false);
        assert!(
            !text.contains(&password),
            "credentials leaked into the report:\n{text}"
        );
        assert!(text.contains("***@db/rolter"), "{text}");
    }

    #[test]
    fn binding_the_control_plane_to_all_interfaces_warns_in_open_mode() {
        let mut env = FakeEnv::healthy().with("ROLTER_CONTROL_HOST", "0.0.0.0");
        env.0.remove("ROLTER_ADMIN_TOKEN");
        let findings = run_checks(&env, Role::Control);
        let exposure = findings
            .iter()
            .find(|f| f.title.contains("0.0.0.0"))
            .expect("exposure not reported");
        assert!(!exposure.fatal);
    }

    #[test]
    fn an_unset_control_host_does_not_warn_because_the_default_is_loopback() {
        let mut env = FakeEnv::healthy();
        env.0.remove("ROLTER_CONTROL_HOST");
        env.0.remove("ROLTER_ADMIN_TOKEN");
        let findings = run_checks(&env, Role::Control);
        assert!(!findings.iter().any(|f| f.title.contains("0.0.0.0")));
    }

    #[test]
    fn an_all_interfaces_bind_behind_an_admin_token_is_intended() {
        // the chart binds 0.0.0.0 so the Service can reach the pod, and sets the token
        let env = FakeEnv::healthy().with("ROLTER_CONTROL_HOST", "0.0.0.0");
        let findings = run_checks(&env, Role::Control);
        assert!(findings.is_empty(), "{:?}", titles(&findings));
    }

    #[test]
    fn a_gateway_is_not_held_to_the_control_planes_rules() {
        // what the chart's gateway pod has: no database, admin token, kek or host
        let env = FakeEnv::default()
            .with("ROLTER_REDIS_URL", "redis://redis:6379")
            .with("ROLTER_KEY_PEPPER", "a-deployment-wide-pepper")
            .with("CLICKHOUSE_URL", "http://clickhouse:8123");
        let findings = run_checks(&env, Role::Gateway);
        assert!(findings.is_empty(), "{:?}", titles(&findings));
        assert!(
            !run_checks(&env, Role::Control).is_empty(),
            "the same environment is not a control plane"
        );
    }

    #[test]
    fn a_gateway_still_gets_the_checks_that_are_its_own() {
        let env = FakeEnv::default();
        let found = run_checks(&env, Role::Gateway);
        let found = titles(&found);
        assert!(found.iter().any(|t| t.contains("ROLTER_REDIS_URL")));
        assert!(found.iter().any(|t| t.contains("ROLTER_KEY_PEPPER")));
    }

    #[test]
    fn strict_promotes_warnings_to_failure() {
        let env = FakeEnv::healthy().with("ROLTER_CONTROL_HOST", "0.0.0.0");
        let env = env.with("ROLTER_KEY_PEPPER", " ");
        let findings = run_checks(&env, Role::Control);
        assert!(findings.iter().all(|f| !f.fatal), "expected warnings only");
        assert!(!report(&findings, false).1, "warnings alone must not fail");
        assert!(report(&findings, true).1, "--strict must fail on warnings");
    }

    #[test]
    fn blank_values_count_as_unset() {
        // `ROLTER_KEK=` in a compose file is absence, not configuration
        let env = FakeEnv::healthy().with(KEK_ENV, "   ");
        let findings = run_checks(&env, Role::Control);
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
    fn a_missing_key_pepper_warns() {
        let mut env = FakeEnv::healthy();
        env.0.remove("ROLTER_KEY_PEPPER");
        let findings = run_checks(&env, Role::Control);
        assert_eq!(titles(&findings), ["ROLTER_KEY_PEPPER is unset"]);
        assert!(!findings[0].fatal, "an unpeppered deployment still works");
    }

    #[test]
    fn a_missing_session_pepper_warns() {
        let mut env = FakeEnv::healthy();
        env.0.remove("ROLTER_SESSION_PEPPER");
        let findings = run_checks(&env, Role::Control);
        assert_eq!(titles(&findings), ["ROLTER_SESSION_PEPPER is unset"]);
        // same severity as the key pepper: the deployment works, it has only
        // lost the defence in depth, and --strict is what makes it block
        assert!(!findings[0].fatal, "an unpeppered deployment still works");
        let (_, failed) = report(&findings, false);
        assert!(!failed, "a warning alone must not fail a plain check");
        let (_, failed) = report(&findings, true);
        assert!(failed, "--strict must fail on it");
        // the operator adding it to a live deployment needs to know what it
        // costs before they do it
        assert!(
            findings[0].detail.contains("signs everyone out"),
            "{}",
            findings[0].detail
        );
    }

    #[test]
    fn a_blank_session_pepper_counts_as_unset() {
        // `ROLTER_SESSION_PEPPER=` is what an un-filled .env.example carries
        let findings = run_checks(
            &FakeEnv::healthy().with("ROLTER_SESSION_PEPPER", " "),
            Role::Control,
        );
        assert_eq!(titles(&findings), ["ROLTER_SESSION_PEPPER is unset"]);
    }

    #[test]
    fn a_wildcard_cors_origin_is_fatal() {
        let findings = run_checks(
            &FakeEnv::healthy().with("ROLTER_CORS_ALLOW_ORIGINS", "*"),
            Role::Control,
        );
        assert_eq!(titles(&findings), ["CORS allows every origin"]);
        assert!(findings[0].fatal);
        // a wildcard hidden in a list is the same hole
        let findings = run_checks(
            &FakeEnv::healthy().with("ROLTER_CORS_ALLOW_ORIGINS", "https://ops.example.com, *"),
            Role::Control,
        );
        assert_eq!(titles(&findings), ["CORS allows every origin"]);
    }

    #[test]
    fn named_cors_origins_are_accepted() {
        let findings = run_checks(
            &FakeEnv::healthy().with("ROLTER_CORS_ALLOW_ORIGINS", "https://ops.example.com"),
            Role::Control,
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
        // bound but never listening: refused, and the port stays reserved so a
        // parallel test cannot be handed it
        let (_closed_guard, closed) = {
            let socket = tokio::net::TcpSocket::new_v4().unwrap();
            socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
            let addr = socket.local_addr().unwrap();
            (socket, addr)
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

    #[test]
    fn project_scoped_in_a_served_file_is_a_warning() {
        let config = rolter_core::GatewayConfig::from_toml_str(
            "[[providers]]\nname = \"p\"\nkind = \"openai\"\napi_base = \"https://api.example.com/v1\"\napi_key_env = \"K\"\nproject_scoped = true\n",
        )
        .expect("parses");
        let findings = project_scoped_findings(&config);
        assert_eq!(findings.len(), 1);
        assert!(!findings[0].fatal);

        let plain = rolter_core::GatewayConfig::from_toml_str(
            "[[providers]]\nname = \"p\"\nkind = \"openai\"\napi_base = \"https://api.example.com/v1\"\napi_key_env = \"K\"\n",
        )
        .expect("parses");
        assert!(project_scoped_findings(&plain).is_empty());
    }

    #[test]
    fn a_route_mixing_decision_and_chat_targets_is_a_warning() {
        let config = rolter_core::GatewayConfig::from_toml_str(
            "[[providers]]\nname = \"d\"\nkind = \"system_one\"\napi_base = \"http://d.internal\"\n\
             [[providers]]\nname = \"c\"\nkind = \"openai\"\napi_base = \"https://api.example.com/v1\"\napi_key_env = \"K\"\n\
             [[routes]]\nmodel = \"m\"\ntargets = [{ provider = \"d\" }, { provider = \"c\" }]\n",
        )
        .expect("parses");
        let findings = mixed_decision_route_findings(&config);
        assert_eq!(findings.len(), 1);
        assert!(!findings[0].fatal);
    }
}
