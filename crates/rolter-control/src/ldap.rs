//! LDAP bind + group mapping (#241, wired to sign-in by #1826).
//!
//! This is the directory half of LDAP sign-in: it authenticates a login against
//! a directory and reports the entry it found. The control-plane half that
//! stores a directory per org, reaches this provider from
//! `POST /api/v1/auth/login` and turns the entry's groups into memberships is
//! `crate::ldap_signin`.
//!
//! It is the third [`IdentityProvider`] after local password login and OIDC
//! SSO. It resolves a [`Credential::Password`] by binding to a directory as
//! that user and reading their attributes. Which rolter roles those directory
//! groups grant is not decided here: group mapping goes through the same
//! scoped-grant model SSO and SCIM use, so one rule set serves all three.
//!
//! ## Two-phase bind
//!
//! A user's DN is rarely their login name, so the provider binds twice: once as
//! a read-only service account to *find* the user, then again as the user to
//! *verify* their password. The second bind is the authentication — rolter
//! never reads, compares or stores a password hash from the directory.
//!
//! ## Least privilege
//!
//! Authenticating proves who someone is, not what they may do. A user in no
//! mapped group and with no directory default role is rejected by the caller,
//! matching the SSO rule that authentication alone grants nothing, with the same
//! [`IdentityError::NotVerified`] a wrong password gets.
//!
//! ## What never leaves this module
//!
//! Distinguished names, directory filters, bind credentials and raw LDAP result
//! codes stay here. Everything a caller sees is an [`IdentityError`], and every
//! authentication failure — unknown user, wrong password, user outside the
//! search base — collapses to the same
//! [`IdentityError::NotVerified`]. A directory that enumerates its users
//! through rolter's error messages would be worse than no LDAP at all.

// spec-ref: ldap
use async_trait::async_trait;

use rolter_auth::{Credential, Identity, IdentityError, IdentityProvider};

/// How to reach and query the directory.
#[derive(Clone)]
pub struct LdapConfig {
    /// `ldap://` or `ldaps://` URL of the directory
    pub url: String,
    /// DN of the read-only service account used to find users
    pub bind_dn: String,
    /// service account password
    pub bind_password: String,
    /// subtree searched for users
    pub base_dn: String,
    /// user search filter; `{login}` is replaced with the escaped login
    pub user_filter: String,
    /// attribute holding the user's email
    pub email_attr: String,
    /// attribute holding the user's display name
    pub display_name_attr: String,
    /// attribute on the user entry listing their groups
    pub group_attr: String,
    /// require TLS. `ldaps://` satisfies this; plain `ldap://` does not
    pub require_tls: bool,
}

// by hand so the service-account password can never reach a log line through a
// stray `{:?}` of a struct that carries it
impl std::fmt::Debug for LdapConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LdapConfig")
            .field("url", &self.url)
            .field("bind_dn", &self.bind_dn)
            .field("bind_password", &"<redacted>")
            .field("base_dn", &self.base_dn)
            .field("user_filter", &self.user_filter)
            .field("email_attr", &self.email_attr)
            .field("display_name_attr", &self.display_name_attr)
            .field("group_attr", &self.group_attr)
            .field("require_tls", &self.require_tls)
            .finish()
    }
}

impl LdapConfig {
    /// Reject a configuration that cannot be operated safely, before it is ever
    /// used to authenticate anyone.
    pub fn validate(&self) -> Result<(), String> {
        let scheme_is_tls = self.url.starts_with("ldaps://");
        if !scheme_is_tls && !self.url.starts_with("ldap://") {
            return Err("ldap url must start with ldap:// or ldaps://".into());
        }
        if self.require_tls && !scheme_is_tls {
            return Err(
                "require_tls is set but the url is plain ldap://; use ldaps:// so the bind \
                 password is not sent in cleartext"
                    .into(),
            );
        }
        if !self.user_filter.contains("{login}") {
            return Err("user_filter must contain the {login} placeholder".into());
        }
        if self.base_dn.trim().is_empty() {
            return Err("base_dn must not be empty".into());
        }
        Ok(())
    }
}

