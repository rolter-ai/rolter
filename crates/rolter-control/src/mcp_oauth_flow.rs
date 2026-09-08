//! Token acquisition for MCP OAuth: authorization code, refresh, and
//! on-behalf-of exchange (#707).
//!
//! [`crate::mcp_oauth`] owns the *shelf* — servers, consent grants, sealed
//! sessions, listing and revocation. Until now nothing filled it: a grant row
//! could only be written directly. This module is the lifecycle that mints and
//! renews one, and it holds to the same rules its sibling states, plus three of
//! its own:
//!
//! 1. **The redirect URI is deployment-owned.** It is derived from
//!    `ROLTER_PUBLIC_URL`, never from the request, so a crafted link cannot
//!    send an authorization code to another host.
//! 2. **State is one-shot and short-lived.** The login row is deleted as it is
//!    read, so a replayed `code`+`state` pair finds nothing to redeem, and it
//!    ages out. PKCE (S256) binds the code to the browser that started the
//!    flow; the verifier is sealed at rest like every other secret here.
//! 3. **A refusal is final.** A refresh the authorization server *refuses* —
//!    `invalid_grant`, the shape a rotated-away or revoked refresh token takes
//!    — revokes the session. Only a transient failure (network, 5xx) is left
//!    for the next sweep. Retrying a refused refresh forever is how a renewal
//!    loop turns into a self-inflicted denial of service against the very
//!    server the tenant asked rolter to talk to.
//!
//! And the ceiling never moves: a session may never hold scopes the grant does
//! not, so neither a refresh nor an exchange can widen access beyond what the
//! user consented to. [`authorize_call`] is the guard that enforces this per
//! request; it has no caller yet, because the MCP request path it belongs on
//! is #423.

use std::collections::HashSet;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_core::Error;
use rolter_store::postgres::crypto::{Kek, KEK_ENV};
use rolter_store::postgres::models::{McpOAuthSession, McpServer};
use rolter_store::postgres::repo::{
    McpOAuthClient, McpOAuthRepo, McpServerRepo, McpSessionContext, McpSessionMaterial, NewMcpLogin,
};

use crate::crud::{
    log_audit, pool, require_allowed_egress, require_non_empty, ApiError, ApiResult, SafeJson,
};
use crate::rbac::{authorize, holds_admin, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::sso::{pkce_pair, public_base_url, random_token, urlencode};
use crate::ControlState;

/// How long an in-flight consent may take. Long enough for a login plus a
/// consent screen, short enough that a leaked `state` is worthless by the time
/// it is found.
const LOGIN_STATE_TTL_SECS: i64 = 600;
/// Access-token lifetime assumed when the authorization server omits
/// `expires_in`. Deliberately short: a wrong guess that is too long leaves a
/// dead token looking live, and the refresher would never wake up for it.
const DEFAULT_ACCESS_TTL_SECS: i64 = 3600;
/// How far before expiry the background refresher renews a session. Covers the
/// clock skew between rolter and the authorization server plus the round trip.
const REFRESH_SKEW_SECS: i64 = 300;
/// How often the refresher sweeps, and how many sessions one pass renews. The
/// batch bounds the burst a restart-after-outage sends at a single upstream.
const REFRESH_SWEEP_SECS: u64 = 60;
const REFRESH_BATCH: i64 = 100;

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/mcp-servers/{id}/oauth-client",
            put(set_oauth_client).get(get_oauth_client),
        )
        .route(
            "/api/v1/mcp-servers/{id}/oauth/authorize",
            post(start_authorize),
        )
        // the browser comes back here, so it is authenticated by the one-shot
        // login state rather than by a session bearer token
        .route("/auth/mcp/callback", get(callback))
        .route("/api/v1/mcp/sessions/{id}/refresh", post(refresh_endpoint))
        .route(
            "/api/v1/mcp/sessions/{id}/exchange",
            post(exchange_endpoint),
        )
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(Error::Config(message.into()))
}

/// The deployment KEK, or a client-visible configuration error. Never a silent
/// plaintext fallback: every MCP secret — exchanged tokens, and the static
/// credentials of #952 — is put at rest sealed or not stored at all.
///
/// `pub(crate)` so the registry's auth endpoint seals with the same helper and
/// reports the same message; two copies of this would be two messages to keep
/// in step.
pub(crate) fn kek() -> ApiResult<Kek> {
    Kek::from_env().ok_or_else(|| {
        invalid(format!(
            "storing an MCP credential requires the {KEK_ENV} environment variable on the \
             control plane to seal it at rest"
        ))
    })
}

/// Where an authorization server sends the code back. Deployment-derived, so a
/// spoofed `Host` header cannot redirect a code elsewhere, and constant across
/// servers so an operator registers one URI with each upstream.
pub(crate) fn callback_uri() -> String {
    format!("{}/auth/mcp/callback", public_base_url())
}

