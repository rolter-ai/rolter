//! Best-effort check for a newer rolter release (#901, #902).
//!
//! The control plane asks GitHub for the latest stable release once at boot
//! and every [`CHECK_INTERVAL`] after that, keeps the answer in a lock-free
//! cell, and serves it from `GET /api/v1/version` so the dashboard's version
//! hint costs one request to the control plane rather than one request to
//! GitHub per browser. The `rolter` launcher reuses [`fetch_latest`] and
//! [`is_newer`] for its stderr notice, so both surfaces agree on what "newer"
//! means.
//!
//! Offline is a normal state, not a fault: every failure is logged at `debug`
//! and the status simply reports no known latest version. `ROLTER_UPDATE_CHECK`
//! set to a falsy value turns the whole thing off for air-gapped deployments,
//! and the request carries nothing but a `User-Agent` — no installation id, no
//! config, no credentials.

use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use serde::Serialize;

/// Environment variable that turns the check off. Anything falsy (`false`,
/// `0`, `no`, `off`, case-insensitive) disables it; unset means enabled.
pub const ENV_VAR: &str = "ROLTER_UPDATE_CHECK";
/// The one endpoint the check ever talks to.
pub const RELEASES_LATEST_API: &str =
    "https://api.github.com/repos/rolter-ai/rolter/releases/latest";
/// Where a human goes to read about the release; used when the API answer
/// carries no `html_url` of its own.
pub const RELEASES_LATEST_URL: &str = "https://github.com/rolter-ai/rolter/releases/latest";
/// The version this binary was built as — the whole workspace shares one.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Hard ceiling on one request. The check must never hold a boot or a command
/// hostage to a slow network.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the control plane re-asks once it is up.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Read the opt-out from the process environment.
pub fn enabled_from_env() -> bool {
    enabled(std::env::var(ENV_VAR).ok().as_deref())
}

/// Whether `value`, the raw `ROLTER_UPDATE_CHECK` string, leaves the check on.
pub fn enabled(value: Option<&str>) -> bool {
    match value.map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) => !matches!(v.as_str(), "false" | "0" | "no" | "off"),
        None => true,
    }
}

/// A stable release GitHub reported as the latest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// bare semantic version, without the tag's `v` prefix
    pub version: String,
    /// the release page
    pub url: String,
}

/// Why one check produced no release. None of these are worth more than a
/// `debug` line: offline, rate-limited and a changed API shape all look the
/// same from inside an air-gapped deployment, and all are fine.
#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("unexpected status {0}")]
    Status(reqwest::StatusCode),
    #[error("malformed release payload: {0}")]
    Malformed(String),
}

/// A parsed `MAJOR.MINOR.PATCH[-PRERELEASE]`, enough of semver to order rolter
/// releases. Build metadata (`+…`) is ignored, as the spec says it must be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    core: [u64; 3],
    prerelease: Option<String>,
}

impl Version {
    /// Parse `1.2.3`, `v1.2.3`, `1.2.3-rc.1` or `1.2.3+build`; `None` for
    /// anything else, so an odd tag is ignored rather than compared wrongly.
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        let raw = raw
            .strip_prefix('v')
            .or_else(|| raw.strip_prefix('V'))
            .unwrap_or(raw);
        let raw = raw.split('+').next().unwrap_or(raw);
        let (core, prerelease) = match raw.split_once('-') {
            Some((core, pre)) if !pre.is_empty() => (core, Some(pre.to_string())),
            Some(_) => return None,
            None => (raw, None),
        };
        let mut parts = core.split('.');
        let mut out = [0u64; 3];
        for slot in &mut out {
            let part = parts.next()?;
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            *slot = part.parse().ok()?;
        }
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            core: out,
            prerelease,
        })
    }

    /// A release candidate, nightly or other pre-release build.
    pub fn is_prerelease(&self) -> bool {
        self.prerelease.is_some()
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match self.core.cmp(&other.core) {
            Ordering::Equal => {}
            other => return other,
        }
        // a pre-release sorts below the release it precedes; two pre-releases
        // of the same core compare identifier by identifier per semver §11
        match (&self.prerelease, &other.prerelease) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => compare_prerelease(a, b),
        }
    }
}

