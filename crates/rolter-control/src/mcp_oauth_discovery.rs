//! The three client-side MUSTs the MCP OAuth flow of #707 predates (#1347).
//!
//! The flow in [`crate::mcp_oauth_flow`] was a correct authorization-code +
//! PKCE client when it landed. The MCP specification has since made three
//! things a client MUST do, and this module is all three, kept apart from the
//! flow because each of them is a small piece of URL and string handling whose
//! *exact* behaviour is the security property:
//!
//! 1. **RFC 9728 discovery.** An MCP server publishes protected resource
//!    metadata naming its authorization servers, and a client discovers the
//!    authorization server from it rather than from an operator's typing. The
//!    hand-configured endpoints survive as the fallback for servers that
//!    publish nothing.
//! 2. **RFC 8707 resource indicators.** Every authorization *and* token request
//!    carries the canonical URI of the MCP server the token is for, so the
//!    token is audience-bound and a compliant server can tell a token minted
//!    for it from one minted for somebody else. [`ResourceUri`] exists so the
//!    two requests cannot disagree about what that URI is: it is the only way
//!    to name a resource in this crate, and both requests take one.
//! 3. **RFC 9207 `iss` validation.** The issuer of the authorization server the
//!    browser was sent to is recorded beside the PKCE verifier, and the
//!    callback judges the `iss` it comes back with against it — the defence
//!    against a mix-up attack, where a code from one authorization server is
//!    fed to another.
//!
//! Targets the MCP specification revision `draft` as published on 2026-09-08.
//!
//! Every fetch this module makes is a URL derived from operator configuration
//! or from a document an operator-registered MCP server served, so all of them
//! go through the same three guards: `https` (or loopback), the deployment's
//! egress policy, and a client that does not follow redirects. See
//! `docs/architecture/mcp-oauth.md` for the SSRF reasoning.

use std::sync::OnceLock;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::ControlState;

/// How long one discovery request may take. Discovery sits in front of an
/// interactive consent, so a server that publishes nothing has to fail fast
/// enough that the operator's fallback is reached while they are still looking
/// at the screen.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the whole of discovery may take before the caller gives up and uses
/// what the operator configured. A per-request timeout alone does not bound it:
/// a black-holed host is probed at several URLs, and their timeouts add up on
/// an endpoint a person is waiting on.
pub(crate) const DISCOVERY_BUDGET: Duration = Duration::from_secs(8);
/// The most metadata rolter will read from an upstream. Both documents are a
/// few hundred bytes; the cap is what stops a hostile or broken server from
/// streaming a control-plane worker to death.
const MAX_METADATA_BYTES: usize = 64 * 1024;
/// At most this many authorization servers of a protected resource are tried
/// before giving up. RFC 9728 lets a resource list many; a client that walks an
/// unbounded list is a request amplifier.
const MAX_AUTHORIZATION_SERVERS: usize = 3;

// ---------------------------------------------------------------------------
// the canonical resource identifier (RFC 8707 §2)
// ---------------------------------------------------------------------------

/// The canonical URI of an MCP server, as RFC 8707 §2 defines a resource
/// identifier and the MCP specification narrows it.
///
/// The type exists to make one class of bug unrepresentable. The `resource`
/// parameter of the authorization request and the `resource` parameter of the
/// token request must be the same string — an authorization server is entitled
/// to refuse the exchange, or to mint a token for the wrong audience, if they
/// differ. Since [`ResourceUri::parse`] is the only constructor and both
/// requests take a `&ResourceUri`, they cannot drift: there is nowhere else for
/// the string to come from.
///
/// The canonical form is:
///
/// - scheme and host lowercased (uppercase is accepted on the way in for
///   robustness, as the specification asks);
/// - no fragment, which RFC 8707 forbids outright;
/// - no trailing slash, the form the specification asks implementations to
///   settle on;
/// - the query preserved, which RFC 8707 permits where it is what scopes the
///   resource.
///
/// Parsing is idempotent — `parse(parse(x)) == parse(x)` — so re-deriving the
/// resource from a stored string yields exactly what was sent the first time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResourceUri(String);

