//! Outbound mail for invitations (#1828).
//!
//! An invitation is a one-time link. Handing it to the creating admin to paste
//! into chat puts a credential-grade link in someone's chat history, which is
//! the exposure invitations exist to avoid, so with a mail transport configured
//! the control plane sends the link to the invitee itself. The link is still
//! returned to the admin as the fallback, and without a transport nothing
//! changes: an air-gapped deployment never needs one.
//!
//! ## Configuration
//!
//! Two environment variables, read once at startup:
//!
//! * `ROLTER_SMTP_URL` — `smtp://[user:password@]host[:port]` (STARTTLS when the
//!   server offers it) or `smtps://…` (implicit TLS); `?tls=required` insists on
//!   STARTTLS. It can carry a credential, so it is treated as the secret it is:
//!   held in the environment (a Kubernetes secret, a compose secret) like
//!   `ROLTER_DATABASE_URL`, hidden from every log line, and refused as a
//!   provider credential name by the same rule as every other `ROLTER_` secret.
//! * `ROLTER_SMTP_FROM` — the sender, `noreply@example.com` or
//!   `Rolter <noreply@example.com>`.
//!
//! The relay's address is held to the egress policy like any other destination
//! the control plane dials: a literal address the policy denies (cloud instance
//! metadata, by default) turns mail off rather than being dialled. Certificates
//! are verified through the platform's trust store, so a private CA installed on
//! the host applies to the relay as well.
//!
//! ## What is never logged
//!
//! Neither the URL, the recipient nor the link. A send that fails is recorded
//! on the invitation row with a short reason from `MailFailure`, written to be
//! safe to show an admin; the relay's own reply is not kept, because it often
//! names the recipient.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use rolter_core::EgressPolicy;

/// How long one delivery attempt may take, connect to final reply. An
/// invitation waits on it, so it is bounded well under a request timeout.
const SEND_TIMEOUT: Duration = Duration::from_secs(15);

/// Why a message was not delivered, in words that are safe to store on the
/// invitation row and show an admin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailFailure {
    pub reason: String,
}

impl MailFailure {
    /// A failure with a reason that is safe to show an admin.
    pub fn new(reason: &str) -> Self {
        Self {
            reason: reason.to_string(),
        }
    }
}

/// How far a relay check got before it failed, so an admin knows which part of
/// the relay's configuration to look at (#3304).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayStage {
    /// the relay could not be reached, or did not answer in time
    Connect,
    /// the connection was made but the secure channel could not be set up
    Tls,
    /// the relay refused the credential in `ROLTER_SMTP_URL`
    Auth,
    /// the relay answered, but not the way a mail server does
    Relay,
}

impl RelayStage {
    /// The stable name an API client branches on.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Tls => "tls",
            Self::Auth => "auth",
            Self::Relay => "relay",
        }
    }
}

/// Why a relay check failed: the stage, and words that are safe to show an
/// admin. Never the relay's own reply, the url or the credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayFailure {
    pub stage: RelayStage,
    pub reason: String,
}

impl RelayFailure {
    fn new(stage: RelayStage, reason: &str) -> Self {
        Self {
            stage,
            reason: reason.to_string(),
        }
    }
}

/// Sends one plain-text message. A trait so the integration suite can record
/// what would have been sent, and fail on demand, without a mail server.
#[async_trait]
pub trait Mailer: Send + Sync {
    async fn send(&self, to: &str, subject: &str, body: &str) -> Result<(), MailFailure>;

    /// Connect, set up TLS and authenticate against the relay without sending
    /// anything (#3304). A recording mailer has no relay, so it always passes.
    async fn check(&self) -> Result<(), RelayFailure> {
        Ok(())
    }
}

/// Delivery through an SMTP relay.
pub(crate) struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl SmtpMailer {
    /// Build a mailer from the relay URL and sender, or say why not. The error
    /// never repeats the URL, which can carry a password.
    pub(crate) fn new(url: &str, from: &str, egress: &EgressPolicy) -> Result<Self, String> {
        if let Some(range) = egress.url_deny_reason(url) {
            return Err(format!(
                "the mail relay is a {range} address, which the egress policy denies \
                 (allow it via egress.allow_hosts if this is intentional)"
            ));
        }
        let from: Mailbox = from
            .parse()
            .map_err(|_| "ROLTER_SMTP_FROM is not a valid mailbox".to_string())?;
        let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(url)
            .map_err(|_| "ROLTER_SMTP_URL is not a valid smtp:// or smtps:// url".to_string())?
            .timeout(Some(SEND_TIMEOUT))
            .build();
        Ok(Self { transport, from })
    }
}

