//! OIDC single sign-on (#240).
//!
//! An org registers an identity provider; a user hits `/auth/sso/{slug}/start`,
//! is bounced through the provider's authorization-code flow with PKCE, and
//! comes back with an id token that rolter verifies against the provider's
//! published JWKS. Groups in the token are mapped to memberships, and the user
//! gets an ordinary rolter session — the same token type local login issues, so
//! everything downstream of authentication is unchanged.
//!
//! What the flow refuses to do is as important as what it does:
//!
//! * **No unsigned trust.** The id token is verified against the JWKS fetched
//!   from the issuer's discovery document, with the issuer, audience, and nonce
//!   all checked. An `alg: none` or HMAC-signed token is rejected: only the
//!   asymmetric algorithms the provider actually publishes are accepted.
//! * **No open redirect.** The `redirect_uri` is derived from the deployment's
//!   own configured base URL, never from the request, so a crafted link cannot
//!   send an authorization code to another host.
//! * **One-shot state.** The login state row is consumed by the callback, so a
//!   replayed `code`+`state` pair finds nothing to redeem, and it expires.
//! * **No implicit access.** A user in no mapped group gets the provider's
//!   `default_role`; when that is unset they are refused rather than silently
//!   admitted with an empty membership set.

use std::collections::HashSet;

use async_trait::async_trait;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use chrono::{Duration, Utc};
use rolter_auth::{Credential, Identity, IdentityError, IdentityProvider};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use rolter_store::postgres::crypto::Kek;
use rolter_store::postgres::models::{Membership, SsoGroupMapping, SsoProvider, User};
use rolter_store::postgres::repo::{
    AuditLogRepo, MembershipRepo, OrgAuthPolicyRepo, SecretUpdate, SessionRepo, SsoProviderUpdate,
    SsoRepo, UserRepo,
};

use crate::auth::session_pepper;
use crate::crud::{log_audit, pool, require_non_empty, ApiError, ApiResult, SafeJson};
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

/// [`IdentityProvider`] for one configured OIDC SSO provider (ROL-35).
///
/// Bound to a specific [`SsoProvider`] row and its [`Discovery`] document and
/// decrypted client secret, since verifying an authorization code is
/// meaningless without knowing which issuer/audience/JWKS to check it
/// against. Constructed fresh per login in [`callback`].
pub(crate) struct OidcIdentityProvider {
    discovery: Discovery,
    provider: SsoProvider,
    secret: Option<String>,
}

impl OidcIdentityProvider {
    pub(crate) fn new(discovery: Discovery, provider: SsoProvider, secret: Option<String>) -> Self {
        Self {
            discovery,
            provider,
            secret,
        }
    }
}

#[async_trait]
impl IdentityProvider for OidcIdentityProvider {
    fn kind(&self) -> &'static str {
        "oidc"
    }

    async fn resolve(&self, credential: Credential) -> Result<Identity, IdentityError> {
        let Credential::AuthorizationCode {
            code,
            verifier,
            nonce,
        } = credential
        else {
            return Err(IdentityError::UnsupportedCredential { provider: "oidc" });
        };

        let id_token = exchange_code(
            &self.discovery,
            &self.provider,
            self.secret.as_deref(),
            &code,
            &verifier,
        )
        .await
        .map_err(api_error_message)
        .map_err(IdentityError::Provider)?;
        let claims = verify_id_token(&self.discovery, &self.provider, &id_token, &nonce)
            .await
            .map_err(api_error_message)
            .map_err(IdentityError::Provider)?;

        let email = claims
            .email
            .clone()
            .or_else(|| {
                claims
                    .preferred_username
                    .clone()
                    .filter(|u| u.contains('@'))
            })
            .ok_or(IdentityError::NotVerified)?;

        let groups = groups_from_claims(&claims, &self.provider.group_claim);
        Ok(Identity {
            subject: claims.sub,
            email,
            display_name: claims.preferred_username,
            groups: groups.into_iter().collect(),
        })
    }
}