impl ResourceUri {
    pub(crate) fn parse(raw: &str) -> Result<Self, DiscoveryError> {
        let raw = raw.trim();
        // a fragment is not merely dropped for tidiness: RFC 8707 §2 says the
        // resource value MUST NOT have one
        let without_fragment = match raw.split_once('#') {
            Some((head, _)) => head,
            None => raw,
        };
        let lower = without_fragment.to_ascii_lowercase();
        let scheme_len = if lower.starts_with("https://") {
            "https://".len()
        } else if lower.starts_with("http://") {
            "http://".len()
        } else {
            return Err(DiscoveryError::Resource(format!(
                "'{raw}' is not an absolute http(s) uri, so it cannot be a resource identifier"
            )));
        };
        let (scheme, rest) = without_fragment.split_at(scheme_len);
        let (authority, tail) = match rest.find(['/', '?']) {
            Some(index) => rest.split_at(index),
            None => (rest, ""),
        };
        if authority.is_empty() {
            return Err(DiscoveryError::Resource(format!("'{raw}' has no host")));
        }
        if authority.contains('@') {
            // userinfo in a resource identifier would put a credential into an
            // authorization url, and identifies a caller rather than a resource
            return Err(DiscoveryError::Resource(format!(
                "'{raw}' carries userinfo, which a resource identifier may not"
            )));
        }
        let (path, query) = match tail.split_once('?') {
            Some((path, query)) => (path, Some(query)),
            None => (tail, None),
        };
        let mut canonical = String::with_capacity(without_fragment.len());
        canonical.push_str(&scheme.to_ascii_lowercase());
        canonical.push_str(&authority.to_ascii_lowercase());
        canonical.push_str(path.trim_end_matches('/'));
        if let Some(query) = query.filter(|q| !q.is_empty()) {
            canonical.push('?');
            canonical.push_str(query);
        }
        Ok(Self(canonical))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ResourceUri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// RFC 9207 §2.4
// ---------------------------------------------------------------------------

/// Why an authorization response was rejected before its code was sent
/// anywhere. Kept apart from [`DiscoveryError`] because these are verdicts on a
/// response rather than failures to fetch something.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum IssuerRejection {
    #[error(
        "the authorization response came back from a different issuer than the one the request \
         was sent to; the authorization code was discarded"
    )]
    Mismatch,
    #[error(
        "the authorization server advertises RFC 9207 issuer identification but returned no \
         'iss'; the authorization code was discarded"
    )]
    Missing,
    #[error(
        "the authorization response carries an 'iss' but no issuer was recorded for this server, \
         so it cannot be checked; register the authorization server's issuer on the oauth client, \
         or let protected resource metadata discovery resolve it"
    )]
    Unverifiable,
}

