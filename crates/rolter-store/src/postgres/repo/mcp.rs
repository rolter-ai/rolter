//! MCP server registry, tool groups, gateway defaults and the OAuth consent /
//! token store.
//!
//! Everything token-shaped in here is sealed with the deployment KEK before it
//! is written; see [`McpOAuthRepo`] for which methods are allowed to open it.

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::{Error, Result};

use super::super::models::{
    McpGatewaySettings, McpLoginState, McpOAuthGrant, McpOAuthSession, McpServer, McpToolGroup,
};
use super::support::{fetch_optional_or_not_found, require_affected, store_err};

/// MCP servers an org has registered.
pub struct McpServerRepo<'a>(pub &'a PgPool);

/// Columns of `mcp_servers` that may leave the store. The sealed client secret
/// is absent by construction: `has_client_secret` is projected instead, so no
/// query in this file can hand the ciphertext to a serializer by accident.
const MCP_SERVER_COLUMNS: &str = "id, org_id, name, slug, url, transport, description, enabled, \
     tools, source, required_scopes, created_at, \
     authorize_url, token_url, client_id, default_scopes, \
     auth_kind, auth_header_name, connect_timeout_ms, request_timeout_ms, max_retries, \
     oauth_issuer, oauth_discovery, oauth_discovered_issuer, oauth_discovered_authorize_url, \
     oauth_discovered_token_url, oauth_discovered_iss_supported, oauth_discovered_at, \
     (client_secret_ciphertext is not null) as has_client_secret, \
     (credential_ciphertext is not null) as has_credential";

/// The immutable identity plus initial settings of a server being registered.
/// A struct rather than ten positional arguments so a caller cannot silently
/// transpose `slug` and `url`.
#[derive(Debug, Clone, Copy)]
pub struct NewMcpServer<'a> {
    pub org_id: Uuid,
    pub name: &'a str,
    pub slug: &'a str,
    pub url: &'a str,
    pub transport: &'a str,
    pub description: &'a str,
    pub enabled: bool,
    pub tools: &'a [String],
    pub source: &'a str,
    pub required_scopes: &'a [String],
}

/// The editable fields of a registered server. `slug` and `source` are absent
/// on purpose: both are identity, not configuration.
#[derive(Debug, Clone, Copy)]
pub struct McpServerUpdate<'a> {
    pub name: &'a str,
    pub url: &'a str,
    pub transport: &'a str,
    pub description: &'a str,
    pub enabled: bool,
    pub tools: &'a [String],
    pub required_scopes: &'a [String],
    /// per-server overrides of `mcp_gateway_settings`; `None` inherits
    pub connect_timeout_ms: Option<i32>,
    pub request_timeout_ms: Option<i32>,
    pub max_retries: Option<i32>,
}

/// How rolter authenticates to a server, as an operator is setting it.
///
/// `credential` follows the same three shapes as the OAuth client secret
/// above, so an edit that is not changing the secret does not have to re-send
/// it: `None` leaves what is stored, `Some("")` clears it, and any other
/// `Some` replaces it. Moving to a kind that carries no credential clears it
/// regardless — the `mcp_servers_auth_kind_shape` constraint would refuse the
/// row otherwise, and silently keeping a credential a server no longer uses is
/// exactly the kind of orphaned secret `rolter kek verify` exists to catch.
#[derive(Debug, Clone, Copy)]
pub struct McpAuthConfig<'a> {
    pub auth_kind: &'a str,
    pub auth_header_name: Option<&'a str>,
    pub credential: Option<&'a str>,
}

/// The OAuth client rolter presents to a server's authorization server.
///
/// The endpoints are optional since #1347: a server whose authorization server
/// publishes RFC 9728 protected resource metadata needs no hand-configured
/// endpoint at all, and one that publishes none keeps them as the fallback.
#[derive(Debug, Clone, Copy)]
pub struct McpOAuthClient<'a> {
    pub authorize_url: Option<&'a str>,
    pub token_url: Option<&'a str>,
    pub client_id: &'a str,
    /// the authorization server's issuer identifier, pinned by hand for a
    /// server that publishes no metadata. RFC 9207 issuer validation has
    /// nothing to compare a returned `iss` against without one
    pub issuer: Option<&'a str>,
    /// `auto` or `manual`; see [`McpServer::oauth_discovery`]
    ///
    /// [`McpServer::oauth_discovery`]: super::super::models::McpServer::oauth_discovery
    pub discovery: &'a str,
    /// `None` leaves the stored secret alone, `Some("")` clears it, which is
    /// how a confidential client is downgraded to a public one.
    pub client_secret: Option<&'a str>,
    pub default_scopes: &'a [String],
}

/// What discovery resolved for a server, cached on its row so the background
/// refresher and the token exchange never have to probe an upstream.
#[derive(Debug, Clone, Copy)]
pub struct McpDiscoveredEndpoints<'a> {
    pub issuer: &'a str,
    pub authorize_url: &'a str,
    pub token_url: &'a str,
    pub iss_supported: bool,
}

