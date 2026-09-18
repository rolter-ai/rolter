//! LDAP bind + group mapping (#241).
//!
//! The third [`IdentityProvider`] after local password login and OIDC SSO. It
//! resolves a [`Credential::Password`] by binding to a directory as that user,
//! reading their attributes, and mapping their directory groups onto rolter
//! roles.
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
//! Group mappings are explicit and additive. A user in no mapped group gets no
//! role and is rejected, matching the SSO rule that authentication alone grants
//! nothing. There is no implicit default role and no wildcard mapping.
//!
//! ## What never leaves this module
//!
//! Distinguished names, directory filters, bind credentials and raw LDAP result
//! codes stay here. Everything a caller sees is an [`IdentityError`], and every
//! authentication failure — unknown user, wrong password, user outside the
//! search base, no mapped group — collapses to the same
//! [`IdentityError::NotVerified`]. A directory that enumerates its users
//! through rolter's error messages would be worse than no LDAP at all.

use std::collections::{BTreeMap, HashSet};

use async_trait::async_trait;

use rolter_auth::{Credential, Identity, IdentityError, IdentityProvider};

/// How to reach and query the directory.
#[derive(Debug, Clone)]
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
    /// directory group DN (or name) -> rolter role. Explicit and additive; a
    /// user in no listed group is rejected
    pub group_role_map: BTreeMap<String, String>,
    /// require TLS. `ldaps://` satisfies this; plain `ldap://` does not
    pub require_tls: bool,
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
        if self.group_role_map.is_empty() {
            return Err(
                "group_role_map is empty, so no directory user could ever be granted a role; \
                 map at least one group"
                    .into(),
            );
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

    /// Roles granted by the user's directory groups.
    ///
    /// Matching is case-insensitive on the group name: directories are
    /// inconsistent about DN case, and an operator who types a group in the
    /// case their directory UI shows should not silently get no access.
    pub fn roles_for(&self, groups: &[String]) -> HashSet<String> {
        let held: HashSet<String> = groups.iter().map(|g| g.trim().to_lowercase()).collect();
        self.config
            .group_role_map
            .iter()
            .filter(|(group, _)| held.contains(&group.trim().to_lowercase()))
            .map(|(_, role)| role.clone())
            .collect()
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

        let roles = self.roles_for(&entry.groups);
        if roles.is_empty() {
            // authenticated but granted nothing. Deliberately indistinguishable
            // from a bad password to a client, and logged without the DN
            tracing::info!(
                target: "rolter::ldap",
                "ldap bind succeeded but no directory group maps to a role; rejecting"
            );
            return Err(IdentityError::NotVerified);
        }

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
        use ldap3::{LdapConnAsync, Scope, SearchEntry};

        // an empty password is an *unauthenticated* bind in LDAP: the server
        // accepts it and returns success without verifying anything. Rejecting
        // it here is what stops that from reading as a valid login
        if password.is_empty() {
            return Err(DirectoryError::NotAuthenticated);
        }

        let unavailable = |e: ldap3::LdapError| DirectoryError::Unavailable(e.to_string());

        // phase 1: find the user as the service account
        let (conn, mut ldap) = LdapConnAsync::new(&self.config.url)
            .await
            .map_err(unavailable)?;
        ldap3::drive!(conn);
        ldap.simple_bind(&self.config.bind_dn, &self.config.bind_password)
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
        let (conn, mut user_ldap) = LdapConnAsync::new(&self.config.url)
            .await
            .map_err(unavailable)?;
        ldap3::drive!(conn);
        let bound = user_ldap
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
            group_role_map: BTreeMap::from([
                (
                    "cn=rolter-admins,ou=groups,dc=example,dc=com".into(),
                    "admin".into(),
                ),
                (
                    "cn=rolter-users,ou=groups,dc=example,dc=com".into(),
                    "member".into(),
                ),
            ]),
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
            password: "correct horse".into(),
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

    // ---- group mapping ---------------------------------------------------

    #[test]
    fn groups_map_to_configured_roles_only() {
        let p = provider(Ok(ada()));
        assert_eq!(
            p.roles_for(&["cn=rolter-admins,ou=groups,dc=example,dc=com".into()]),
            HashSet::from(["admin".to_string()])
        );
        // a real directory group that is simply not mapped grants nothing
        assert!(p
            .roles_for(&["cn=everyone,ou=groups,dc=example,dc=com".into()])
            .is_empty());
    }

    #[test]
    fn group_matching_ignores_case_and_surrounding_space() {
        let p = provider(Ok(ada()));
        assert_eq!(
            p.roles_for(&["  CN=Rolter-Admins,OU=Groups,DC=example,DC=com  ".into()]),
            HashSet::from(["admin".to_string()])
        );
    }

    #[test]
    fn multiple_groups_grant_the_union_of_their_roles() {
        let p = provider(Ok(ada()));
        let roles = p.roles_for(&[
            "cn=rolter-admins,ou=groups,dc=example,dc=com".into(),
            "cn=rolter-users,ou=groups,dc=example,dc=com".into(),
        ]);
        assert_eq!(
            roles,
            HashSet::from(["admin".to_string(), "member".to_string()])
        );
    }

    #[tokio::test]
    async fn a_user_in_no_mapped_group_is_rejected_despite_a_valid_password() {
        // least privilege: authenticating proves who you are, not what you may do
        let mut entry = ada();
        entry.groups = vec!["cn=everyone,ou=groups,dc=example,dc=com".into()];
        let err = provider(Ok(entry)).resolve(password()).await.unwrap_err();
        assert!(
            matches!(err, IdentityError::NotVerified),
            "must be indistinguishable from a bad password, got {err:?}"
        );
    }

    #[tokio::test]
    async fn a_user_with_no_groups_at_all_is_rejected() {
        let mut entry = ada();
        entry.groups = vec![];
        assert!(matches!(
            provider(Ok(entry)).resolve(password()).await.unwrap_err(),
            IdentityError::NotVerified
        ));
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
    fn an_empty_group_map_is_refused() {
        let mut c = config();
        c.group_role_map.clear();
        let err = c.validate().unwrap_err();
        assert!(
            err.contains("no directory user could ever be granted a role"),
            "{err}"
        );
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
        c.group_role_map.clear();
        assert!(LdapIdentityProvider::new(c, Box::new(FakeDirectory(Ok(ada())))).is_err());
    }

    #[test]
    fn the_provider_reports_its_kind() {
        assert_eq!(provider(Ok(ada())).kind(), "ldap");
    }
}
