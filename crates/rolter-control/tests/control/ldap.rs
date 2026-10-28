//! LDAP sign-in (#1826): the directory settings API, the step the password
//! login takes through a directory, and the grants it produces.
//!
//! The directory is a stub installed through `test_app_with_directory`, so the
//! whole path from `POST /api/v1/auth/login` to the membership rows is covered
//! without a directory server; the two-phase bind itself is covered in
//! `crate::ldap`'s unit tests.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use rolter_control::ldap::{
    Directory, DirectoryEntry, DirectoryError, DirectoryFactory, LdapConfig,
};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::support::*;

const ADMINS: &str = "cn=rolter-admins,ou=groups,dc=example,dc=com";
const USERS: &str = "cn=rolter-users,ou=groups,dc=example,dc=com";

/// What the stub directory knows: login -> (password, entry).
#[derive(Default)]
struct Entries {
    users: BTreeMap<String, (String, DirectoryEntry)>,
    down: bool,
    /// the bind password of the last config a sign-in opened the directory with
    last_bind_password: Option<String>,
}

#[derive(Clone, Default)]
struct StubDirectory(Arc<Mutex<Entries>>);

impl StubDirectory {
    fn add(&self, login: &str, password: &str, entry: DirectoryEntry) {
        self.0
            .lock()
            .users
            .insert(login.to_string(), (password.to_string(), entry));
    }

    fn set_groups(&self, login: &str, groups: &[&str]) {
        let mut entries = self.0.lock();
        let (_, entry) = entries.users.get_mut(login).expect("a known login");
        entry.groups = groups.iter().map(|g| g.to_string()).collect();
    }
}

struct Open(StubDirectory);

#[async_trait]
impl Directory for Open {
    async fn bind_user(
        &self,
        login: &str,
        password: &str,
    ) -> Result<DirectoryEntry, DirectoryError> {
        let entries = self.0 .0.lock();
        if entries.down {
            return Err(DirectoryError::Unavailable("connection refused".into()));
        }
        match entries.users.get(login) {
            Some((expected, entry)) if expected == password && !password.is_empty() => {
                Ok(entry.clone())
            }
            _ => Err(DirectoryError::NotAuthenticated),
        }
    }
}

impl DirectoryFactory for StubDirectory {
    fn open(&self, config: LdapConfig) -> Option<Box<dyn Directory>> {
        self.0.lock().last_bind_password = Some(config.bind_password);
        Some(Box::new(Open(self.clone())))
    }

    fn available(&self) -> bool {
        true
    }
}

fn entry(dn: &str, mail: &str, name: Option<&str>, groups: &[&str]) -> DirectoryEntry {
    DirectoryEntry {
        dn: dn.to_string(),
        email: Some(mail.to_string()),
        display_name: name.map(str::to_string),
        groups: groups.iter().map(|g| g.to_string()).collect(),
    }
}

struct Fixture {
    base: String,
    client: reqwest::Client,
    pool: sqlx::PgPool,
    org: Uuid,
    directory: Uuid,
    stub: StubDirectory,
    _db: rolter_store::postgres::test_schema::TestSchema,
}

impl Fixture {
    async fn post(&self, path: &str, body: Value) -> reqwest::Response {
        self.client
            .post(format!("{}{path}", self.base))
            .bearer_auth(admin_token())
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    async fn login(&self, login: &str, password: &str) -> reqwest::Response {
        self.client
            .post(format!("{}/api/v1/auth/login", self.base))
            .json(&json!({"email": login, "password": password}))
            .send()
            .await
            .unwrap()
    }

    async fn map(&self, group: &str, role: &str) {
        let response = self
            .post(
                &format!("/api/v1/ldap-directories/{}/group-mappings", self.directory),
                json!({"group_name": group, "role": role}),
            )
            .await;
        assert_eq!(response.status(), 200, "mapping {group}");
    }

    async fn roles(&self, email: &str) -> Vec<(String, String)> {
        sqlx::query_as(
            "select m.role, m.source from memberships m join users u on u.id = m.user_id
             where u.email = $1 order by m.role, m.source",
        )
        .bind(email)
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }
}

fn directory_body(extra: Value) -> Value {
    let mut body = json!({
        "name": "Corporate directory",
        "url": "ldaps://dir.example.com",
        "bind_dn": "cn=svc,dc=example,dc=com",
        "bind_password": "svc-secret-value",
        "base_dn": "ou=people,dc=example,dc=com",
        "user_filter": "(|(uid={login})(mail={login}))",
    });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        body[key] = value;
    }
    body
}

