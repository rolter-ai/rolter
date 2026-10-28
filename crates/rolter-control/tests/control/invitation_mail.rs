//! Invitation delivery by email (#1828): the link is sent to the invitee when a
//! mail transport is configured, the outcome is kept on the invitation, and a
//! failure never fails the invitation.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use rolter_control::mail::{MailFailure, Mailer, RelayFailure, RelayStage};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::support::*;

/// Records every message and answers as scripted.
#[derive(Default)]
struct Recorder {
    sent: Mutex<Vec<(String, String, String)>>,
    fail_with: Mutex<Option<String>>,
    relay: Mutex<Option<RelayFailure>>,
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

    async fn check(&self) -> Result<(), RelayFailure> {
        match self.relay.lock().clone() {
            Some(failure) => Err(failure),
            None => Ok(()),
        }
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
    // a resend reads the link back from its sealed copy, which needs the KEK
    std::env::set_var("ROLTER_KEK", TEST_KEK);
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

impl Fixture {
    /// make the last mail attempt old enough that a resend is not throttled
    async fn age_last_attempt(&self) {
        sqlx::query("update invitations set mail_attempted_at = now() - interval '2 minutes'")
            .execute(&self.pool)
            .await
            .unwrap();
    }

    async fn post(&self, path: &str) -> (u16, Value) {
        let response = self
            .client
            .post(format!("{}{path}", self.base))
            .bearer_auth(admin_token())
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }
}

#[tokio::test]
async fn a_resend_mails_the_same_link_and_is_audited_like_the_first_send() {
    skip_without_db!();
    let mailer = Arc::new(Recorder::default());
    *mailer.fail_with.lock() = Some("the mail server could not be reached".to_string());
    let f = fixture(Some(mailer.clone()), "Acme").await;
    let created = f.invite("new@example.com").await;
    assert_eq!(created["invitation"]["mail_status"], "failed");
    let id = created["invitation"]["id"].as_str().unwrap();
    let url = created["accept_url"].as_str().unwrap();

    // the relay recovers; the admin resends instead of revoking and re-inviting
    *mailer.fail_with.lock() = None;
    f.age_last_attempt().await;
    let (status, resent) = f.post(&format!("/api/v1/invitations/{id}/resend")).await;
    assert_eq!(status, 200, "{resent}");
    assert_eq!(resent["mail_status"], "sent");
    assert!(resent["mail_error"].is_null());
    assert!(
        resent.get("token").is_none(),
        "the link is not handed out twice"
    );

    let sent = mailer.sent.lock().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, "new@example.com");
    assert!(sent[0].2.contains(url), "the same link goes out again");

    // the original token still opens the invitation: nothing was reissued
    let token = created["token"].as_str().unwrap();
    let preview = f
        .client
        .get(format!("{}/api/v1/invitations/accept/{token}", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(preview.status(), 200);

    assert_eq!(f.audited("invitation.resend").await.len(), 1);
    assert_eq!(f.audited("invitation.mail_failed").await.len(), 1);
    let delivered = f.audited("invitation.mail_sent").await;
    assert_eq!(delivered.len(), 1);
    let rendered = delivered[0].to_string();
    assert!(
        !rendered.contains("new@example.com") && !rendered.contains(url),
        "{rendered}"
    );
}

#[tokio::test]
async fn an_invitation_that_cannot_be_resent_says_why() {
    skip_without_db!();
    let mailer = Arc::new(Recorder::default());
    let f = fixture(Some(mailer.clone()), "Acme").await;
    let created = f.invite("new@example.com").await;
    let id = created["invitation"]["id"].as_str().unwrap().to_string();

    // a link that was never kept (created before resending existed, say)
    sqlx::query("update invitations set token_ciphertext = null, token_nonce = null")
        .execute(&f.pool)
        .await
        .unwrap();
    let (status, body) = f.post(&format!("/api/v1/invitations/{id}/resend")).await;
    assert_eq!(status, 409, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("revoke"));

    // an invitation that was revoked is not pending
    let second = f.invite("other@example.com").await;
    let second_id = second["invitation"]["id"].as_str().unwrap().to_string();
    let revoked = f
        .client
        .delete(format!("{}/api/v1/invitations/{second_id}", f.base))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 200);
    let sealed: Option<Vec<u8>> =
        sqlx::query_scalar("select token_ciphertext from invitations where id = $1::uuid")
            .bind(&second_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(sealed.is_none(), "revoking clears the sealed link");
    let (status, _) = f
        .post(&format!("/api/v1/invitations/{second_id}/resend"))
        .await;
    assert_eq!(status, 409);

    // and without a relay there is nothing to resend through
    let bare = fixture(None, "Bare").await;
    let created = bare.invite("new@example.com").await;
    let id = created["invitation"]["id"].as_str().unwrap();
    let (status, body) = bare.post(&format!("/api/v1/invitations/{id}/resend")).await;
    assert_eq!(status, 409, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("not configured"));
}

#[tokio::test]
async fn the_relay_check_names_the_stage_that_failed_and_nothing_else() {
    skip_without_db!();
    let mailer = Arc::new(Recorder::default());
    let f = fixture(Some(mailer.clone()), "Acme").await;
    let path = format!("/api/v1/orgs/{}/invitations/mail-check", f.org);

    let (status, body) = f.post(&path).await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({"ok": true}));

    // a second run straight away is refused, with a wait to honour (#3394)
    let response = f
        .client
        .post(format!("{}{path}", f.base))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 429);
    let wait: u64 = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .expect("a refused check says when to retry");
    assert!(wait >= 1);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    *mailer.relay.lock() = Some(RelayFailure {
        stage: RelayStage::Auth,
        reason: "the mail server refused the credential in the relay url".to_string(),
    });
    let (status, body) = f.post(&path).await;
    assert_eq!(status, 200);
    assert_eq!(body["ok"], false);
    assert_eq!(body["stage"], "auth");
    assert!(body["reason"].as_str().unwrap().contains("credential"));
    assert!(mailer.sent.lock().is_empty(), "a check sends no message");

    let bare = fixture(None, "Bare").await;
    let (status, _) = bare
        .post(&format!("/api/v1/orgs/{}/invitations/mail-check", bare.org))
        .await;
    assert_eq!(status, 409);
}

#[tokio::test]
async fn a_resend_is_refused_while_the_last_attempt_is_under_a_minute_old() {
    skip_without_db!();
    let mailer = Arc::new(Recorder::default());
    let f = fixture(Some(mailer.clone()), "Acme").await;
    let created = f.invite("new@example.com").await;
    let id = created["invitation"]["id"].as_str().unwrap().to_string();
    assert_eq!(mailer.sent.lock().len(), 1);

    // the first send was a moment ago
    let response = f
        .client
        .post(format!("{}/api/v1/invitations/{id}/resend", f.base))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 429);
    let wait: u64 = response
        .headers()
        .get("retry-after")
        .expect("a Retry-After")
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&wait), "{wait}");
    assert_eq!(
        mailer.sent.lock().len(),
        1,
        "a refused resend sends nothing"
    );

    // once the minute has passed the same request goes out
    f.age_last_attempt().await;
    let (status, _) = f.post(&format!("/api/v1/invitations/{id}/resend")).await;
    assert_eq!(status, 200);
    assert_eq!(mailer.sent.lock().len(), 2);
    // and starts the next minute
    let (status, _) = f.post(&format!("/api/v1/invitations/{id}/resend")).await;
    assert_eq!(status, 429);
}

