//! Pluggable identity resolution (ROL-35).
//!
//! Local password login and OIDC SSO both end at the same place: a verified
//! [`Identity`] that the control plane turns into a session and reconciled
//! memberships. [`IdentityProvider`] is the seam between "how was this person
//! verified" (provider-specific: an argon2 hash comparison, a JWKS-checked id
//! token, an LDAP bind once #1826 wires it) and "what rolter does once they
//! are" (identical for every provider, so it lives outside this trait).
//!
//! Concrete providers own the dependencies verification actually needs
//! (a database pool, an HTTP client, a directory connection) and live next to
//! that code — `rolter-control::auth::LocalIdentityProvider` and
//! `rolter-control::sso::OidcIdentityProvider` today. This crate only defines
//! the shape they agree on, so it stays free of `sqlx`/`reqwest`/`ldap3`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A verified identity, independent of which provider proved it.
///
/// `subject` is the provider's own stable identifier for the account (a user
/// id for local login, the id token's `sub` for OIDC, a distinguished name for
/// LDAP); it is opaque outside the provider that issued it. `email` is what
/// rolter keys accounts on across providers, matching the existing SSO
/// adopt-by-email behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub subject: String,
    pub email: String,
    #[serde(default)]
    pub display_name: Option<String>,
    /// group names the provider asserts for this identity, for role-mapping
    /// callers (SSO group mappings today; empty for local login, which has no
    /// group concept of its own)
    #[serde(default)]
    pub groups: Vec<String>,
}

/// What a provider is asked to verify. Each provider only understands the
/// variant(s) that correspond to how it authenticates; anything else is a
/// [`IdentityError::UnsupportedCredential`].
#[derive(Debug, Clone)]
pub enum Credential {
    /// email + plaintext password, as submitted to `/api/v1/auth/login`
    Password { email: String, password: String },
    /// an OIDC authorization code plus the PKCE verifier and nonce recorded
    /// when the login started; the provider exchanges and verifies it
    AuthorizationCode {
        code: String,
        verifier: String,
        nonce: String,
    },
}

/// Why a provider could not resolve a [`Credential`] to an [`Identity`].
///
/// This is deliberately coarse: callers map it to a single generic rejection
/// (see `rolter-control::auth::AuthError::InvalidCredentials`) rather than
/// echoing provider internals to the client, the same way local login today
/// gives identical responses to "no such account" and "wrong password".
#[derive(Debug, Clone, thiserror::Error)]
pub enum IdentityError {
    /// the credential did not verify (bad password, invalid/expired id token,
    /// unknown account, ...)
    #[error("credential could not be verified")]
    NotVerified,
    /// this provider does not accept the [`Credential`] variant it was given
    #[error("provider {provider} does not accept this credential type")]
    UnsupportedCredential { provider: &'static str },
    /// the credential itself may be valid, but a deployment policy refuses it
    /// (e.g. an org that requires SSO rejecting a password login) — distinct
    /// from [`IdentityError::NotVerified`] because callers may want to say so
    /// plainly rather than respond as if the credential were simply wrong
    #[error("{0}")]
    PolicyDenied(String),
    /// the provider itself failed (network error, database error, ...),
    /// distinct from the credential being wrong
    #[error("identity provider error: {0}")]
    Provider(String),
}

/// A pluggable source of verified identities.
///
/// New providers (LDAP bind + group mapping, #241, wired by #1826) implement
/// this trait; nothing downstream of a resolved [`Identity`] needs to know
/// which provider produced it.
#[async_trait]
pub trait IdentityProvider: Send + Sync {
    /// stable identifier of the provider kind, e.g. `"local"`, `"oidc"`,
    /// `"ldap"`; used in audit logs and error messages, not for dispatch
    fn kind(&self) -> &'static str;

    /// Verify `credential` and return the identity it proves.
    async fn resolve(&self, credential: Credential) -> Result<Identity, IdentityError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AlwaysRejects;

    #[async_trait]
    impl IdentityProvider for AlwaysRejects {
        fn kind(&self) -> &'static str {
            "always-rejects"
        }

        async fn resolve(&self, _credential: Credential) -> Result<Identity, IdentityError> {
            Err(IdentityError::NotVerified)
        }
    }

    struct EchoesPassword;

    #[async_trait]
    impl IdentityProvider for EchoesPassword {
        fn kind(&self) -> &'static str {
            "echoes-password"
        }

        async fn resolve(&self, credential: Credential) -> Result<Identity, IdentityError> {
            match credential {
                Credential::Password { email, .. } => Ok(Identity {
                    subject: email.clone(),
                    email,
                    display_name: None,
                    groups: Vec::new(),
                }),
                Credential::AuthorizationCode { .. } => Err(IdentityError::UnsupportedCredential {
                    provider: self.kind(),
                }),
            }
        }
    }

    #[tokio::test]
    async fn dyn_dispatch_works_across_providers() {
        // the whole point of the trait: callers hold a `Box<dyn IdentityProvider>`
        // and never match on the concrete type
        let providers: Vec<Box<dyn IdentityProvider>> =
            vec![Box::new(AlwaysRejects), Box::new(EchoesPassword)];

        let cred = Credential::Password {
            email: "ada@example.com".into(),
            password: "irrelevant".into(),
        };

        let mut kinds_seen = Vec::new();
        for provider in &providers {
            kinds_seen.push(provider.kind());
        }
        assert_eq!(kinds_seen, ["always-rejects", "echoes-password"]);

        assert!(matches!(
            providers[0].resolve(cred.clone()).await,
            Err(IdentityError::NotVerified)
        ));
        let identity = providers[1].resolve(cred).await.unwrap();
        assert_eq!(identity.email, "ada@example.com");
        assert_eq!(identity.subject, "ada@example.com");
    }

    #[tokio::test]
    async fn unsupported_credential_is_reported_by_kind() {
        struct OnlyAuthorizationCode;

        #[async_trait]
        impl IdentityProvider for OnlyAuthorizationCode {
            fn kind(&self) -> &'static str {
                "oidc"
            }

            async fn resolve(&self, credential: Credential) -> Result<Identity, IdentityError> {
                match credential {
                    Credential::AuthorizationCode { .. } => Ok(Identity {
                        subject: "sub-1".into(),
                        email: "ada@example.com".into(),
                        display_name: None,
                        groups: vec![],
                    }),
                    Credential::Password { .. } => {
                        Err(IdentityError::UnsupportedCredential { provider: "oidc" })
                    }
                }
            }
        }

        let provider = OnlyAuthorizationCode;
        let err = provider
            .resolve(Credential::Password {
                email: "ada@example.com".into(),
                password: "x".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            IdentityError::UnsupportedCredential { provider: "oidc" }
        ));

        let identity = provider
            .resolve(Credential::AuthorizationCode {
                code: "c".into(),
                verifier: "v".into(),
                nonce: "n".into(),
            })
            .await
            .unwrap();
        assert_eq!(identity.subject, "sub-1");
    }

    #[test]
    fn identity_error_messages_do_not_leak_provider_internals() {
        let err = IdentityError::NotVerified;
        assert_eq!(err.to_string(), "credential could not be verified");
    }
}