/// Assignments that drop the discovery cache when, and only when, the update
/// is actually moving the server's URL (#1416).
///
/// The cache is keyed by the server's canonical resource identifier, which is
/// derived from `url`; pointing the row at a different server leaves endpoints
/// belonging to the old one behind. A refresh landing between the edit and the
/// next interactive authorize would then post to the *old* authorization
/// server's token endpoint while naming the *new* canonical URI in the RFC 8707
/// `resource` parameter.
///
/// `$3` is the new URL, and the bare `url` on the right-hand side is the row's
/// value before this statement — Postgres evaluates every `set` expression
/// against the old row, so one statement can both write the URL and compare
/// against what it replaced. Folding this into the existing update rather than
/// issuing a second one is deliberate: `mcp_servers` carries a
/// statement-level `bump_config_version()` trigger, so a separate clearing
/// statement would bump the snapshot version twice for one logical edit.
const CLEAR_DISCOVERY_ON_URL_CHANGE: &str = "\
    oauth_discovered_issuer = case when url is distinct from $3 \
        then null else oauth_discovered_issuer end, \
    oauth_discovered_authorize_url = case when url is distinct from $3 \
        then null else oauth_discovered_authorize_url end, \
    oauth_discovered_token_url = case when url is distinct from $3 \
        then null else oauth_discovered_token_url end, \
    oauth_discovered_iss_supported = case when url is distinct from $3 \
        then false else oauth_discovered_iss_supported end, \
    oauth_discovered_at = case when url is distinct from $3 \
        then null else oauth_discovered_at end";

