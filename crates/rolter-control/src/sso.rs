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
use axum::http::{header, HeaderMap, StatusCode};
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
    AuditLogRepo, LockoutGuard, MembershipRepo, OrgAuthPolicyRepo, SecretUpdate, SessionRepo,
    SsoProviderUpdate, SsoRepo, UserRepo,
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
    /// the redirect URI recorded when the login started, replayed verbatim on
    /// the token exchange. OAuth requires the two to match, so it is read back
    /// from the login state rather than recomputed
    redirect_uri: String,
}

impl OidcIdentityProvider {
    pub(crate) fn new(
        discovery: Discovery,
        provider: SsoProvider,
        secret: Option<String>,
        redirect_uri: String,
    ) -> Self {
        Self {
            discovery,
            provider,
            secret,
            redirect_uri,
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
            &self.redirect_uri,
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
        .route("/auth/sso/exchange", post(exchange))
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
        ApiError::Curated(msg)
        | ApiError::Conflict(msg)
        | ApiError::CodedConflict { message: msg, .. } => msg,
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
///
/// It is resolved once when the state is built, so every read within a login
/// flow sees the same value; see [`crate::ControlState::public_url`].
pub(crate) fn public_base_url(state: &ControlState) -> &str {
    &state.public_url.base
}

/// The callback a provider sends the authorization code to, and so the value
/// an operator registers with the identity provider.
fn redirect_uri(base: &str, slug: &str) -> String {
    format!("{base}/auth/sso/{slug}/callback")
}

/// Where a user's sign-in through a provider starts: the address the login
/// screen's button points at, absolute so it can be bookmarked or linked.
fn login_url(base: &str, slug: &str) -> String {
    format!("{base}/auth/sso/{slug}/start")
}

/// A provider as the admin API returns it: the stored row, plus the two
/// addresses derived from the deployment's public base URL (#2083).
///
/// Both are built by the same functions the login flow uses, so the redirect
/// URI an operator copies into the identity provider is byte for byte the one
/// [`start_login`] sends. The dashboard used to assemble them from the
/// browser's own origin, which disagrees with the configured base behind a
/// proxy or under a second hostname, and the IdP then refuses the login with a
/// redirect mismatch.
#[derive(Debug, Serialize)]
struct ProviderView {
    #[serde(flatten)]
    provider: SsoProvider,
    redirect_uri: String,
    login_url: String,
}

impl ProviderView {
    fn new(provider: SsoProvider, base: &str) -> Self {
        Self {
            redirect_uri: redirect_uri(base, &provider.slug),
            login_url: login_url(base, &provider.slug),
            provider,
        }
    }
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
    let redirect = redirect_uri(public_base_url(&state), &slug);
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

/// The dashboard route a browser's sign-in ends on (#2297): the login screen,
/// which reads the outcome from its query string. The route and the parameters
/// below are the contract with the dashboard.
///
/// * success: `/login?sso_code=<one-time code>`, redeemed by
///   `POST /auth/sso/exchange`
/// * refusal: `/login?sso_error=<code>` and, once the state has named the
///   provider, `&sso=<slug>`
const LOGIN_PATH: &str = "/login";

/// How long an exchange code may sit between the redirect and the dashboard
/// redeeming it. The dashboard posts it the moment it loads, so a minute is
/// generous; a code lifted from history or a log is dead by the time anyone
/// reads it.
const EXCHANGE_TTL_SECS: i64 = 60;

/// Why a browser sign-in did not complete, as the stable `sso_error` the
/// dashboard is redirected with (#2297).
///
/// The dashboard translates each code into its own words, so a code is part of
/// the contract with it: never rename one, and add a translation for every new
/// one. Nothing the identity provider said reaches the URL, only the family
/// the failure falls in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SsoFailure {
    /// the provider answered with an `error`: the user declined, or its policy
    /// refused
    IdpError,
    /// no `state` or `code`, or a state that is unknown, expired, already
    /// redeemed or belongs to another provider; a replay lands here
    StateExpired,
    /// the org turned sso off
    SsoDisabled,
    /// the user is in no mapped group and the provider has no default role
    NoMappedGroup,
    /// the account is deactivated
    AccountDeactivated,
    /// the provider could not be reached, or its token or id token did not
    /// verify
    IdpVerificationFailed,
    /// the deployment cannot finish a sign-in, e.g. no KEK
    NotConfigured,
    /// anything else, a database error most likely
    InternalError,
}

impl SsoFailure {
    /// The code the dashboard is sent.
    const fn code(self) -> &'static str {
        match self {
            Self::IdpError => "idp_error",
            Self::StateExpired => "state_expired",
            Self::SsoDisabled => "sso_disabled",
            Self::NoMappedGroup => "no_mapped_group",
            Self::AccountDeactivated => "account_deactivated",
            Self::IdpVerificationFailed => "idp_verification_failed",
            Self::NotConfigured => "not_configured",
            Self::InternalError => "internal_error",
        }
    }
}

/// A callback that did not complete: the family for a browser, the error a
/// JSON caller has always been given, and the provider's slug once the login
/// state has vouched for it.
#[derive(Debug)]
struct CallbackFailure {
    reason: SsoFailure,
    slug: Option<String>,
    error: ApiError,
}

/// `map_err` for one step of [`complete_login`].
fn failed<E: Into<ApiError>>(
    reason: SsoFailure,
    slug: Option<&str>,
) -> impl FnOnce(E) -> CallbackFailure + '_ {
    move |error| CallbackFailure {
        reason,
        slug: slug.map(str::to_string),
        error: error.into(),
    }
}

/// An identity that cleared every check and is owed a session.
struct SignedIn {
    provider: SsoProvider,
    user: User,
    subject: String,
    granted: Vec<String>,
}

/// Where a browser is sent when the sign-in did not complete. The query string
/// carries a code from a closed set and the provider's slug, never anything the
/// identity provider said.
fn refusal_url(base: &str, failure: &CallbackFailure) -> String {
    let mut url = format!("{base}{LOGIN_PATH}?sso_error={}", failure.reason.code());
    if let Some(slug) = &failure.slug {
        url.push_str(&format!("&sso={}", urlencode(slug)));
    }
    url
}

/// Where a browser is sent once it is signed in: the login screen, holding a
/// one-time code. The code is not a credential until it is redeemed, and only
/// once.
fn success_url(base: &str, code: &str) -> String {
    format!("{base}{LOGIN_PATH}?sso_code={}", urlencode(code))
}

fn exchange_code_hash(code: &str) -> String {
    hex_digest(code)
}

fn hex_digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The provider's redirect target.
///
/// A browser is answered with a `303` to the login screen (#2297): holding a
/// one-time code on success, a stable `sso_error` on a refusal. It used to be
/// left on a raw JSON body, with the session token printed in the tab and
/// nothing to move it into the dashboard. Any other caller gets the JSON it
/// always did. The response varies on `Accept` either way.
async fn callback(
    State(state): State<ControlState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let browser = crate::mcp_oauth_flow::prefers_html(&headers);
    let response = match complete_login(&state, &slug, query).await {
        Ok(signed) if browser => {
            let base = public_base_url(&state);
            match issue_exchange(&state, &signed).await {
                Ok(code) => Redirect::to(&success_url(base, &code)).into_response(),
                Err(error) => {
                    let failure = CallbackFailure {
                        reason: SsoFailure::InternalError,
                        slug: Some(signed.provider.slug.clone()),
                        error,
                    };
                    tracing::warn!(reason = failure.reason.code(), error = ?failure.error, "sso sign-in did not complete");
                    Redirect::to(&refusal_url(base, &failure)).into_response()
                }
            }
        }
        Ok(signed) => match sign_in_json(&state, signed).await {
            Ok(body) => Json(body).into_response(),
            Err(error) => error.into_response(),
        },
        Err(failure) => {
            // a browser only learns the family, so the detail an operator
            // needs to act on is kept here
            tracing::info!(
                reason = failure.reason.code(),
                slug = ?failure.slug,
                error = ?failure.error,
                "sso sign-in did not complete"
            );
            if browser {
                Redirect::to(&refusal_url(public_base_url(&state), &failure)).into_response()
            } else {
                failure.error.into_response()
            }
        }
    };
    ([(header::VARY, "accept")], response).into_response()
}

/// Mint the session and audit the sign-in: what a JSON caller is answered with.
async fn sign_in_json(state: &ControlState, signed: SignedIn) -> ApiResult<SsoLoginResponse> {
    let pool_ref = pool(state);
    let (token, token_hash) = generate_session_token(&session_pepper());
    let expires_at = Utc::now() + Duration::hours(SESSION_TTL_HOURS);
    SessionRepo(pool_ref)
        .create(signed.user.id, &token_hash, expires_at)
        .await?;
    audit_sign_in(state, &signed).await;
    Ok(SsoLoginResponse {
        token,
        expires_at,
        user: signed.user,
        granted_roles: signed.granted,
    })
}

/// Store the one-time code a browser is handed in place of a session. The
/// session itself is minted when the code is redeemed, so no bearer token
/// rests in the database between the two.
async fn issue_exchange(state: &ControlState, signed: &SignedIn) -> ApiResult<String> {
    let code = random_token();
    SsoRepo(pool(state))
        .issue_exchange(
            &exchange_code_hash(&code),
            signed.user.id,
            signed.provider.id,
            &signed.granted,
            EXCHANGE_TTL_SECS,
        )
        .await?;
    // the sign-in is audited here, where the identity was accepted; the
    // redemption mints a session for it and does not audit a second time
    audit_sign_in(state, signed).await;
    Ok(code)
}

async fn audit_sign_in(state: &ControlState, signed: &SignedIn) {
    let _ = AuditLogRepo(pool(state))
        .create(
            Some(signed.provider.org_id),
            Some(signed.user.id),
            "auth.sso_login",
            Some("user"),
            Some(signed.user.id),
            Some(json!({
                "provider": signed.provider.slug,
                "subject": signed.subject,
                "granted_roles": signed.granted,
            })),
        )
        .await;
}

#[derive(Debug, Deserialize)]
struct ExchangeRequest {
    code: String,
}

/// `POST /auth/sso/exchange`: redeem the one-time code a browser sign-in ended
/// with for the session it stands for. Public, like the callback it follows:
/// the code is the credential. It is 256 random bits, single-use and gone in a
/// minute, so guessing one is not a thing a throttle would help with; the login
/// throttle is keyed on an email and an address and has nothing to key on here.
///
/// An unknown, spent and expired code are all the same `400`, so the endpoint
/// is no oracle for which codes once existed.
async fn exchange(
    State(state): State<ControlState>,
    Json(body): Json<ExchangeRequest>,
) -> Response {
    match redeem_exchange(&state, &body.code).await {
        Ok(Some(login)) => Json(login).into_response(),
        Ok(None) => {
            tracing::info!("sso exchange refused: unknown, spent or expired code");
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": {
                    "code": "invalid_exchange_code",
                    "message": "the sign-in code is unknown, already used or expired; sign in again",
                }})),
            )
                .into_response()
        }
        Err(error) => {
            tracing::warn!(?error, "sso exchange failed");
            error.into_response()
        }
    }
}