/// A directory entry found for a login, before rolter interprets it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub dn: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub groups: Vec<String>,
}

/// Why a directory operation did not produce an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectoryError {
    /// the search ran and matched nothing, or the user bind was rejected —
    /// i.e. the credential is wrong. Carries no detail by construction
    NotAuthenticated,
    /// the directory could not be reached or misbehaved. The string is for
    /// rolter's logs; it never reaches a client
    Unavailable(String),
}

/// The directory operations this provider needs.
///
/// Extracted as a trait so every rule below — filter escaping, group mapping,
/// error normalization — is unit-tested against a fake, with no live server and
/// no network. `ldap3` lives behind `Ldap3Directory` and nowhere else.
#[async_trait]
pub trait Directory: Send + Sync {
    /// Find the entry for `login`, then bind as it with `password`. Returns the
    /// entry only when that second bind succeeds.
    async fn bind_user(
        &self,
        login: &str,
        password: &str,
    ) -> Result<DirectoryEntry, DirectoryError>;
}

/// Opens a [`Directory`] for a configured LDAP server.
///
/// The seam between the stored directory settings and the network: production
/// installs [`Ldap3Factory`], the integration suite installs a stub that
/// answers from a fixed list of entries, so sign-in is tested end to end
/// without a directory server.
pub trait DirectoryFactory: Send + Sync {
    /// The directory for `config`, or `None` when this build cannot talk LDAP.
    fn open(&self, config: LdapConfig) -> Option<Box<dyn Directory>>;

    /// Whether this build can reach a directory at all. A control plane built
    /// without the `ldap` cargo feature refuses to store a directory rather
    /// than accept settings nothing could ever use.
    fn available(&self) -> bool;
}

/// The production factory: `ldap3` when the `ldap` feature is compiled in, and
/// nothing otherwise.
#[derive(Debug, Default, Clone, Copy)]
pub struct Ldap3Factory;

impl DirectoryFactory for Ldap3Factory {
    #[cfg(feature = "ldap")]
    fn open(&self, config: LdapConfig) -> Option<Box<dyn Directory>> {
        Some(Box::new(Ldap3Directory::new(config)))
    }

    #[cfg(not(feature = "ldap"))]
    fn open(&self, _config: LdapConfig) -> Option<Box<dyn Directory>> {
        None
    }

    fn available(&self) -> bool {
        cfg!(feature = "ldap")
    }
}

/// Escape a value for use inside an LDAP search filter (RFC 4515).
///
/// Without this a login of `*` matches every user, and `)(uid=admin` rewrites
/// the filter entirely — LDAP injection, the directory analogue of SQL
/// injection. Applied to the login before it is ever substituted into
/// `user_filter`.
pub fn escape_filter(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\5c"),
            '*' => out.push_str("\\2a"),
            '(' => out.push_str("\\28"),
            ')' => out.push_str("\\29"),
            '\0' => out.push_str("\\00"),
            '/' => out.push_str("\\2f"),
            _ => out.push(ch),
        }
    }
    out
}

/// The LDAP identity provider.
pub struct LdapIdentityProvider {
    config: LdapConfig,
    directory: Box<dyn Directory>,
}

impl LdapIdentityProvider {
    /// Build a provider over any [`Directory`]. Fails on a configuration that
    /// could not authenticate anyone safely.
    pub fn new(config: LdapConfig, directory: Box<dyn Directory>) -> Result<Self, String> {
        config.validate()?;
        Ok(Self { config, directory })
    }

    /// Build the search filter for a login, with the login escaped.
    pub fn filter_for(&self, login: &str) -> String {
        self.config
            .user_filter
            .replace("{login}", &escape_filter(login))
    }
}