fn compare_prerelease(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let ordering = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    // numeric identifiers sort below alphanumeric ones
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
        }
    }
}

/// Whether `latest` is a stable release strictly newer than `current`.
///
/// A pre-release `latest` never counts — the check advertises stable upgrades
/// only — while a pre-release `current` (a release candidate, a dev build) is
/// told about the stable release it precedes. Anything that does not parse as
/// a version compares as "not newer", so a stray tag stays silent.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let (Some(latest), Some(current)) = (Version::parse(latest), Version::parse(current)) else {
        return false;
    };
    !latest.is_prerelease() && latest > current
}

/// Ask GitHub for the latest stable release, bounded by [`CHECK_TIMEOUT`].
///
/// GitHub's `releases/latest` already excludes pre-releases and drafts, and
/// the tag is validated as a version here so a malformed answer is an error
/// rather than a hint to upgrade to nothing.
pub async fn fetch_latest(client: &reqwest::Client) -> Result<Release, CheckError> {
    let response = client
        .get(RELEASES_LATEST_API)
        .timeout(CHECK_TIMEOUT)
        .header(
            reqwest::header::USER_AGENT,
            format!("rolter/{CURRENT_VERSION}"),
        )
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(CheckError::Status(status));
    }
    let payload: serde_json::Value = response.json().await?;
    release_from_payload(&payload)
}

/// Pull the release out of a `releases/latest` document.
pub fn release_from_payload(payload: &serde_json::Value) -> Result<Release, CheckError> {
    let tag = payload
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CheckError::Malformed("no tag_name".into()))?;
    let version = Version::parse(tag)
        .ok_or_else(|| CheckError::Malformed(format!("tag {tag:?} is not a version")))?;
    if version.is_prerelease() {
        return Err(CheckError::Malformed(format!(
            "tag {tag:?} is a pre-release"
        )));
    }
    let url = payload
        .get("html_url")
        .and_then(|v| v.as_str())
        .filter(|u| u.starts_with("https://github.com/"))
        .unwrap_or(RELEASES_LATEST_URL)
        .to_string();
    Ok(Release {
        version: tag.trim().trim_start_matches(['v', 'V']).to_string(),
        url,
    })
}

/// What `GET /api/v1/version` answers.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct UpdateStatus {
    /// the running version
    pub current: &'static str,
    /// the latest stable release, when a check has succeeded
    pub latest: Option<String>,
    /// the page describing it
    pub release_url: Option<String>,
    /// `latest` is strictly newer than `current`
    pub update_available: bool,
    /// when the last *successful* check completed; `None` until one has
    pub checked_at: Option<DateTime<Utc>>,
    /// whether the check runs at all in this deployment
    pub enabled: bool,
}

#[derive(Default)]
struct Snapshot {
    release: Option<Release>,
    checked_at: Option<DateTime<Utc>>,
}

/// The last successful check, shared between the background task and the
/// endpoint. Reads never block: the task swaps in a new snapshot and readers
/// keep whichever they loaded.
#[derive(Clone)]
pub struct UpdateChecker {
    enabled: bool,
    snapshot: Arc<ArcSwap<Snapshot>>,
}

impl UpdateChecker {
    /// A checker that will run when [`spawn`](Self::spawn) is called, or one
    /// that reports itself disabled and never talks to the network.
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            snapshot: Arc::new(ArcSwap::from_pointee(Snapshot::default())),
        }
    }

    /// The opted-out checker.
    pub fn disabled() -> Self {
        Self::new(false)
    }

    /// Build from the process environment.
    pub fn from_env() -> Self {
        Self::new(enabled_from_env())
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Record one successful check.
    pub fn record(&self, release: Release, at: DateTime<Utc>) {
        self.snapshot.store(Arc::new(Snapshot {
            release: Some(release),
            checked_at: Some(at),
        }));
    }

    /// The current answer, computed against [`CURRENT_VERSION`].
    pub fn status(&self) -> UpdateStatus {
        self.status_for(CURRENT_VERSION)
    }

    fn status_for(&self, current: &'static str) -> UpdateStatus {
        let snapshot = self.snapshot.load();
        let release = snapshot.release.as_ref();
        UpdateStatus {
            current,
            latest: release.map(|r| r.version.clone()),
            release_url: release.map(|r| r.url.clone()),
            update_available: release.is_some_and(|r| is_newer(&r.version, current)),
            checked_at: snapshot.checked_at,
            enabled: self.enabled,
        }
    }

    /// Run one check now and store the result. Failures are `debug` only.
    pub async fn check_once(&self, client: &reqwest::Client) {
        match fetch_latest(client).await {
            Ok(release) => {
                tracing::debug!(latest = %release.version, "update check completed");
                self.record(release, Utc::now());
            }
            Err(err) => {
                tracing::debug!(error = %err, "update check failed; offline is fine");
            }
        }
    }

    /// Start the background loop: one check at boot, then every
    /// [`CHECK_INTERVAL`]. A no-op when disabled.
    pub fn spawn(&self, client: reqwest::Client) {
        if !self.enabled {
            tracing::debug!("update check disabled by {ENV_VAR}");
            return;
        }
        let checker = self.clone();
        tokio::spawn(async move {
            loop {
                checker.check_once(&client).await;
                tokio::time::sleep(CHECK_INTERVAL).await;
            }
        });
    }
}