/// Apply RFC 9207 §2.4 as the MCP specification tabulates it.
///
/// | advertised | `iss` present | action |
/// | --- | --- | --- |
/// | `true` | yes | compare against the recorded issuer |
/// | `true` | no | reject |
/// | `false`/absent | yes | compare against the recorded issuer |
/// | `false`/absent | no | proceed |
///
/// The comparison is byte equality on purpose. RFC 9207 §2.4 says simple string
/// comparison (RFC 3986 §6.2.1), and the MCP specification spells out that a
/// client MUST NOT case-fold, elide a default port, add or drop a trailing
/// slash, or normalise percent-encoding first. Reaching for a URL parser here
/// would re-introduce exactly the equivalences an attacker needs.
///
/// The third row is the specification's local-policy choice: a present `iss` is
/// checked whether or not the metadata advertised one, so an authorization
/// server that emits `iss` before updating its metadata is still protected.
///
/// The fourth row is the only one that proceeds without a comparison, and it is
/// the one a hand-configured server that publishes no metadata lands on.
pub(crate) fn validate_issuer(
    expected: Option<&str>,
    advertised: bool,
    received: Option<&str>,
) -> Result<(), IssuerRejection> {
    match (received, expected) {
        (Some(received), Some(expected)) => {
            if received == expected {
                Ok(())
            } else {
                Err(IssuerRejection::Mismatch)
            }
        }
        // an `iss` arrived with nothing authentic to judge it against; failing
        // closed is the only honest answer, since accepting it would be
        // validation in name only
        (Some(_), None) => Err(IssuerRejection::Unverifiable),
        (None, _) if advertised => Err(IssuerRejection::Missing),
        (None, _) => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// discovery
// ---------------------------------------------------------------------------

/// Why discovery did not produce endpoints. Never fatal on its own: the caller
/// falls back to whatever the operator configured, and only fails the request
/// when there is no fallback either.
#[derive(Debug, thiserror::Error)]
pub(crate) enum DiscoveryError {
    #[error("{0}")]
    Resource(String),
    #[error("{0}")]
    Refused(String),
    #[error("no protected resource metadata is published for '{0}'")]
    NoResourceMetadata(String),
    #[error("the protected resource metadata for '{0}' names no authorization server")]
    NoAuthorizationServer(String),
    #[error("no usable authorization server metadata was found for '{0}'")]
    NoAuthorizationServerMetadata(String),
    #[error("the discovery http client could not be built")]
    NoClient,
}

/// What discovery resolved: the endpoints to use plus the two facts RFC 9207
/// validation needs later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveredEndpoints {
    pub(crate) issuer: String,
    pub(crate) authorize_url: String,
    pub(crate) token_url: String,
    pub(crate) iss_supported: bool,
}

/// RFC 9728 protected resource metadata, narrowed to what rolter reads.
#[derive(Debug, Deserialize)]
struct ProtectedResourceMetadata {
    #[serde(default)]
    resource: String,
    #[serde(default)]
    authorization_servers: Vec<String>,
}

/// RFC 8414 / OpenID Connect Discovery authorization server metadata, narrowed
/// the same way.
#[derive(Debug, Deserialize)]
struct AuthorizationServerMetadata {
    #[serde(default)]
    issuer: String,
    #[serde(default)]
    authorization_endpoint: String,
    #[serde(default)]
    token_endpoint: String,
    #[serde(default)]
    authorization_response_iss_parameter_supported: bool,
}

/// Resolve a server's authorization server from what it publishes.
///
/// The order is the specification's: the `WWW-Authenticate` challenge of an
/// unauthenticated request first, then the well-known URIs — the one with the
/// server's path inserted, then the one at the root.
pub(crate) async fn discover(
    state: &ControlState,
    resource: &ResourceUri,
) -> Result<DiscoveredEndpoints, DiscoveryError> {
    let metadata = protected_resource_metadata(state, resource).await?;
    let servers = metadata.authorization_servers;
    if servers.is_empty() {
        return Err(DiscoveryError::NoAuthorizationServer(resource.to_string()));
    }
    let mut last: Option<DiscoveryError> = None;
    for issuer in servers.iter().take(MAX_AUTHORIZATION_SERVERS) {
        match authorization_server_metadata(state, issuer).await {
            Ok(endpoints) => return Ok(endpoints),
            Err(error) => {
                tracing::debug!(%issuer, error = %error, "mcp oauth: authorization server metadata unusable");
                last = Some(error);
            }
        }
    }
    Err(last.unwrap_or_else(|| DiscoveryError::NoAuthorizationServerMetadata(resource.to_string())))
}

/// Fetch and validate the protected resource metadata of `resource`.
async fn protected_resource_metadata(
    state: &ControlState,
    resource: &ResourceUri,
) -> Result<ProtectedResourceMetadata, DiscoveryError> {
    let mut candidates = Vec::new();
    if let Some(from_challenge) = challenge_metadata_url(state, resource).await {
        candidates.push(from_challenge);
    }
    candidates.extend(protected_resource_metadata_urls(resource));
    for url in candidates {
        let Ok(metadata) = fetch_metadata::<ProtectedResourceMetadata>(state, &url).await else {
            continue;
        };
        // RFC 9728 §3.3: the document must claim to be about the resource it
        // was fetched for. without this check a server can hand a client an
        // authorization server for somebody else's resource
        match ResourceUri::parse(&metadata.resource) {
            Ok(declared) if declared == *resource => return Ok(metadata),
            _ => tracing::debug!(
                %url,
                declared = %metadata.resource,
                expected = %resource,
                "mcp oauth: protected resource metadata is about a different resource"
            ),
        }
    }
    Err(DiscoveryError::NoResourceMetadata(resource.to_string()))
}

/// The `resource_metadata` URL an unauthenticated request is challenged with,
/// if the server answers with one.
///
/// A failure here is not an error: the well-known URIs are the documented
/// fallback, and an MCP server that answers an unauthenticated `GET` with
/// something other than a `401` has simply not told us anything.
async fn challenge_metadata_url(state: &ControlState, resource: &ResourceUri) -> Option<String> {
    let url = resource.as_str();
    guard_url(state, url).ok()?;
    let response = client().ok()?.get(url).send().await.ok()?;
    let header = response
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)?
        .to_str()
        .ok()?;
    resource_metadata_from_challenge(header)
}