/// How long an in-flight login may take. Long enough for a password + MFA
/// prompt, short enough that a leaked state is worthless by the time it is
/// found.
const LOGIN_STATE_TTL_SECS: i64 = 600;
/// Session lifetime for an SSO login, matching local login.
const SESSION_TTL_HOURS: i64 = 12;
/// Signature algorithms accepted on an id token. Symmetric and `none` are
/// absent on purpose: with a shared client secret, an HMAC-signed token is
/// forgeable by anything that has read the config.
const ACCEPTED_ALGS: &[jsonwebtoken::Algorithm] = &[
    jsonwebtoken::Algorithm::RS256,
    jsonwebtoken::Algorithm::RS384,
    jsonwebtoken::Algorithm::RS512,
    jsonwebtoken::Algorithm::ES256,
    jsonwebtoken::Algorithm::ES384,
    jsonwebtoken::Algorithm::PS256,
];

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route("/auth/sso/{slug}/start", get(start_login))
        .route("/auth/sso/{slug}/callback", get(callback))
        .route(
            "/api/v1/orgs/{org_id}/sso-providers",
            post(create_provider).get(list_providers),
        )
        .route(
            "/api/v1/sso-providers/{id}",
            axum::routing::put(update_provider).delete(delete_provider),
        )
        .route(
            "/api/v1/sso-providers/{id}/group-mappings",
            post(create_mapping).get(list_mappings),
        )
        .route(
            "/api/v1/sso-group-mappings/{id}",
            axum::routing::delete(delete_mapping),
        )
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(rolter_core::Error::Config(message.into()))
}

/// Human-readable message for an [`ApiError`], for wrapping into
/// [`IdentityError::Provider`] rather than exposing `ApiError` outside this
/// module's HTTP handlers.
fn api_error_message(err: ApiError) -> String {
    match err {
        ApiError::Core(e) => e.to_string(),
        ApiError::Conflict(msg) => msg,
        ApiError::Unauthenticated => "unauthenticated".to_string(),
        ApiError::Forbidden => "forbidden".to_string(),
        ApiError::TooManyAttempts(remaining) => {
            format!("too many attempts; retry in {}s", remaining.as_secs())
        }
    }
}

/// Public base URL of this control plane, used to build the redirect URI the
/// provider will send the code back to. Derived from configuration rather than
/// the request so a spoofed `Host` header cannot redirect a code elsewhere.
pub(crate) fn public_base_url() -> String {
    std::env::var("ROLTER_PUBLIC_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| "http://localhost:4001".to_string())
        .trim_end_matches('/')
        .to_string()
}

fn redirect_uri(slug: &str) -> String {
    format!("{}/auth/sso/{slug}/callback", public_base_url())
}

// ---------------------------------------------------------------------------
// discovery + jwks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

/// Fetch the issuer's discovery document. The issuer in the document must match
/// the one configured: a provider that answers for a different issuer is either
/// misconfigured or hostile, and either way its tokens must not be trusted.
async fn discover(issuer: &str) -> ApiResult<Discovery> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let doc: Discovery = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .map_err(|e| invalid(format!("sso discovery failed: {e}")))?
        .error_for_status()
        .map_err(|e| invalid(format!("sso discovery failed: {e}")))?
        .json()
        .await
        .map_err(|e| invalid(format!("sso discovery document is not valid json: {e}")))?;
    if doc.issuer.trim_end_matches('/') != issuer.trim_end_matches('/') {
        return Err(invalid(format!(
            "discovery document declares issuer '{}', expected '{issuer}'",
            doc.issuer
        )));
    }
    Ok(doc)
}

#[derive(Debug, Deserialize)]
struct Jwks {
    keys: Vec<Value>,
}

/// Find the signing key for a token's `kid` in the provider's JWKS.
async fn signing_key(jwks_uri: &str, kid: &str) -> ApiResult<jsonwebtoken::jwk::Jwk> {
    let jwks: Jwks = reqwest::Client::new()
        .get(jwks_uri)
        .send()
        .await
        .map_err(|e| invalid(format!("jwks fetch failed: {e}")))?
        .error_for_status()
        .map_err(|e| invalid(format!("jwks fetch failed: {e}")))?
        .json()
        .await
        .map_err(|e| invalid(format!("jwks is not valid json: {e}")))?;
    for key in jwks.keys {
        if key.get("kid").and_then(Value::as_str) == Some(kid) {
            return serde_json::from_value(key)
                .map_err(|e| invalid(format!("unsupported jwk for kid {kid}: {e}")));
        }
    }
    Err(invalid(format!("no signing key for kid {kid} in the jwks")))
}

// ---------------------------------------------------------------------------
// login
// ---------------------------------------------------------------------------

/// Generate the PKCE verifier and its S256 challenge.
pub(crate) fn pkce_pair() -> (String, String) {
    let verifier = random_token();
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    (verifier, challenge)
}

pub(crate) fn random_token() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    rolter_auth::hex::encode(&bytes)
}