// ---------------------------------------------------------------------------
// client registration
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct OAuthClientBody {
    authorize_url: String,
    token_url: String,
    client_id: String,
    /// omitted leaves the stored secret alone; `""` clears it, which is how a
    /// confidential client is downgraded to a public one
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    default_scopes: Option<Vec<String>>,
}

/// What a client registration looks like from outside. The secret is absent by
/// construction — `has_client_secret` is the only thing said about it.
#[derive(Debug, Serialize)]
struct OAuthClientView {
    server_id: Uuid,
    authorize_url: Option<String>,
    token_url: Option<String>,
    client_id: Option<String>,
    default_scopes: Vec<String>,
    has_client_secret: bool,
    /// the redirect URI the operator must register with the upstream; returned
    /// so it never has to be guessed from the deployment's configuration
    redirect_uri: String,
}

impl From<McpServer> for OAuthClientView {
    fn from(server: McpServer) -> Self {
        Self {
            server_id: server.id,
            authorize_url: server.authorize_url,
            token_url: server.token_url,
            client_id: server.client_id,
            default_scopes: server.default_scopes,
            has_client_secret: server.has_client_secret,
            redirect_uri: callback_uri(),
        }
    }
}

async fn set_oauth_client(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<OAuthClientBody>,
) -> ApiResult<Json<OAuthClientView>> {
    let repo = McpServerRepo(pool(&state));
    let server = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(server.org_id),
        cap!("mcp_oauth_client", Update),
    )
    .await?;
    require_non_empty(&body.authorize_url, "authorize_url")?;
    require_non_empty(&body.token_url, "token_url")?;
    require_non_empty(&body.client_id, "client_id")?;
    require_https(&body.authorize_url, "authorize_url")?;
    require_https(&body.token_url, "token_url")?;
    // the control plane will POST the token endpoint itself, so it is held to
    // the same egress policy as any other upstream: an SSRF target must not
    // reach the database in the first place
    require_allowed_egress(&state, &body.authorize_url, "authorize_url")?;
    require_allowed_egress(&state, &body.token_url, "token_url")?;
    let scopes = body.default_scopes.unwrap_or_default();
    let updated = repo
        .set_oauth_client(
            &kek()?,
            id,
            McpOAuthClient {
                authorize_url: &body.authorize_url,
                token_url: &body.token_url,
                client_id: &body.client_id,
                client_secret: body.client_secret.as_deref(),
                default_scopes: &scopes,
            },
        )
        .await?;
    log_audit(
        &state,
        &principal,
        Some(server.org_id),
        "mcp_oauth_client.update",
        "mcp_server",
        id,
        serde_json::json!({
            "token_url": updated.token_url,
            "has_client_secret": updated.has_client_secret,
        }),
    )
    .await;
    Ok(Json(updated.into()))
}

async fn get_oauth_client(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<OAuthClientView>> {
    let server = McpServerRepo(pool(&state)).get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(server.org_id),
        cap!("mcp_oauth_client", Read),
    )
    .await?;
    Ok(Json(server.into()))
}

/// An OAuth endpoint must be `https`, with `http` allowed only on loopback so
/// a developer can run a stub locally. A bearer token crossing plaintext to a
/// non-local host is not a thing to make configurable.
fn require_https(url: &str, field: &str) -> ApiResult<()> {
    if url.starts_with("https://") {
        return Ok(());
    }
    if let Some(rest) = url.strip_prefix("http://") {
        let host = rest
            .split('/')
            .next()
            .unwrap_or_default()
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or_else(|| rest.split('/').next().unwrap_or_default());
        if matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1") {
            return Ok(());
        }
    }
    Err(invalid(format!(
        "{field} must be an https url (http is accepted only on loopback)"
    )))
}