/// An org with a registered directory and a stub that knows `ada`.
async fn fixture() -> Fixture {
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let stub = StubDirectory::default();
    let app = rolter_control::test_app_with_directory(
        pool.clone(),
        Some(admin_token().to_string()),
        Arc::new(stub.clone()),
    )
    .await
    .unwrap();
    let addr = serve(app).await;
    let base = format!("http://{addr}");
    let org: Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    stub.add(
        "ada",
        "ada-password",
        entry(
            "uid=ada,ou=people,dc=example,dc=com",
            "ada@example.com",
            Some("Ada Lovelace"),
            &[USERS],
        ),
    );
    let mut fixture = Fixture {
        base,
        client: reqwest::Client::new(),
        pool,
        org,
        directory: Uuid::nil(),
        stub,
        _db: db,
    };
    let created = fixture
        .post(
            &format!("/api/v1/orgs/{org}/ldap-directories"),
            directory_body(json!({})),
        )
        .await;
    assert_eq!(created.status(), 200);
    let body: Value = created.json().await.unwrap();
    fixture.directory = body["id"].as_str().unwrap().parse().unwrap();
    fixture
}

#[tokio::test]
async fn a_directory_user_signs_in_and_gets_the_grants_their_groups_map_to() {
    skip_without_db!();
    let f = fixture().await;
    f.map(USERS, "member").await;

    let response = f.login("ada", "ada-password").await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert!(body["token"].as_str().unwrap().starts_with("rolter_sess_"));
    assert_eq!(body["user"]["email"], "ada@example.com");

    assert_eq!(
        f.roles("ada@example.com").await,
        vec![("member".to_string(), "ldap".to_string())]
    );
    // the stored bind password is what the directory was opened with, opened
    // from its sealed form
    assert_eq!(
        f.stub.0.lock().last_bind_password.as_deref(),
        Some("svc-secret-value")
    );

    // the session is a real one
    let me = f
        .client
        .get(format!("{}/api/v1/auth/me", f.base))
        .bearer_auth(body["token"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200);

    let audited: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'auth.ldap_sign_in'")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(audited, 1);
}

#[tokio::test]
async fn the_first_sign_in_defaults_a_missing_display_name_and_never_replaces_one() {
    skip_without_db!();
    let f = fixture().await;
    f.map(USERS, "member").await;

    assert_eq!(f.login("ada", "ada-password").await.status(), 200);
    let name: Option<String> =
        sqlx::query_scalar("select display_name from users where email = 'ada@example.com'")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(name.as_deref(), Some("Ada Lovelace"));

    // the user renames themselves; a later sign-in does not revert it
    sqlx::query("update users set display_name = 'Countess' where email = 'ada@example.com'")
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(f.login("ada", "ada-password").await.status(), 200);
    let name: Option<String> =
        sqlx::query_scalar("select display_name from users where email = 'ada@example.com'")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(name.as_deref(), Some("Countess"));
}

#[tokio::test]
async fn a_wrong_password_and_an_unknown_login_are_the_same_refusal() {
    skip_without_db!();
    let f = fixture().await;
    f.map(USERS, "member").await;

    let wrong = f.login("ada", "not-her-password").await;
    let unknown = f.login("nobody", "whatever").await;
    assert_eq!(wrong.status(), 401);
    assert_eq!(unknown.status(), 401);
    let wrong: Value = wrong.json().await.unwrap();
    let unknown: Value = unknown.json().await.unwrap();
    assert_eq!(wrong, unknown);
    assert_eq!(wrong["error"]["code"], "invalid_credentials");
    assert!(f.roles("ada@example.com").await.is_empty());
}

#[tokio::test]
async fn a_user_in_no_mapped_group_is_refused_unless_the_directory_has_a_default_role() {
    skip_without_db!();
    let f = fixture().await;
    f.map(ADMINS, "admin").await;

    // authenticated, but ada is only in the unmapped users group
    let refused = f.login("ada", "ada-password").await;
    assert_eq!(refused.status(), 401);
    let created: i64 =
        sqlx::query_scalar("select count(*) from users where email = 'ada@example.com'")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(created, 0, "a refused sign-in creates no account");

    let updated = f
        .client
        .put(format!(
            "{}/api/v1/ldap-directories/{}",
            f.base, f.directory
        ))
        .bearer_auth(admin_token())
        .json(&directory_body(json!({"default_role": "viewer"})))
        .send()
        .await
        .unwrap();
    assert_eq!(updated.status(), 200);
    assert_eq!(f.login("ada", "ada-password").await.status(), 200);
    assert_eq!(
        f.roles("ada@example.com").await,
        vec![("viewer".to_string(), "ldap".to_string())]
    );
}

#[tokio::test]
async fn dropping_a_group_revokes_what_it_granted_and_leaves_manual_grants_alone() {
    skip_without_db!();
    let f = fixture().await;
    f.map(USERS, "member").await;
    f.map(ADMINS, "admin").await;
    f.stub.set_groups("ada", &[USERS, ADMINS]);

    assert_eq!(f.login("ada", "ada-password").await.status(), 200);
    assert_eq!(
        f.roles("ada@example.com").await,
        vec![
            ("admin".to_string(), "ldap".to_string()),
            ("member".to_string(), "ldap".to_string()),
        ]
    );
    // an administrator also granted ada a role by hand
    let ada: Uuid = sqlx::query_scalar("select id from users where email = 'ada@example.com'")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    seed_membership(&f.pool, ada, Some(f.org), None, None, "viewer").await;
    // a second admin keeps the org from losing its last admin grant
    let other = seed_user(&f.pool, "other@example.com", false).await;
    seed_membership(&f.pool, other, Some(f.org), None, None, "admin").await;

    f.stub.set_groups("ada", &[USERS]);
    assert_eq!(f.login("ada", "ada-password").await.status(), 200);
    assert_eq!(
        f.roles("ada@example.com").await,
        vec![
            ("member".to_string(), "ldap".to_string()),
            ("viewer".to_string(), "manual".to_string()),
        ]
    );
}

#[tokio::test]
async fn group_names_match_regardless_of_case() {
    skip_without_db!();
    let f = fixture().await;
    f.map(&USERS.to_uppercase(), "member").await;
    assert_eq!(f.login("ada", "ada-password").await.status(), 200);
    assert_eq!(f.roles("ada@example.com").await.len(), 1);
}

#[tokio::test]
async fn a_superadmins_address_is_never_adopted_by_a_directory_entry() {
    skip_without_db!();
    let f = fixture().await;
    f.map(USERS, "member").await;
    seed_user(&f.pool, "ada@example.com", true).await;

    assert_eq!(f.login("ada", "ada-password").await.status(), 401);
    assert!(f.roles("ada@example.com").await.is_empty());
}

#[tokio::test]
async fn an_account_holding_a_deployment_role_is_never_adopted() {
    skip_without_db!();
    let f = fixture().await;
    f.map(USERS, "member").await;
    let ops = seed_user(&f.pool, "ada@example.com", false).await;
    sqlx::query("update users set deployment_role = 'operator' where id = $1")
        .bind(ops)
        .execute(&f.pool)
        .await
        .unwrap();

    assert_eq!(f.login("ada", "ada-password").await.status(), 401);
    assert!(f.roles("ada@example.com").await.is_empty());
    let sessions: i64 = sqlx::query_scalar("select count(*) from sessions where user_id = $1")
        .bind(ops)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(sessions, 0, "no session is issued");
}

#[tokio::test]
async fn an_account_with_a_role_in_another_org_is_never_adopted() {
    skip_without_db!();
    let f = fixture().await;
    f.map(USERS, "member").await;
    // ada already belongs to another org; acme's directory runs its own entry
    // for her address, with a password acme's admins chose
    let user = seed_user(&f.pool, "ada@example.com", false).await;
    let other: Uuid = sqlx::query_scalar(
        "insert into orgs (name, slug) values ('globex', 'globex') returning id",
    )
    .fetch_one(&f.pool)
    .await
    .unwrap();
    sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'admin')")
        .bind(user)
        .bind(other)
        .execute(&f.pool)
        .await
        .unwrap();

    assert_eq!(f.login("ada", "ada-password").await.status(), 401);
    assert_eq!(
        f.roles("ada@example.com").await,
        vec![("admin".to_string(), "manual".to_string())],
        "the refused sign-in granted nothing in acme"
    );

    // an account whose roles are all inside acme is still the directory's
    let local = seed_user(&f.pool, "grace@example.com", false).await;
    sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'viewer')")
        .bind(local)
        .bind(f.org)
        .execute(&f.pool)
        .await
        .unwrap();
    f.stub.add(
        "grace",
        "grace-password",
        entry(
            "uid=grace,ou=people,dc=example,dc=com",
            "grace@example.com",
            None,
            &[USERS],
        ),
    );
    assert_eq!(f.login("grace", "grace-password").await.status(), 200);
}