/// Begin a login: record the state and redirect to the provider.
async fn start_login(
    State(state): State<ControlState>,
    Path(slug): Path<String>,
) -> ApiResult<Response> {
    let provider = SsoRepo(pool(&state))
        .find_provider_by_slug(&slug)
        .await?
        .ok_or_else(|| invalid(format!("no enabled sso provider '{slug}'")))?;
    let discovery = discover(&provider.issuer).await?;
    let (verifier, challenge) = pkce_pair();
    let csrf_state = random_token();
    let nonce = random_token();
    let redirect = redirect_uri(&slug);
    SsoRepo(pool(&state))
        .start_login(&csrf_state, provider.id, &verifier, &nonce, &redirect)
        .await?;
    let url = authorize_url(
        &discovery,
        &provider,
        &redirect,
        &csrf_state,
        &nonce,
        &challenge,
    );
    Ok(Redirect::to(&url).into_response())
}

/// Build the authorization URL. Split out from the handler so the parameter set
/// can be asserted without a live provider.
fn authorize_url(
    discovery: &Discovery,
    provider: &SsoProvider,
    redirect: &str,
    state: &str,
    nonce: &str,
    challenge: &str,
) -> String {
    let scopes = if provider.scopes.is_empty() {
        "openid email profile".to_string()
    } else {
        provider.scopes.join(" ")
    };
    let sep = if discovery.authorization_endpoint.contains('?') {
        '&'
    } else {
        '?'
    };
    format!(
        "{}{sep}response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&nonce={}\
         &code_challenge={}&code_challenge_method=S256",
        discovery.authorization_endpoint,
        urlencode(&provider.client_id),
        urlencode(redirect),
        urlencode(&scopes),
        urlencode(state),
        urlencode(nonce),
        urlencode(challenge),
    )
}

/// Percent-encode a query parameter value. Small and dependency-free: the
/// alphabet below is RFC 3986 unreserved, so everything else is escaped.
pub(crate) fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    id_token: String,
}

/// Claims rolter reads off a verified id token.
#[derive(Debug, Deserialize)]
struct IdClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Serialize)]
struct SsoLoginResponse {
    token: String,
    expires_at: chrono::DateTime<Utc>,
    user: User,
    /// scopes the mapped groups granted, for the login screen to show
    granted_roles: Vec<String>,
}

async fn callback(
    State(state): State<ControlState>,
    Path(slug): Path<String>,
    Query(query): Query<CallbackQuery>,
) -> ApiResult<Json<SsoLoginResponse>> {
    if let Some(error) = query.error {
        let detail = query.error_description.unwrap_or_default();
        return Err(invalid(format!(
            "identity provider refused the login: {error} {detail}"
        )));
    }
    let code = query
        .code
        .ok_or_else(|| invalid("callback is missing code"))?;
    let csrf_state = query
        .state
        .ok_or_else(|| invalid("callback is missing state"))?;

    // one-shot: a replayed callback finds nothing to consume
    let login = SsoRepo(pool(&state))
        .consume_login(&csrf_state, LOGIN_STATE_TTL_SECS)
        .await?
        .ok_or_else(|| invalid("login state is unknown or expired; start the login again"))?;
    let provider = SsoRepo(pool(&state))
        .get_provider(login.provider_id)
        .await?;
    if provider.slug != slug {
        return Err(invalid("login state does not belong to this provider"));
    }
    if !OrgAuthPolicyRepo(pool(&state))
        .get(provider.org_id)
        .await?
        .allow_sso
    {
        // the org turned sso off; refuse without deleting the provider so it
        // can be switched back on
        return Err(ApiError::Forbidden);
    }
    let discovery = discover(&provider.issuer).await?;

    let kek = Kek::from_env()
        .ok_or_else(|| invalid("ROLTER_KEK must be configured to use the sso client secret"))?;
    let secret = SsoRepo(pool(&state)).client_secret(&kek, &provider).await?;

    let identity_provider = OidcIdentityProvider::new(discovery, provider.clone(), secret);
    let identity = identity_provider
        .resolve(Credential::AuthorizationCode {
            code,
            verifier: login.code_verifier.clone(),
            nonce: login.nonce.clone(),
        })
        .await
        .map_err(|e| match e {
            IdentityError::NotVerified => {
                invalid("the id token carries no email claim to key an account on")
            }
            IdentityError::UnsupportedCredential { .. } => {
                invalid("oidc provider was given an unsupported credential")
            }
            IdentityError::PolicyDenied(msg) | IdentityError::Provider(msg) => invalid(msg),
        })?;
    let email = identity.email.clone();

    let groups: HashSet<String> = identity.groups.iter().cloned().collect();
    let mappings = SsoRepo(pool(&state)).list_mappings(provider.id).await?;
    let matched: Vec<&SsoGroupMapping> = mappings
        .iter()
        .filter(|m| groups.contains(&m.group_name))
        .collect();
    // adopt an existing account by email, or create an sso-only one (no local
    // password: an sso account must not gain a second, weaker credential)
    let pool_ref = pool(&state);
    let known = UserRepo(pool_ref).find_by_email(&email).await?;
    if matched.is_empty() && provider.default_role.is_none() {
        // the IdP authenticated them but no group grants anything. if they held
        // sso-granted roles from an earlier login, this is exactly the moment
        // to take those away: being dropped from every mapped group is how an
        // operator deprovisions through the IdP
        if let Some(user) = &known {
            reconcile_grants(&state, provider.org_id, &[], user.id).await?;
        }
        return Err(ApiError::Forbidden);
    }
    let user = match known {
        Some(existing) => existing,
        None => UserRepo(pool_ref).create(&email, None, false).await?,
    };
    if user.deactivated_at.is_some() {
        // a deactivated account stays out regardless of what the IdP says
        return Err(ApiError::Forbidden);
    }

    let granted = apply_mappings(&state, &provider, &matched, user.id).await?;

    let (token, token_hash) = generate_session_token(&session_pepper());
    let expires_at = Utc::now() + Duration::hours(SESSION_TTL_HOURS);
    SessionRepo(pool_ref)
        .create(user.id, &token_hash, expires_at)
        .await?;
    let _ = AuditLogRepo(pool_ref)
        .create(
            Some(provider.org_id),
            Some(user.id),
            "auth.sso_login",
            Some("user"),
            Some(user.id),
            Some(json!({
                "provider": provider.slug,
                "subject": identity.subject,
                "granted_roles": granted,
            })),
        )
        .await;
    Ok(Json(SsoLoginResponse {
        token,
        expires_at,
        user,
        granted_roles: granted,
    }))
}