// ---------------------------------------------------------------------------
// authorization code
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct AuthorizeBody {
    /// scopes to request; falls back to the server's `default_scopes`
    #[serde(default)]
    scopes: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct AuthorizeStarted {
    authorization_url: String,
    state: String,
    expires_in: i64,
}

/// Begin consent: record the state and hand back the URL to send the browser
/// to. The control plane does not redirect here — the caller is the dashboard
/// over `fetch`, which cannot follow a cross-origin 302 usefully.
async fn start_authorize(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    body: Option<SafeJson<AuthorizeBody>>,
) -> ApiResult<Json<AuthorizeStarted>> {
    let server = McpServerRepo(pool(&state)).get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(server.org_id),
        cap!("mcp_oauth_grant", Create),
    )
    .await?;
    // consent belongs to a person: a superadmin acting as nobody has no
    // identity to hang a grant off, and inventing one would put a token in the
    // org that no user can see or revoke
    let Principal::User(user) = &principal else {
        return Err(invalid(
            "MCP consent is granted by a user; a superadmin token has no identity to grant it",
        ));
    };
    let client = require_client(&server)?;
    let requested = body
        .and_then(|SafeJson(b)| b.scopes)
        .unwrap_or_else(|| server.default_scopes.clone());

    let (verifier, challenge) = pkce_pair();
    let csrf_state = random_token();
    let redirect = callback_uri();
    McpOAuthRepo(pool(&state))
        .start_login(
            &kek()?,
            NewMcpLogin {
                state: &csrf_state,
                server_id: server.id,
                user_id: user.id,
                code_verifier: &verifier,
                scopes: &requested,
                redirect_uri: &redirect,
            },
        )
        .await?;
    Ok(Json(AuthorizeStarted {
        authorization_url: authorization_url(
            &client.authorize_url,
            &client.client_id,
            &redirect,
            &csrf_state,
            &challenge,
            &requested,
        ),
        state: csrf_state,
        expires_in: LOGIN_STATE_TTL_SECS,
    }))
}

/// The registered OAuth client of a server, or a 400 naming what is missing.
struct RegisteredClient {
    authorize_url: String,
    token_url: String,
    client_id: String,
}

fn require_client(server: &McpServer) -> ApiResult<RegisteredClient> {
    match (
        server.authorize_url.clone(),
        server.token_url.clone(),
        server.client_id.clone(),
    ) {
        (Some(authorize_url), Some(token_url), Some(client_id)) => Ok(RegisteredClient {
            authorize_url,
            token_url,
            client_id,
        }),
        _ => Err(invalid(format!(
            "mcp server '{}' has no oauth client registered; PUT its \
             /api/v1/mcp-servers/{}/oauth-client first",
            server.slug, server.id
        ))),
    }
}

/// Build the authorization URL. Split out from the handler so the parameter set
/// can be asserted without a live authorization server.
fn authorization_url(
    endpoint: &str,
    client_id: &str,
    redirect: &str,
    csrf_state: &str,
    challenge: &str,
    scopes: &[String],
) -> String {
    let sep = if endpoint.contains('?') { '&' } else { '?' };
    let mut url = format!(
        "{endpoint}{sep}response_type=code&client_id={}&redirect_uri={}&state={}\
         &code_challenge={}&code_challenge_method=S256",
        urlencode(client_id),
        urlencode(redirect),
        urlencode(csrf_state),
        urlencode(challenge),
    );
    // an empty scope set means "whatever the server defaults to"; sending
    // `scope=` would ask for nothing instead, which is not the same request
    if !scopes.is_empty() {
        url.push_str(&format!("&scope={}", urlencode(&scopes.join(" "))));
    }
    url
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Serialize)]
struct ConsentCompleted {
    grant_id: Uuid,
    session_id: Uuid,
    server_id: Uuid,
    scopes: Vec<String>,
    expires_at: DateTime<Utc>,
    has_refresh_token: bool,
}

/// Redeem the authorization code and store the grant plus its first session.
async fn callback(
    State(state): State<ControlState>,
    Query(query): Query<CallbackQuery>,
) -> ApiResult<Json<ConsentCompleted>> {
    if let Some(error) = query.error {
        // the authorization server's own words, but only its words: a
        // description is echoed, nothing of ours is added to it
        let detail = query
            .error_description
            .map(|d| format!(": {d}"))
            .unwrap_or_default();
        return Err(invalid(format!(
            "authorization server refused consent ({error}){detail}"
        )));
    }
    let (Some(code), Some(csrf_state)) = (query.code, query.state) else {
        return Err(invalid("callback requires both 'code' and 'state'"));
    };
    let kek = kek()?;
    let repo = McpOAuthRepo(pool(&state));
    let login = repo
        .consume_login(
            &kek,
            &csrf_state,
            Utc::now(),
            Duration::seconds(LOGIN_STATE_TTL_SECS),
        )
        .await?
        .ok_or_else(|| invalid("unknown, expired or already-redeemed consent state"))?;

    let server = McpServerRepo(pool(&state)).get(login.server_id).await?;
    let client = require_client(&server)?;
    let secret = McpServerRepo(pool(&state))
        .client_secret(&kek, server.id)
        .await?;

    let form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code),
        ("redirect_uri", login.redirect_uri.clone()),
        ("client_id", client.client_id.clone()),
        ("code_verifier", login.code_verifier.clone()),
    ];
    let tokens = post_token(&state, &client.token_url, form, secret.as_deref())
        .await
        .map_err(|e| e.into_api())?;

    // what the server actually granted, which may be less than was asked for
    let granted = tokens.granted_scopes(&login.scopes);
    let grant = repo
        .upsert_grant(server.id, login.user_id, &granted)
        .await?;
    let now = Utc::now();
    let session = repo
        .store_session(
            &kek,
            grant.id,
            McpSessionMaterial {
                access_token: &tokens.access_token,
                refresh_token: tokens.refresh_token.as_deref(),
                scopes: &granted,
                expires_at: tokens.access_expires_at(now),
                refresh_expires_at: tokens.refresh_expires_at(now),
            },
        )
        .await?;
    log_audit_system(
        &state,
        server.org_id,
        "mcp_oauth_grant.consent",
        "mcp_oauth_grant",
        grant.id,
        serde_json::json!({
            "server_id": server.id,
            "user_id": login.user_id,
            "session_id": session.id,
            "scopes": granted,
        }),
    )
    .await;
    Ok(Json(ConsentCompleted {
        grant_id: grant.id,
        session_id: session.id,
        server_id: server.id,
        scopes: session.scopes.clone(),
        expires_at: session.expires_at,
        has_refresh_token: session.has_refresh_token,
    }))
}

