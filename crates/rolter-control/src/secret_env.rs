//! Which environment variables an operator may name as an outbound credential.
//!
//! A provider's `api_key_env` and a guardrail webhook's `auth_env` are read
//! from the control plane's own environment when a test runs, and the url they
//! are sent to is the operator's to choose. Without a refusal, naming
//! `ROLTER_KEK` and pointing the url at a host of one's own sends the control
//! plane's secret out. Every path that reads a caller-named variable asks
//! [`refuse`] first, so the list lives in one place.

use crate::crud::{invalid_field, ApiError};

/// Variables the control plane runs on that carry no `ROLTER_` prefix, or that
/// are named explicitly so the list reads as documentation.
const DENIED: &[&str] = &[
    "ROLTER_KEK",
    "ROLTER_KEK_OLD",
    "ROLTER_MASTER_KEY",
    "ROLTER_KEY_PEPPER",
    "ROLTER_SESSION_PEPPER",
    "ROLTER_ADMIN_TOKEN",
    "ROLTER_INTERNAL_TOKEN",
    "ROLTER_PLUGIN_TOKEN",
    "ROLTER_DATABASE_URL",
    "ROLTER_REDIS_URL",
    "DATABASE_URL",
    "REDIS_URL",
    "CLICKHOUSE_URL",
    "CLICKHOUSE_PASSWORD",
    "CLICKHOUSE_USER",
    "PGPASSWORD",
    "PGPASSFILE",
];

/// Every `ROLTER_` name configures the deployment, so none is a provider key.
const PREFIX: &str = "ROLTER_";

/// `ROLTER_` names that are outbound credentials by design: the documented
/// bearer token for an external guardrail service.
const ALLOWED: &[&str] = &["ROLTER_GUARDRAIL_TOKEN"];

/// Whether `name` is a variable the control plane's own secrets live in.
/// Case-insensitive, since a name that matches on one platform's environment
/// must not slip past on another's.
pub(crate) fn is_control_plane_secret(name: &str) -> bool {
    let name = name.trim();
    if ALLOWED.iter().any(|ok| ok.eq_ignore_ascii_case(name)) {
        return false;
    }
    if DENIED.iter().any(|bad| bad.eq_ignore_ascii_case(name)) {
        return true;
    }
    name.len() >= PREFIX.len() && name[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)
}

/// A 400 `invalid_field` on `field` when `name` is a control-plane secret.
/// `what` completes "… cannot be used to test …".
pub(crate) fn refuse(field: &str, name: &str, what: &str) -> Result<(), ApiError> {
    if is_control_plane_secret(name) {
        return Err(invalid_field(
            field,
            format!("{name} holds a control-plane secret and cannot be used {what}"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deployment_secrets_are_refused_whatever_the_case() {
        for name in [
            "ROLTER_KEK",
            "rolter_kek",
            " ROLTER_DATABASE_URL ",
            "DATABASE_URL",
            "ROLTER_TEST_SOMETHING_NEW",
            "ClickHouse_Password",
        ] {
            assert!(is_control_plane_secret(name), "{name}");
        }
    }

    #[test]
    fn ordinary_provider_keys_and_the_guardrail_token_pass() {
        for name in [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "AWS_BEARER_TOKEN_BEDROCK",
            "ROLTER_GUARDRAIL_TOKEN",
            "MY_ROLTER_KEY",
            "ROLTE",
        ] {
            assert!(!is_control_plane_secret(name), "{name}");
        }
    }
}