/// A directory sign-in is a password typed into rolter's form, so an org that
/// turns password login off to send its members through its identity provider
/// is not reachable through its directory either, and turning it back on
/// restores the directory.
#[tokio::test]
async fn a_directory_does_not_sign_in_while_its_org_has_passwords_off() {
    skip_without_db!();
    let f = fixture().await;
    f.map(USERS, "member").await;
    assert_eq!(f.login("ada", "ada-password").await.status(), 200);

    // written straight to the table: the policy endpoint refuses passwords off
    // until the org has an identity provider, which this fixture does not set up
    let set = |allow: bool| {
        sqlx::query(
            "insert into org_auth_policies (org_id, allow_password_login, allow_sso)
             values ($1, $2, true)
             on conflict (org_id) do update set allow_password_login = excluded.allow_password_login",
        )
        .bind(f.org)
        .bind(allow)
        .execute(&f.pool)
    };
    set(false).await.unwrap();
    assert_eq!(f.login("ada", "ada-password").await.status(), 401);

    set(true).await.unwrap();
    assert_eq!(f.login("ada", "ada-password").await.status(), 200);
}

#[tokio::test]
async fn a_deactivated_account_stays_out() {
    skip_without_db!();
    let f = fixture().await;
    f.map(USERS, "member").await;
    assert_eq!(f.login("ada", "ada-password").await.status(), 200);
    sqlx::query("update users set deactivated_at = now() where email = 'ada@example.com'")
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(f.login("ada", "ada-password").await.status(), 401);
}