// ---------------------------------------------------------------------------
// token endpoint
// ---------------------------------------------------------------------------

/// The subset of RFC 6749 §5.1 rolter reads. Everything else an authorization
/// server returns is ignored rather than stored.
#[derive(Debug, Deserialize, Default)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    refresh_expires_in: Option<i64>,
    /// space-delimited, per the spec; absent means "exactly what was asked for"
    #[serde(default)]
    scope: Option<String>,
}

impl TokenResponse {
    /// What this response actually granted. A server that echoes no `scope`
    /// granted what was requested; one that echoes a set granted that set,
    /// which may be narrower.
    fn granted_scopes(&self, requested: &[String]) -> Vec<String> {
        match self.scope.as_deref() {
            Some(scope) => parse_scope(scope),
            None => requested.to_vec(),
        }
    }

    fn access_expires_at(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        now + Duration::seconds(self.expires_in.unwrap_or(DEFAULT_ACCESS_TTL_SECS).max(1))
    }

    fn refresh_expires_at(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.refresh_expires_in
            .filter(|s| *s > 0)
            .map(|s| now + Duration::seconds(s))
    }
}

fn parse_scope(scope: &str) -> Vec<String> {
    scope
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>()
}

/// Why a token request failed. The distinction is the whole point: a refusal
/// is a permanent answer about this grant and must revoke, while a transient
/// failure must not — see the module docs.
#[derive(Debug)]
pub(crate) enum TokenError {
    /// the authorization server said no (4xx / `invalid_grant`)
    Refused(String),
    /// network, timeout, 5xx, or an unparsable body
    Transient(String),
}

impl TokenError {
    fn into_api(self) -> ApiError {
        match self {
            Self::Refused(message) | Self::Transient(message) => invalid(message),
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::Refused(m) | Self::Transient(m) => m,
        }
    }
}

/// Classify a token-endpoint status. 4xx is the authorization server's verdict
/// on the grant; 5xx and anything else is its own bad day.
fn classify(status: u16, body: &str) -> TokenError {
    let message = format!("token endpoint returned {status}");
    if (400..500).contains(&status) {
        // the body of an error response is the server's, and RFC 6749 §5.2
        // puts no credential in it — but it is not echoed anyway, only probed
        // for the one code that changes what rolter does next
        if body.contains("invalid_grant") || body.contains("invalid_scope") {
            return TokenError::Refused(format!("{message} (grant refused)"));
        }
        return TokenError::Refused(message);
    }
    TokenError::Transient(message)
}

/// POST a grant to an authorization server's token endpoint.
///
/// The response body is never logged or surfaced: it carries a bearer token on
/// success, and on failure it is the upstream's prose, which has no business in
/// a rolter error. Only the status crosses back.
async fn post_token(
    state: &ControlState,
    token_url: &str,
    mut form: Vec<(&'static str, String)>,
    client_secret: Option<&str>,
) -> Result<TokenResponse, TokenError> {
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret.to_string()));
    }
    let response = state
        .http
        .post(token_url)
        .form(&form)
        .send()
        .await
        .map_err(|e| TokenError::Transient(format!("token endpoint unreachable: {e}")))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response.text().await.unwrap_or_default();
        return Err(classify(status, &body));
    }
    let tokens: TokenResponse = response
        .json()
        .await
        .map_err(|e| TokenError::Transient(format!("token response is not valid json: {e}")))?;
    if tokens.access_token.is_empty() {
        return Err(TokenError::Refused(
            "token endpoint returned no access_token".to_string(),
        ));
    }
    Ok(tokens)
}