#[cfg(feature = "postgres")]
pub(crate) fn router() -> axum::Router<crate::ControlState> {
    axum::Router::new().route("/api/v1/version", axum::routing::get(get_version))
}

/// The running version and the latest stable release, for the dashboard's
/// footer hint. Every authenticated caller may read it: it is a fact about the
/// deployment, not about any tenant.
#[cfg(feature = "postgres")]
async fn get_version(
    principal: crate::rbac::Principal,
    axum::extract::State(state): axum::extract::State<crate::ControlState>,
) -> crate::crud::ApiResult<axum::Json<UpdateStatus>> {
    crate::rbac::authorize(
        &state,
        &principal,
        crate::rbac::ScopeChain::default(),
        crate::rbac_matrix::cap!("version", Read),
    )
    .await?;
    Ok(axum::Json(state.update_check.status()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_env_var_is_falsy_only_when_it_says_so() {
        assert!(enabled(None));
        assert!(enabled(Some("")));
        assert!(enabled(Some("true")));
        assert!(enabled(Some("1")));
        assert!(enabled(Some("yes")));
        for off in ["false", "FALSE", " 0 ", "no", "off", "Off"] {
            assert!(!enabled(Some(off)), "{off:?} must disable the check");
        }
    }

    #[test]
    fn versions_parse_with_and_without_a_v_prefix() {
        assert_eq!(Version::parse("1.2.3"), Version::parse("v1.2.3"));
        assert_eq!(Version::parse("V1.2.3"), Version::parse(" 1.2.3 "));
        assert_eq!(Version::parse("1.2.3+build.7"), Version::parse("1.2.3"));
        assert!(Version::parse("1.2.3-rc.1").is_some_and(|v| v.is_prerelease()));
        for bad in [
            "", "v", "1.2", "1.2.3.4", "1.x.3", "latest", "1.2.3-", "-1.2.3",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad:?} must not parse");
        }
    }

    #[test]
    fn a_newer_stable_release_is_reported() {
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(is_newer("0.1.10", "0.1.9"), "numeric, not lexical");
    }

    #[test]
    fn the_same_or_an_older_release_is_not() {
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(
            !is_newer("0.1.0", "0.2.0"),
            "a dev build ahead of the release stays quiet"
        );
    }

    #[test]
    fn prereleases_are_offered_the_stable_release_but_never_advertised() {
        // a release candidate is older than the release it precedes
        assert!(is_newer("0.2.0", "0.2.0-rc.1"));
        assert!(is_newer("0.2.0", "0.2.0-alpha"));
        // but the check never points at a pre-release
        assert!(!is_newer("0.3.0-rc.1", "0.2.0"));
        assert!(!is_newer("0.2.0-rc.2", "0.2.0-rc.1"));
    }

    #[test]
    fn prerelease_identifiers_order_per_semver() {
        let v = |s: &str| Version::parse(s).expect("valid");
        assert!(v("1.0.0-alpha") < v("1.0.0-alpha.1"));
        assert!(v("1.0.0-alpha.1") < v("1.0.0-alpha.beta"));
        assert!(v("1.0.0-alpha.beta") < v("1.0.0-beta"));
        assert!(v("1.0.0-beta.2") < v("1.0.0-beta.11"));
        assert!(v("1.0.0-rc.1") < v("1.0.0"));
    }

    #[test]
    fn unparsable_versions_never_report_an_update() {
        assert!(!is_newer("latest", "0.1.0"));
        assert!(!is_newer("0.2.0", "dev"));
        assert!(!is_newer("", ""));
    }

    #[test]
    fn a_release_payload_yields_the_bare_version_and_its_page() {
        let payload = serde_json::json!({
            "tag_name": "v0.2.0",
            "html_url": "https://github.com/rolter-ai/rolter/releases/tag/v0.2.0",
        });
        let release = release_from_payload(&payload).expect("valid");
        assert_eq!(release.version, "0.2.0");
        assert_eq!(
            release.url,
            "https://github.com/rolter-ai/rolter/releases/tag/v0.2.0"
        );
    }

    #[test]
    fn a_payload_without_a_github_url_falls_back_to_the_releases_page() {
        let payload =
            serde_json::json!({ "tag_name": "0.2.0", "html_url": "https://evil.example/x" });
        assert_eq!(
            release_from_payload(&payload).expect("valid").url,
            RELEASES_LATEST_URL
        );
        let payload = serde_json::json!({ "tag_name": "0.2.0" });
        assert_eq!(
            release_from_payload(&payload).expect("valid").url,
            RELEASES_LATEST_URL
        );
    }

    #[test]
    fn malformed_payloads_are_errors_not_updates() {
        for payload in [
            serde_json::json!({}),
            serde_json::json!({ "tag_name": 7 }),
            serde_json::json!({ "tag_name": "nightly" }),
            serde_json::json!({ "tag_name": "v0.2.0-rc.1" }),
            serde_json::json!("just a string"),
        ] {
            assert!(
                matches!(
                    release_from_payload(&payload),
                    Err(CheckError::Malformed(_))
                ),
                "{payload} must be rejected"
            );
        }
    }

    #[test]
    fn a_disabled_checker_reports_nothing_but_the_running_version() {
        let checker = UpdateChecker::disabled();
        assert_eq!(
            checker.status(),
            UpdateStatus {
                current: CURRENT_VERSION,
                latest: None,
                release_url: None,
                update_available: false,
                checked_at: None,
                enabled: false,
            }
        );
    }

    #[test]
    fn an_enabled_checker_with_no_result_yet_is_quiet() {
        let status = UpdateChecker::new(true).status();
        assert!(status.enabled);
        assert!(!status.update_available);
        assert_eq!(status.latest, None);
        assert_eq!(status.checked_at, None);
    }

    #[test]
    fn a_recorded_release_drives_the_answer() {
        let checker = UpdateChecker::new(true);
        let at = Utc::now();
        checker.record(
            Release {
                version: "99.0.0".into(),
                url: RELEASES_LATEST_URL.into(),
            },
            at,
        );
        let status = checker.status_for("0.1.0");
        assert!(status.update_available);
        assert_eq!(status.latest.as_deref(), Some("99.0.0"));
        assert_eq!(status.release_url.as_deref(), Some(RELEASES_LATEST_URL));
        assert_eq!(status.checked_at, Some(at));

        // the same release seen from a build that already ships it
        let status = checker.status_for("99.0.0");
        assert!(!status.update_available);
        assert_eq!(status.latest.as_deref(), Some("99.0.0"));
    }

    #[tokio::test]
    async fn a_disabled_checker_does_not_spawn_a_loop() {
        // nothing observable happens; the point is that this returns at once
        // and never touches the network
        let checker = UpdateChecker::disabled();
        checker.spawn(reqwest::Client::new());
        assert!(!checker.status().enabled);
    }

    #[tokio::test]
    async fn an_unreachable_endpoint_leaves_the_status_untouched() {
        // a closed local port fails fast and stands in for "offline"
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        let client = reqwest::Client::builder()
            .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).expect("proxy"))
            .build()
            .expect("client");
        let checker = UpdateChecker::new(true);
        checker.check_once(&client).await;
        let status = checker.status();
        assert!(status.enabled);
        assert_eq!(status.latest, None);
        assert_eq!(status.checked_at, None);
        assert!(!status.update_available);
    }
}
