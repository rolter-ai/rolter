//! The launcher's one-line "a newer rolter is available" notice (#901).
//!
//! Runs beside the command, never ahead of it: [`spawn`] returns at once and
//! the command starts regardless of what the network does. The answer is cached
//! in `~/.cache/rolter/update-check.json` for [`CACHE_INTERVAL`], so a
//! repeated invocation neither re-asks GitHub nor repeats the notice, and a
//! long-running `gateway`/`control`/`easy-up` prints it once. The control plane
//! runs its own checker for the dashboard's hint and never writes to stderr,
//! so `easy-up` — one process supervising both — still says it once.
//!
//! `ROLTER_UPDATE_CHECK=false` silences it. Offline, rate-limited and malformed
//! answers all print nothing. The request carries only a `User-Agent`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use rolter_control::update_check::{
    enabled_from_env, fetch_latest, is_newer, CHECK_TIMEOUT, CURRENT_VERSION,
};

/// How long one answer is trusted before the launcher asks again.
pub const CACHE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// The cache file's name under the rolter cache directory.
pub const CACHE_FILE: &str = "update-check.json";

/// What one launcher run leaves behind for the next.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cache {
    /// unix seconds of the last *successful* fetch
    #[serde(default)]
    pub checked_at: Option<i64>,
    /// the latest stable release that fetch reported
    #[serde(default)]
    pub latest: Option<String>,
    /// its release page
    #[serde(default)]
    pub release_url: Option<String>,
    /// unix seconds of the last notice printed; a notice is printed once per
    /// fetch, so this at or after `checked_at` means "already said"
    #[serde(default)]
    pub notified_at: Option<i64>,
}

/// What to do with a cache at `now`.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// the last fetch is recent enough
    UseCache,
    /// no fetch yet, the interval has elapsed, or the clock went backwards
    Refresh,
}

/// Decide whether the cache is still fresh at `now` (unix seconds).
pub fn next_step(cache: &Cache, now: i64) -> Step {
    let interval = CACHE_INTERVAL.as_secs() as i64;
    match cache.checked_at {
        Some(at) if at <= now && now - at < interval => Step::UseCache,
        _ => Step::Refresh,
    }
}

/// The one-line notice, exactly as the issue words it.
pub fn format_notice(current: &str, latest: &str, url: &str) -> String {
    format!("rolter {current} is installed; {latest} is available: {url}")
}

/// The notice to print for `cache`, if the cached release is newer than
/// `current` and this fetch has not been announced yet.
pub fn notice(cache: &Cache, current: &str) -> Option<String> {
    let latest = cache.latest.as_deref()?;
    let checked_at = cache.checked_at?;
    if !is_newer(latest, current) {
        return None;
    }
    if cache.notified_at.is_some_and(|at| at >= checked_at) {
        return None;
    }
    let url = cache
        .release_url
        .as_deref()
        .unwrap_or(rolter_control::update_check::RELEASES_LATEST_URL);
    Some(format_notice(current, latest, url))
}

/// `$XDG_CACHE_HOME/rolter/update-check.json`, falling back to
/// `$HOME/.cache/rolter/…`; `None` when neither is set, in which case the
/// launcher simply does not check (no home, no cache, no network).
pub fn cache_path() -> Option<PathBuf> {
    let dir = match std::env::var_os("XDG_CACHE_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => {
            let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
            PathBuf::from(home).join(".cache")
        }
    };
    Some(dir.join("rolter").join(CACHE_FILE))
}