#[async_trait]
impl Mailer for SmtpMailer {
    async fn check(&self) -> Result<(), RelayFailure> {
        match tokio::time::timeout(SEND_TIMEOUT, self.transport.test_connection()).await {
            Ok(Ok(true)) => Ok(()),
            Ok(Ok(false)) => Err(RelayFailure::new(
                RelayStage::Relay,
                "the mail server did not answer a status check",
            )),
            Ok(Err(error)) => {
                let failure = classify_relay(&error);
                // the stage and nothing else, as for a delivery
                tracing::warn!(target: "rolter::mail", stage = failure.stage.as_str(), "mail relay check failed");
                Err(failure)
            }
            Err(_) => Err(RelayFailure::new(
                RelayStage::Connect,
                "the mail server did not answer in time",
            )),
        }
    }

    async fn send(&self, to: &str, subject: &str, body: &str) -> Result<(), MailFailure> {
        let to: Mailbox = to
            .parse()
            .map_err(|_| MailFailure::new("the invitation address is not a deliverable mailbox"))?;
        let message = Message::builder()
            .from(self.from.clone())
            .to(to)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_string())
            .map_err(|_| MailFailure::new("the message could not be built"))?;
        match self.transport.send(message).await {
            Ok(_) => Ok(()),
            Err(error) => {
                let failure = classify(&error);
                // the class and nothing else: the relay's reply often names the
                // recipient, and the transport's text can echo the url
                tracing::warn!(target: "rolter::mail", reason = %failure.reason, "invitation mail not delivered");
                Err(failure)
            }
        }
    }
}

fn classify(error: &lettre::transport::smtp::Error) -> MailFailure {
    if error.is_permanent() {
        MailFailure::new("the mail server rejected the message")
    } else if error.is_transient() {
        MailFailure::new("the mail server could not accept the message right now")
    } else if error.is_tls() {
        MailFailure::new("a secure connection to the mail server could not be made")
    } else if error.is_timeout() {
        MailFailure::new("the mail server did not answer in time")
    } else if error.is_client() {
        MailFailure::new("the control plane could not use the mail server's reply")
    } else {
        MailFailure::new("the mail server could not be reached")
    }
}

/// SMTP replies that mean the credential was refused: 530 (authentication
/// required), 534 (mechanism too weak), 535 (credentials rejected), 538
/// (encryption required for the mechanism) and 454 (temporary failure).
fn is_auth_reply(error: &lettre::transport::smtp::Error) -> bool {
    error
        .status()
        .is_some_and(|code| matches!(u16::from(code), 454 | 530 | 534 | 535 | 538))
}

/// Whether a failed handshake is behind the error: the transport reports a
/// rejected TLS handshake as a connection error whose cause is an
/// `InvalidData` I/O error, so the kind alone cannot tell it from a refusal.
fn is_handshake_failure(error: &lettre::transport::smtp::Error) -> bool {
    let mut cause = std::error::Error::source(error);
    while let Some(next) = cause {
        if let Some(io) = next.downcast_ref::<std::io::Error>() {
            return io.kind() == std::io::ErrorKind::InvalidData;
        }
        cause = next.source();
    }
    false
}