// ---------------------------------------------------------------------------
// refresh
// ---------------------------------------------------------------------------

async fn refresh_endpoint(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<McpOAuthSession>> {
    let context = owned_session(&state, &principal, id, cap!("mcp_oauth_session", Update)).await?;
    let session = refresh_session(&state, id)
        .await
        .map_err(|e| e.into_api())?;
    let _ = context;
    Ok(Json(session))
}

/// Renew one session from its stored refresh token.
///
/// Shared by the endpoint and the background sweeper so both handle a refusal
/// the same way — there is exactly one place that decides a session is dead.
pub(crate) async fn refresh_session(
    state: &ControlState,
    session_id: Uuid,
) -> Result<McpOAuthSession, TokenError> {
    let kek = kek().map_err(|_| {
        TokenError::Transient(format!("{KEK_ENV} is not configured on the control plane"))
    })?;
    let repo = McpOAuthRepo(pool(state));
    let material = repo
        .open_refresh(&kek, session_id, Utc::now())
        .await
        .map_err(|e| TokenError::Transient(e.to_string()))?
        .ok_or_else(|| {
            // no refresh token, or the consent is gone: nothing to renew, and
            // nothing to retry either
            TokenError::Refused(
                "session has no usable refresh token, or its grant is revoked or expired"
                    .to_string(),
            )
        })?;
    let server = McpServerRepo(pool(state))
        .get(material.server_id)
        .await
        .map_err(|e| TokenError::Transient(e.to_string()))?;
    let client = require_client(&server).map_err(|_| {
        TokenError::Refused(format!(
            "mcp server '{}' no longer has an oauth client registered",
            server.slug
        ))
    })?;
    let secret = McpServerRepo(pool(state))
        .client_secret(&kek, server.id)
        .await
        .map_err(|e| TokenError::Transient(e.to_string()))?;

    let form = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", material.refresh_token.clone()),
        ("client_id", client.client_id.clone()),
        // ask for what the session already holds, never more
        ("scope", material.scopes.join(" ")),
    ];
    let tokens = match post_token(state, &client.token_url, form, secret.as_deref()).await {
        Ok(tokens) => tokens,
        Err(TokenError::Refused(message)) => {
            // the answer is permanent: stop holding a token the server will
            // never honour again, and stop asking
            let _ = repo.revoke_session(session_id).await;
            log_audit_system(
                state,
                server.org_id,
                "mcp_oauth_session.refresh_refused",
                "mcp_oauth_session",
                session_id,
                serde_json::json!({"server_id": server.id, "reason": message}),
            )
            .await;
            return Err(TokenError::Refused(message));
        }
        Err(transient) => return Err(transient),
    };

    // a rotated refresh token replaces the old one; a server that returns none
    // means "keep using the one you have", so it is carried forward rather
    // than dropped — dropping it would strand the session at the next expiry
    let refresh = tokens
        .refresh_token
        .clone()
        .unwrap_or_else(|| material.refresh_token.clone());
    // the ceiling never moves: a refresh cannot widen what the session holds
    let scopes = narrow_to(&tokens.granted_scopes(&material.scopes), &material.scopes);
    let now = Utc::now();
    repo.rotate_session(
        &kek,
        session_id,
        McpSessionMaterial {
            access_token: &tokens.access_token,
            refresh_token: Some(&refresh),
            scopes: &scopes,
            expires_at: tokens.access_expires_at(now),
            refresh_expires_at: tokens.refresh_expires_at(now),
        },
    )
    .await
    .map_err(|e| TokenError::Transient(e.to_string()))
}

/// Spawn the background renewer. Sessions near expiry are refreshed without the
/// user present; that is the whole point of storing a refresh token.
pub(crate) fn start_refresher(state: ControlState) {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(REFRESH_SWEEP_SECS));
        interval.tick().await;
        loop {
            interval.tick().await;
            sweep_refreshes(&state).await;
        }
    });
}

/// One refresh pass. Every outcome is swallowed after being logged: this runs
/// off the request path, and one unhappy upstream must not stop the sweep.
pub(crate) async fn sweep_refreshes(state: &ControlState) {
    let due = match MinimalRepo(state)
        .due(Utc::now() + Duration::seconds(REFRESH_SKEW_SECS))
        .await
    {
        Ok(due) => due,
        Err(error) => {
            tracing::warn!(error = %error, "could not list mcp oauth sessions due for refresh");
            return;
        }
    };
    for id in due {
        match refresh_session(state, id).await {
            Ok(session) => tracing::debug!(
                session_id = %id,
                expires_at = %session.expires_at,
                "renewed mcp oauth session"
            ),
            Err(TokenError::Refused(reason)) => tracing::info!(
                session_id = %id,
                %reason,
                "mcp oauth refresh refused; session revoked"
            ),
            Err(error) => tracing::warn!(
                session_id = %id,
                reason = error.message(),
                "mcp oauth refresh failed; will retry on the next sweep"
            ),
        }
    }
}