/// Read the cache; a missing or unreadable file is an empty cache.
pub fn load(path: &Path) -> Cache {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Write the cache atomically (temp file + rename) so a concurrent launcher
/// never reads half a document.
pub fn store(path: &Path, cache: &Cache) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("cache path has no parent"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{CACHE_FILE}.{}", std::process::id()));
    let bytes = serde_json::to_vec_pretty(cache).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// One launcher run: refresh the cache when it is stale, print the notice once
/// per fetch, and write back. Returns the notice it printed, for tests.
pub async fn run(client: &reqwest::Client, path: &Path, current: &str, now: i64) -> Option<String> {
    let mut cache = load(path);
    if next_step(&cache, now) == Step::Refresh {
        match fetch_latest(client).await {
            Ok(release) => {
                cache = Cache {
                    checked_at: Some(now),
                    latest: Some(release.version),
                    release_url: Some(release.url),
                    notified_at: cache.notified_at,
                };
            }
            Err(err) => tracing::debug!(error = %err, "update check failed; offline is fine"),
        }
    }
    let message = notice(&cache, current);
    if let Some(message) = &message {
        eprintln!("{message}");
        cache.notified_at = Some(now);
    }
    if let Err(err) = store(path, &cache) {
        tracing::debug!(error = %err, path = %path.display(), "could not write the update cache");
    }
    message
}

/// Start the check beside the command. Returns immediately; the command it
/// accompanies never waits on it, and an early exit simply drops the task.
pub fn spawn() {
    if !enabled_from_env() {
        return;
    }
    let Some(path) = cache_path() else {
        return;
    };
    let Ok(client) = reqwest::Client::builder().timeout(CHECK_TIMEOUT).build() else {
        return;
    };
    tokio::spawn(async move {
        run(&client, &path, CURRENT_VERSION, unix_now()).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 24 * 60 * 60;

    /// A client routed through a proxy nobody listens on: fails fast and
    /// stands in for "offline" without touching the real network.
    fn offline_client() -> reqwest::Client {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        reqwest::Client::builder()
            .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).expect("proxy"))
            .build()
            .expect("client")
    }

    fn cached(checked_at: i64, latest: &str) -> Cache {
        Cache {
            checked_at: Some(checked_at),
            latest: Some(latest.to_string()),
            release_url: Some("https://github.com/rolter-ai/rolter/releases/tag/v9.9.9".into()),
            notified_at: None,
        }
    }

    #[test]
    fn an_empty_cache_refreshes() {
        assert_eq!(next_step(&Cache::default(), 1_000), Step::Refresh);
    }

    #[test]
    fn a_recent_check_is_reused_until_the_interval_elapses() {
        let cache = cached(1_000, "9.9.9");
        assert_eq!(next_step(&cache, 1_000), Step::UseCache);
        assert_eq!(next_step(&cache, 1_000 + DAY - 1), Step::UseCache);
        assert_eq!(next_step(&cache, 1_000 + DAY), Step::Refresh);
        assert_eq!(next_step(&cache, 1_000 + 3 * DAY), Step::Refresh);
    }

    #[test]
    fn a_check_from_the_future_is_not_trusted() {
        // the clock went backwards; asking again beats trusting a stamp that
        // would otherwise stay "fresh" for however far ahead it was
        assert_eq!(next_step(&cached(5_000, "9.9.9"), 1_000), Step::Refresh);
    }

    #[test]
    fn a_newer_cached_release_is_announced_once_per_fetch() {
        let mut cache = cached(1_000, "9.9.9");
        let first = notice(&cache, "0.1.0").expect("newer");
        assert_eq!(
            first,
            "rolter 0.1.0 is installed; 9.9.9 is available: https://github.com/rolter-ai/rolter/releases/tag/v9.9.9"
        );
        cache.notified_at = Some(1_000);
        assert_eq!(notice(&cache, "0.1.0"), None, "same fetch, already said");
        // a later fetch of the same release says it again
        cache.checked_at = Some(1_000 + DAY);
        assert!(notice(&cache, "0.1.0").is_some());
    }

    #[test]
    fn the_current_or_a_newer_build_prints_nothing() {
        let cache = cached(1_000, "0.1.0");
        assert_eq!(notice(&cache, "0.1.0"), None);
        assert_eq!(notice(&cache, "0.2.0"), None);
        assert_eq!(
            notice(&cache, "0.1.0-rc.1"),
            Some(format_notice(
                "0.1.0-rc.1",
                "0.1.0",
                "https://github.com/rolter-ai/rolter/releases/tag/v9.9.9"
            )),
            "a release candidate is told about its release"
        );
    }

    #[test]
    fn a_cache_without_a_fetch_prints_nothing() {
        assert_eq!(notice(&Cache::default(), "0.1.0"), None);
        let cache = Cache {
            latest: Some("9.9.9".into()),
            ..Cache::default()
        };
        assert_eq!(notice(&cache, "0.1.0"), None);
    }

    #[test]
    fn a_missing_url_falls_back_to_the_releases_page() {
        let cache = Cache {
            release_url: None,
            ..cached(1_000, "9.9.9")
        };
        assert!(notice(&cache, "0.1.0")
            .expect("newer")
            .ends_with(rolter_control::update_check::RELEASES_LATEST_URL));
    }

    #[test]
    fn the_cache_round_trips_and_tolerates_garbage() {
        let dir = std::env::temp_dir().join(format!(
            "rolter-update-notice-{}-{}",
            std::process::id(),
            unix_now()
        ));
        let path = dir.join("nested").join(CACHE_FILE);
        assert_eq!(load(&path), Cache::default(), "missing file is empty");

        let cache = Cache {
            notified_at: Some(7),
            ..cached(1_000, "9.9.9")
        };
        store(&path, &cache).expect("store");
        assert_eq!(load(&path), cache);
        assert!(
            std::fs::read_dir(path.parent().unwrap())
                .unwrap()
                .all(|e| e.unwrap().file_name() == CACHE_FILE),
            "no temp file left behind"
        );

        std::fs::write(&path, b"{ not json").expect("write");
        assert_eq!(load(&path), Cache::default(), "garbage is empty");
        // an older document without a field the launcher grew later
        std::fs::write(&path, br#"{"checked_at": 5}"#).expect("write");
        assert_eq!(load(&path).checked_at, Some(5));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_cache_path_honours_xdg_then_home() {
        // env is process-global; the assertions read the value the function
        // derives rather than mutating what other tests may rely on
        match (
            std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()),
            std::env::var_os("HOME").filter(|v| !v.is_empty()),
        ) {
            (Some(xdg), _) => assert_eq!(
                cache_path(),
                Some(PathBuf::from(xdg).join("rolter").join(CACHE_FILE))
            ),
            (None, Some(home)) => assert_eq!(
                cache_path(),
                Some(
                    PathBuf::from(home)
                        .join(".cache")
                        .join("rolter")
                        .join(CACHE_FILE)
                )
            ),
            (None, None) => assert_eq!(cache_path(), None),
        }
    }

    /// Offline: the fetch fails, nothing is printed, and the cache does not
    /// gain a `checked_at` it did not earn — the next run asks again.
    #[tokio::test]
    async fn an_unreachable_endpoint_prints_nothing_and_keeps_the_cache_stale() {
        let dir = std::env::temp_dir().join(format!(
            "rolter-update-notice-offline-{}-{}",
            std::process::id(),
            unix_now()
        ));
        let path = dir.join(CACHE_FILE);
        let printed = run(&offline_client(), &path, "0.1.0", 1_000).await;
        assert_eq!(printed, None);
        assert_eq!(load(&path).checked_at, None);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A fresh cache is served without any network at all.
    #[tokio::test]
    async fn a_fresh_cache_is_announced_without_a_fetch() {
        let dir = std::env::temp_dir().join(format!(
            "rolter-update-notice-fresh-{}-{}",
            std::process::id(),
            unix_now()
        ));
        let path = dir.join(CACHE_FILE);
        store(&path, &cached(1_000, "9.9.9")).expect("store");
        let client = offline_client();
        let printed = run(&client, &path, "0.1.0", 1_000 + 60).await;
        assert!(printed.is_some_and(|m| m.contains("9.9.9 is available")));
        let after = load(&path);
        assert_eq!(after.checked_at, Some(1_000), "no fetch happened");
        assert_eq!(after.notified_at, Some(1_060));
        // the second run inside the interval is silent
        assert_eq!(run(&client, &path, "0.1.0", 1_000 + 120).await, None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
