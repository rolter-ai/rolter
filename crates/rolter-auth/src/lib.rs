//! Authentication and authorization primitives for rolter.
//!
//! The MVP covers virtual-key verification and the role model used by the
//! control plane. OAuth2/OIDC and LDAP providers implement a pluggable
//! `IdentityProvider` trait added in a later phase (see ROADMAP).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// RBAC role, scoped to an org/team/project by the control plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Admin,
    Member,
    Viewer,
}

impl Role {
    /// Whether the role may mutate configuration.
    pub fn can_write(&self) -> bool {
        matches!(self, Role::Admin | Role::Member)
    }

    /// Whether the role has administrative privileges.
    pub fn is_admin(&self) -> bool {
        matches!(self, Role::Admin)
    }
}

/// Compare a presented key against the expected key in constant time.
///
/// Returns `false` immediately on length mismatch; equal-length inputs are
/// compared without short-circuiting to avoid timing side channels.
pub fn verify_key(presented: &str, expected: &str) -> bool {
    let a = presented.as_bytes();
    let b = expected.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

/// Derive the peppered lookup digest for a virtual key.
///
/// Returns the lowercase-hex SHA-256 of `pepper || 0x1f || key`. The digest is
/// deterministic, so it can key an in-memory map or a database column without
/// the plaintext key ever being stored. The pepper is a deployment-wide secret
/// (env/config): the same key under different peppers yields different digests,
/// so a leaked hash cannot be matched without it. An empty pepper still hashes
/// the key, which keeps plaintext out of memory even when no secret is set.
pub fn hash_key(pepper: &str, key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(pepper.as_bytes());
    hasher.update([0x1f]); // domain separator between pepper and key
    hasher.update(key.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap());
    }
    out
}

/// Whether `model` is permitted by an allow-list. An empty list allows all.
pub fn model_allowed(allowed: &[String], model: &str) -> bool {
    allowed.is_empty() || allowed.iter().any(|m| m == model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_matching_key() {
        assert!(verify_key("sk-abc", "sk-abc"));
        assert!(!verify_key("sk-abc", "sk-abd"));
        assert!(!verify_key("short", "longer-key"));
    }

    #[test]
    fn hash_key_is_deterministic_and_peppered() {
        let a = hash_key("pepper", "sk-abc");
        assert_eq!(a, hash_key("pepper", "sk-abc"));
        // different pepper => different digest
        assert_ne!(a, hash_key("other", "sk-abc"));
        // different key => different digest
        assert_ne!(a, hash_key("pepper", "sk-abd"));
        // sha-256 hex is 64 chars, all lowercase hex
        assert_eq!(a.len(), 64);
        assert!(a
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn allow_list_semantics() {
        assert!(model_allowed(&[], "anything"));
        assert!(model_allowed(&["gpt-4o".to_string()], "gpt-4o"));
        assert!(!model_allowed(&["gpt-4o".to_string()], "claude"));
    }

    #[test]
    fn role_capabilities() {
        assert!(Role::Admin.is_admin());
        assert!(Role::Member.can_write());
        assert!(!Role::Viewer.can_write());
    }
}