/// Thin wrapper so the sweep reads as one call rather than three lines of
/// repo plumbing.
struct MinimalRepo<'a>(&'a ControlState);

impl MinimalRepo<'_> {
    async fn due(&self, before: DateTime<Utc>) -> Result<Vec<Uuid>, Error> {
        McpOAuthRepo(pool(self.0))
            .sessions_due_for_refresh(before, REFRESH_BATCH)
            .await
    }
}

// ---------------------------------------------------------------------------
// on-behalf-of exchange
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ExchangeBody {
    /// scopes the exchanged token should carry; must be within the grant
    #[serde(default)]
    scopes: Option<Vec<String>>,
    /// RFC 8693 `audience` — the downstream service the token is for
    #[serde(default)]
    audience: Option<String>,
}

/// Exchange a live session for a narrower one against a downstream audience
/// (RFC 8693 `urn:ietf:params:oauth:grant-type:token-exchange`).
///
/// This is the server-to-server half: a service acting for the user gets its
/// own session row, so it can be revoked on its own without taking the user's
/// interactive session with it — and it can never be broader than the consent
/// it descends from.
async fn exchange_endpoint(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    body: Option<SafeJson<ExchangeBody>>,
) -> ApiResult<Json<McpOAuthSession>> {
    let context = owned_session(&state, &principal, id, cap!("mcp_oauth_session", Create)).await?;
    let body = body.map(|SafeJson(b)| b);
    let requested = body
        .as_ref()
        .and_then(|b| b.scopes.clone())
        .unwrap_or_else(|| context.session_scopes.clone());
    // the ceiling is the *grant*, not the session: an exchange may not ask for
    // anything the user did not consent to
    reject_scope_escalation(&requested, &context.grant_scopes)?;

    let kek = kek()?;
    let repo = McpOAuthRepo(pool(&state));
    let subject = repo
        .open_session(&kek, id, Utc::now())
        .await?
        .ok_or_else(|| invalid("session is revoked, expired, or its consent was withdrawn"))?;
    let server = McpServerRepo(pool(&state)).get(context.server_id).await?;
    let client = require_client(&server)?;
    let secret = McpServerRepo(pool(&state))
        .client_secret(&kek, server.id)
        .await?;

    let mut form = vec![
        (
            "grant_type",
            "urn:ietf:params:oauth:grant-type:token-exchange".to_string(),
        ),
        ("client_id", client.client_id.clone()),
        ("subject_token", subject.access_token.clone()),
        (
            "subject_token_type",
            "urn:ietf:params:oauth:token-type:access_token".to_string(),
        ),
        ("scope", requested.join(" ")),
    ];
    if let Some(audience) = body.as_ref().and_then(|b| b.audience.clone()) {
        form.push(("audience", audience));
    }
    let tokens = post_token(&state, &client.token_url, form, secret.as_deref())
        .await
        .map_err(|e| e.into_api())?;

    let granted = narrow_to(&tokens.granted_scopes(&requested), &context.grant_scopes);
    let now = Utc::now();
    let session = repo
        .store_session(
            &kek,
            context.grant_id,
            McpSessionMaterial {
                access_token: &tokens.access_token,
                refresh_token: tokens.refresh_token.as_deref(),
                scopes: &granted,
                expires_at: tokens.access_expires_at(now),
                refresh_expires_at: tokens.refresh_expires_at(now),
            },
        )
        .await?;
    log_audit(
        &state,
        &principal,
        Some(server.org_id),
        "mcp_oauth_session.exchange",
        "mcp_oauth_session",
        session.id,
        serde_json::json!({
            "grant_id": context.grant_id,
            "parent_session_id": id,
            "scopes": granted,
        }),
    )
    .await;
    Ok(Json(session))
}

// ---------------------------------------------------------------------------
// authorization on the call path
// ---------------------------------------------------------------------------

/// Whether `requested` is within `allowed`.
fn scopes_within(requested: &[String], allowed: &[String]) -> bool {
    let allowed: HashSet<&str> = allowed.iter().map(String::as_str).collect();
    requested.iter().all(|s| allowed.contains(s.as_str()))
}

/// Intersect `granted` with `ceiling`, preserving the order of `granted`. Used
/// wherever an authorization server's answer is written back: a server that
/// hands out more than was consented to does not get to widen the row.
fn narrow_to(granted: &[String], ceiling: &[String]) -> Vec<String> {
    let ceiling: HashSet<&str> = ceiling.iter().map(String::as_str).collect();
    granted
        .iter()
        .filter(|s| ceiling.contains(s.as_str()))
        .cloned()
        .collect()
}

