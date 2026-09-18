//! Stable identity for one rolter process (#543, #1644).
//!
//! Three subsystems have to agree on the answer to "which node is this": the
//! cluster inventory the gateway heartbeats into on every snapshot poll, the
//! adaptive-routing telemetry it posts beside it, and the
//! `service.instance.id` resource attribute on every exported span. Two
//! different answers would be worse than one missing answer, so the resolution
//! lives here once and each caller reads it rather than re-deriving it.
//!
//! The identity is also what the control plane keys those rows on. A node that
//! cannot name itself is dropped by the ingest, so resolution deliberately
//! ends in a value the control plane will accept — bounded and free of control
//! characters — or in nothing at all, never in something that looks resolved
//! and is discarded on arrival.

use std::sync::OnceLock;

/// Longest id the control plane's ingest accepts; anything longer is dropped
/// on arrival, so it is rejected here instead of being reported as resolved.
const MAX_LEN: usize = 128;

/// Where a resolved identity came from, so the boot log can say why this node
/// is called what it is called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeIdSource {
    /// `ROLTER_NODE_ID` — the deployment's own name for the replica
    Configured,
    /// `HOSTNAME` — set by every container runtime, by almost no shell
    HostnameEnv,
    /// the `gethostname` syscall, which a shell-launched process does have
    /// even though it has no `HOSTNAME` in its environment
    HostnameSyscall,
}

impl NodeIdSource {
    /// Short label for structured logs.
    pub fn as_str(self) -> &'static str {
        match self {
            NodeIdSource::Configured => "ROLTER_NODE_ID",
            NodeIdSource::HostnameEnv => "HOSTNAME",
            NodeIdSource::HostnameSyscall => "hostname",
        }
    }
}

/// This process's identity and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeIdentity {
    pub id: String,
    pub source: NodeIdSource,
}

/// This process's stable identity, resolved once and cached.
///
/// Precedence is `ROLTER_NODE_ID`, then `HOSTNAME`, then the host's own name
/// from `gethostname`. The syscall is the tail of the list rather than the
/// head so an operator who names a replica keeps that name, and it exists at
/// all because a gateway started from a shell, a systemd unit or a launchd job
/// has no `HOSTNAME` in its environment and used to fall off both the cluster
/// inventory and the adaptive-routing scoreboard without a word (#1644).
///
/// Nothing is invented when even the syscall fails: a per-restart id would
/// churn the inventory on every deploy, which is worse than an absent row.
pub fn node_identity() -> Option<&'static NodeIdentity> {
    static IDENTITY: OnceLock<Option<NodeIdentity>> = OnceLock::new();
    IDENTITY
        .get_or_init(|| resolve(env_var, system_hostname))
        .as_ref()
}

/// This process's identity as a plain string, for callers that only need the
/// value.
pub fn node_id() -> Option<String> {
    node_identity().map(|identity| identity.id.clone())
}

/// Resolution with its two environment lookups injected, so the precedence and
/// the sanitizing can be tested without mutating the process environment.
fn resolve(
    env: impl Fn(&str) -> Option<String>,
    hostname: impl Fn() -> Option<String>,
) -> Option<NodeIdentity> {
    let candidates = [
        (NodeIdSource::Configured, env("ROLTER_NODE_ID")),
        (NodeIdSource::HostnameEnv, env("HOSTNAME")),
        (NodeIdSource::HostnameSyscall, hostname()),
    ];
    candidates
        .into_iter()
        .find_map(|(source, value)| sanitize(value?).map(|id| NodeIdentity { id, source }))
}

/// Trim and accept, or reject outright. Mirrors the control plane's own header
/// check so a value that would be dropped on arrival never counts as resolved.
fn sanitize(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_LEN || trimmed.chars().any(char::is_control) {
        return None;
    }
    Some(trimmed.to_string())
}

fn env_var(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// The host's own name. `gethostname` is the only portable way to ask, and
/// there is no std wrapper for it.
#[cfg(unix)]
fn system_hostname() -> Option<String> {
    // posix caps a hostname at 255 bytes; the extra byte is the terminator
    let mut buffer = vec![0_u8; 256];
    // safety: the pointer and length describe `buffer`, which outlives the
    // call, and the buffer is one byte longer than the largest name posix
    // allows, so the result is always nul-terminated
    let rc = unsafe { libc::gethostname(buffer.as_mut_ptr().cast::<libc::c_char>(), buffer.len()) };
    if rc != 0 {
        return None;
    }
    let end = buffer.iter().position(|byte| *byte == 0)?;
    buffer.truncate(end);
    String::from_utf8(buffer).ok()
}

#[cfg(not(unix))]
fn system_hostname() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_map(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |key| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    #[test]
    fn a_configured_id_wins_over_both_hostnames() {
        let identity = resolve(
            env_map(&[("ROLTER_NODE_ID", "gw-1"), ("HOSTNAME", "container")]),
            || Some("laptop".to_string()),
        )
        .expect("an identity");
        assert_eq!(identity.id, "gw-1");
        assert_eq!(identity.source, NodeIdSource::Configured);
    }

    #[test]
    fn the_hostname_env_wins_over_the_syscall() {
        let identity = resolve(env_map(&[("HOSTNAME", "container")]), || {
            Some("laptop".to_string())
        })
        .expect("an identity");
        assert_eq!(identity.id, "container");
        assert_eq!(identity.source, NodeIdSource::HostnameEnv);
    }

    #[test]
    fn a_shell_launched_process_falls_back_to_the_syscall() {
        let identity = resolve(env_map(&[]), || Some("laptop".to_string())).expect("an identity");
        assert_eq!(identity.id, "laptop");
        assert_eq!(identity.source, NodeIdSource::HostnameSyscall);
    }

    #[test]
    fn an_empty_or_blank_value_falls_through_to_the_next_source() {
        let identity = resolve(
            env_map(&[("ROLTER_NODE_ID", "  "), ("HOSTNAME", "")]),
            || Some("laptop".to_string()),
        )
        .expect("an identity");
        assert_eq!(identity.id, "laptop");
        assert_eq!(identity.source, NodeIdSource::HostnameSyscall);
    }

    #[test]
    fn a_value_is_trimmed_rather_than_sent_with_its_whitespace() {
        let identity =
            resolve(env_map(&[("ROLTER_NODE_ID", " gw-1\n")]), || None).expect("an identity");
        assert_eq!(identity.id, "gw-1");
    }

    #[test]
    fn a_value_the_control_plane_would_drop_is_not_resolved() {
        let too_long = "n".repeat(MAX_LEN + 1);
        assert_eq!(
            resolve(env_map(&[("ROLTER_NODE_ID", too_long.as_str())]), || None),
            None
        );
        assert_eq!(
            resolve(env_map(&[("ROLTER_NODE_ID", "gw\u{7}1")]), || None),
            None
        );
    }

    #[test]
    fn nothing_is_invented_when_no_source_answers() {
        assert_eq!(resolve(env_map(&[]), || None), None);
    }

    #[test]
    #[cfg(unix)]
    fn the_syscall_answers_on_this_host() {
        let name = system_hostname().expect("a hostname");
        assert!(!name.is_empty());
        assert!(!name.contains('\0'));
    }
}