#[async_trait]
impl IdentityProvider for LdapIdentityProvider {
    fn kind(&self) -> &'static str {
        "ldap"
    }

    async fn resolve(&self, credential: Credential) -> Result<Identity, IdentityError> {
        let Credential::Password { email, password } = credential else {
            return Err(IdentityError::UnsupportedCredential { provider: "ldap" });
        };

        let entry = match self.directory.bind_user(&email, &password).await {
            Ok(entry) => entry,
            // an unknown user and a wrong password are the same answer: telling
            // them apart turns the login form into a user enumeration oracle
            Err(DirectoryError::NotAuthenticated) => return Err(IdentityError::NotVerified),
            Err(DirectoryError::Unavailable(detail)) => {
                // the detail is logged, never returned
                tracing::warn!(target: "rolter::ldap", error = %detail, "ldap directory unavailable");
                return Err(IdentityError::Provider("directory unavailable".into()));
            }
        };

        Ok(Identity {
            // the DN is the directory's own stable identifier for the account,
            // which is exactly what `subject` is specified to carry
            subject: entry.dn,
            // rolter keys accounts on email across providers; fall back to the
            // login when the directory has no mail attribute for the entry
            email: entry.email.unwrap_or(email),
            display_name: entry.display_name,
            groups: entry.groups,
        })
    }
}

/// How long one directory operation may take before the sign-in gives up.
#[cfg(feature = "ldap")]
const DIRECTORY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The real directory, backed by `ldap3`.
///
/// Deliberately thin: it performs the two binds and the attribute read, and
/// converts every failure into a [`DirectoryError`]. All policy lives above it
/// in [`LdapIdentityProvider`], which is why that policy is testable without a
/// server.
#[cfg(feature = "ldap")]
pub struct Ldap3Directory {
    config: LdapConfig,
}

#[cfg(feature = "ldap")]
impl Ldap3Directory {
    pub fn new(config: LdapConfig) -> Self {
        Self { config }
    }
}