/// Pull `resource_metadata` out of an RFC 9728 §5.1 `WWW-Authenticate`
/// challenge. Written by hand rather than with a header-parsing crate because
/// the value is one quoted parameter among several and the scheme may not be
/// `Bearer`.
pub(crate) fn resource_metadata_from_challenge(header: &str) -> Option<String> {
    const KEY: &str = "resource_metadata";
    let mut rest = header;
    while let Some(index) = rest.find(KEY) {
        let (before, at_key) = rest.split_at(index);
        // only a whole parameter counts: `x_resource_metadata` is a different
        // parameter that happens to end in this name
        let boundary = before
            .chars()
            .next_back()
            .is_none_or(|c| c == ' ' || c == ',' || c == '\t');
        let after = at_key[KEY.len()..].trim_start();
        rest = &at_key[KEY.len()..];
        if !boundary {
            continue;
        }
        let Some(value) = after.strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        let parsed = match value.strip_prefix('"') {
            Some(quoted) => quoted.split('"').next().map(str::to_string),
            None => value
                .split([',', ' ', '\t'])
                .next()
                .filter(|v| !v.is_empty())
                .map(str::to_string),
        };
        if let Some(parsed) = parsed.filter(|v| !v.is_empty()) {
            return Some(parsed);
        }
    }
    None
}

/// The well-known protected resource metadata URLs of `resource`, in the order
/// the specification requires them to be tried: the server's path inserted
/// after the well-known suffix first, then the root document.
///
/// RFC 9728 §3.1 inserts `/.well-known/oauth-protected-resource` between the
/// host and the path, which is the opposite of where a `.well-known` usually
/// goes and is easy to get backwards.
pub(crate) fn protected_resource_metadata_urls(resource: &ResourceUri) -> Vec<String> {
    const SUFFIX: &str = "/.well-known/oauth-protected-resource";
    let (origin, path) = split_origin_and_path(resource.as_str());
    let mut urls = Vec::with_capacity(2);
    if !path.is_empty() {
        urls.push(format!("{origin}{SUFFIX}{path}"));
    }
    urls.push(format!("{origin}{SUFFIX}"));
    urls
}

