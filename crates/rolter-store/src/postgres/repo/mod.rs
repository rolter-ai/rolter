//! Thin CRUD repositories over the `postgres` feature's schema. Each
//! repository wraps a [`sqlx::PgPool`] and exposes `list`/`get`/`create`/`delete`
//! (plus narrow `update`s where a resource has mutable fields worth editing
//! independently). Domain interpretation of row contents (e.g. balancer
//! strategy parsing) is left to callers; see [`super::PostgresConfigStore`]
//! for the read path the gateway uses.
//!
//! Each domain lives in its own submodule and is re-exported here, so a caller
//! writes `postgres::repo::McpServerRepo` regardless of which file it is in (#1042).

mod access;
mod api_tokens;
mod audit;
mod auth;
mod billing;
mod cluster;
mod guardrails;
mod keys;
mod labels;
mod ldap;
mod lockout;
mod log_erasures;
mod mcp;
mod membership;
mod mfa;
mod prompts;
mod providers;
mod routing;
mod saved_views;
mod scim;
mod settings;
mod sso;
pub(super) mod support;
mod tenancy;
#[cfg(test)]
mod tests;
mod users;

pub use access::*;
pub use api_tokens::*;
pub use audit::*;
pub use auth::*;
pub use billing::*;
pub use cluster::*;
pub use guardrails::*;
pub use keys::*;
pub use labels::*;
pub use ldap::*;
pub use lockout::*;
pub use log_erasures::*;
pub use mcp::*;
pub use membership::*;
pub use mfa::*;
pub use prompts::*;
pub use providers::*;
pub use routing::*;
pub use saved_views::*;
pub use scim::*;
pub use settings::*;
pub use sso::*;
pub use tenancy::*;
pub use users::*;