impl McpServerRepo<'_> {
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<McpServer>> {
        sqlx::query_as(&format!(
            "select {MCP_SERVER_COLUMNS} from mcp_servers where org_id = $1 order by name"
        ))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<McpServer> {
        fetch_optional_or_not_found(
            sqlx::query_as(&format!(
                "select {MCP_SERVER_COLUMNS} from mcp_servers where id = $1"
            ))
            .bind(id),
            self.0,
            || format!("mcp server {id}"),
        )
        .await
    }

    pub async fn create(&self, server: NewMcpServer<'_>) -> Result<McpServer> {
        sqlx::query_as(&format!(
            "insert into mcp_servers \
             (org_id, name, slug, url, transport, description, enabled, tools, source, required_scopes) \
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
             returning {MCP_SERVER_COLUMNS}"
        ))
        .bind(server.org_id)
        .bind(server.name)
        .bind(server.slug)
        .bind(server.url)
        .bind(server.transport)
        .bind(server.description)
        .bind(server.enabled)
        .bind(server.tools)
        .bind(server.source)
        .bind(server.required_scopes)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Apply an operator's edit, invalidating the OAuth discovery cache when
    /// the edit moves the server's URL — the cached endpoints belong to the
    /// authorization server the old URL named, so a refresh before the next
    /// interactive authorize would otherwise use them (#1416).
    ///
    /// The clearing rides on this statement rather than following it because
    /// `mcp_servers` carries a statement-level `bump_config_version()` trigger;
    /// see `CLEAR_DISCOVERY_ON_URL_CHANGE` above for the whole argument.
    pub async fn update(&self, id: Uuid, server: McpServerUpdate<'_>) -> Result<McpServer> {
        fetch_optional_or_not_found(
            sqlx::query_as(&format!(
                "update mcp_servers set name = $2, url = $3, transport = $4, description = $5, \
                 enabled = $6, tools = $7, required_scopes = $8, connect_timeout_ms = $9, \
                 request_timeout_ms = $10, max_retries = $11, \
                 {CLEAR_DISCOVERY_ON_URL_CHANGE} \
                 where id = $1 returning {MCP_SERVER_COLUMNS}"
            ))
            .bind(id)
            .bind(server.name)
            .bind(server.url)
            .bind(server.transport)
            .bind(server.description)
            .bind(server.enabled)
            .bind(server.tools)
            .bind(server.required_scopes)
            .bind(server.connect_timeout_ms)
            .bind(server.request_timeout_ms)
            .bind(server.max_retries),
            self.0,
            || format!("mcp server {id}"),
        )
        .await
    }

    /// Set how rolter authenticates to this server, sealing the credential
    /// before it is written.
    ///
    /// The `case when` on the credential columns is what lets `None` mean
    /// "leave it": binding the existing ciphertext back would need it read out
    /// first, and reading a sealed credential to write it again is a decrypt
    /// this path has no reason to perform.
    pub async fn set_auth(
        &self,
        kek: &super::super::crypto::Kek,
        id: Uuid,
        auth: McpAuthConfig<'_>,
    ) -> Result<McpServer> {
        let carries_credential = matches!(auth.auth_kind, "bearer" | "header");
        // same three shapes as set_oauth_client, plus: a kind that carries no
        // credential always clears, whatever the caller sent
        let sealed = match auth.credential {
            _ if !carries_credential => Some((None, None)),
            None => None,
            Some("") => Some((None, None)),
            Some(credential) => {
                let (c, n) = kek.encrypt(credential)?;
                Some((Some(c), Some(n)))
            }
        };
        let (touch, ciphertext, nonce) = match sealed {
            None => (false, None, None),
            Some((c, n)) => (true, c, n),
        };
        let header_name = if auth.auth_kind == "header" {
            auth.auth_header_name
        } else {
            None
        };
        fetch_optional_or_not_found(
            sqlx::query_as(&format!(
                "update mcp_servers set auth_kind = $2, auth_header_name = $3, \
                 credential_ciphertext = case when $4 then $5 else credential_ciphertext end, \
                 credential_nonce = case when $4 then $6 else credential_nonce end \
                 where id = $1 returning {MCP_SERVER_COLUMNS}"
            ))
            .bind(id)
            .bind(auth.auth_kind)
            .bind(header_name)
            .bind(touch)
            .bind(ciphertext)
            .bind(nonce),
            self.0,
            || format!("mcp server {id}"),
        )
        .await
    }

    /// Open the sealed static credential for `id`, or `None` when the server
    /// stores none. The only caller that should reach for this is the code
    /// actually dialling the server: the value is returned as a bare `String`
    /// and never travels on a `Serialize` type.
    pub async fn credential(
        &self,
        kek: &super::super::crypto::Kek,
        id: Uuid,
    ) -> Result<Option<String>> {
        let row: Option<SealedSecretRow> = sqlx::query_as(
            "select credential_ciphertext, credential_nonce from mcp_servers where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        let Some((Some(ciphertext), Some(nonce))) = row else {
            return Ok(None);
        };
        Ok(Some(kek.decrypt(&ciphertext, &nonce)?))
    }

    /// Register (or replace) the OAuth client rolter presents to this server's
    /// authorization server. [`McpOAuthClient::client_secret`] is sealed before
    /// it is written.
    pub async fn set_oauth_client(
        &self,
        kek: &super::super::crypto::Kek,
        id: Uuid,
        client: McpOAuthClient<'_>,
    ) -> Result<McpServer> {
        // three shapes: leave the secret, clear it, or replace it
        let sealed = match client.client_secret {
            None => None,
            Some("") => Some((None, None)),
            Some(secret) => {
                let (c, n) = kek.encrypt(secret)?;
                Some((Some(c), Some(n)))
            }
        };
        let (touch_secret, ciphertext, nonce) = match sealed {
            None => (false, None, None),
            Some((c, n)) => (true, c, n),
        };
        fetch_optional_or_not_found(
            sqlx::query_as(&format!(
                "update mcp_servers set authorize_url = $2, token_url = $3, client_id = $4, \
                        default_scopes = $5, oauth_issuer = $9, oauth_discovery = $10, \
                        client_secret_ciphertext = case when $6 then $7 else client_secret_ciphertext end, \
                        client_secret_nonce = case when $6 then $8 else client_secret_nonce end \
                 where id = $1 returning {MCP_SERVER_COLUMNS}"
            ))
            .bind(id)
            .bind(client.authorize_url)
            .bind(client.token_url)
            .bind(client.client_id)
            .bind(client.default_scopes)
            .bind(touch_secret)
            .bind(ciphertext)
            .bind(nonce)
            .bind(client.issuer)
            .bind(client.discovery),
            self.0,
            || format!("mcp server {id}"),
        )
        .await
    }

    /// Cache what RFC 9728 discovery resolved for a server (#1347).
    ///
    /// The write is skipped when nothing changed. `mcp_servers` carries a
    /// statement-level `bump_config_version()` trigger, which fires per
    /// statement rather than per affected row, so an unconditional update here
    /// would make every consent start bump the snapshot version and wake every
    /// gateway for a value none of them read.
    pub async fn record_discovery(
        &self,
        id: Uuid,
        endpoints: McpDiscoveredEndpoints<'_>,
    ) -> Result<()> {
        let current: Option<DiscoveredRow> = sqlx::query_as(
            "select oauth_discovered_issuer, oauth_discovered_authorize_url, \
                        oauth_discovered_token_url, oauth_discovered_iss_supported \
                 from mcp_servers where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        let unchanged = current.is_some_and(|(issuer, authorize, token, iss_supported)| {
            issuer.as_deref() == Some(endpoints.issuer)
                && authorize.as_deref() == Some(endpoints.authorize_url)
                && token.as_deref() == Some(endpoints.token_url)
                && iss_supported == endpoints.iss_supported
        });
        if unchanged {
            return Ok(());
        }
        sqlx::query(
            "update mcp_servers set oauth_discovered_issuer = $2, \
                    oauth_discovered_authorize_url = $3, oauth_discovered_token_url = $4, \
                    oauth_discovered_iss_supported = $5, oauth_discovered_at = now() \
             where id = $1",
        )
        .bind(id)
        .bind(endpoints.issuer)
        .bind(endpoints.authorize_url)
        .bind(endpoints.token_url)
        .bind(endpoints.iss_supported)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(())
    }

    /// Open the sealed client secret for `id`, or `None` when the client is
    /// public. The only way ciphertext leaves this table.
    pub async fn client_secret(
        &self,
        kek: &super::super::crypto::Kek,
        id: Uuid,
    ) -> Result<Option<String>> {
        let row: Option<SealedSecretRow> = sqlx::query_as(
            "select client_secret_ciphertext, client_secret_nonce from mcp_servers where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        let Some((Some(ciphertext), Some(nonce))) = row else {
            return Ok(None);
        };
        Ok(Some(kek.decrypt(&ciphertext, &nonce)?))
    }

    /// Delete a server. Its grants and sessions cascade, which is the intended
    /// blast radius: removing the server withdraws access to it entirely.
    pub async fn delete(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from mcp_servers where id = $1").bind(id),
            self.0,
            || format!("mcp server {id}"),
        )
        .await
    }
}