fn classify_relay(error: &lettre::transport::smtp::Error) -> RelayFailure {
    if error.is_tls() || is_handshake_failure(error) {
        RelayFailure::new(
            RelayStage::Tls,
            "a secure connection to the mail server could not be made",
        )
    } else if is_auth_reply(error) {
        RelayFailure::new(
            RelayStage::Auth,
            "the mail server refused the credential in the relay url",
        )
    } else if error.is_client() {
        // the server was reached and the client refused to go on: which part of
        // the relay's configuration it disagreed with is in the cause's text,
        // which is read here and never repeated
        let cause = std::error::Error::source(error)
            .map(|cause| cause.to_string().to_ascii_lowercase())
            .unwrap_or_default();
        if cause.contains("starttls") {
            RelayFailure::new(
                RelayStage::Tls,
                "the mail server does not offer the secure connection the relay url asks for",
            )
        } else if cause.contains("authentication") {
            RelayFailure::new(
                RelayStage::Auth,
                "the mail server offers no way to sign in that the control plane can use",
            )
        } else {
            RelayFailure::new(
                RelayStage::Relay,
                "the mail server answered, but not in a way the control plane can use",
            )
        }
    } else if error.is_timeout() {
        RelayFailure::new(
            RelayStage::Connect,
            "the mail server did not answer in time",
        )
    } else if error.is_permanent() || error.is_transient() || error.is_response() {
        RelayFailure::new(
            RelayStage::Relay,
            "the mail server answered, but refused to talk to the control plane",
        )
    } else {
        RelayFailure::new(RelayStage::Connect, "the mail server could not be reached")
    }
}

/// Check the relay the environment describes, for `rolter check`: `None` when
/// mail is off (no `ROLTER_SMTP_URL`), otherwise how the check went. A
/// configuration that cannot build a mailer is reported as a failure at the
/// `relay` stage with the same words the control plane logs at startup.
pub async fn check_env_relay(egress: &EgressPolicy) -> Option<Result<(), RelayFailure>> {
    let url = std::env::var("ROLTER_SMTP_URL").ok()?;
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let from = std::env::var("ROLTER_SMTP_FROM").unwrap_or_default();
    if from.trim().is_empty() {
        return Some(Err(RelayFailure::new(
            RelayStage::Relay,
            "ROLTER_SMTP_URL is set but ROLTER_SMTP_FROM is not",
        )));
    }
    Some(match SmtpMailer::new(url, from.trim(), egress) {
        Ok(mailer) => mailer.check().await,
        Err(problem) => Err(RelayFailure {
            stage: RelayStage::Relay,
            reason: problem,
        }),
    })
}