#[tokio::test]
async fn an_entry_without_a_usable_email_is_refused() {
    skip_without_db!();
    let f = fixture().await;
    f.map(USERS, "member").await;
    let mut no_mail = entry("uid=bob,ou=people,dc=example,dc=com", "x", None, &[USERS]);
    no_mail.email = None;
    f.stub.add("bob", "bob-password", no_mail);

    // the login `bob` is not an address, and the directory supplied none
    assert_eq!(f.login("bob", "bob-password").await.status(), 401);
}

#[tokio::test]
async fn a_directory_that_is_down_answers_like_a_wrong_password_and_local_login_still_works() {
    skip_without_db!();
    let f = fixture().await;
    f.map(USERS, "member").await;
    let password = random_password();
    seed_local_user(&f.pool, "root@example.com", &password).await;
    f.stub.0.lock().down = true;

    let refused = f.login("ada", "ada-password").await;
    assert_eq!(refused.status(), 401);
    let body: Value = refused.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_credentials");
    // the local break-glass account is unaffected by the directory
    assert_eq!(f.login("root@example.com", &password).await.status(), 200);
}

#[tokio::test]
async fn a_disabled_directory_takes_no_part_in_sign_in_and_is_not_advertised() {
    skip_without_db!();
    let f = fixture().await;
    f.map(USERS, "member").await;

    let methods: Value = f
        .client
        .get(format!("{}/api/v1/auth/methods", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(methods["ldap"], true);
    assert_eq!(
        methods["password"], true,
        "the form is still a password form"
    );

    let disabled = f
        .client
        .put(format!(
            "{}/api/v1/ldap-directories/{}",
            f.base, f.directory
        ))
        .bearer_auth(admin_token())
        .json(&directory_body(json!({"enabled": false})))
        .send()
        .await
        .unwrap();
    assert_eq!(disabled.status(), 200);

    let methods: Value = f
        .client
        .get(format!("{}/api/v1/auth/methods", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(methods["ldap"], false);
    assert_eq!(f.login("ada", "ada-password").await.status(), 401);
}

#[tokio::test]
async fn a_second_factor_policy_still_applies_to_a_directory_user() {
    skip_without_db!();
    let f = fixture().await;
    f.map(USERS, "member").await;
    sqlx::query(
        "insert into org_auth_policies (org_id, mfa_policy) values ($1, 'required_all')
         on conflict (org_id) do update set mfa_policy = 'required_all'",
    )
    .bind(f.org)
    .execute(&f.pool)
    .await
    .unwrap();

    let body: Value = f.login("ada", "ada-password").await.json().await.unwrap();
    // no session: the account has no factor and the org requires one, so what
    // comes back is the enrolment challenge, exactly as for a local account
    assert!(body.get("token").is_none(), "{body}");
}

// ---- the settings API ------------------------------------------------------

#[tokio::test]
async fn the_bind_password_is_sealed_and_never_returned() {
    skip_without_db!();
    let f = fixture().await;

    let listed: Value = f
        .client
        .get(format!("{}/api/v1/orgs/{}/ldap-directories", f.base, f.org))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["has_bind_password"], true);
    assert!(!listed.to_string().contains("svc-secret-value"));

    let (ciphertext, plaintext_hit): (Vec<u8>, bool) = sqlx::query_as(
        "select bind_secret_ciphertext, position('svc-secret-value'::bytea in bind_secret_ciphertext) > 0
         from ldap_directories where id = $1",
    )
    .bind(f.directory)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert!(!ciphertext.is_empty());
    assert!(!plaintext_hit, "the stored bind password is sealed");

    // absent keeps it, an empty string clears it
    let kept = f
        .client
        .put(format!(
            "{}/api/v1/ldap-directories/{}",
            f.base, f.directory
        ))
        .bearer_auth(admin_token())
        .json(&{
            let mut body = directory_body(json!({}));
            body.as_object_mut().unwrap().remove("bind_password");
            body
        })
        .send()
        .await
        .unwrap();
    assert_eq!(kept.status(), 200);
    assert_eq!(
        kept.json::<Value>().await.unwrap()["has_bind_password"],
        true
    );
    let cleared = f
        .client
        .put(format!(
            "{}/api/v1/ldap-directories/{}",
            f.base, f.directory
        ))
        .bearer_auth(admin_token())
        .json(&directory_body(json!({"bind_password": ""})))
        .send()
        .await
        .unwrap();
    assert_eq!(
        cleared.json::<Value>().await.unwrap()["has_bind_password"],
        false
    );

    let audit: Vec<Value> = sqlx::query_scalar(
        "select detail from audit_log where action like 'ldap_directory.%' order by at",
    )
    .fetch_all(&f.pool)
    .await
    .unwrap();
    assert!(!audit
        .iter()
        .any(|d| d.to_string().contains("svc-secret-value")));
}

#[tokio::test]
async fn an_org_has_one_directory_and_a_bad_setting_is_refused_at_write_time() {
    skip_without_db!();
    let f = fixture().await;
    let path = format!("/api/v1/orgs/{}/ldap-directories", f.org);

    let again = f.post(&path, directory_body(json!({}))).await;
    assert_eq!(again.status(), 409);

    let other_org: Uuid = sqlx::query_scalar(
        "insert into orgs (name, slug) values ('globex', 'globex') returning id",
    )
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let other = format!("/api/v1/orgs/{other_org}/ldap-directories");
    for (what, patch) in [
        (
            "plain ldap with tls required",
            json!({"url": "ldap://dir.example.com"}),
        ),
        (
            "a filter without {login}",
            json!({"user_filter": "(objectClass=person)"}),
        ),
        (
            "an unknown scheme",
            json!({"url": "https://dir.example.com"}),
        ),
        (
            "a metadata address",
            json!({"url": "ldaps://169.254.169.254"}),
        ),
        ("an unknown role", json!({"default_role": "owner"})),
        ("an empty base", json!({"base_dn": "  "})),
    ] {
        let response = f.post(&other, directory_body(patch)).await;
        assert_eq!(response.status(), 400, "{what}");
    }
    // plain ldap is allowed once the operator opts out of the requirement
    let opted_out = f
        .post(
            &other,
            directory_body(json!({"url": "ldap://dir.example.com", "require_tls": false})),
        )
        .await;
    assert_eq!(opted_out.status(), 200);
}

#[tokio::test]
async fn a_mapping_cannot_grant_outside_the_directory_org() {
    skip_without_db!();
    let f = fixture().await;
    let other_org: Uuid = sqlx::query_scalar(
        "insert into orgs (name, slug) values ('globex', 'globex') returning id",
    )
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let refused = f
        .post(
            &format!("/api/v1/ldap-directories/{}/group-mappings", f.directory),
            json!({"group_name": USERS, "role": "member", "org_id": other_org}),
        )
        .await;
    assert_eq!(refused.status(), 400);

    let deleted = f
        .client
        .delete(format!(
            "{}/api/v1/ldap-directories/{}",
            f.base, f.directory
        ))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204);
}

#[tokio::test]
async fn only_an_org_admin_can_manage_a_directory() {
    skip_without_db!();
    let f = fixture().await;
    let member = seed_user(&f.pool, "member@example.com", false).await;
    seed_membership(&f.pool, member, Some(f.org), None, None, "member").await;
    let token = seed_session(&f.pool, member, "ldap-member").await;

    let read = f
        .client
        .get(format!("{}/api/v1/orgs/{}/ldap-directories", f.base, f.org))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(read.status(), 403);
    let write = f
        .client
        .delete(format!(
            "{}/api/v1/ldap-directories/{}",
            f.base, f.directory
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(write.status(), 403);
}
