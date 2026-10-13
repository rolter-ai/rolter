//! Which environment variables a config row may name as an outbound credential.
//!
//! A provider's `api_key_env` or a webhook's token variable is read from the
//! process environment of whichever binary serves the row, and the url it is
//! sent to is the operator's to choose. Without a refusal, naming `ROLTER_KEK`
//! and pointing the url at a host of one's own sends the deployment's secret
//! out. The control plane refuses at save time; the gateway refuses again when
//! it resolves a credential, because a row saved before the rule, or imported
//! by `rolter-seed`, never passed the save-time check. One predicate serves
//! both so the list lives in one place.

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

/// Whether `name` is a variable the deployment's own secrets live in.
/// Case-insensitive, since a name that matches on one platform's environment
/// must not slip past on another's.
pub fn is_control_plane_secret(name: &str) -> bool {
    let name = name.trim();
    if ALLOWED.iter().any(|ok| ok.eq_ignore_ascii_case(name)) {
        return false;
    }
    if DENIED.iter().any(|bad| bad.eq_ignore_ascii_case(name)) {
        return true;
    }
    name.len() >= PREFIX.len() && name[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)
}

/// [`is_control_plane_secret`] for a purpose that documents one more `ROLTER_`
/// variable as its own credential (the plugin bearer token), named in `allowed`.
pub fn is_secret_unless_allowed(name: &str, allowed: &[&str]) -> bool {
    let name = name.trim();
    !allowed.iter().any(|ok| ok.eq_ignore_ascii_case(name)) && is_control_plane_secret(name)
}

/// Read `name` from the environment unless it is a deployment secret.
/// `allowed` is the extra documented variables of the caller's purpose.
pub fn credential_from_env(name: &str, allowed: &[&str]) -> Option<String> {
    if is_secret_unless_allowed(name, allowed) {
        return None;
    }
    std::env::var(name).ok()
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

    #[test]
    fn a_webhook_secret_is_read_unless_the_variable_is_a_deployment_secret() {
        use crate::guardrail_webhook::WebhookAuth;
        // SAFETY: test-local env vars, no concurrent access to these keys elsewhere
        unsafe {
            std::env::set_var("ROLTER_TEST_3097_HOOK", "refused");
            std::env::set_var("ROLTER_TEST_3097_PLUGIN", "allowed");
            std::env::set_var("HOOK_TEST_3097_TOKEN", "plain");
        }
        let bearer = |name: &str| WebhookAuth::Bearer {
            token_env: name.to_string(),
        };
        assert_eq!(bearer("ROLTER_TEST_3097_HOOK").resolve_secret(&[]), None);
        assert_eq!(
            bearer("ROLTER_TEST_3097_PLUGIN")
                .resolve_secret(&["rolter_test_3097_plugin"])
                .as_deref(),
            Some("allowed")
        );
        assert_eq!(
            bearer("HOOK_TEST_3097_TOKEN")
                .resolve_secret(&[])
                .as_deref(),
            Some("plain")
        );
        // SAFETY: cleanup of the test-local vars set above
        unsafe {
            std::env::remove_var("ROLTER_TEST_3097_HOOK");
            std::env::remove_var("ROLTER_TEST_3097_PLUGIN");
            std::env::remove_var("HOOK_TEST_3097_TOKEN");
        }
    }
}