/// The mailer the environment describes, or `None` when mail is off.
///
/// A blank or absent `ROLTER_SMTP_URL` is the common case and says nothing. A
/// configuration that does not work is logged once, loudly, and turns mail off
/// rather than the control plane: the link is still returned to the admin, so a
/// typo costs the convenience and not the ability to invite.
pub(crate) fn from_env(egress: &EgressPolicy) -> Option<Arc<dyn Mailer>> {
    let url = std::env::var("ROLTER_SMTP_URL").ok()?;
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let from = std::env::var("ROLTER_SMTP_FROM").unwrap_or_default();
    if from.trim().is_empty() {
        tracing::error!(
            "ROLTER_SMTP_URL is set but ROLTER_SMTP_FROM is not: invitation mail is off"
        );
        return None;
    }
    match SmtpMailer::new(url, from.trim(), egress) {
        Ok(mailer) => {
            tracing::info!("invitation mail is on");
            Some(Arc::new(mailer))
        }
        Err(problem) => {
            tracing::error!(problem = %problem, "invitation mail is off");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn egress() -> EgressPolicy {
        EgressPolicy::default()
    }

    #[test]
    fn a_relay_url_and_sender_build_a_mailer() {
        assert!(SmtpMailer::new(
            "smtp://user:secret@mail.example.com:587",
            "Rolter <noreply@example.com>",
            &egress()
        )
        .is_ok());
        assert!(
            SmtpMailer::new("smtps://mail.example.com", "noreply@example.com", &egress()).is_ok()
        );
    }

    #[test]
    fn a_bad_url_or_sender_is_refused_without_repeating_the_url() {
        for url in ["not a url", "https://mail.example.com", "smtp://:::"] {
            let error = SmtpMailer::new(url, "noreply@example.com", &egress())
                .err()
                .unwrap_or_default();
            assert!(!error.is_empty(), "{url} was accepted");
            assert!(!error.contains(url), "{error}");
        }
        let error = SmtpMailer::new("smtp://u:pw-1234@mail.example.com", "nobody", &egress())
            .err()
            .unwrap_or_default();
        assert!(error.contains("ROLTER_SMTP_FROM"));
        assert!(!error.contains("pw-1234"));
    }

    #[test]
    fn a_relay_the_egress_policy_denies_is_not_dialled() {
        let error = SmtpMailer::new(
            "smtp://user:pw-1234@169.254.169.254:25",
            "noreply@example.com",
            &egress(),
        )
        .err()
        .unwrap_or_default();
        assert!(error.contains("egress policy"), "{error}");
        assert!(
            !error.contains("pw-1234") && !error.contains("169.254"),
            "{error}"
        );
        // and an explicit allow lifts it
        let allowed = EgressPolicy {
            allow_hosts: vec!["169.254.169.254".into()],
            ..EgressPolicy::default()
        };
        assert!(
            SmtpMailer::new("smtp://169.254.169.254:25", "noreply@example.com", &allowed).is_ok()
        );
    }

    /// A one-connection stand-in relay that answers each line the client sends
    /// with the next scripted reply, after greeting it.
    async fn relay(greeting: &'static str, replies: &'static [&'static str]) -> u16 {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            if write.write_all(greeting.as_bytes()).await.is_err() {
                return;
            }
            for reply in replies {
                if lines.next_line().await.ok().flatten().is_none() {
                    return;
                }
                if write.write_all(reply.as_bytes()).await.is_err() {
                    return;
                }
            }
            // hold the connection open until the client hangs up
            let _ = lines.next_line().await;
        });
        port
    }

    async fn check_url(url: &str) -> Result<(), RelayFailure> {
        SmtpMailer::new(url, "noreply@example.com", &egress())
            .unwrap()
            .check()
            .await
    }

    #[tokio::test]
    async fn a_relay_nobody_listens_on_fails_at_connect() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let failure = check_url(&format!("smtp://u:pw-1234@127.0.0.1:{port}"))
            .await
            .unwrap_err();
        assert_eq!(failure.stage, RelayStage::Connect);
        assert!(!failure.reason.contains("pw-1234"), "{}", failure.reason);
    }

    #[tokio::test]
    async fn a_refused_credential_fails_at_auth_and_is_not_echoed() {
        let port = relay(
            "220 relay ready\r\n",
            &[
                "250-relay\r\n250 AUTH PLAIN LOGIN\r\n",
                "535 5.7.8 pw-1234 rejected for u\r\n",
            ],
        )
        .await;
        let failure = check_url(&format!("smtp://u:pw-1234@127.0.0.1:{port}"))
            .await
            .unwrap_err();
        assert_eq!(failure.stage, RelayStage::Auth, "{failure:?}");
        assert!(!failure.reason.contains("pw-1234"), "{}", failure.reason);
        assert!(!failure.reason.contains("127.0.0.1"), "{}", failure.reason);
    }

    #[tokio::test]
    async fn a_relay_that_does_not_speak_tls_fails_at_tls() {
        let port = relay("220 plaintext only\r\n", &[]).await;
        let failure = check_url(&format!("smtps://u:pw-1234@127.0.0.1:{port}"))
            .await
            .unwrap_err();
        assert_eq!(failure.stage, RelayStage::Tls, "{failure:?}");
        assert!(!failure.reason.contains("pw-1234"), "{}", failure.reason);
    }

    #[tokio::test]
    async fn a_relay_without_starttls_fails_at_tls_when_it_is_required() {
        let port = relay("220 relay ready\r\n", &["250 relay\r\n"]).await;
        let failure = check_url(&format!("smtp://127.0.0.1:{port}?tls=required"))
            .await
            .unwrap_err();
        assert_eq!(failure.stage, RelayStage::Tls, "{failure:?}");
        assert!(!failure.reason.contains("127.0.0.1"), "{}", failure.reason);
    }

    #[tokio::test]
    async fn a_relay_with_no_auth_fails_at_auth_when_a_credential_is_given() {
        let port = relay("220 relay ready\r\n", &["250 relay\r\n"]).await;
        let failure = check_url(&format!("smtp://u:pw-1234@127.0.0.1:{port}"))
            .await
            .unwrap_err();
        assert_eq!(failure.stage, RelayStage::Auth, "{failure:?}");
        assert!(!failure.reason.contains("pw-1234"), "{}", failure.reason);
    }

    #[tokio::test]
    async fn a_relay_that_answers_passes() {
        let port = relay(
            "220 relay ready\r\n",
            &["250 relay\r\n", "250 ok\r\n", "221 bye\r\n"],
        )
        .await;
        assert_eq!(check_url(&format!("smtp://127.0.0.1:{port}")).await, Ok(()));
    }
}