/// Organization-owned tool-group policy manifests.
pub struct McpToolGroupRepo<'a>(pub &'a PgPool);

const MCP_TOOL_GROUP_COLUMNS: &str =
    "id, org_id, name, slug, description, enabled, tools, created_at, updated_at";

impl McpToolGroupRepo<'_> {
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<McpToolGroup>> {
        sqlx::query_as(&format!(
            "select {MCP_TOOL_GROUP_COLUMNS} from mcp_tool_groups \
             where org_id = $1 order by name"
        ))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<McpToolGroup> {
        fetch_optional_or_not_found(
            sqlx::query_as(&format!(
                "select {MCP_TOOL_GROUP_COLUMNS} from mcp_tool_groups where id = $1"
            ))
            .bind(id),
            self.0,
            || format!("mcp tool group {id}"),
        )
        .await
    }

    pub async fn create(
        &self,
        org_id: Uuid,
        name: &str,
        slug: &str,
        description: &str,
        enabled: bool,
        tools: &serde_json::Value,
    ) -> Result<McpToolGroup> {
        sqlx::query_as(&format!(
            "insert into mcp_tool_groups (org_id, name, slug, description, enabled, tools) \
             values ($1, $2, $3, $4, $5, $6) returning {MCP_TOOL_GROUP_COLUMNS}"
        ))
        .bind(org_id)
        .bind(name)
        .bind(slug)
        .bind(description)
        .bind(enabled)
        .bind(tools)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        enabled: bool,
        tools: &serde_json::Value,
    ) -> Result<McpToolGroup> {
        fetch_optional_or_not_found(
            sqlx::query_as(&format!(
                "update mcp_tool_groups set name = $2, description = $3, enabled = $4, \
                 tools = $5, updated_at = now() where id = $1 returning {MCP_TOOL_GROUP_COLUMNS}"
            ))
            .bind(id)
            .bind(name)
            .bind(description)
            .bind(enabled)
            .bind(tools),
            self.0,
            || format!("mcp tool group {id}"),
        )
        .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from mcp_tool_groups where id = $1").bind(id),
            self.0,
            || format!("mcp tool group {id}"),
        )
        .await
    }
}

/// Per-organization MCP gateway defaults.
pub struct McpGatewaySettingsRepo<'a>(pub &'a PgPool);

/// The writable half of [`McpGatewaySettings`]; `org_id` and `updated_at` stay
/// out because the repository owns both.
#[derive(Debug, Clone, Copy)]
pub struct McpGatewaySettingsUpdate<'a> {
    pub default_transport: &'a str,
    pub connect_timeout_ms: i32,
    pub request_timeout_ms: i32,
    pub max_retries: i32,
    pub default_failure_mode: &'a str,
    pub allow_unlisted_tools: bool,
}

impl McpGatewaySettingsRepo<'_> {
    pub async fn get(&self, org_id: Uuid) -> Result<McpGatewaySettings> {
        sqlx::query_as(
            "select org_id, default_transport, connect_timeout_ms, request_timeout_ms, \
             max_retries, default_failure_mode, allow_unlisted_tools, updated_at \
             from mcp_gateway_settings where org_id = $1",
        )
        .bind(org_id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
        .map(|stored| {
            stored.unwrap_or_else(|| McpGatewaySettings {
                org_id,
                default_transport: "streamable_http".to_string(),
                connect_timeout_ms: 5_000,
                request_timeout_ms: 30_000,
                max_retries: 1,
                default_failure_mode: "fail_closed".to_string(),
                allow_unlisted_tools: false,
                updated_at: Utc::now(),
            })
        })
    }

    pub async fn update(
        &self,
        org_id: Uuid,
        settings: McpGatewaySettingsUpdate<'_>,
    ) -> Result<McpGatewaySettings> {
        sqlx::query_as(
            "insert into mcp_gateway_settings \
             (org_id, default_transport, connect_timeout_ms, request_timeout_ms, max_retries, \
              default_failure_mode, allow_unlisted_tools, updated_at) \
             values ($1, $2, $3, $4, $5, $6, $7, now()) \
             on conflict (org_id) do update set default_transport = excluded.default_transport, \
             connect_timeout_ms = excluded.connect_timeout_ms, request_timeout_ms = excluded.request_timeout_ms, \
             max_retries = excluded.max_retries, default_failure_mode = excluded.default_failure_mode, \
             allow_unlisted_tools = excluded.allow_unlisted_tools, updated_at = now() \
             returning org_id, default_transport, connect_timeout_ms, request_timeout_ms, \
             max_retries, default_failure_mode, allow_unlisted_tools, updated_at",
        )
        .bind(org_id)
        .bind(settings.default_transport)
        .bind(settings.connect_timeout_ms)
        .bind(settings.request_timeout_ms)
        .bind(settings.max_retries)
        .bind(settings.default_failure_mode)
        .bind(settings.allow_unlisted_tools)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }
}