#[cfg(feature = "ldap")]
#[async_trait]
impl Directory for Ldap3Directory {
    async fn bind_user(
        &self,
        login: &str,
        password: &str,
    ) -> Result<DirectoryEntry, DirectoryError> {
        use ldap3::{LdapConnAsync, LdapConnSettings, Scope, SearchEntry};

        // an empty password is an *unauthenticated* bind in LDAP: the server
        // accepts it and returns success without verifying anything. Rejecting
        // it here is what stops that from reading as a valid login
        if password.is_empty() {
            return Err(DirectoryError::NotAuthenticated);
        }

        let unavailable = |e: ldap3::LdapError| DirectoryError::Unavailable(e.to_string());
        // a directory that accepts the connection and then says nothing must
        // not hold a sign-in request open
        let settings = || LdapConnSettings::new().set_conn_timeout(DIRECTORY_TIMEOUT);

        // phase 1: find the user as the service account
        let (conn, mut ldap) = LdapConnAsync::with_settings(settings(), &self.config.url)
            .await
            .map_err(unavailable)?;
        ldap3::drive!(conn);
        ldap.with_timeout(DIRECTORY_TIMEOUT)
            .simple_bind(&self.config.bind_dn, &self.config.bind_password)
            .await
            .map_err(unavailable)?
            .success()
            .map_err(|e| {
                DirectoryError::Unavailable(format!("service account bind failed: {e}"))
            })?;

        let filter = self
            .config
            .user_filter
            .replace("{login}", &escape_filter(login));
        let attrs = [
            self.config.email_attr.as_str(),
            self.config.display_name_attr.as_str(),
            self.config.group_attr.as_str(),
        ];
        let (results, _) = ldap
            .with_timeout(DIRECTORY_TIMEOUT)
            .search(&self.config.base_dn, Scope::Subtree, &filter, attrs)
            .await
            .map_err(unavailable)?
            .success()
            .map_err(unavailable)?;
        let _ = ldap.unbind().await;

        // exactly one match, or this is not a login we can verify. Several
        // matches means the filter is ambiguous — binding as an arbitrary one
        // of them would be a security bug, so it is a hard failure
        if results.len() > 1 {
            return Err(DirectoryError::Unavailable(format!(
                "user_filter matched {} entries; it must identify at most one user",
                results.len()
            )));
        }
        let Some(entry) = results.into_iter().next() else {
            return Err(DirectoryError::NotAuthenticated);
        };
        let entry = SearchEntry::construct(entry);

        // phase 2: the authentication itself — bind as the user we found
        let (conn, mut user_ldap) = LdapConnAsync::with_settings(settings(), &self.config.url)
            .await
            .map_err(unavailable)?;
        ldap3::drive!(conn);
        let bound = user_ldap
            .with_timeout(DIRECTORY_TIMEOUT)
            .simple_bind(&entry.dn, password)
            .await
            .map_err(unavailable)?
            .success();
        let _ = user_ldap.unbind().await;
        if bound.is_err() {
            return Err(DirectoryError::NotAuthenticated);
        }

        let first = |attr: &str| entry.attrs.get(attr).and_then(|v| v.first()).cloned();
        Ok(DirectoryEntry {
            email: first(&self.config.email_attr),
            display_name: first(&self.config.display_name_attr),
            groups: entry
                .attrs
                .get(&self.config.group_attr)
                .cloned()
                .unwrap_or_default(),
            dn: entry.dn,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> LdapConfig {
        LdapConfig {
            url: "ldaps://dir.example.com".into(),
            bind_dn: "cn=svc,dc=example,dc=com".into(),
            bind_password: "svc-password".into(),
            base_dn: "ou=people,dc=example,dc=com".into(),
            user_filter: "(uid={login})".into(),
            email_attr: "mail".into(),
            display_name_attr: "cn".into(),
            group_attr: "memberOf".into(),
            require_tls: true,
        }
    }

    /// A directory scripted per test — no server, no network.
    struct FakeDirectory(Result<DirectoryEntry, DirectoryError>);

    #[async_trait]
    impl Directory for FakeDirectory {
        async fn bind_user(
            &self,
            _login: &str,
            _password: &str,
        ) -> Result<DirectoryEntry, DirectoryError> {
            self.0.clone()
        }
    }

    fn provider(result: Result<DirectoryEntry, DirectoryError>) -> LdapIdentityProvider {
        LdapIdentityProvider::new(config(), Box::new(FakeDirectory(result))).unwrap()
    }

    fn ada() -> DirectoryEntry {
        DirectoryEntry {
            dn: "uid=ada,ou=people,dc=example,dc=com".into(),
            email: Some("ada@example.com".into()),
            display_name: Some("Ada Lovelace".into()),
            groups: vec!["cn=rolter-admins,ou=groups,dc=example,dc=com".into()],
        }
    }

    fn password() -> Credential {
        Credential::Password {
            email: "ada".into(),
            password: format!("pw-{}", uuid::Uuid::new_v4()),
        }
    }

    // ---- successful bind -------------------------------------------------

    #[tokio::test]
    async fn a_successful_bind_yields_the_directory_identity() {
        let identity = provider(Ok(ada())).resolve(password()).await.unwrap();
        assert_eq!(identity.subject, "uid=ada,ou=people,dc=example,dc=com");
        assert_eq!(identity.email, "ada@example.com");
        assert_eq!(identity.display_name.as_deref(), Some("Ada Lovelace"));
    }

    #[tokio::test]
    async fn the_login_is_used_as_email_when_the_directory_has_none() {
        let mut entry = ada();
        entry.email = None;
        let identity = provider(Ok(entry)).resolve(password()).await.unwrap();
        assert_eq!(identity.email, "ada");
    }

    // ---- invalid credentials ---------------------------------------------

    #[tokio::test]
    async fn invalid_credentials_are_not_verified() {
        let err = provider(Err(DirectoryError::NotAuthenticated))
            .resolve(password())
            .await
            .unwrap_err();
        assert!(matches!(err, IdentityError::NotVerified));
        assert_eq!(err.to_string(), "credential could not be verified");
    }

    // ---- unavailable directory -------------------------------------------

    #[tokio::test]
    async fn an_unavailable_directory_is_distinct_from_a_bad_password() {
        // an operator needs to tell "the directory is down" from "wrong
        // password"; a client must not be able to
        let err = provider(Err(DirectoryError::Unavailable(
            "connection refused to ldaps://dir.example.com".into(),
        )))
        .resolve(password())
        .await
        .unwrap_err();
        assert!(matches!(err, IdentityError::Provider(_)));
        assert_eq!(
            err.to_string(),
            "identity provider error: directory unavailable"
        );
    }

    #[tokio::test]
    async fn directory_detail_never_reaches_the_error_message() {
        let err = provider(Err(DirectoryError::Unavailable(
            "bind to cn=svc,dc=example,dc=com failed: invalidCredentials(49)".into(),
        )))
        .resolve(password())
        .await
        .unwrap_err();
        let rendered = err.to_string();
        for leak in ["cn=svc", "dc=example", "invalidCredentials", "49"] {
            assert!(!rendered.contains(leak), "leaked {leak:?} in {rendered:?}");
        }
    }

    #[tokio::test]
    async fn the_directory_groups_are_reported_unfiltered() {
        // which groups grant what is the caller's mapping model, not this
        // provider's: it reports every group the entry carries
        let mut entry = ada();
        entry.groups = vec!["cn=everyone,ou=groups,dc=example,dc=com".into()];
        let identity = provider(Ok(entry)).resolve(password()).await.unwrap();
        assert_eq!(
            identity.groups,
            vec!["cn=everyone,ou=groups,dc=example,dc=com".to_string()]
        );
    }

    // ---- filter injection -------------------------------------------------

    #[test]
    fn a_wildcard_login_cannot_match_every_user() {
        let p = provider(Ok(ada()));
        assert_eq!(p.filter_for("*"), "(uid=\\2a)");
    }

    #[test]
    fn a_login_cannot_rewrite_the_filter() {
        let p = provider(Ok(ada()));
        // the classic LDAP injection: close the clause and open another
        let filter = p.filter_for(")(uid=admin");
        assert_eq!(filter, "(uid=\\29\\28uid=admin)");
        assert!(!filter.contains(")("), "filter was rewritten: {filter}");
    }

    #[test]
    fn escaping_covers_every_rfc4515_special_character() {
        assert_eq!(escape_filter("a*b(c)d\\e/f"), "a\\2ab\\28c\\29d\\5ce\\2ff");
        assert_eq!(escape_filter("ada"), "ada", "ordinary logins are untouched");
    }

    // ---- credential type ---------------------------------------------------

    #[tokio::test]
    async fn an_authorization_code_is_not_an_ldap_credential() {
        let err = provider(Ok(ada()))
            .resolve(Credential::AuthorizationCode {
                code: "c".into(),
                verifier: "v".into(),
                nonce: "n".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            IdentityError::UnsupportedCredential { provider: "ldap" }
        ));
    }

    // ---- configuration validation ------------------------------------------

    #[test]
    fn plain_ldap_is_refused_when_tls_is_required() {
        let mut c = config();
        c.url = "ldap://dir.example.com".into();
        let err = c.validate().unwrap_err();
        assert!(err.contains("cleartext"), "{err}");
        // and is allowed when the operator explicitly opts out
        c.require_tls = false;
        assert!(c.validate().is_ok());
    }

    #[test]
    fn an_unknown_url_scheme_is_refused() {
        let mut c = config();
        c.url = "https://dir.example.com".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn a_filter_without_the_login_placeholder_is_refused() {
        let mut c = config();
        // would authenticate whoever the filter happens to match, for any login
        c.user_filter = "(objectClass=person)".into();
        assert!(c.validate().unwrap_err().contains("{login}"));
    }

    #[test]
    fn an_empty_base_dn_is_refused() {
        let mut c = config();
        c.base_dn = "  ".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn the_constructor_refuses_an_invalid_config() {
        let mut c = config();
        c.base_dn.clear();
        assert!(LdapIdentityProvider::new(c, Box::new(FakeDirectory(Ok(ada())))).is_err());
    }

    #[test]
    fn debug_output_never_carries_the_bind_password() {
        let rendered = format!("{:?}", config());
        assert!(!rendered.contains("svc-password"), "{rendered}");
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn the_provider_reports_its_kind() {
        assert_eq!(provider(Ok(ada())).kind(), "ldap");
    }
}