async fn redeem_exchange(state: &ControlState, code: &str) -> ApiResult<Option<SsoLoginResponse>> {
    let pool_ref = pool(state);
    let Some(redeemed) = SsoRepo(pool_ref)
        .redeem_exchange(&exchange_code_hash(code))
        .await?
    else {
        return Ok(None);
    };
    let user = UserRepo(pool_ref).get(redeemed.user_id).await?;
    if user.deactivated_at.is_some() {
        // deactivated between the callback and the redemption
        return Ok(None);
    }
    let (token, token_hash) = generate_session_token(&session_pepper());
    let expires_at = Utc::now() + Duration::hours(SESSION_TTL_HOURS);
    SessionRepo(pool_ref)
        .create(user.id, &token_hash, expires_at)
        .await?;
    Ok(Some(SsoLoginResponse {
        token,
        expires_at,
        user,
        granted_roles: redeemed.granted_roles,
    }))
}

/// Verify the provider's response and resolve the account it is for, up to but
/// not including the session.
async fn complete_login(
    state: &ControlState,
    slug: &str,
    query: CallbackQuery,
) -> Result<SignedIn, CallbackFailure> {
    use SsoFailure::*;
    if let Some(error) = query.error {
        let detail = query.error_description.unwrap_or_default();
        return Err(failed(IdpError, None)(invalid(format!(
            "identity provider refused the login: {error} {detail}"
        ))));
    }
    let code = query
        .code
        .ok_or_else(|| failed(StateExpired, None)(invalid("callback is missing code")))?;
    let csrf_state = query
        .state
        .ok_or_else(|| failed(StateExpired, None)(invalid("callback is missing state")))?;

    // one-shot: a replayed callback finds nothing to consume
    let login = SsoRepo(pool(state))
        .consume_login(&csrf_state, LOGIN_STATE_TTL_SECS)
        .await
        .map_err(failed(InternalError, None))?
        .ok_or_else(|| {
            failed(StateExpired, None)(invalid(
                "login state is unknown or expired; start the login again",
            ))
        })?;
    let provider = SsoRepo(pool(state))
        .get_provider(login.provider_id)
        .await
        .map_err(failed(InternalError, None))?;
    if provider.slug != slug {
        return Err(failed(StateExpired, None)(invalid(
            "login state does not belong to this provider",
        )));
    }
    // the state has vouched for the provider, so its slug is safe to name
    let named = Some(provider.slug.as_str());
    if !OrgAuthPolicyRepo(pool(state))
        .get(provider.org_id)
        .await
        .map_err(failed(InternalError, named))?
        .allow_sso
    {
        // the org turned sso off; refuse without deleting the provider so it
        // can be switched back on
        return Err(failed(SsoDisabled, named)(ApiError::Forbidden));
    }
    let discovery = discover(&provider.issuer)
        .await
        .map_err(failed(IdpVerificationFailed, named))?;

    let kek = Kek::from_env().ok_or_else(|| {
        failed(NotConfigured, named)(invalid(
            "ROLTER_KEK must be configured to use the sso client secret",
        ))
    })?;
    let secret = SsoRepo(pool(state))
        .client_secret(&kek, &provider)
        .await
        .map_err(failed(InternalError, named))?;

    let identity_provider = OidcIdentityProvider::new(
        discovery,
        provider.clone(),
        secret,
        login.redirect_uri.clone(),
    );
    let identity = identity_provider
        .resolve(Credential::AuthorizationCode {
            code,
            verifier: login.code_verifier.clone(),
            nonce: login.nonce.clone(),
        })
        .await
        .map_err(|e| {
            let message = match e {
                IdentityError::NotVerified => {
                    "the id token carries no email claim to key an account on".to_string()
                }
                IdentityError::UnsupportedCredential { .. } => {
                    "oidc provider was given an unsupported credential".to_string()
                }
                IdentityError::PolicyDenied(msg) | IdentityError::Provider(msg) => msg,
            };
            failed(IdpVerificationFailed, named)(invalid(message))
        })?;
    let email = identity.email.clone();

    let groups: HashSet<String> = identity.groups.iter().cloned().collect();
    let mappings = SsoRepo(pool(state))
        .list_mappings(provider.id)
        .await
        .map_err(failed(InternalError, named))?;
    let matched: Vec<&SsoGroupMapping> = mappings
        .iter()
        .filter(|m| groups.contains(&m.group_name))
        .collect();
    // adopt an existing account by email, or create an sso-only one (no local
    // password: an sso account must not gain a second, weaker credential)
    let pool_ref = pool(state);
    let known = UserRepo(pool_ref)
        .find_by_email(&email)
        .await
        .map_err(failed(InternalError, named))?;
    if matched.is_empty() && provider.default_role.is_none() {
        // the IdP authenticated them but no group grants anything. if they held
        // sso-granted roles from an earlier login, this is exactly the moment
        // to take those away: being dropped from every mapped group is how an
        // operator deprovisions through the IdP
        if let Some(user) = &known {
            reconcile_grants(state, provider.org_id, &[], user.id)
                .await
                .map_err(failed(InternalError, named))?;
        }
        return Err(failed(NoMappedGroup, named)(ApiError::Forbidden));
    }
    let user = match known {
        Some(existing) => existing,
        None => UserRepo(pool_ref)
            .create(&email, None, false)
            .await
            .map_err(failed(InternalError, named))?,
    };
    if user.deactivated_at.is_some() {
        // a deactivated account stays out regardless of what the IdP says
        return Err(failed(AccountDeactivated, named)(ApiError::Forbidden));
    }

    let granted = apply_mappings(state, &provider, &matched, user.id)
        .await
        .map_err(failed(InternalError, named))?;
    Ok(SignedIn {
        provider,
        user,
        subject: identity.subject,
        granted,
    })
}