/// Exchange the authorization code for tokens.
async fn exchange_code(
    discovery: &Discovery,
    provider: &SsoProvider,
    secret: Option<&str>,
    code: &str,
    verifier: &str,
) -> ApiResult<String> {
    let mut form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("redirect_uri", redirect_uri(&provider.slug)),
        ("client_id", provider.client_id.clone()),
        ("code_verifier", verifier.to_string()),
    ];
    if let Some(secret) = secret {
        form.push(("client_secret", secret.to_string()));
    }
    let response = reqwest::Client::new()
        .post(&discovery.token_endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|e| invalid(format!("token exchange failed: {e}")))?;
    if !response.status().is_success() {
        let status = response.status();
        // the body may carry the client secret back in an error echo; report
        // the status only
        return Err(invalid(format!("token exchange rejected with {status}")));
    }
    let tokens: TokenResponse = response
        .json()
        .await
        .map_err(|e| invalid(format!("token response is not valid json: {e}")))?;
    Ok(tokens.id_token)
}

/// Verify the id token against the provider's JWKS, issuer, audience and the
/// nonce recorded when the login started.
async fn verify_id_token(
    discovery: &Discovery,
    provider: &SsoProvider,
    id_token: &str,
    nonce: &str,
) -> ApiResult<IdClaims> {
    let header = jsonwebtoken::decode_header(id_token)
        .map_err(|e| invalid(format!("id token header is unreadable: {e}")))?;
    if !ACCEPTED_ALGS.contains(&header.alg) {
        return Err(invalid(format!(
            "id token algorithm {:?} is not accepted",
            header.alg
        )));
    }
    let kid = header
        .kid
        .ok_or_else(|| invalid("id token has no kid to select a signing key"))?;
    let jwk = signing_key(&discovery.jwks_uri, &kid).await?;
    let key = jsonwebtoken::DecodingKey::from_jwk(&jwk)
        .map_err(|e| invalid(format!("signing key is unusable: {e}")))?;
    let mut validation = jsonwebtoken::Validation::new(header.alg);
    validation.set_issuer(&[discovery.issuer.as_str()]);
    validation.set_audience(&[provider.client_id.as_str()]);
    let data = jsonwebtoken::decode::<IdClaims>(id_token, &key, &validation)
        .map_err(|e| invalid(format!("id token verification failed: {e}")))?;
    // the nonce ties this token to the login that started here, so a token
    // minted for another session cannot be replayed into this one
    if data.claims.nonce.as_deref() != Some(nonce) {
        return Err(invalid("id token nonce does not match the login"));
    }
    Ok(data.claims)
}

