//! Invitation delivery by email (#1828): the link is sent to the invitee when a
//! mail transport is configured, the outcome is kept on the invitation, and a
//! failure never fails the invitation.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use rolter_control::mail::{MailFailure, Mailer};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::support::*;

/// Records every message and answers as scripted.
#[derive(Default)]
struct Recorder {
    sent: Mutex<Vec<(String, String, String)>>,
    fail_with: Mutex<Option<String>>,
}

#[async_trait]
impl Mailer for Recorder {
    async fn send(&self, to: &str, subject: &str, body: &str) -> Result<(), MailFailure> {
        if let Some(reason) = self.fail_with.lock().clone() {
            return Err(MailFailure::new(&reason));
        }
        self.sent
            .lock()
            .push((to.to_string(), subject.to_string(), body.to_string()));
        Ok(())
    }
}

struct Fixture {
    base: String,
    client: reqwest::Client,
    pool: sqlx::PgPool,
    org: Uuid,
    _db: rolter_store::postgres::test_schema::TestSchema,
}

async fn fixture(mailer: Option<Arc<Recorder>>, org_name: &str) -> Fixture {
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = match mailer {
        Some(mailer) => rolter_control::test_app_with_mailer(
            pool.clone(),
            Some(admin_token().to_string()),
            mailer,
        )
        .await
        .unwrap(),
        None => {
            rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
                .await
                .unwrap()
        }
    };
    let addr = serve(app).await;
    let org: Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ($1, 'acme') returning id")
            .bind(org_name)
            .fetch_one(&pool)
            .await
            .unwrap();
    Fixture {
        base: format!("http://{addr}"),
        client: reqwest::Client::new(),
        pool,
        org,
        _db: db,
    }
}

impl Fixture {
    async fn invite(&self, email: &str) -> Value {
        let response = self
            .client
            .post(format!(
                "{}/api/v1/orgs/{}/invitations",
                self.base, self.org
            ))
            .bearer_auth(admin_token())
            .json(&json!({"email": email, "role": "member"}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            200,
            "an invitation is never refused over mail"
        );
        response.json().await.unwrap()
    }

    async fn audited(&self, action: &str) -> Vec<Value> {
        sqlx::query_scalar(
            "select coalesce(detail, 'null'::jsonb) from audit_log where action = $1",
        )
        .bind(action)
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn without_a_mail_transport_the_invitation_is_exactly_what_it_was() {
    skip_without_db!();
    let f = fixture(None, "Acme").await;
    let created = f.invite("new@example.com").await;

    assert!(created["token"].as_str().unwrap().len() > 10);
    assert!(created["accept_url"].as_str().unwrap().contains("/invite/"));
    assert_eq!(created["invitation"]["mail_status"], "not_configured");
    assert!(created["invitation"]["mail_attempted_at"].is_null());
    assert!(f.audited("invitation.mail_sent").await.is_empty());
    assert!(f.audited("invitation.mail_failed").await.is_empty());
}

#[tokio::test]
async fn the_link_is_emailed_to_the_invitee_and_still_returned() {
    skip_without_db!();
    let mailer = Arc::new(Recorder::default());
    let f = fixture(Some(mailer.clone()), "Acme").await;
    let created = f.invite("new@example.com").await;

    let sent = mailer.sent.lock().clone();
    assert_eq!(sent.len(), 1);
    let (to, subject, body) = &sent[0];
    assert_eq!(to, "new@example.com");
    assert!(subject.contains("Acme"), "{subject}");
    let url = created["accept_url"].as_str().unwrap();
    assert!(body.contains(url), "the message carries the link");
    assert!(body.contains("member"));

    assert_eq!(created["invitation"]["mail_status"], "sent");
    assert!(created["invitation"]["mail_attempted_at"].is_string());
    assert!(created["invitation"]["mail_error"].is_null());

    // the list says the same, so the admin sees it next to the row
    let listed: Vec<Value> = f
        .client
        .get(format!("{}/api/v1/orgs/{}/invitations", f.base, f.org))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed[0]["mail_status"], "sent");

    // the audit row names the invitation and nothing that identifies a person
    // or opens an account
    let audit = f.audited("invitation.mail_sent").await;
    assert_eq!(audit.len(), 1);
    let rendered = audit[0].to_string();
    assert!(
        !rendered.contains("new@example.com") && !rendered.contains(url),
        "{rendered}"
    );
}

#[tokio::test]
async fn a_failed_send_is_recorded_and_never_fails_the_invitation() {
    skip_without_db!();
    let mailer = Arc::new(Recorder::default());
    *mailer.fail_with.lock() = Some("the mail server could not be reached".to_string());
    let f = fixture(Some(mailer.clone()), "Acme").await;
    let created = f.invite("new@example.com").await;

    assert!(
        created["token"].as_str().is_some(),
        "the link is the fallback"
    );
    assert_eq!(created["invitation"]["mail_status"], "failed");
    assert_eq!(
        created["invitation"]["mail_error"],
        "the mail server could not be reached"
    );
    let audit = f.audited("invitation.mail_failed").await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0]["reason"], "the mail server could not be reached");

    // and the invitation can still be accepted
    let token = created["token"].as_str().unwrap();
    let preview = f
        .client
        .get(format!("{}/api/v1/invitations/accept/{token}", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(preview.status(), 200);
    let stored: String = sqlx::query_scalar(
        "select mail_status from invitations where lower(email) = 'new@example.com'",
    )
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(stored, "failed");
}

#[tokio::test]
async fn an_organization_name_cannot_inject_a_header() {
    skip_without_db!();
    let mailer = Arc::new(Recorder::default());
    let f = fixture(Some(mailer.clone()), "Acme\r\nBcc: attacker@example.com").await;
    f.invite("new@example.com").await;

    let sent = mailer.sent.lock().clone();
    let subject = &sent[0].1;
    assert!(
        !subject.contains('\r') && !subject.contains('\n'),
        "{subject:?}"
    );
}