#[tokio::test]
async fn a_viewer_cannot_resend_an_invitation() {
    skip_without_db!();
    let mailer = Arc::new(Recorder::default());
    let f = fixture(Some(mailer.clone()), "Acme").await;
    let created = f.invite("new@example.com").await;
    let id = created["invitation"]["id"].as_str().unwrap().to_string();
    f.age_last_attempt().await;

    let viewer: Uuid = sqlx::query_scalar(
        "insert into users (email, password_hash, is_superadmin) \
         values ('viewer@example.com', null, false) returning id",
    )
    .fetch_one(&f.pool)
    .await
    .unwrap();
    sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'viewer')")
        .bind(viewer)
        .bind(f.org)
        .execute(&f.pool)
        .await
        .unwrap();
    let session = format!("rolter_sess_resend_{}", Uuid::new_v4().simple());
    sqlx::query(
        "insert into sessions (user_id, token_hash, expires_at) \
         values ($1, $2, now() + interval '1 hour')",
    )
    .bind(viewer)
    .bind(rolter_auth::hash_key("", &session))
    .execute(&f.pool)
    .await
    .unwrap();
    for path in [
        format!("/api/v1/invitations/{id}/resend"),
        format!("/api/v1/orgs/{}/invitations/mail-check", f.org),
    ] {
        let response = f
            .client
            .post(format!("{}{path}", f.base))
            .bearer_auth(&session)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403, "{path}");
    }
    assert_eq!(mailer.sent.lock().len(), 1, "nothing was sent for a viewer");
}

#[tokio::test]
async fn an_expired_invitation_loses_its_sealed_token() {
    skip_without_db!();
    let mailer = Arc::new(Recorder::default());
    let f = fixture(Some(mailer), "Acme").await;
    let live = f.invite("live@example.com").await;
    let live_id: Uuid = live["invitation"]["id"].as_str().unwrap().parse().unwrap();
    let old = f.invite("old@example.com").await;
    let old_id: Uuid = old["invitation"]["id"].as_str().unwrap().parse().unwrap();
    let sealed = |id: Uuid| {
        let pool = f.pool.clone();
        async move {
            sqlx::query_scalar::<_, bool>(
                "select token_ciphertext is not null and token_nonce is not null \
                 from invitations where id = $1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert!(sealed(old_id).await && sealed(live_id).await);

    // nothing has expired yet
    let repo = rolter_store::postgres::repo::InvitationRepo(&f.pool);
    assert_eq!(repo.sweep_expired_sealed_tokens(100).await.unwrap(), 0);

    sqlx::query("update invitations set expires_at = now() - interval '1 minute' where id = $1")
        .bind(old_id)
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(repo.sweep_expired_sealed_tokens(100).await.unwrap(), 1);
    assert!(!sealed(old_id).await, "the expired copy is cleared");
    assert!(sealed(live_id).await, "a live invitation keeps its copy");
    // and it stays cleared: a second pass finds nothing
    assert_eq!(repo.sweep_expired_sealed_tokens(100).await.unwrap(), 0);
}