/// Sealed client-secret columns of an `mcp_servers` row.
type SealedSecretRow = (Option<Vec<u8>>, Option<Vec<u8>>);

/// The cached discovery columns of an `mcp_servers` row: `(issuer, authorize
/// url, token url, iss advertised)`.
type DiscoveredRow = (Option<String>, Option<String>, Option<String>, bool);

/// One `mcp_oauth_login_states` row as stored: `(state, server, user, verifier
/// ciphertext, verifier nonce, scopes, redirect uri, created at, expected
/// issuer, iss supported, resource, token url)`.
type SealedLoginRow = (
    String,
    Uuid,
    Uuid,
    Vec<u8>,
    Vec<u8>,
    Vec<String>,
    String,
    DateTime<Utc>,
    Option<String>,
    bool,
    Option<String>,
    Option<String>,
);

/// Refresh material of one session: `(grant, server, refresh ciphertext,
/// refresh nonce, scopes)`.
type SealedRefreshRow = (Uuid, Uuid, Option<Vec<u8>>, Option<Vec<u8>>, Vec<String>);

/// Who a session belongs to: `(session, grant, user, server, grant scopes,
/// session scopes)`.
type SessionContextRow = (Uuid, Uuid, Uuid, Uuid, Vec<String>, Vec<String>);

/// Sealed columns of one session row: `(access ciphertext, access nonce,
/// refresh ciphertext, refresh nonce, scopes)`.
type SealedSessionRow = (
    Vec<u8>,
    Vec<u8>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Vec<String>,
);

/// OAuth consent grants and token sessions for MCP servers.
///
/// Token material is sealed with the deployment KEK ([`super::super::crypto::Kek`])
/// before it is written and is only ever returned by [`Self::open_session`],
/// which the future MCP proxy calls on the request path. Every other method
/// returns metadata only, so an API handler cannot leak a token by reaching
/// for the wrong function.
pub struct McpOAuthRepo<'a>(pub &'a PgPool);

const GRANT_COLUMNS: &str = "id, server_id, user_id, scopes, granted_at, revoked_at, revoked_by";
const SESSION_COLUMNS: &str = "id, grant_id, scopes, expires_at, refresh_expires_at, revoked_at, \
     created_at, last_used_at, (refresh_ciphertext is not null) as has_refresh_token";

/// Token material for one session, shared by [`McpOAuthRepo::store_session`]
/// and [`McpOAuthRepo::rotate_session`] because storing and rotating write
/// exactly the same columns. Not `Serialize`: the tokens may not reach an API
/// response or a log line.
#[derive(Debug, Clone, Copy)]
pub struct McpSessionMaterial<'a> {
    pub access_token: &'a str,
    pub refresh_token: Option<&'a str>,
    pub scopes: &'a [String],
    pub expires_at: DateTime<Utc>,
    pub refresh_expires_at: Option<DateTime<Utc>>,
}

/// An in-flight consent about to be recorded. `code_verifier` is sealed by
/// [`McpOAuthRepo::start_login`] on the way in.
#[derive(Debug, Clone, Copy)]
pub struct NewMcpLogin<'a> {
    pub state: &'a str,
    pub server_id: Uuid,
    pub user_id: Uuid,
    pub code_verifier: &'a str,
    pub scopes: &'a [String],
    pub redirect_uri: &'a str,
    /// the issuer RFC 9207 §2.4 will judge the callback's `iss` against, and
    /// whether the authorization server advertised that it sends one (#1347)
    pub expected_issuer: Option<&'a str>,
    pub iss_supported: bool,
    /// the RFC 8707 canonical resource identifier this request carries; the
    /// token request must carry the identical one
    pub resource: &'a str,
    /// the token endpoint of the authorization server the browser was sent to
    pub token_url: &'a str,
}