/// Exchange the authorization code for tokens.
async fn exchange_code(
    discovery: &Discovery,
    provider: &SsoProvider,
    secret: Option<&str>,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> ApiResult<String> {
    let mut form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("redirect_uri", redirect_uri.to_string()),
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

/// Longest slug the store accepts, in characters.
const SLUG_MAX_LEN: usize = 63;

/// Whether `slug` satisfies the store's `sso_providers_slug_charset`
/// constraint, `^[a-z0-9][a-z0-9-]{0,62}$` (migration `0047`).
///
/// Written out byte by byte rather than as a regex: the rule is ASCII-only, so
/// a byte is a character wherever it can pass, and any non-ASCII byte fails the
/// charset test anyway.
fn slug_is_valid(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    let starts_alphanumeric = bytes
        .first()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    starts_alphanumeric
        && bytes.len() <= SLUG_MAX_LEN
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// Refuse a slug the store's check constraint would, with a message that states
/// the rule (#2304).
///
/// Without this the insert fails and the caller reads a store error carrying
/// the constraint's name, which says nothing about what to type instead. The
/// slug is registered at the identity provider as part of the redirect URI, so
/// it is checked exactly as sent and never trimmed or lowercased on the way in.
fn validate_slug(slug: &str) -> ApiResult<()> {
    if slug_is_valid(slug) {
        Ok(())
    } else {
        Err(invalid(format!(
            "slug must be lowercase letters, digits and hyphens, start with a letter or digit, \
             and be at most {SLUG_MAX_LEN} characters"
        )))
    }
}

async fn create_provider(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateSsoProvider>,
) -> ApiResult<Json<ProviderView>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("sso_provider", Create),
    )
    .await?;
    require_non_empty(&body.name, "name")?;
    require_non_empty(&body.slug, "slug")?;
    validate_slug(&body.slug)?;
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
    Ok(Json(ProviderView::new(provider, public_base_url(&state))))
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
) -> ApiResult<Json<Vec<ProviderView>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("sso_provider", Read),
    )
    .await?;
    let base = public_base_url(&state);
    let providers = SsoRepo(pool(&state)).list_providers(org_id).await?;
    Ok(Json(
        providers
            .into_iter()
            .map(|provider| ProviderView::new(provider, base))
            .collect(),
    ))
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