/// Read the group list out of the configured claim. Providers disagree on
/// shape: an array of strings, a single string, or a space-separated list.
fn groups_from_claims(claims: &IdClaims, claim: &str) -> HashSet<String> {
    let Some(value) = claims.extra.get(claim) else {
        return HashSet::new();
    };
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(normalize_group)
            .collect(),
        Value::String(single) if single.contains(' ') => {
            single.split_whitespace().map(normalize_group).collect()
        }
        Value::String(single) => HashSet::from([normalize_group(single)]),
        _ => HashSet::new(),
    }
}

/// Keycloak prefixes realm groups with `/`; strip it so an operator maps the
/// group name they see in the IdP UI.
fn normalize_group(group: &str) -> String {
    group.trim().trim_start_matches('/').to_string()
}

/// One membership a login should produce: `(org, team, project, role)`, with
/// the same "most specific non-null id" convention as `memberships`.
type ScopedGrant = (Option<Uuid>, Option<Uuid>, Option<Uuid>, String);

/// Reconcile the memberships this login implies: the matched mappings, or the
/// provider's default role when nothing matched.
///
/// Only `source = 'sso'` rows inside the provider's org are reconciled. A role
/// an operator granted by hand — through the admin API or an invitation —
/// carries `source = 'manual'` and survives untouched, so the two enrolment
/// paths can be used side by side. Removing a user from an IdP group does
/// revoke the role that group granted, on their next login.
async fn apply_mappings(
    state: &ControlState,
    provider: &SsoProvider,
    matched: &[&SsoGroupMapping],
    user_id: Uuid,
) -> ApiResult<Vec<String>> {
    let mut wanted: Vec<ScopedGrant> = matched
        .iter()
        .map(|m| (m.org_id, m.team_id, m.project_id, m.role.clone()))
        .collect();
    if wanted.is_empty() {
        if let Some(role) = &provider.default_role {
            wanted.push((Some(provider.org_id), None, None, role.clone()));
        }
    }
    reconcile_grants(state, provider.org_id, &wanted, user_id).await
}

/// Make the user's `sso`-sourced memberships inside `org_id` be exactly
/// `wanted`, leaving `manual` rows alone, and return the roles now in force.
async fn reconcile_grants(
    state: &ControlState,
    org_id: Uuid,
    wanted: &[ScopedGrant],
    user_id: Uuid,
) -> ApiResult<Vec<String>> {
    let repo = MembershipRepo(pool(state));
    // scoped to the provider's org tree: another org's sso grants belong to
    // that org's provider and are none of this login's business
    let in_org = repo.list_in_org(org_id).await?;
    let existing: Vec<_> = in_org
        .into_iter()
        .filter(|m| m.user_id == user_id)
        .collect();

    for stale in existing
        .iter()
        .filter(|m| m.source == "sso")
        .filter(|m| !wanted.iter().any(|w| grant_matches(m, w)))
    {
        repo.delete(stale.id).await?;
    }

    let mut granted = Vec::new();
    for want in wanted {
        granted.push(want.3.clone());
        if existing.iter().any(|m| grant_matches(m, want)) {
            continue;
        }
        repo.create_with_source(user_id, want.0, want.1, want.2, &want.3, "sso")
            .await?;
    }
    Ok(granted)
}

fn grant_matches(membership: &Membership, want: &ScopedGrant) -> bool {
    membership.org_id == want.0
        && membership.team_id == want.1
        && membership.project_id == want.2
        && membership.role == want.3
}

fn generate_session_token(pepper: &str) -> (String, String) {
    let token = format!("rolter_sess_{}", random_token());
    let hash = rolter_auth::hash_key(pepper, &token);
    (token, hash)
}

// ---------------------------------------------------------------------------
// admin api
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateSsoProvider {
    name: String,
    slug: String,
    issuer: String,
    client_id: String,
    /// write-only; sealed before it reaches the database and never returned
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    scopes: Option<Vec<String>>,
    #[serde(default)]
    group_claim: Option<String>,
    /// role for a user in no mapped group; omit to refuse those users
    #[serde(default)]
    default_role: Option<String>,
}