impl McpOAuthRepo<'_> {
    /// Record (or refresh) a user's consent for a server. Re-consenting
    /// updates the scope set on the live grant rather than accumulating rows.
    pub async fn upsert_grant(
        &self,
        server_id: Uuid,
        user_id: Uuid,
        scopes: &[String],
    ) -> Result<McpOAuthGrant> {
        sqlx::query_as(&format!(
            "insert into mcp_oauth_grants (server_id, user_id, scopes) values ($1, $2, $3) \
             on conflict (server_id, user_id) where revoked_at is null \
             do update set scopes = excluded.scopes, granted_at = now() \
             returning {GRANT_COLUMNS}"
        ))
        .bind(server_id)
        .bind(user_id)
        .bind(scopes)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get_grant(&self, id: Uuid) -> Result<McpOAuthGrant> {
        fetch_optional_or_not_found(
            sqlx::query_as(&format!(
                "select {GRANT_COLUMNS} from mcp_oauth_grants where id = $1"
            ))
            .bind(id),
            self.0,
            || format!("mcp oauth grant {id}"),
        )
        .await
    }

    /// Grants across an org, newest first. `user_id` narrows the listing to one
    /// owner, which is how a non-admin caller sees only their own.
    pub async fn list_grants(
        &self,
        org_id: Uuid,
        user_id: Option<Uuid>,
    ) -> Result<Vec<McpOAuthGrant>> {
        sqlx::query_as(
            "select g.id, g.server_id, g.user_id, g.scopes, g.granted_at, g.revoked_at, \
                    g.revoked_by \
             from mcp_oauth_grants g join mcp_servers s on s.id = g.server_id \
             where s.org_id = $1 and ($2::uuid is null or g.user_id = $2) \
             order by g.granted_at desc",
        )
        .bind(org_id)
        .bind(user_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Revoke a grant and every session under it in one transaction, so a
    /// revoked consent can never leave a live token behind.
    pub async fn revoke_grant(&self, id: Uuid, revoked_by: Option<Uuid>) -> Result<McpOAuthGrant> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        let grant: McpOAuthGrant = fetch_optional_or_not_found(
            sqlx::query_as(&format!(
                "update mcp_oauth_grants set revoked_at = coalesce(revoked_at, now()), \
                        revoked_by = coalesce(revoked_by, $2) \
                 where id = $1 returning {GRANT_COLUMNS}"
            ))
            .bind(id)
            .bind(revoked_by),
            &mut *tx,
            || format!("mcp oauth grant {id}"),
        )
        .await?;
        sqlx::query(
            "update mcp_oauth_sessions set revoked_at = coalesce(revoked_at, now()) \
             where grant_id = $1",
        )
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;
        Ok(grant)
    }

    /// Seal and store a token session for a live grant.
    pub async fn store_session(
        &self,
        kek: &super::super::crypto::Kek,
        grant_id: Uuid,
        material: McpSessionMaterial<'_>,
    ) -> Result<McpOAuthSession> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        let grant_scopes: Option<Vec<String>> = sqlx::query_scalar(
            "select scopes from mcp_oauth_grants \
             where id = $1 and revoked_at is null for share",
        )
        .bind(grant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(store_err)?;
        let grant_scopes = grant_scopes
            .ok_or_else(|| Error::Config("MCP OAuth grant is revoked or missing".to_string()))?;
        if material
            .scopes
            .iter()
            .any(|scope| !grant_scopes.contains(scope))
        {
            return Err(Error::Config(
                "MCP OAuth session scopes exceed the consent grant".to_string(),
            ));
        }
        let (access_ciphertext, access_nonce) = kek.encrypt(material.access_token)?;
        let refresh = material.refresh_token.map(|t| kek.encrypt(t)).transpose()?;
        let (refresh_ciphertext, refresh_nonce) = match refresh {
            Some((c, n)) => (Some(c), Some(n)),
            None => (None, None),
        };
        let session = sqlx::query_as(&format!(
            "insert into mcp_oauth_sessions (grant_id, access_ciphertext, access_nonce, \
                    refresh_ciphertext, refresh_nonce, scopes, expires_at, refresh_expires_at) \
             values ($1, $2, $3, $4, $5, $6, $7, $8) returning {SESSION_COLUMNS}"
        ))
        .bind(grant_id)
        .bind(access_ciphertext)
        .bind(access_nonce)
        .bind(refresh_ciphertext)
        .bind(refresh_nonce)
        .bind(material.scopes)
        .bind(material.expires_at)
        .bind(material.refresh_expires_at)
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;
        Ok(session)
    }

    /// Sessions across an org, newest first; `user_id` narrows to one owner.
    pub async fn list_sessions(
        &self,
        org_id: Uuid,
        user_id: Option<Uuid>,
    ) -> Result<Vec<McpOAuthSession>> {
        sqlx::query_as(
            "select s.id, s.grant_id, s.scopes, s.expires_at, s.refresh_expires_at, \
                    s.revoked_at, s.created_at, s.last_used_at, \
                    (s.refresh_ciphertext is not null) as has_refresh_token \
             from mcp_oauth_sessions s \
             join mcp_oauth_grants g on g.id = s.grant_id \
             join mcp_servers srv on srv.id = g.server_id \
             where srv.org_id = $1 and ($2::uuid is null or g.user_id = $2) \
             order by s.created_at desc",
        )
        .bind(org_id)
        .bind(user_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get_session(&self, id: Uuid) -> Result<McpOAuthSession> {
        fetch_optional_or_not_found(
            sqlx::query_as(&format!(
                "select {SESSION_COLUMNS} from mcp_oauth_sessions where id = $1"
            ))
            .bind(id),
            self.0,
            || format!("mcp oauth session {id}"),
        )
        .await
    }

    pub async fn revoke_session(&self, id: Uuid) -> Result<McpOAuthSession> {
        fetch_optional_or_not_found(
            sqlx::query_as(&format!(
                "update mcp_oauth_sessions set revoked_at = coalesce(revoked_at, now()) \
                 where id = $1 returning {SESSION_COLUMNS}"
            ))
            .bind(id),
            self.0,
            || format!("mcp oauth session {id}"),
        )
        .await
    }

    /// Open the sealed tokens for a session that is live *and* whose grant is
    /// live. Returns `None` for a revoked or expired session, or one whose
    /// consent was withdrawn — the caller cannot accidentally use a token the
    /// user has taken back.
    pub async fn open_session(
        &self,
        kek: &super::super::crypto::Kek,
        id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Option<McpSessionTokens>> {
        let row: Option<SealedSessionRow> = sqlx::query_as(
            "select s.access_ciphertext, s.access_nonce, s.refresh_ciphertext, \
                        s.refresh_nonce, s.scopes \
                 from mcp_oauth_sessions s join mcp_oauth_grants g on g.id = s.grant_id \
                 where s.id = $1 and s.revoked_at is null and g.revoked_at is null \
                   and s.expires_at > $2",
        )
        .bind(id)
        .bind(now)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        let Some((access_c, access_n, refresh_c, refresh_n, scopes)) = row else {
            return Ok(None);
        };
        let access_token = kek.decrypt(&access_c, &access_n)?;
        let refresh_token = match (refresh_c, refresh_n) {
            (Some(c), Some(n)) => Some(kek.decrypt(&c, &n)?),
            _ => None,
        };
        Ok(Some(McpSessionTokens {
            access_token,
            refresh_token,
            scopes,
        }))
    }

    /// Stamp a session as used. Best-effort bookkeeping for the sessions
    /// screen; a failure must never fail the MCP call it belongs to.
    pub async fn touch_session(&self, id: Uuid) -> Result<()> {
        sqlx::query("update mcp_oauth_sessions set last_used_at = now() where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        Ok(())
    }

    // -- authorization-code exchange (#707) ---------------------------------

    /// Record an in-flight consent. The PKCE verifier is sealed on the way in,
    /// so a database read alone cannot redeem a stolen authorization code.
    pub async fn start_login(
        &self,
        kek: &super::super::crypto::Kek,
        login: NewMcpLogin<'_>,
    ) -> Result<()> {
        let (ciphertext, nonce) = kek.encrypt(login.code_verifier)?;
        sqlx::query(
            "insert into mcp_oauth_login_states \
                 (state, server_id, user_id, verifier_ciphertext, verifier_nonce, scopes, \
                  redirect_uri, expected_issuer, iss_supported, resource, token_url) \
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(login.state)
        .bind(login.server_id)
        .bind(login.user_id)
        .bind(ciphertext)
        .bind(nonce)
        .bind(login.scopes)
        .bind(login.redirect_uri)
        .bind(login.expected_issuer)
        .bind(login.iss_supported)
        .bind(login.resource)
        .bind(login.token_url)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(())
    }

    /// Consume an in-flight consent: the row is deleted as it is read, so a
    /// replayed `code`+`state` pair finds nothing. Rows older than `max_age`
    /// are treated as absent (and swept), which bounds how long a leaked
    /// `state` is worth anything.
    pub async fn consume_login(
        &self,
        kek: &super::super::crypto::Kek,
        state: &str,
        now: DateTime<Utc>,
        max_age: Duration,
    ) -> Result<Option<McpLoginState>> {
        let cutoff = now - max_age;
        // sweep first: expired rows are garbage whether or not this call
        // matches one, and this is the only traffic the table sees
        sqlx::query("delete from mcp_oauth_login_states where created_at < $1")
            .bind(cutoff)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        let row: Option<SealedLoginRow> = sqlx::query_as(
            "delete from mcp_oauth_login_states where state = $1 and created_at >= $2 \
                 returning state, server_id, user_id, verifier_ciphertext, verifier_nonce, \
                           scopes, redirect_uri, created_at, expected_issuer, iss_supported, \
                           resource, token_url",
        )
        .bind(state)
        .bind(cutoff)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        let Some((
            state,
            server_id,
            user_id,
            ciphertext,
            nonce,
            scopes,
            redirect_uri,
            created_at,
            expected_issuer,
            iss_supported,
            resource,
            token_url,
        )) = row
        else {
            return Ok(None);
        };
        Ok(Some(McpLoginState {
            state,
            server_id,
            user_id,
            code_verifier: kek.decrypt(&ciphertext, &nonce)?,
            scopes,
            redirect_uri,
            created_at,
            expected_issuer,
            iss_supported,
            resource,
            token_url,
        }))
    }

    /// Open the sealed *refresh* token of a session whose access token may
    /// already have expired — the one case [`Self::open_session`] deliberately
    /// refuses. Consent is still checked: a revoked session or a withdrawn
    /// grant yields `None`, as does an expired refresh token, so a renewal can
    /// never outlive the consent it hangs off.
    pub async fn open_refresh(
        &self,
        kek: &super::super::crypto::Kek,
        id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Option<McpRefreshMaterial>> {
        let row: Option<SealedRefreshRow> = sqlx::query_as(
            "select g.id, g.server_id, s.refresh_ciphertext, s.refresh_nonce, s.scopes \
                 from mcp_oauth_sessions s join mcp_oauth_grants g on g.id = s.grant_id \
                 where s.id = $1 and s.revoked_at is null and g.revoked_at is null \
                   and (s.refresh_expires_at is null or s.refresh_expires_at > $2)",
        )
        .bind(id)
        .bind(now)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        let Some((grant_id, server_id, Some(ciphertext), Some(nonce), scopes)) = row else {
            return Ok(None);
        };
        Ok(Some(McpRefreshMaterial {
            grant_id,
            server_id,
            refresh_token: kek.decrypt(&ciphertext, &nonce)?,
            scopes,
        }))
    }

    /// Replace the token material on a live session in place. Rotation is the
    /// point: an authorization server that hands back a new refresh token must
    /// not leave the old one readable, and one that omits it must not leave the
    /// session unrenewable, so `refresh_token` is written unconditionally.
    ///
    /// The row id is stable across a renewal on purpose — a session is the
    /// user's mental unit of "this app is connected", and churning its id on
    /// every silent refresh would make the sessions screen unreadable.
    pub async fn rotate_session(
        &self,
        kek: &super::super::crypto::Kek,
        id: Uuid,
        material: McpSessionMaterial<'_>,
    ) -> Result<McpOAuthSession> {
        let (access_ciphertext, access_nonce) = kek.encrypt(material.access_token)?;
        let refresh = material.refresh_token.map(|t| kek.encrypt(t)).transpose()?;
        let (refresh_ciphertext, refresh_nonce) = match refresh {
            Some((c, n)) => (Some(c), Some(n)),
            None => (None, None),
        };
        fetch_optional_or_not_found(
            sqlx::query_as(&format!(
                "update mcp_oauth_sessions set access_ciphertext = $2, access_nonce = $3, \
                        refresh_ciphertext = $4, refresh_nonce = $5, scopes = $6, \
                        expires_at = $7, refresh_expires_at = $8 \
                 where id = $1 and revoked_at is null returning {SESSION_COLUMNS}"
            ))
            .bind(id)
            .bind(access_ciphertext)
            .bind(access_nonce)
            .bind(refresh_ciphertext)
            .bind(refresh_nonce)
            .bind(material.scopes)
            .bind(material.expires_at)
            .bind(material.refresh_expires_at),
            self.0,
            || format!("live mcp oauth session {id}"),
        )
        .await
    }

    /// Live sessions whose access token expires before `before` and that hold a
    /// refresh token — the work list for the background renewer. Sessions
    /// without one are skipped: there is nothing to renew them with, and they
    /// simply lapse.
    pub async fn sessions_due_for_refresh(
        &self,
        before: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<Uuid>> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "select s.id from mcp_oauth_sessions s \
                 join mcp_oauth_grants g on g.id = s.grant_id \
             where s.revoked_at is null and g.revoked_at is null \
               and s.refresh_ciphertext is not null and s.expires_at < $1 \
               and (s.refresh_expires_at is null or s.refresh_expires_at > now()) \
             order by s.expires_at limit $2",
        )
        .bind(before)
        .bind(limit)
        .fetch_all(self.0)
        .await
        .map_err(store_err)?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// The grant a session hangs off together with its server and owner, in one
    /// round trip. Used by the authorization guard on the MCP call path, which
    /// runs per request and cannot afford three.
    pub async fn session_context(&self, id: Uuid) -> Result<Option<McpSessionContext>> {
        let row: Option<SessionContextRow> = sqlx::query_as(
            "select s.id, g.id, g.user_id, srv.id, g.scopes, s.scopes \
             from mcp_oauth_sessions s \
             join mcp_oauth_grants g on g.id = s.grant_id \
             join mcp_servers srv on srv.id = g.server_id \
             where s.id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        Ok(row.map(
            |(session_id, grant_id, user_id, server_id, grant_scopes, session_scopes)| {
                McpSessionContext {
                    session_id,
                    grant_id,
                    user_id,
                    server_id,
                    grant_scopes,
                    session_scopes,
                }
            },
        ))
    }
}

/// The refresh token of a session plus the consent it belongs to. Not
/// `Serialize`: the token may not reach an API response or a log line.
#[derive(Debug, Clone)]
pub struct McpRefreshMaterial {
    pub grant_id: Uuid,
    pub server_id: Uuid,
    pub refresh_token: String,
    pub scopes: Vec<String>,
}

/// Who a session belongs to and what it is allowed to ask for. Carries no
/// token material, so it is safe to hold across an authorization decision.
#[derive(Debug, Clone)]
pub struct McpSessionContext {
    pub session_id: Uuid,
    pub grant_id: Uuid,
    pub user_id: Uuid,
    pub server_id: Uuid,
    /// what the user consented to — the ceiling for everything below
    pub grant_scopes: Vec<String>,
    /// what this particular session actually holds
    pub session_scopes: Vec<String>,
}

/// Opened token material. Deliberately not `Serialize`: nothing in this struct
/// may reach an API response or a log line.
#[derive(Debug, Clone)]
pub struct McpSessionTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub scopes: Vec<String>,
}