/// The refusal for a write that would leave the org with no sign-in method.
///
/// The mirror of the guard in `auth_policy.rs` that refuses turning passwords
/// off before a provider exists: together they keep "password sign-in off and
/// no enabled provider" from ever being reachable (#2233). Only a superadmin,
/// who is exempt from the password setting, could still sign in.
fn last_sign_in_method(verb: &str) -> ApiError {
    ApiError::Conflict(format!(
        "cannot {verb} the last enabled sso provider while password sign-in is off: no member \
         could sign in. Enable password sign-in or another sso provider first"
    ))
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
) -> ApiResult<Json<ProviderView>> {
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
    let provider = match repo
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
        .await?
    {
        LockoutGuard::Done(provider) => provider,
        LockoutGuard::WouldLockOut => return Err(last_sign_in_method("disable")),
    };
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
    Ok(Json(ProviderView::new(provider, public_base_url(&state))))
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
    if repo.delete_provider(id).await? == LockoutGuard::WouldLockOut {
        return Err(last_sign_in_method("delete"));
    }
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
        // controls the Host header, so the base is deployment-owned and passed
        // in rather than derived from anything the caller sent
        let uri = redirect_uri("https://rolter.example.com", "keycloak");
        assert_eq!(uri, "https://rolter.example.com/auth/sso/keycloak/callback");
    }

    /// The admin API advertises the two addresses an operator needs, built by
    /// the same functions the flow itself uses (#2083), and the flattened row
    /// still hides the sealed secret behind `has_client_secret`.
    #[test]
    fn a_provider_row_carries_the_uris_the_flow_uses() {
        let provider = SsoProvider {
            id: Uuid::nil(),
            org_id: Uuid::nil(),
            name: "Keycloak".into(),
            slug: "keycloak".into(),
            issuer: "https://idp.example.com".into(),
            client_id: "rolter".into(),
            secret_ciphertext: Some(vec![1, 2, 3]),
            secret_nonce: Some(vec![4, 5, 6]),
            scopes: vec!["openid".into()],
            group_claim: "groups".into(),
            default_role: None,
            enabled: true,
            created_at: Utc::now(),
        };
        let base = "https://rolter.example.com";
        let row = serde_json::to_value(ProviderView::new(provider, base)).expect("serializes");

        assert_eq!(row["redirect_uri"], redirect_uri(base, "keycloak"));
        assert_eq!(
            row["redirect_uri"],
            "https://rolter.example.com/auth/sso/keycloak/callback"
        );
        assert_eq!(
            row["login_url"],
            "https://rolter.example.com/auth/sso/keycloak/start"
        );
        // flattened, so a client reading the old fields finds them where they were
        assert_eq!(row["slug"], "keycloak");
        assert_eq!(row["has_client_secret"], true);
        assert!(row.get("secret_ciphertext").is_none());
        assert!(row.get("secret_nonce").is_none());
    }

    #[test]
    fn a_slug_inside_the_charset_is_accepted() {
        let longest = "a".repeat(SLUG_MAX_LEN);
        for slug in [
            "okta",
            "a",
            "0",
            "9lives",
            "entra-staging",
            "a--b",
            // a trailing hyphen is inside the store's rule, so it is inside this one
            "okta-",
            longest.as_str(),
        ] {
            assert!(validate_slug(slug).is_ok(), "{slug:?} should be accepted");
        }
    }

    #[test]
    fn a_slug_outside_the_charset_is_refused() {
        let too_long = "a".repeat(SLUG_MAX_LEN + 1);
        for slug in [
            "",
            "Okta",
            "OKTA",
            "acme okta",
            "-okta",
            "okta_prod",
            "okta.prod",
            "okta/callback",
            "okta%2Fcallback",
            // whitespace is not trimmed away: the slug is checked as sent
            " okta",
            "okta ",
            "okta\n",
            // a Cyrillic "о" looks like the Latin one and is not
            "\u{43e}kta",
            "r\u{e9}sum\u{e9}",
            too_long.as_str(),
        ] {
            assert!(validate_slug(slug).is_err(), "{slug:?} should be refused");
        }
    }

    /// The message is what the caller reads in place of the store's constraint
    /// name, so it has to carry the whole rule.
    #[test]
    fn the_refusal_states_the_rule_and_is_a_bad_request() {
        let err = validate_slug("Okta").expect_err("an uppercase slug is refused");
        assert!(
            matches!(err, ApiError::Core(rolter_core::Error::Config(_))),
            "a refused slug is a 400, not a store error: {err:?}"
        );
        let message = api_error_message(err);
        for part in [
            "lowercase letters",
            "digits",
            "hyphens",
            "start with a letter or digit",
            "at most 63 characters",
        ] {
            assert!(message.contains(part), "{part:?} missing from {message:?}");
        }
        assert!(
            !message.contains("sso_providers_slug_charset"),
            "the constraint name is an implementation detail: {message:?}"
        );
    }

    #[test]
    fn only_the_three_built_in_roles_are_mappable() {
        assert!(parse_role("admin").is_ok());
        assert!(parse_role("member").is_ok());
        assert!(parse_role("viewer").is_ok());
        assert!(parse_role("superadmin").is_err());
        assert!(parse_role("").is_err());
    }

    #[test]
    fn a_refusal_url_carries_only_a_stable_code_and_a_vouched_slug() {
        let unnamed = CallbackFailure {
            reason: SsoFailure::IdpError,
            slug: None,
            error: invalid("idp said something <script>"),
        };
        assert_eq!(
            refusal_url("https://r.example", &unnamed),
            "https://r.example/login?sso_error=idp_error"
        );
        let named = CallbackFailure {
            reason: SsoFailure::NoMappedGroup,
            slug: Some("a b".to_string()),
            error: ApiError::Forbidden,
        };
        assert_eq!(
            refusal_url("https://r.example", &named),
            "https://r.example/login?sso_error=no_mapped_group&sso=a%20b"
        );
    }

    #[test]
    fn failure_codes_are_the_published_set() {
        use SsoFailure::*;
        let codes = [
            IdpError,
            StateExpired,
            SsoDisabled,
            NoMappedGroup,
            AccountDeactivated,
            IdpVerificationFailed,
            NotConfigured,
            InternalError,
        ]
        .map(SsoFailure::code);
        assert_eq!(
            codes,
            [
                "idp_error",
                "state_expired",
                "sso_disabled",
                "no_mapped_group",
                "account_deactivated",
                "idp_verification_failed",
                "not_configured",
                "internal_error",
            ]
        );
    }

    #[test]
    fn the_exchange_code_is_stored_as_its_digest() {
        let hash = exchange_code_hash("abc");
        assert_eq!(hash.len(), 64);
        assert_ne!(hash, "abc");
        assert_eq!(hash, exchange_code_hash("abc"));
    }
}