fn reject_scope_escalation(requested: &[String], allowed: &[String]) -> ApiResult<()> {
    if scopes_within(requested, allowed) {
        return Ok(());
    }
    let excess: Vec<&str> = requested
        .iter()
        .map(String::as_str)
        .filter(|s| !allowed.iter().any(|a| a == s))
        .collect();
    Err(invalid(format!(
        "requested scope exceeds the grant: {}",
        excess.join(", ")
    )))
}

/// Resolve the session's context and check the caller may act on it: an org
/// admin may act on any session in the org, anyone else only on their own.
async fn owned_session(
    state: &ControlState,
    principal: &Principal,
    session_id: Uuid,
    requirement: crate::rbac_matrix::Requirement,
) -> ApiResult<McpSessionContext> {
    let context = McpOAuthRepo(pool(state))
        .session_context(session_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("mcp oauth session {session_id}")))?;
    let server = McpServerRepo(pool(state)).get(context.server_id).await?;
    authorize(
        state,
        principal,
        ScopeChain::org(server.org_id),
        requirement,
    )
    .await?;
    if holds_admin(state, principal, ScopeChain::org(server.org_id)).await? {
        return Ok(context);
    }
    match principal {
        Principal::Superadmin => Ok(context),
        Principal::User(user) if user.id == context.user_id => Ok(context),
        Principal::User(_) => Err(ApiError::Forbidden),
    }
}

/// Authorize one MCP call against a session, and stamp the session as used.
///
/// This is the guard #707 asks for on the MCP call path: the session must be
/// live (which [`McpOAuthRepo::open_session`] already fails closed on), must
/// belong to `user_id`, must target `server_id`, and the scopes the call needs
/// must be within *both* the session and the grant behind it.
///
/// It has no caller yet — the gateway has no MCP request path to guard until
/// #423 lands. It is written, tested and exported here rather than left for
/// then so that path has a guard to call on day one instead of inventing a
/// second, subtly different one.
#[allow(dead_code)]
pub(crate) async fn authorize_call(
    state: &ControlState,
    session_id: Uuid,
    user_id: Uuid,
    server_id: Uuid,
    required_scopes: &[String],
) -> ApiResult<McpSessionContext> {
    let repo = McpOAuthRepo(pool(state));
    let context = repo
        .session_context(session_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("mcp oauth session {session_id}")))?;
    if context.user_id != user_id || context.server_id != server_id {
        // deliberately the same answer as "no such session": whether a session
        // exists for another user is not something a caller gets to probe
        return Err(ApiError::Core(Error::NotFound(format!(
            "mcp oauth session {session_id}"
        ))));
    }
    reject_scope_escalation(required_scopes, &context.grant_scopes)?;
    reject_scope_escalation(required_scopes, &context.session_scopes)?;
    // liveness is the store's call, not ours; it also covers withdrawn consent
    let kek = kek()?;
    if repo
        .open_session(&kek, session_id, Utc::now())
        .await?
        .is_none()
    {
        return Err(invalid(
            "session is revoked, expired, or its consent was withdrawn",
        ));
    }
    // best-effort bookkeeping: a failed stamp must not fail the call
    let _ = repo.touch_session(session_id).await;
    Ok(context)
}