/// The authorization server metadata URLs of `issuer`, in the priority order
/// the MCP specification requires: RFC 8414 with the issuer's path inserted,
/// then OpenID Connect Discovery inserted, then OpenID Connect Discovery
/// appended. An issuer with no path has only the first two forms.
pub(crate) fn authorization_server_metadata_urls(issuer: &str) -> Vec<String> {
    const OAUTH: &str = "/.well-known/oauth-authorization-server";
    const OIDC: &str = "/.well-known/openid-configuration";
    let issuer = issuer.trim_end_matches('/');
    let (origin, path) = split_origin_and_path(issuer);
    if path.is_empty() {
        return vec![format!("{origin}{OAUTH}"), format!("{origin}{OIDC}")];
    }
    vec![
        format!("{origin}{OAUTH}{path}"),
        format!("{origin}{OIDC}{path}"),
        format!("{origin}{path}{OIDC}"),
    ]
}

/// Split `https://host:port/some/path` into its origin and its path. Query and
/// fragment are already gone from a [`ResourceUri`]; an issuer that carries one
/// is not an issuer identifier, so anything from `?` on is dropped here too.
fn split_origin_and_path(url: &str) -> (&str, &str) {
    let after_scheme = match url.find("://") {
        Some(index) => index + "://".len(),
        None => 0,
    };
    let rest = &url[after_scheme..];
    let authority_len = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let origin_len = after_scheme + authority_len;
    let tail = &url[origin_len..];
    let path = match tail.find(['?', '#']) {
        Some(index) => &tail[..index],
        None => tail,
    };
    (&url[..origin_len], path)
}

/// Fetch and validate the metadata of one authorization server.
async fn authorization_server_metadata(
    state: &ControlState,
    issuer: &str,
) -> Result<DiscoveredEndpoints, DiscoveryError> {
    for url in authorization_server_metadata_urls(issuer) {
        let Ok(metadata) = fetch_metadata::<AuthorizationServerMetadata>(state, &url).await else {
            continue;
        };
        // RFC 8414 §3.3 / OIDC Discovery §4.3: the document's issuer must be
        // identical to the identifier the URL was built from, or a document
        // served by an attacker could claim to speak for an honest issuer.
        // identical means identical — the same simple string comparison
        // RFC 9207 will make later against the same value
        if metadata.issuer != issuer {
            tracing::debug!(
                %url,
                declared = %metadata.issuer,
                expected = %issuer,
                "mcp oauth: authorization server metadata claims a different issuer"
            );
            continue;
        }
        if metadata.authorization_endpoint.is_empty() || metadata.token_endpoint.is_empty() {
            continue;
        }
        if guard_url(state, &metadata.authorization_endpoint).is_err()
            || guard_url(state, &metadata.token_endpoint).is_err()
        {
            continue;
        }
        return Ok(DiscoveredEndpoints {
            issuer: metadata.issuer,
            authorize_url: metadata.authorization_endpoint,
            token_url: metadata.token_endpoint,
            iss_supported: metadata.authorization_response_iss_parameter_supported,
        });
    }
    Err(DiscoveryError::NoAuthorizationServerMetadata(
        issuer.to_string(),
    ))
}

/// `GET` one metadata document, with every guard applied.
async fn fetch_metadata<T: DeserializeOwned>(
    state: &ControlState,
    url: &str,
) -> Result<T, DiscoveryError> {
    guard_url(state, url)?;
    let response = client()?
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| DiscoveryError::Refused(format!("{url} is unreachable: {e}")))?;
    if !response.status().is_success() {
        return Err(DiscoveryError::Refused(format!(
            "{url} answered {}",
            response.status().as_u16()
        )));
    }
    let body = read_capped(response, url).await?;
    serde_json::from_slice(&body)
        .map_err(|e| DiscoveryError::Refused(format!("{url} is not valid metadata json: {e}")))
}