async fn create_provider(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateSsoProvider>,
) -> ApiResult<Json<SsoProvider>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("sso_provider", Create),
    )
    .await?;
    require_non_empty(&body.name, "name")?;
    require_non_empty(&body.slug, "slug")?;
    require_non_empty(&body.issuer, "issuer")?;
    require_non_empty(&body.client_id, "client_id")?;
    if !body.issuer.starts_with("https://") && !body.issuer.starts_with("http://") {
        return Err(invalid("issuer must be an http(s) url"));
    }
    if let Some(role) = &body.default_role {
        parse_role(role)?;
    }
    let sealed = match &body.client_secret {
        Some(secret) if !secret.is_empty() => {
            let kek = Kek::from_env()
                .ok_or_else(|| invalid("ROLTER_KEK must be configured to store a client secret"))?;
            Some(kek.encrypt(secret)?)
        }
        _ => None,
    };
    let scopes = body
        .scopes
        .unwrap_or_else(|| vec!["openid".into(), "email".into(), "profile".into()]);
    let provider = SsoRepo(pool(&state))
        .create_provider(
            org_id,
            &body.name,
            &body.slug,
            body.issuer.trim_end_matches('/'),
            &body.client_id,
            sealed.as_ref().map(|(c, n)| (c.as_slice(), n.as_slice())),
            &scopes,
            body.group_claim.as_deref().unwrap_or("groups"),
            body.default_role.as_deref(),
        )
        .await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "sso_provider.create",
        "sso_provider",
        provider.id,
        json!({"slug": provider.slug, "issuer": provider.issuer}),
    )
    .await;
    Ok(Json(provider))
}

/// The roles a group mapping may grant. Shared with
/// [`crate::scim_groups`], so both provisioning paths accept exactly the same
/// set and neither can hand out something the other refuses.
pub(crate) fn parse_role(role: &str) -> ApiResult<()> {
    if matches!(role, "admin" | "member" | "viewer") {
        Ok(())
    } else {
        Err(invalid("role must be one of admin, member, viewer"))
    }
}

async fn list_providers(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<SsoProvider>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("sso_provider", Read),
    )
    .await?;
    Ok(Json(SsoRepo(pool(&state)).list_providers(org_id).await?))
}

#[derive(Debug, Deserialize)]
struct UpdateSsoProvider {
    name: String,
    issuer: String,
    client_id: String,
    /// write-only and three-valued: absent leaves the sealed secret alone, an
    /// empty string clears it (the provider becomes a public PKCE client), and
    /// a value replaces it. That is what makes a rotation at the IdP possible
    /// without deleting the provider and losing its group mappings (#1233).
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    scopes: Option<Vec<String>>,
    #[serde(default)]
    group_claim: Option<String>,
    /// role for a user in no mapped group; omit to refuse those users
    #[serde(default)]
    default_role: Option<String>,
    /// taking a provider out of service without deleting it
    #[serde(default = "default_enabled")]
    enabled: bool,
}

fn default_enabled() -> bool {
    true
}

/// Edit a registered provider in place.
///
/// `slug` is not accepted: it is in the login URL, so renaming it would break
/// every bookmark and IdP redirect already pointing at the old one. A provider
/// that genuinely needs a different slug is a different provider.
async fn update_provider(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateSsoProvider>,
) -> ApiResult<Json<SsoProvider>> {
    let repo = SsoRepo(pool(&state));
    let existing = repo.get_provider(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("sso_provider", Update),
    )
    .await?;
    require_non_empty(&body.name, "name")?;
    require_non_empty(&body.issuer, "issuer")?;
    require_non_empty(&body.client_id, "client_id")?;
    if !body.issuer.starts_with("https://") && !body.issuer.starts_with("http://") {
        return Err(invalid("issuer must be an http(s) url"));
    }
    if let Some(role) = &body.default_role {
        parse_role(role)?;
    }
    // sealing happens before anything is written, so a missing ROLTER_KEK
    // fails the whole request rather than half-applying the edit
    let sealed = match &body.client_secret {
        Some(secret) if !secret.is_empty() => {
            let kek = Kek::from_env()
                .ok_or_else(|| invalid("ROLTER_KEK must be configured to store a client secret"))?;
            Some(kek.encrypt(secret)?)
        }
        _ => None,
    };
    let secret = match (&body.client_secret, &sealed) {
        (Some(_), Some((ciphertext, nonce))) => SecretUpdate::Set(ciphertext, nonce),
        (Some(_), None) => SecretUpdate::Clear,
        (None, _) => SecretUpdate::Keep,
    };
    let scopes = body.scopes.clone().unwrap_or(existing.scopes.clone());
    let issuer = body.issuer.trim_end_matches('/');
    let provider = repo
        .update_provider(
            id,
            SsoProviderUpdate {
                name: &body.name,
                issuer,
                client_id: &body.client_id,
                secret,
                scopes: &scopes,
                group_claim: body
                    .group_claim
                    .as_deref()
                    .unwrap_or(existing.group_claim.as_str()),
                default_role: body.default_role.as_deref(),
                enabled: body.enabled,
            },
        )
        .await?;
    // the audit line says what moved, never what the secret is: whether it was
    // rotated is the interesting fact, and the only one safe to record
    log_audit(
        &state,
        &principal,
        Some(provider.org_id),
        "sso_provider.update",
        "sso_provider",
        provider.id,
        json!({
            "slug": provider.slug,
            "issuer": provider.issuer,
            "enabled": provider.enabled,
            "client_secret": match secret {
                SecretUpdate::Keep => "unchanged",
                SecretUpdate::Clear => "cleared",
                SecretUpdate::Set(..) => "rotated",
            },
        }),
    )
    .await;
    Ok(Json(provider))
}