/// Audit an event the system performed with no principal in the request — the
/// callback (whose caller is a browser redirect) and the background refresher.
async fn log_audit_system(
    state: &ControlState,
    org_id: Uuid,
    action: &str,
    resource: &str,
    resource_id: Uuid,
    detail: serde_json::Value,
) {
    log_audit(
        state,
        &Principal::Superadmin,
        Some(org_id),
        action,
        resource,
        resource_id,
        detail,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_authorization_url_carries_pkce_and_state() {
        let url = authorization_url(
            "https://mcp.example.com/authorize",
            "client-abc",
            "https://rolter.example.com/auth/mcp/callback",
            "state-xyz",
            "challenge-123",
            &s(&["tools:read", "tools:write"]),
        );
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=client-abc"));
        assert!(url.contains("code_challenge=challenge-123"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=state-xyz"));
        // the redirect uri and the space in the scope list must be encoded
        assert!(
            url.contains("redirect_uri=https%3A%2F%2Frolter.example.com%2Fauth%2Fmcp%2Fcallback")
        );
        assert!(url.contains("scope=tools%3Aread%20tools%3Awrite"));
    }

    #[test]
    fn an_endpoint_with_a_query_string_keeps_it() {
        let url = authorization_url(
            "https://mcp.example.com/authorize?tenant=acme",
            "c",
            "https://r/cb",
            "st",
            "ch",
            &[],
        );
        assert!(url.contains("?tenant=acme&response_type=code"));
        // asking for no scopes must not send `scope=`, which asks for none
        assert!(!url.contains("scope="));
    }

    #[test]
    fn only_https_or_loopback_endpoints_are_accepted() {
        assert!(require_https("https://mcp.example.com/token", "token_url").is_ok());
        assert!(require_https("http://localhost:9000/token", "token_url").is_ok());
        assert!(require_https("http://127.0.0.1:9000/token", "token_url").is_ok());
        assert!(require_https("http://mcp.example.com/token", "token_url").is_err());
        assert!(require_https("ftp://mcp.example.com/token", "token_url").is_err());
        // a hostname that merely *starts* with a loopback name is not loopback
        assert!(require_https("http://localhost.evil.com/token", "token_url").is_err());
    }

    #[test]
    fn a_refusal_and_a_bad_day_are_told_apart() {
        assert!(matches!(
            classify(400, r#"{"error":"invalid_grant"}"#),
            TokenError::Refused(_)
        ));
        assert!(matches!(classify(401, "{}"), TokenError::Refused(_)));
        assert!(matches!(classify(403, "{}"), TokenError::Refused(_)));
        // a server having a bad minute is not a verdict on the grant
        assert!(matches!(classify(500, ""), TokenError::Transient(_)));
        assert!(matches!(classify(502, ""), TokenError::Transient(_)));
        assert!(matches!(classify(503, ""), TokenError::Transient(_)));
    }

    #[test]
    fn a_token_error_never_carries_the_upstream_body() {
        // the upstream's prose may quote the request, which carried a secret
        let error = classify(400, "client_secret=hunter2 is wrong");
        assert!(!error.message().contains("hunter2"));
    }

    #[test]
    fn scopes_are_bounded_by_the_grant() {
        let grant = s(&["tools:read", "tools:write"]);
        assert!(reject_scope_escalation(&s(&["tools:read"]), &grant).is_ok());
        assert!(reject_scope_escalation(&grant, &grant).is_ok());
        assert!(reject_scope_escalation(&[], &grant).is_ok());
        let err = reject_scope_escalation(&s(&["tools:read", "admin:all"]), &grant);
        assert!(err.is_err());
        // and an empty grant admits nothing
        assert!(reject_scope_escalation(&s(&["tools:read"]), &[]).is_err());
    }

    #[test]
    fn a_generous_authorization_server_cannot_widen_a_row() {
        let ceiling = s(&["tools:read"]);
        // the server handed back more than was consented to; the extra is
        // dropped rather than stored
        assert_eq!(
            narrow_to(&s(&["tools:read", "tools:write", "admin:all"]), &ceiling),
            s(&["tools:read"])
        );
        assert!(narrow_to(&s(&["admin:all"]), &ceiling).is_empty());
    }

    #[test]
    fn an_omitted_scope_means_what_was_asked_for() {
        let requested = s(&["tools:read"]);
        let response = TokenResponse {
            access_token: "t".into(),
            ..Default::default()
        };
        assert_eq!(response.granted_scopes(&requested), requested);
        let narrowed = TokenResponse {
            access_token: "t".into(),
            scope: Some("tools:read".into()),
            ..Default::default()
        };
        assert_eq!(
            narrowed.granted_scopes(&s(&["tools:read", "tools:write"])),
            s(&["tools:read"])
        );
    }

    #[test]
    fn a_missing_expires_in_still_yields_a_bounded_lifetime() {
        let now = Utc::now();
        let response = TokenResponse {
            access_token: "t".into(),
            ..Default::default()
        };
        assert_eq!(
            response.access_expires_at(now),
            now + Duration::seconds(DEFAULT_ACCESS_TTL_SECS)
        );
        assert_eq!(response.refresh_expires_at(now), None);
        // a nonsensical zero or negative lifetime must not produce a token
        // that is already expired at rest, nor one that never expires
        let zero = TokenResponse {
            access_token: "t".into(),
            expires_in: Some(0),
            refresh_expires_in: Some(0),
            ..Default::default()
        };
        assert!(zero.access_expires_at(now) > now);
        assert_eq!(zero.refresh_expires_at(now), None);
    }

    #[test]
    fn scope_parsing_follows_the_spec() {
        assert_eq!(parse_scope("a b  c"), s(&["a", "b", "c"]));
        assert_eq!(parse_scope("   "), Vec::<String>::new());
    }

    #[test]
    fn the_refresher_wakes_before_the_token_dies() {
        // the sweep interval must be shorter than the skew, or a session can
        // expire in the gap between two passes
        assert!((REFRESH_SWEEP_SECS as i64) < REFRESH_SKEW_SECS);
    }
}