/// Read a response body, refusing one that runs past [`MAX_METADATA_BYTES`].
async fn read_capped(
    mut response: reqwest::Response,
    url: &str,
) -> Result<Vec<u8>, DiscoveryError> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| DiscoveryError::Refused(format!("{url} stopped mid-body: {e}")))?
    {
        if body.len() + chunk.len() > MAX_METADATA_BYTES {
            return Err(DiscoveryError::Refused(format!(
                "{url} returned more than {MAX_METADATA_BYTES} bytes of metadata"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// The two checks every discovery target passes before it is fetched: the
/// transport must be `https` (loopback excepted, for a local stub), and the
/// deployment's egress policy must permit the host.
fn guard_url(state: &ControlState, url: &str) -> Result<(), DiscoveryError> {
    crate::mcp_oauth_flow::require_https(url, "discovery url")
        .map_err(|_| DiscoveryError::Refused(format!("{url} is not an https url")))?;
    state
        .egress
        .check_url(url, "MCP OAuth discovery")
        .map_err(DiscoveryError::Refused)
}

/// The client discovery fetches with. Separate from [`ControlState::http`] for
/// one reason: it does not follow redirects. A `302` from an upstream is how a
/// host that passed the egress check hands the request to one that would not
/// have, and a redirect chain is not something an allowlist can see.
fn client() -> Result<&'static reqwest::Client, DiscoveryError> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(DISCOVERY_TIMEOUT)
                .build()
                .ok()
        })
        .as_ref()
        .ok_or(DiscoveryError::NoClient)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resource_uri_is_canonicalised_the_way_rfc_8707_asks() {
        let cases = [
            ("https://mcp.example.com", "https://mcp.example.com"),
            ("https://mcp.example.com/", "https://mcp.example.com"),
            (
                "https://mcp.example.com/mcp/",
                "https://mcp.example.com/mcp",
            ),
            (
                "https://mcp.example.com/server/mcp",
                "https://mcp.example.com/server/mcp",
            ),
            (
                "https://mcp.example.com:8443",
                "https://mcp.example.com:8443",
            ),
            // uppercase scheme and host are accepted and lowered, as the
            // specification asks for robustness
            ("HTTPS://MCP.Example.COM/MCP", "https://mcp.example.com/MCP"),
            // a fragment is forbidden outright, so it is dropped rather than
            // sent
            (
                "https://mcp.example.com/mcp#tools",
                "https://mcp.example.com/mcp",
            ),
            // a query may be what scopes the resource, so it survives
            (
                "https://mcp.example.com/mcp?tenant=acme",
                "https://mcp.example.com/mcp?tenant=acme",
            ),
        ];
        for (raw, expected) in cases {
            let parsed = ResourceUri::parse(raw).expect(raw);
            assert_eq!(parsed.as_str(), expected, "canonicalising {raw}");
            // idempotence is what stops the authorization request and the token
            // request from disagreeing when one of them re-parses a stored value
            assert_eq!(
                ResourceUri::parse(parsed.as_str())
                    .expect(expected)
                    .as_str(),
                expected
            );
        }
    }

    #[test]
    fn a_resource_uri_must_be_absolute_and_carry_no_credential() {
        assert!(ResourceUri::parse("mcp.example.com").is_err());
        assert!(ResourceUri::parse("/mcp").is_err());
        assert!(ResourceUri::parse("ftp://mcp.example.com").is_err());
        assert!(ResourceUri::parse("https://").is_err());
        assert!(ResourceUri::parse("https://user:pw@mcp.example.com").is_err());
    }

    #[test]
    fn the_rfc_9207_table_is_implemented_row_by_row() {
        let issuer = "https://auth.example.com";
        // row 1: advertised and present, matching
        assert_eq!(validate_issuer(Some(issuer), true, Some(issuer)), Ok(()));
        // row 2: advertised and absent — the silent failure this exists for
        assert_eq!(
            validate_issuer(Some(issuer), true, None),
            Err(IssuerRejection::Missing)
        );
        // row 3: not advertised but present — still compared, per the
        // specification's local-policy choice
        assert_eq!(validate_issuer(Some(issuer), false, Some(issuer)), Ok(()));
        assert_eq!(
            validate_issuer(Some(issuer), false, Some("https://evil.example.com")),
            Err(IssuerRejection::Mismatch)
        );
        // row 4: neither advertised nor present — a hand-configured server
        // that publishes no metadata lands here and keeps working
        assert_eq!(validate_issuer(Some(issuer), false, None), Ok(()));
        assert_eq!(validate_issuer(None, false, None), Ok(()));
    }

    #[test]
    fn a_mismatched_issuer_is_rejected_however_it_is_dressed_up() {
        let issuer = "https://auth.example.com";
        // every one of these is a *different* issuer under simple string
        // comparison, and treating any of them as equal is the mix-up attack
        for received in [
            "https://auth.example.com/",
            "https://AUTH.example.com",
            "HTTPS://auth.example.com",
            "https://auth.example.com:443",
            "https://auth.example.com.evil.test",
            "https://auth.example.com%2f",
            "",
        ] {
            assert_eq!(
                validate_issuer(Some(issuer), true, Some(received)),
                Err(IssuerRejection::Mismatch),
                "'{received}' must not be accepted for '{issuer}'"
            );
        }
    }

    #[test]
    fn an_iss_with_nothing_to_check_it_against_is_refused() {
        // a login recorded before an issuer was known cannot validate one, and
        // accepting it would be validation in name only
        assert_eq!(
            validate_issuer(None, false, Some("https://auth.example.com")),
            Err(IssuerRejection::Unverifiable)
        );
    }

    #[test]
    fn the_well_known_suffix_goes_between_the_host_and_the_path() {
        let resource = ResourceUri::parse("https://example.com/public/mcp").unwrap();
        assert_eq!(
            protected_resource_metadata_urls(&resource),
            vec![
                "https://example.com/.well-known/oauth-protected-resource/public/mcp".to_string(),
                "https://example.com/.well-known/oauth-protected-resource".to_string(),
            ]
        );
        // a resource at the root has only the root document
        let root = ResourceUri::parse("https://example.com/").unwrap();
        assert_eq!(
            protected_resource_metadata_urls(&root),
            vec!["https://example.com/.well-known/oauth-protected-resource".to_string()]
        );
    }

    #[test]
    fn authorization_server_metadata_is_probed_in_the_specified_order() {
        assert_eq!(
            authorization_server_metadata_urls("https://auth.example.com/tenant1"),
            vec![
                "https://auth.example.com/.well-known/oauth-authorization-server/tenant1"
                    .to_string(),
                "https://auth.example.com/.well-known/openid-configuration/tenant1".to_string(),
                "https://auth.example.com/tenant1/.well-known/openid-configuration".to_string(),
            ]
        );
        assert_eq!(
            authorization_server_metadata_urls("https://auth.example.com"),
            vec![
                "https://auth.example.com/.well-known/oauth-authorization-server".to_string(),
                "https://auth.example.com/.well-known/openid-configuration".to_string(),
            ]
        );
    }

    #[test]
    fn the_challenge_parameter_is_read_off_a_www_authenticate_header() {
        assert_eq!(
            resource_metadata_from_challenge(
                r#"Bearer resource_metadata="https://mcp.example.com/.well-known/oauth-protected-resource", scope="files:read""#
            )
            .as_deref(),
            Some("https://mcp.example.com/.well-known/oauth-protected-resource")
        );
        // unquoted, and last rather than first
        assert_eq!(
            resource_metadata_from_challenge(
                "Bearer error=\"insufficient_scope\", resource_metadata=https://m.example/prm"
            )
            .as_deref(),
            Some("https://m.example/prm")
        );
        // a parameter that merely ends in the name is a different parameter
        assert_eq!(
            resource_metadata_from_challenge(r#"Bearer x_resource_metadata="https://evil.test""#),
            None
        );
        assert_eq!(resource_metadata_from_challenge("Bearer"), None);
        assert_eq!(
            resource_metadata_from_challenge("Bearer resource_metadata=\"\""),
            None
        );
    }
}
