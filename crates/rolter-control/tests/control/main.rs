//! End-to-end control-plane integration tests against a real Postgres.
//!
//! Gated on the `postgres` feature and the `ROLTER_TEST_DATABASE_URL` env var:
//! when the var is unset (local runs without a database) the tests self-skip.
//! CI provides a Postgres service and the var, so they run there.
#![cfg(feature = "postgres")]

macro_rules! skip_without_db {
    () => {
        if !rolter_store::postgres::test_database::is_configured() {
            eprintln!(
                "skipping: {} not set",
                rolter_store::postgres::test_database::URL_ENV
            );
            return;
        }
    };
}

mod alert_scopes;
mod analytics_export;
mod api_tokens;
mod audit;
mod auth;
mod batch_reads;
mod breaker_state;
mod budgets;
mod cluster_summary;
mod crud;
mod guardrails;
mod invitation_mail;
mod key_rotation;
mod ldap;
mod log_erasure;
mod mcp;
mod mcp_discovery;
mod mcp_enforcement;
mod mcp_ingest;
mod mcp_probe;
mod mcp_scopes;
mod profile;
mod rbac;
mod scim;
mod settings;
mod snapshot;
mod sso;
mod support;
mod users;
mod variants;
