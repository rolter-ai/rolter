//! Authentication and authorization primitives for rolter.
//!
//! The MVP covers virtual-key verification and the role model used by the
//! control plane. OAuth2/OIDC and LDAP providers implement a pluggable
//! `IdentityProvider` trait added in a later phase (see ROADMAP).

use serde::{Deserialize, Serialize};
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