async fn delete_provider(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = SsoRepo(pool(&state));
    let provider = repo.get_provider(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(provider.org_id),
        cap!("sso_provider", Delete),
    )
    .await?;
    repo.delete_provider(id).await?;
    log_audit(
        &state,
        &principal,
        Some(provider.org_id),
        "sso_provider.delete",
        "sso_provider",
        id,
        json!({"slug": provider.slug}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct CreateMapping {
    group_name: String,
    role: String,
    #[serde(default)]
    org_id: Option<Uuid>,
    #[serde(default)]
    team_id: Option<Uuid>,
    #[serde(default)]
    project_id: Option<Uuid>,
}

async fn create_mapping(
    principal: Principal,
    State(state): State<ControlState>,
    Path(provider_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateMapping>,
) -> ApiResult<Json<SsoGroupMapping>> {
    let repo = SsoRepo(pool(&state));
    let provider = repo.get_provider(provider_id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(provider.org_id),
        cap!("sso_group_mapping", Create),
    )
    .await?;
    require_non_empty(&body.group_name, "group_name")?;
    parse_role(&body.role)?;
    // a mapping may only grant inside the provider's own org, or an IdP admin
    // could hand their users a foothold in someone else's tenant
    let scope_org = match (body.org_id, body.team_id, body.project_id) {
        (Some(org), None, None) => org,
        (_, Some(team), None) => ScopeChain::from_team(pool(&state), team)
            .await?
            .org
            .unwrap_or_default(),
        (_, _, Some(project)) => ScopeChain::from_project(pool(&state), project)
            .await?
            .org
            .unwrap_or_default(),
        (None, None, None) => provider.org_id,
    };
    if scope_org != provider.org_id {
        return Err(invalid(
            "a group mapping may only grant a role inside the provider's own org",
        ));
    }
    let mapping = repo
        .add_mapping(
            provider_id,
            &body.group_name,
            // an org-scoped grant only when the mapping names neither a team
            // nor a project; a narrower scope carries the org implicitly
            (body.team_id.is_none() && body.project_id.is_none())
                .then(|| body.org_id.unwrap_or(provider.org_id)),
            body.team_id,
            body.project_id,
            &body.role,
        )
        .await?;
    log_audit(
        &state,
        &principal,
        Some(provider.org_id),
        "sso_group_mapping.create",
        "sso_group_mapping",
        mapping.id,
        json!({"group": mapping.group_name, "role": mapping.role}),
    )
    .await;
    Ok(Json(mapping))
}

async fn list_mappings(
    principal: Principal,
    State(state): State<ControlState>,
    Path(provider_id): Path<Uuid>,
) -> ApiResult<Json<Vec<SsoGroupMapping>>> {
    let repo = SsoRepo(pool(&state));
    let provider = repo.get_provider(provider_id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(provider.org_id),
        cap!("sso_group_mapping", Read),
    )
    .await?;
    Ok(Json(repo.list_mappings(provider_id).await?))
}

async fn delete_mapping(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    // the mapping's provider decides who may remove it
    let repo = SsoRepo(pool(&state));
    let mapping = sqlx_mapping(&state, id).await?;
    let provider = repo.get_provider(mapping.provider_id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(provider.org_id),
        cap!("sso_group_mapping", Delete),
    )
    .await?;
    repo.delete_mapping(id).await?;
    log_audit(
        &state,
        &principal,
        Some(provider.org_id),
        "sso_group_mapping.delete",
        "sso_group_mapping",
        id,
        json!({"group": mapping.group_name}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Look a mapping up by id so its provider (and therefore its org) can be
/// authorized before deletion.
async fn sqlx_mapping(state: &ControlState, id: Uuid) -> ApiResult<SsoGroupMapping> {
    let mapping: Option<SsoGroupMapping> = sqlx::query_as(
        "select id, provider_id, group_name, org_id, team_id, project_id, role, created_at \
         from sso_group_mappings where id = $1",
    )
    .bind(id)
    .fetch_optional(pool(state))
    .await
    .map_err(|e| ApiError::Core(rolter_core::Error::Store(e.to_string())))?;
    mapping.ok_or_else(|| {
        ApiError::Core(rolter_core::Error::NotFound(format!(
            "sso group mapping {id}"
        )))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims_with(groups: Value) -> IdClaims {
        let mut extra = serde_json::Map::new();
        extra.insert("groups".to_string(), groups);
        IdClaims {
            sub: "sub-1".into(),
            email: Some("ada@example.com".into()),
            preferred_username: None,
            nonce: Some("n".into()),
            extra,
        }
    }

    #[test]
    fn reads_groups_in_every_shape_providers_send() {
        let array = claims_with(json!(["/platform", "sre"]));
        let parsed = groups_from_claims(&array, "groups");
        // keycloak's leading slash is stripped so operators map what they see
        assert!(parsed.contains("platform") && parsed.contains("sre"));

        let single = claims_with(json!("platform"));
        assert!(groups_from_claims(&single, "groups").contains("platform"));

        let spaced = claims_with(json!("platform sre"));
        let parsed = groups_from_claims(&spaced, "groups");
        assert_eq!(parsed.len(), 2);

        // an absent or unusable claim is no groups, never an error that would
        // block a login the default role should have allowed
        assert!(groups_from_claims(&array, "roles").is_empty());
        assert!(groups_from_claims(&claims_with(json!(42)), "groups").is_empty());
    }

    #[test]
    fn symmetric_and_none_algorithms_are_not_accepted() {
        // with a shared client secret an HS256 token is forgeable by anything
        // that can read the config, so only asymmetric algorithms are allowed
        assert!(!ACCEPTED_ALGS.contains(&jsonwebtoken::Algorithm::HS256));
        assert!(ACCEPTED_ALGS.contains(&jsonwebtoken::Algorithm::RS256));
    }

    #[test]
    fn pkce_challenge_is_the_s256_of_the_verifier() {
        let (verifier, challenge) = pkce_pair();
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, expected);
        // and two logins never share a verifier
        let (other, _) = pkce_pair();
        assert_ne!(verifier, other);
    }

    #[test]
    fn authorize_url_carries_pkce_state_and_nonce() {
        let discovery = Discovery {
            issuer: "https://idp.example.com".into(),
            authorization_endpoint: "https://idp.example.com/auth".into(),
            token_endpoint: "https://idp.example.com/token".into(),
            jwks_uri: "https://idp.example.com/jwks".into(),
        };
        let provider = SsoProvider {
            id: Uuid::nil(),
            org_id: Uuid::nil(),
            name: "Keycloak".into(),
            slug: "keycloak".into(),
            issuer: "https://idp.example.com".into(),
            client_id: "rolter dashboard".into(),
            secret_ciphertext: None,
            secret_nonce: None,
            scopes: vec!["openid".into(), "email".into()],
            group_claim: "groups".into(),
            default_role: None,
            enabled: true,
            created_at: Utc::now(),
        };
        let url = authorize_url(
            &discovery,
            &provider,
            "https://rolter.example.com/auth/sso/keycloak/callback",
            "state-1",
            "nonce-1",
            "challenge-1",
        );
        assert!(url.starts_with("https://idp.example.com/auth?response_type=code"));
        assert!(url.contains("code_challenge=challenge-1&code_challenge_method=S256"));
        assert!(url.contains("state=state-1"));
        assert!(url.contains("nonce=nonce-1"));
        // the client id and redirect are escaped, not interpolated raw
        assert!(url.contains("client_id=rolter%20dashboard"));
        assert!(url.contains(
            "redirect_uri=https%3A%2F%2Frolter.example.com%2Fauth%2Fsso%2Fkeycloak%2Fcallback"
        ));
        assert!(url.contains("scope=openid%20email"));
    }

    #[test]
    fn redirect_uri_comes_from_configuration_not_the_request() {
        // an open redirect here would hand an authorization code to whoever
        // controls the Host header, so the value is deployment-owned
        let uri = redirect_uri("keycloak");
        assert!(uri.ends_with("/auth/sso/keycloak/callback"));
        assert!(uri.starts_with(&public_base_url()));
    }

    #[test]
    fn only_the_three_built_in_roles_are_mappable() {
        assert!(parse_role("admin").is_ok());
        assert!(parse_role("member").is_ok());
        assert!(parse_role("viewer").is_ok());
        assert!(parse_role("superadmin").is_err());
        assert!(parse_role("").is_err());
    }
}
