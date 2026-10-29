//! OIDC single sign-on providers, group mapping and the browser sign-in.

use crate::support::*;
use serde_json::{json, Value};

/// An account that signs in through single sign-on has no local password to
/// change. The refusal carries a stable code the dashboard translates, the
/// session payload says so up front, and nothing is audited as a change.
#[tokio::test]
async fn an_sso_only_account_is_refused_with_a_code_the_dashboard_can_translate() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let user_id = seed_user(&pool, "sso@example.com", false).await;
    let token = seed_session(&pool, user_id, "sso-only").await;

    let me: Value = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["has_local_password"], false);

    let refused = post_password_change(
        &client,
        &base,
        &token,
        &random_password(),
        &random_password(),
    )
    .await;
    assert_eq!(refused.status(), 409);
    let body: Value = refused.json().await.unwrap();
    assert_eq!(body["error"]["code"], "no_local_password");
    assert!(body["error"]["message"].as_str().is_some());

    let rows: i64 = sqlx::query_scalar(
        "select count(*) from audit_log where action like 'auth.password_change%'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rows, 0, "a refusal that guessed nothing writes nothing");
}

/// A stub OIDC provider: discovery, JWKS and a token endpoint that signs id
/// tokens with the fixture RSA key. Lets the SSO flow be exercised end to end
/// without a container, while the Keycloak suite covers a real IdP.
mod stub_idp {
    use super::*;
    use axum::Router;
    use std::sync::{Arc, Mutex};

    pub const KID: &str = "stub-key-1";

    /// The stub's signing key, generated once per test process rather than
    /// checked in: a private key in the repository is a private key in the
    /// repository, whatever the comment above it says, and every secret
    /// scanner is right to flag one. ES256 keeps generation instant.
    fn signing_key() -> &'static (jsonwebtoken::EncodingKey, String, String) {
        static KEY: std::sync::OnceLock<(jsonwebtoken::EncodingKey, String, String)> =
            std::sync::OnceLock::new();
        KEY.get_or_init(|| {
            use base64::Engine;
            use p256::elliptic_curve::sec1::ToSec1Point;
            use p256::elliptic_curve::Generate;
            use p256::pkcs8::EncodePrivateKey;

            let secret = p256::SecretKey::generate();
            let pem = secret.to_pkcs8_pem(p256::pkcs8::LineEnding::LF).unwrap();
            let encoding = jsonwebtoken::EncodingKey::from_ec_pem(pem.as_bytes()).unwrap();
            // the jwk carries the affine coordinates, so publish them from the
            // uncompressed sec1 point: 0x04 || x || y
            let point = secret.public_key().to_sec1_point(false);
            let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
            (
                encoding,
                b64.encode(point.x().unwrap()),
                b64.encode(point.y().unwrap()),
            )
        })
    }

    #[derive(Clone, Default)]
    pub struct Stub {
        /// claims the next token exchange will mint, set by the test
        pub next_claims: Arc<Mutex<Value>>,
        /// form fields of the last token request, so PKCE can be asserted
        pub last_form: Arc<Mutex<String>>,
        pub issuer: Arc<Mutex<String>>,
    }

    pub async fn serve_stub() -> (String, Stub) {
        let stub = Stub::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let issuer = format!("http://{addr}");
        *stub.issuer.lock().unwrap() = issuer.clone();

        let app = Router::new()
            .route(
                "/.well-known/openid-configuration",
                axum::routing::get({
                    let stub = stub.clone();
                    move || {
                        let issuer = stub.issuer.lock().unwrap().clone();
                        async move {
                            axum::Json(json!({
                                "issuer": issuer,
                                "authorization_endpoint": format!("{issuer}/authorize"),
                                "token_endpoint": format!("{issuer}/token"),
                                "jwks_uri": format!("{issuer}/jwks"),
                            }))
                        }
                    }
                }),
            )
            .route(
                "/jwks",
                axum::routing::get(|| async {
                    let (_, x, y) = signing_key();
                    axum::Json(json!({
                        "keys": [{
                            "kty": "EC",
                            "use": "sig",
                            "alg": "ES256",
                            "crv": "P-256",
                            "kid": KID,
                            "x": x,
                            "y": y,
                        }]
                    }))
                }),
            )
            .route(
                "/token",
                axum::routing::post({
                    let stub = stub.clone();
                    move |body: String| {
                        let stub = stub.clone();
                        async move {
                            *stub.last_form.lock().unwrap() = body;
                            let claims = stub.next_claims.lock().unwrap().clone();
                            axum::Json(json!({
                                "access_token": "stub-access",
                                "token_type": "Bearer",
                                "id_token": sign(&claims),
                            }))
                        }
                    }
                }),
            );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (issuer, stub)
    }

    /// Sign a claim set as an ES256 id token with the process's stub key.
    pub fn sign(claims: &Value) -> String {
        let (key, _, _) = signing_key();
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
        header.kid = Some(KID.to_string());
        jsonwebtoken::encode(&header, claims, key).unwrap()
    }

    pub fn claims(issuer: &str, audience: &str, nonce: &str, groups: Value) -> Value {
        let now = chrono::Utc::now().timestamp();
        json!({
            "iss": issuer,
            "aud": audience,
            "sub": "idp-subject-1",
            "email": "ada@example.com",
            "preferred_username": "ada",
            "nonce": nonce,
            "groups": groups,
            "iat": now,
            "exp": now + 300,
        })
    }
}

/// #2304: the slug's charset is a check constraint in the database
/// (`sso_providers_slug_charset`), and the handler used to let a bad one reach
/// the insert, so the caller read a store error carrying the constraint's name
/// rather than a 400 that says what to type. The endpoint and the constraint
/// are compared on a table of slugs instead of trusted to agree: for each one
/// the store is asked directly, then the endpoint, and the answers must match.
#[tokio::test]
async fn sso_slug_outside_the_charset_is_a_400_that_states_the_rule() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "SlugOrg", "slug": "slug-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    let longest = "a".repeat(63);
    let too_long = "a".repeat(64);
    let accepted = [
        "okta",
        "a",
        "0",
        "9lives",
        "entra-staging",
        "a--b",
        "okta-",
        longest.as_str(),
    ];
    let refused = [
        "",
        "Okta",
        "acme okta",
        "-okta",
        "okta_prod",
        "okta.prod",
        " okta",
        "okta ",
        "okta\n",
        // a Cyrillic "о", which looks like the Latin one
        "\u{43e}kta",
        "r\u{e9}sum\u{e9}",
        too_long.as_str(),
    ];

    for slug in accepted.iter().chain(refused.iter()).copied() {
        // the store's own answer, with the probe row taken out again so the
        // handler's insert of the same slug below cannot collide with it
        let stored = sqlx::query(
            "insert into sso_providers (org_id, name, slug, issuer, client_id) \
             values ($1::uuid, 'probe', $2, 'https://idp.example.com', 'probe')",
        )
        .bind(&org_id)
        .bind(slug)
        .execute(&pool)
        .await
        .is_ok();
        if stored {
            sqlx::query("delete from sso_providers where org_id = $1::uuid and slug = $2")
                .bind(&org_id)
                .bind(slug)
                .execute(&pool)
                .await
                .unwrap();
        }

        let response = client
            .post(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
            .bearer_auth(admin_token())
            .json(&json!({
                "name": "Probe",
                "slug": slug,
                "issuer": "https://idp.example.com",
                "client_id": "probe"
            }))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body: Value = response.json().await.unwrap();

        assert_eq!(
            status == 200,
            stored,
            "slug {slug:?}: the endpoint answered {status} but the store {} it: {body}",
            if stored { "accepts" } else { "refuses" }
        );
        if stored {
            assert_eq!(body["slug"], slug);
        } else {
            assert_eq!(status, 400, "slug {slug:?} must be a 400: {body}");
            let message = body["error"]["message"].as_str().unwrap_or_default();
            assert!(
                !message.contains("sso_providers_slug_charset"),
                "slug {slug:?}: the constraint name reached the caller: {message}"
            );
            if !slug.trim().is_empty() {
                assert!(
                    message.contains("lowercase letters, digits and hyphens")
                        && message.contains("start with a letter or digit")
                        && message.contains("at most 63 characters"),
                    "slug {slug:?}: the 400 does not state the rule: {message}"
                );
            }
        }
    }

    // only the accepted ones were registered; a refused slug left nothing behind
    let listed: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), accepted.len());
}

/// #1233: a provider is editable in place. Before this, rotating a client
/// secret or taking a provider out of service meant deleting it and
/// registering it again, which dropped every group mapping hanging off it and
/// changed the provider id in the audit trail.
#[tokio::test]
async fn sso_provider_updates_in_place_and_keeps_its_slug_and_mappings() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "UpdOrg", "slug": "upd-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    let provider: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Okta",
            "slug": "okta",
            "issuer": "https://acme.okta.com",
            "client_id": "0oa1",
            "client_secret": "original-secret",
            "group_claim": "groups"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = provider["id"].as_str().unwrap().to_string();
    assert_eq!(provider["has_client_secret"], json!(true));

    // a mapping hangs off the provider. it is the thing delete-and-recreate
    // used to destroy, so every assertion below re-checks it survived
    let mapping: Value = client
        .post(format!("{base}/api/v1/sso-providers/{id}/group-mappings"))
        .bearer_auth(admin_token())
        .json(&json!({"group_name": "platform", "role": "admin"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mapping_id = mapping["id"].as_str().unwrap().to_string();

    // omitting client_secret leaves the sealed one alone while everything
    // else moves, and the slug is untouched because it is in the login url
    let updated: Value = client
        .put(format!("{base}/api/v1/sso-providers/{id}"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Okta (prod)",
            "issuer": "https://acme.okta.com/",
            "client_id": "0oa2",
            "scopes": ["openid", "email"],
            "group_claim": "roles",
            "default_role": "member",
            "enabled": false
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["id"].as_str().unwrap(), id);
    assert_eq!(updated["name"], json!("Okta (prod)"));
    assert_eq!(updated["slug"], json!("okta"), "the slug is immutable");
    // a trailing slash is trimmed the same way create does it, so the issuer
    // still matches the one in the id token
    assert_eq!(updated["issuer"], json!("https://acme.okta.com"));
    assert_eq!(updated["client_id"], json!("0oa2"));
    assert_eq!(updated["group_claim"], json!("roles"));
    assert_eq!(updated["default_role"], json!("member"));
    assert_eq!(
        updated["enabled"],
        json!(false),
        "a provider can be disabled"
    );
    assert_eq!(
        updated["has_client_secret"],
        json!(true),
        "an absent client_secret leaves the stored one alone"
    );

    // a disabled provider is not offered on the login screen
    let methods: Value = client
        .get(format!("{base}/api/v1/auth/methods"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !methods.to_string().contains("okta"),
        "a disabled provider must not be advertised: {methods}"
    );

    // rotating: a new secret replaces the sealed one and is never echoed back
    let rotated: Value = client
        .put(format!("{base}/api/v1/sso-providers/{id}"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Okta (prod)",
            "issuer": "https://acme.okta.com",
            "client_id": "0oa2",
            "client_secret": "rotated-secret",
            "enabled": true
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(rotated["has_client_secret"], json!(true));
    assert!(
        !rotated.to_string().contains("rotated-secret"),
        "the client secret leaked into the api response: {rotated}"
    );
    // omitting scopes keeps the ones already stored rather than resetting
    // them to the create-time defaults
    assert_eq!(rotated["scopes"], json!(["openid", "email"]));

    // an empty string clears it: the provider becomes a public pkce client
    let cleared: Value = client
        .put(format!("{base}/api/v1/sso-providers/{id}"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Okta (prod)",
            "issuer": "https://acme.okta.com",
            "client_id": "0oa2",
            "client_secret": "",
            "enabled": true
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cleared["has_client_secret"], json!(false));

    // the mapping is still there: this is the whole point of editing in place
    let mappings: Value = client
        .get(format!("{base}/api/v1/sso-providers/{id}/group-mappings"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(mappings.as_array().unwrap().len(), 1);
    assert_eq!(mappings[0]["id"].as_str().unwrap(), mapping_id);

    // a bad issuer is refused before anything is written
    let bad = client
        .put(format!("{base}/api/v1/sso-providers/{id}"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "Okta", "issuer": "not-a-url", "client_id": "0oa2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);

    // and every edit is audited, naming what happened to the secret without
    // ever recording the secret
    let actions: Vec<String> = sqlx::query_scalar(
        "select detail->>'client_secret' from audit_log \
         where action = 'sso_provider.update' order by at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(actions, vec!["unchanged", "rotated", "cleared"]);
}

/// OIDC SSO (#240) end to end against a stub identity provider: the login
/// redirect carries PKCE, the callback verifies the id token, groups become
/// memberships, and every rejection path fails closed.
#[tokio::test]
async fn sso_login_maps_groups_to_memberships_and_fails_closed() {
    skip_without_db!();
    let client_secret = random_password();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    // the redirect uri is deployment-owned, so the control plane must know its
    // own public url for the flow to be coherent
    let addr = serve_with_public_url(pool.clone(), Some(admin_token().to_string())).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("http://{addr}");

    let (issuer, stub) = stub_idp::serve_stub().await;

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "SsoOrg", "slug": "sso-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let team: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/teams"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "Platform"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let team_id = team["id"].as_str().unwrap().to_string();

    // a non-http issuer is refused before anything is stored
    let bad = client
        .post(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Bad", "slug": "bad", "issuer": "not-a-url", "client_id": "x"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);

    let provider: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Stub IdP",
            "slug": "stub",
            "issuer": issuer,
            "client_id": "rolter",
            "client_secret": client_secret,
            "group_claim": "groups"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let provider_id = provider["id"].as_str().unwrap().to_string();
    // the client secret is sealed and never echoed back
    let provider_text = provider.to_string();
    assert!(
        !provider_text.contains(&client_secret) && !provider_text.contains("secret_ciphertext"),
        "client secret leaked into the api response: {provider_text}"
    );
    // the row names the two addresses an operator needs, built from the
    // deployment's public url rather than left for the dashboard to guess
    // from its own origin (#2083)
    assert_eq!(
        provider["redirect_uri"],
        format!("{base}/auth/sso/stub/callback")
    );
    assert_eq!(provider["login_url"], format!("{base}/auth/sso/stub/start"));
    let listed: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed[0]["redirect_uri"], provider["redirect_uri"]);
    assert_eq!(listed[0]["login_url"], provider["login_url"]);

    // map an IdP group to a team-scoped admin role
    let mapping = client
        .post(format!(
            "{base}/api/v1/sso-providers/{provider_id}/group-mappings"
        ))
        .bearer_auth(admin_token())
        .json(&json!({"group_name": "platform", "role": "admin", "team_id": team_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(mapping.status(), 200);

    // starting a login redirects to the provider with PKCE + state + nonce
    let start = client
        .get(format!("{base}/auth/sso/stub/start"))
        .send()
        .await
        .unwrap();
    assert_eq!(start.status(), 303);
    let location = start
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(location.starts_with(&format!("{issuer}/authorize?response_type=code")));
    assert!(location.contains("code_challenge_method=S256"));
    // and the redirect uri it carries is the one the provider row advertised,
    // so what an operator registered in the IdP is what the IdP is sent
    let advertised = provider["redirect_uri"].as_str().unwrap();
    assert_eq!(
        url_param(&location, "redirect_uri"),
        advertised.replace(':', "%3A").replace('/', "%2F")
    );
    let state = url_param(&location, "state");

    // an id token minted for a different login (wrong nonce) is refused
    *stub.next_claims.lock().unwrap() =
        stub_idp::claims(&issuer, "rolter", "other-nonce", json!(["/platform"]));
    let replayed = client
        .get(format!(
            "{base}/auth/sso/stub/callback?code=abc&state={state}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(replayed.status(), 400, "a mismatched nonce must be refused");

    // ...and that consumed the state, so the real callback needs a fresh login
    let start = client
        .get(format!("{base}/auth/sso/stub/start"))
        .send()
        .await
        .unwrap();
    let location = start
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let state = url_param(&location, "state");
    let nonce = url_param(&location, "nonce");

    *stub.next_claims.lock().unwrap() =
        stub_idp::claims(&issuer, "rolter", &nonce, json!(["/platform"]));
    let logged_in: Value = client
        .get(format!(
            "{base}/auth/sso/stub/callback?code=abc&state={state}"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(logged_in["user"]["email"], "ada@example.com");
    assert_eq!(logged_in["granted_roles"][0], "admin");
    let session_token = logged_in["token"].as_str().unwrap().to_string();

    // the token exchange used the authorization code with a PKCE verifier
    let form = stub.last_form.lock().unwrap().clone();
    assert!(form.contains("grant_type=authorization_code"));
    assert!(form.contains("code_verifier="));

    // the session works, and the mapped membership is real: the team-scoped
    // admin can create a project inside that team
    let created = client
        .post(format!("{base}/api/v1/teams/{team_id}/projects"))
        .bearer_auth(&session_token)
        .json(&json!({"name": "FromSso"}))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 200);
    // but holds nothing at the org level, which no mapping granted
    let denied = client
        .post(format!("{base}/api/v1/orgs/{org_id}/teams"))
        .bearer_auth(&session_token)
        .json(&json!({"name": "Nope"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);

    // a repeat login does not accumulate duplicate memberships
    let before: i64 = sqlx::query_scalar("select count(*) from memberships")
        .fetch_one(&pool)
        .await
        .unwrap();
    let start = client
        .get(format!("{base}/auth/sso/stub/start"))
        .send()
        .await
        .unwrap();
    let location = start
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let state = url_param(&location, "state");
    let nonce = url_param(&location, "nonce");
    *stub.next_claims.lock().unwrap() =
        stub_idp::claims(&issuer, "rolter", &nonce, json!(["/platform"]));
    let again = client
        .get(format!(
            "{base}/auth/sso/stub/callback?code=abc&state={state}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 200);
    let after: i64 = sqlx::query_scalar("select count(*) from memberships")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before, after, "a repeat sso login must be idempotent");

    // a user in no mapped group is refused, because the provider set no
    // default_role: SSO authenticates, it does not implicitly authorize
    let start = client
        .get(format!("{base}/auth/sso/stub/start"))
        .send()
        .await
        .unwrap();
    let location = start
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let state = url_param(&location, "state");
    let nonce = url_param(&location, "nonce");
    *stub.next_claims.lock().unwrap() =
        stub_idp::claims(&issuer, "rolter", &nonce, json!(["/unmapped"]));
    let ungrouped = client
        .get(format!(
            "{base}/auth/sso/stub/callback?code=abc&state={state}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(ungrouped.status(), 403);

    // an id token signed for another audience never becomes a session
    let start = client
        .get(format!("{base}/auth/sso/stub/start"))
        .send()
        .await
        .unwrap();
    let location = start
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let state = url_param(&location, "state");
    let nonce = url_param(&location, "nonce");
    *stub.next_claims.lock().unwrap() =
        stub_idp::claims(&issuer, "some-other-client", &nonce, json!(["/platform"]));
    let wrong_audience = client
        .get(format!(
            "{base}/auth/sso/stub/callback?code=abc&state={state}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_audience.status(), 400);

    // an unknown state (never issued, or already consumed) is refused
    let unknown = client
        .get(format!(
            "{base}/auth/sso/stub/callback?code=abc&state=made-up"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 400);

    let action: Option<String> = sqlx::query_scalar(
        "select action from audit_log where action = 'auth.sso_login' order by at desc limit 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(action.as_deref(), Some("auth.sso_login"));
}

/// Invitations and single sign-on co-exist (#240): an operator-granted role is
/// never reconciled away by a later SSO login, an IdP group that disappears
/// does revoke the role it granted, and an org can require SSO without locking
/// out the break-glass superadmin.
#[tokio::test]
async fn the_last_enabled_sso_provider_cannot_go_while_passwords_are_off() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "LastIdp", "slug": "last-idp"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    let mut ids = Vec::new();
    for slug in ["idp-a", "idp-b"] {
        let p: Value = client
            .post(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
            .bearer_auth(admin_token())
            .json(&json!({
                "name": slug,
                "slug": slug,
                "issuer": "https://idp.example.com",
                "client_id": "client"
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        ids.push(p["id"].as_str().unwrap().to_string());
    }
    let disable = |id: String| {
        let client = client.clone();
        let base = base.clone();
        async move {
            client
                .put(format!("{base}/api/v1/sso-providers/{id}"))
                .bearer_auth(admin_token())
                .json(&json!({
                    "name": "idp",
                    "issuer": "https://idp.example.com",
                    "client_id": "client",
                    "enabled": false
                }))
                .send()
                .await
                .unwrap()
        }
    };
    let set_passwords = |allow: bool| {
        let client = client.clone();
        let base = base.clone();
        let org_id = org_id.clone();
        async move {
            client
                .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
                .bearer_auth(admin_token())
                .json(&json!({"allow_password_login": allow, "allow_sso": true}))
                .send()
                .await
                .unwrap()
        }
    };
    let enabled_count = || async {
        sqlx::query_scalar::<_, i64>("select count(*) from sso_providers where enabled")
            .fetch_one(&pool)
            .await
            .unwrap()
    };

    // passwords on: the last provider is free to go, nobody is locked out
    assert_eq!(disable(ids[0].clone()).await.status(), 200);
    assert_eq!(disable(ids[1].clone()).await.status(), 200);
    assert_eq!(enabled_count().await, 0);
    // re-enable both, then turn passwords off (the reverse guard)
    for id in &ids {
        let r = client
            .put(format!("{base}/api/v1/sso-providers/{id}"))
            .bearer_auth(admin_token())
            .json(&json!({
                "name": "idp",
                "issuer": "https://idp.example.com",
                "client_id": "client",
                "enabled": true
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
    }
    assert_eq!(set_passwords(false).await.status(), 200);

    // two enabled: one may go, the second is refused and stays enabled
    assert_eq!(disable(ids[0].clone()).await.status(), 200);
    let refused = disable(ids[1].clone()).await;
    assert_eq!(refused.status(), 409);
    let body: Value = refused.json().await.unwrap();
    assert!(
        body.to_string().contains("another sso provider first"),
        "the refusal names the fix: {body}"
    );
    assert_eq!(enabled_count().await, 1);

    // deleting the last enabled one is refused too
    let del = client
        .delete(format!("{base}/api/v1/sso-providers/{}", ids[1]))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 409);
    let exists: i64 = sqlx::query_scalar("select count(*) from sso_providers where id = $1::uuid")
        .bind(&ids[1])
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(exists, 1);

    // the disabled provider is not a way to sign in, so it can still be deleted
    let del_disabled = client
        .delete(format!("{base}/api/v1/sso-providers/{}", ids[0]))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(del_disabled.status(), 204);

    // the reverse guard still holds: no enabled provider, no passwords-off
    assert_eq!(set_passwords(true).await.status(), 200);
    let del_last = client
        .delete(format!("{base}/api/v1/sso-providers/{}", ids[1]))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(del_last.status(), 204, "passwords on: the last one may go");
    assert_eq!(set_passwords(false).await.status(), 409);
}

#[tokio::test]
async fn sso_and_password_login_coexist_per_org_policy() {
    skip_without_db!();
    let password = random_password();
    let client_secret = random_password();
    let root_password = random_password();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve_with_public_url(pool.clone(), Some(admin_token().to_string())).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "MixedOrg", "slug": "mixed-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    // with no provider registered, the login screen offers a password form and
    // nothing else: an operator who never wants an IdP never sees one
    let methods: Value = client
        .get(format!("{base}/api/v1/auth/methods"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(methods["password"], true);
    assert_eq!(methods["sso"].as_array().unwrap().len(), 0);

    // an invited local account: created with a password, granted a role by an
    // operator, and able to log in
    let invited: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/users"))
        .bearer_auth(admin_token())
        .json(&json!({"email": "ada@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let user_id = invited["user"]["id"].as_str().unwrap().to_string();
    assert_eq!(invited["membership"]["role"], "member");
    let source: String = sqlx::query_scalar("select source from memberships where user_id = $1")
        .bind(uuid::Uuid::parse_str(&user_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(source, "manual", "an operator grant must be tagged manual");

    let logged_in = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "ada@example.com", "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(logged_in.status(), 200);

    // now the same deployment gains an IdP, and the same person arrives
    // through it: the account is adopted by email, not duplicated
    let (issuer, stub) = stub_idp::serve_stub().await;
    let provider: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Stub IdP", "slug": "mixed", "issuer": issuer,
            "client_id": "rolter", "client_secret": client_secret
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let provider_id = provider["id"].as_str().unwrap().to_string();
    let mapping: Value = client
        .post(format!(
            "{base}/api/v1/sso-providers/{provider_id}/group-mappings"
        ))
        .bearer_auth(admin_token())
        .json(&json!({"group_name": "admins", "role": "admin", "org_id": org_id}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mapping_id = mapping["id"].as_str().unwrap().to_string();

    // the login screen now offers both
    let methods: Value = client
        .get(format!("{base}/api/v1/auth/methods"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(methods["password"], true);
    assert_eq!(methods["sso"][0]["slug"], "mixed");
    assert_eq!(methods["sso"][0]["start_url"], "/auth/sso/mixed/start");

    sso_login(&client, &base, &stub, &issuer, json!(["admins"]))
        .await
        .unwrap();
    let user_uuid = uuid::Uuid::parse_str(&user_id).unwrap();
    let users: i64 = sqlx::query_scalar("select count(*) from users where email = $1")
        .bind("ada@example.com")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(users, 1, "sso must adopt the invited account, not fork it");
    let rows: Vec<(String, String)> =
        sqlx::query_as("select role, source from memberships where user_id = $1 order by source")
            .bind(user_uuid)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        rows,
        vec![
            ("member".to_string(), "manual".to_string()),
            ("admin".to_string(), "sso".to_string()),
        ],
        "the operator grant and the sso grant must co-exist"
    );

    // the IdP drops the group; the next login revokes the role it granted and
    // leaves the operator's grant alone. someone else administers the org, so
    // that admin grant is not its last one (#2558)
    let other_admin = seed_user(&pool, "grace@example.com", false).await;
    seed_membership(
        &pool,
        other_admin,
        Some(org_id.parse().unwrap()),
        None,
        None,
        "admin",
    )
    .await;
    client
        .delete(format!("{base}/api/v1/sso-group-mappings/{mapping_id}"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    let refused = sso_login(&client, &base, &stub, &issuer, json!(["admins"])).await;
    assert_eq!(
        refused, None,
        "with no mapping left and no default_role, the login must fail closed"
    );
    let remaining: Vec<(String, String)> =
        sqlx::query_as("select role, source from memberships where user_id = $1")
            .bind(user_uuid)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        remaining,
        vec![("member".to_string(), "manual".to_string())],
        "losing every mapped group revokes the sso grant, not the operator's"
    );

    // an org cannot disable password login before an IdP can carry the load...
    let other: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "NoIdp", "slug": "no-idp"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_id = other["id"].as_str().unwrap();
    let premature = client
        .put(format!("{base}/api/v1/orgs/{other_id}/auth-policy"))
        .bearer_auth(admin_token())
        .json(&json!({"allow_password_login": false, "allow_sso": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(premature.status(), 409);

    // ...nor turn both methods off
    let neither = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .bearer_auth(admin_token())
        .json(&json!({"allow_password_login": false, "allow_sso": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(neither.status(), 409);

    // enforcing sso for the org that has a provider refuses that member's
    // password, and the login screen stops offering the form
    let enforced = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .bearer_auth(admin_token())
        .json(&json!({"allow_password_login": false, "allow_sso": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(enforced.status(), 200);
    let blocked = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "ada@example.com", "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 403);

    // break-glass: a superadmin who belongs to the same org still gets in with
    // a password, which is the only reason enforcing sso is a safe switch to
    // flip — a mistyped issuer must not be unrecoverable
    let root: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/users"))
        .bearer_auth(admin_token())
        .json(&json!({"email": "root@example.com", "password": root_password, "role": "admin"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let root_id = uuid::Uuid::parse_str(root["user"]["id"].as_str().unwrap()).unwrap();
    sqlx::query("update users set is_superadmin = true where id = $1")
        .bind(root_id)
        .execute(&pool)
        .await
        .unwrap();
    let super_login = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "root@example.com", "password": root_password}))
        .send()
        .await
        .unwrap();
    assert_eq!(super_login.status(), 200);

    // sso can be switched off without deleting the provider
    let off = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .bearer_auth(admin_token())
        .json(&json!({"allow_password_login": true, "allow_sso": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(off.status(), 200);
    let refused = sso_login(&client, &base, &stub, &issuer, json!(["admins"])).await;
    assert_eq!(
        refused, None,
        "sso must be refused while the org disables it"
    );
}

/// Drive one full SSO round trip, returning the session token on success and
/// `None` when the callback refused.
async fn sso_login(
    client: &reqwest::Client,
    base: &str,
    stub: &stub_idp::Stub,
    issuer: &str,
    groups: Value,
) -> Option<String> {
    let start = client
        .get(format!("{base}/auth/sso/mixed/start"))
        .send()
        .await
        .unwrap();
    let location = start
        .headers()
        .get("location")?
        .to_str()
        .unwrap()
        .to_string();
    let state = url_param(&location, "state");
    let nonce = url_param(&location, "nonce");
    *stub.next_claims.lock().unwrap() = stub_idp::claims(issuer, "rolter", &nonce, groups);
    let response = client
        .get(format!(
            "{base}/auth/sso/mixed/callback?code=abc&state={state}"
        ))
        .send()
        .await
        .unwrap();
    if !response.status().is_success() {
        return None;
    }
    let body: Value = response.json().await.unwrap();
    Some(body["token"].as_str().unwrap().to_string())
}

/// #2339: an org that turns single sign-on off stops offering its providers
/// on the login screen, and refuses them at the start of a login as well as at
/// the callback, instead of sending the member to the identity provider first.
/// Another org's provider and password sign-in are untouched, and turning sso
/// back on restores the button and the round trip.
#[tokio::test]
async fn sso_off_hides_the_provider_and_refuses_its_login() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve_with_public_url(pool.clone(), Some(admin_token().to_string())).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("http://{addr}");
    let (issuer, stub) = stub_idp::serve_stub().await;

    let create_org = |name: &'static str, slug: &'static str| {
        let (client, base) = (&client, &base);
        async move {
            let org: Value = client
                .post(format!("{base}/api/v1/orgs"))
                .bearer_auth(admin_token())
                .json(&json!({"name": name, "slug": slug}))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            org["id"].as_str().unwrap().to_string()
        }
    };
    let create_provider = |org_id: String, slug: &'static str| {
        let (client, base, issuer) = (&client, &base, &issuer);
        async move {
            let provider: Value = client
                .post(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
                .bearer_auth(admin_token())
                .json(&json!({
                    "name": format!("IdP {slug}"), "slug": slug, "issuer": issuer,
                    "client_id": "rolter", "client_secret": format!("idp-{}", uuid::Uuid::new_v4()),
                    "default_role": "member"
                }))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(provider["slug"], slug, "{provider}");
        }
    };
    let listed = || {
        let (client, base) = (&client, &base);
        async move {
            let methods: Value = client
                .get(format!("{base}/api/v1/auth/methods"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let slugs: Vec<String> = methods["sso"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["slug"].as_str().unwrap().to_string())
                .collect();
            (methods["password"].as_bool().unwrap(), slugs)
        }
    };
    let set_sso = |org_id: String, allow_sso: bool| {
        let (client, base) = (&client, &base);
        async move {
            let response = client
                .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
                .bearer_auth(admin_token())
                .json(&json!({"allow_password_login": true, "allow_sso": allow_sso}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
        }
    };

    let org_id = create_org("MixedOrg", "mixed-org").await;
    create_provider(org_id.clone(), "mixed").await;
    // a second org with no policy row at all: sso reads as on for it
    let other_id = create_org("OtherOrg", "other-org").await;
    create_provider(other_id, "other").await;
    let member_password = random_password();
    let member = client
        .post(format!("{base}/api/v1/orgs/{org_id}/users"))
        .bearer_auth(admin_token())
        .json(&json!({"email": "local@example.com", "password": member_password}))
        .send()
        .await
        .unwrap();
    assert_eq!(member.status(), 200);

    assert_eq!(
        listed().await,
        (true, vec!["mixed".to_string(), "other".to_string()])
    );
    // a login begun while sso is on, finished after it is turned off
    let in_flight = client
        .get(format!("{base}/auth/sso/mixed/start"))
        .send()
        .await
        .unwrap();
    assert_eq!(in_flight.status(), 303);
    let in_flight_state = url_param(in_flight.headers()["location"].to_str().unwrap(), "state");

    set_sso(org_id.clone(), false).await;

    // the login screen no longer offers the org's provider, and still offers
    // the other org's and the password form
    assert_eq!(listed().await, (true, vec!["other".to_string()]));

    // a stale button is refused before the identity provider: a JSON caller
    // gets the 403 with a stable code and no redirect
    let start = client
        .get(format!("{base}/auth/sso/mixed/start"))
        .send()
        .await
        .unwrap();
    assert_eq!(start.status(), 403);
    assert!(start.headers().get("location").is_none());
    let body: Value = start.json().await.unwrap();
    assert_eq!(body["error"]["code"], "sso_disabled", "{body}");
    assert!(body["error"]["message"].as_str().is_some());

    // and a browser is sent back to the login screen with the same code the
    // callback uses
    let browser_start = client
        .get(format!("{base}/auth/sso/mixed/start"))
        .header("accept", "text/html,application/xhtml+xml;q=0.9,*/*;q=0.8")
        .send()
        .await
        .unwrap();
    assert_eq!(browser_start.status(), 303);
    assert_eq!(
        browser_start.headers()["location"].to_str().unwrap(),
        format!("{base}/login?sso_error=sso_disabled&sso=mixed")
    );
    let states: i64 = sqlx::query_scalar("select count(*) from sso_login_states")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(states, 1, "a refused start must not record a login state");

    // the callback of the login begun before still refuses, with the same body
    *stub.next_claims.lock().unwrap() = stub_idp::claims(
        &issuer,
        "rolter",
        &url_param(in_flight.headers()["location"].to_str().unwrap(), "nonce"),
        json!([]),
    );
    let callback = client
        .get(format!(
            "{base}/auth/sso/mixed/callback?code=abc&state={in_flight_state}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(callback.status(), 403);
    let body: Value = callback.json().await.unwrap();
    assert_eq!(body["error"]["code"], "sso_disabled", "{body}");

    // a slug no enabled provider answers to: a browser goes back to the login
    // screen, a JSON caller keeps the 400
    let unknown_json = client
        .get(format!("{base}/auth/sso/nobody/start"))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown_json.status(), 400);
    assert!(unknown_json.headers().get("location").is_none());
    let unknown_body: Value = unknown_json.json().await.unwrap();
    assert_eq!(unknown_body["error"]["code"], "unknown_provider");
    let unknown_browser = client
        .get(format!("{base}/auth/sso/nobody/start"))
        .header("accept", "text/html,application/xhtml+xml;q=0.9,*/*;q=0.8")
        .send()
        .await
        .unwrap();
    assert_eq!(unknown_browser.status(), 303);
    assert_eq!(
        unknown_browser.headers()["location"].to_str().unwrap(),
        format!("{base}/login?sso_error=unknown_provider")
    );

    // the other org's provider still starts
    let other = client
        .get(format!("{base}/auth/sso/other/start"))
        .send()
        .await
        .unwrap();
    assert_eq!(other.status(), 303);
    assert!(other.headers()["location"]
        .to_str()
        .unwrap()
        .starts_with(&issuer));

    // local sign-in for the same org keeps working
    let local = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "local@example.com", "password": member_password}))
        .send()
        .await
        .unwrap();
    assert_eq!(local.status(), 200);

    // switching sso back on restores the button and the round trip
    set_sso(org_id, true).await;
    assert_eq!(
        listed().await,
        (true, vec!["mixed".to_string(), "other".to_string()])
    );
    assert!(sso_login(&client, &base, &stub, &issuer, json!([]))
        .await
        .is_some());
}

/// #2297: the provider sends the browser to the callback, so the callback must
/// end on the dashboard. A success hands over a one-time code (never the
/// token), redeemed once; a refusal names a stable code and none of the IdP's
/// own words.
#[tokio::test]
async fn browser_sso_sign_in_ends_on_the_dashboard_with_a_one_time_code() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve_with_public_url(pool.clone(), Some(admin_token().to_string())).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("http://{addr}");
    let (issuer, stub) = stub_idp::serve_stub().await;

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "BrowserOrg", "slug": "browser-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let client_secret = format!("idp-{}", uuid::Uuid::new_v4());
    let provider: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Stub IdP", "slug": "browser", "issuer": issuer,
            "client_id": "rolter", "client_secret": client_secret
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let provider_id = provider["id"].as_str().unwrap().to_string();
    let mapping = client
        .post(format!(
            "{base}/api/v1/sso-providers/{provider_id}/group-mappings"
        ))
        .bearer_auth(admin_token())
        .json(&json!({"group_name": "admins", "role": "admin", "org_id": org_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(mapping.status(), 200);

    // one browser navigation through the provider, answered with `groups`
    let navigate = |groups: Value| {
        let (client, base, stub, issuer) = (&client, &base, &stub, &issuer);
        async move {
            let start = client
                .get(format!("{base}/auth/sso/browser/start"))
                .send()
                .await
                .unwrap();
            let location = start
                .headers()
                .get("location")
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            let state = url_param(&location, "state");
            let nonce = url_param(&location, "nonce");
            *stub.next_claims.lock().unwrap() = stub_idp::claims(issuer, "rolter", &nonce, groups);
            client
                .get(format!(
                    "{base}/auth/sso/browser/callback?code=abc&state={state}"
                ))
                .header("accept", "text/html,application/xhtml+xml;q=0.9,*/*;q=0.8")
                .send()
                .await
                .unwrap()
        }
    };
    let location_of = |response: &reqwest::Response| {
        response
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    };
    let exchange = |code: String| {
        let (client, base) = (&client, &base);
        async move {
            client
                .post(format!("{base}/auth/sso/exchange"))
                .json(&json!({"code": code}))
                .send()
                .await
                .unwrap()
        }
    };

    // success: a 303 to the login screen carrying a code that is not the token
    let response = navigate(json!(["admins"])).await;
    assert_eq!(response.status(), 303);
    let location = location_of(&response);
    assert!(
        location.starts_with(&format!("{base}/login?sso_code=")),
        "{location}"
    );
    assert!(!location.contains("rolter_sess_"), "{location}");
    let body = response.text().await.unwrap();
    assert!(
        !body.contains("rolter_sess_") && !body.contains("token"),
        "{body}"
    );
    let code = url_param(&location, "sso_code");
    assert!(!code.is_empty());

    // only a digest is stored
    let stored: Vec<String> = sqlx::query_scalar("select code_hash from sso_exchange_codes")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_ne!(stored[0], code, "the plain code must not rest in the table");

    // redeeming it yields a working session, once
    let redeemed = exchange(code.clone()).await;
    assert_eq!(redeemed.status(), 200);
    let session: Value = redeemed.json().await.unwrap();
    assert_eq!(session["user"]["email"], "ada@example.com");
    assert_eq!(session["granted_roles"][0], "admin");
    let token = session["token"].as_str().unwrap().to_string();
    assert_ne!(token, code);
    let me = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200);

    let replay = exchange(code).await;
    assert_eq!(replay.status(), 400, "a code is single-use");
    let refusal: Value = replay.json().await.unwrap();
    assert_eq!(refusal["error"]["code"], "invalid_exchange_code");
    assert!(!refusal.to_string().contains(&token));
    let unknown = exchange("never-issued".to_string()).await;
    assert_eq!(unknown.status(), 400);

    // one sign-in, one audit line: the redemption does not add a second
    let audited: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'auth.sso_login'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(audited, 1);

    // an expired code is refused (backdated rather than slept on)
    let response = navigate(json!(["admins"])).await;
    let code = url_param(&location_of(&response), "sso_code");
    sqlx::query("update sso_exchange_codes set expires_at = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(exchange(code).await.status(), 400);

    // refusals: a stable code, the provider once known, and no IdP words
    let idp_words = "idp-free-text-should-never-leak";
    let start = client
        .get(format!("{base}/auth/sso/browser/start"))
        .send()
        .await
        .unwrap();
    let state = url_param(&location_of(&start), "state");
    let declined = client
        .get(format!(
            "{base}/auth/sso/browser/callback?error=access_denied&error_description={idp_words}&state={state}"
        ))
        .header("accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(declined.status(), 303);
    assert_eq!(
        location_of(&declined),
        format!("{base}/login?sso_error=idp_error")
    );

    let stale = client
        .get(format!(
            "{base}/auth/sso/browser/callback?code=abc&state=made-up"
        ))
        .header("accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(
        location_of(&stale),
        format!("{base}/login?sso_error=state_expired")
    );

    let ungrouped = navigate(json!(["nobody"])).await;
    assert_eq!(ungrouped.status(), 303);
    assert_eq!(
        location_of(&ungrouped),
        format!("{base}/login?sso_error=no_mapped_group&sso=browser")
    );

    let deactivate = sqlx::query("update users set deactivated_at = now() where email = $1")
        .bind("ada@example.com")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(deactivate.rows_affected(), 1);
    let deactivated = navigate(json!(["admins"])).await;
    assert_eq!(
        location_of(&deactivated),
        format!("{base}/login?sso_error=account_deactivated&sso=browser")
    );
    sqlx::query("update users set deactivated_at = null")
        .execute(&pool)
        .await
        .unwrap();

    // a caller that is not a browser still gets the JSON refusal
    let start = client
        .get(format!("{base}/auth/sso/browser/start"))
        .send()
        .await
        .unwrap();
    let state = url_param(&location_of(&start), "state");
    let json_refusal = client
        .get(format!(
            "{base}/auth/sso/browser/callback?error=access_denied&state={state}"
        ))
        .header("accept", "application/json")
        .send()
        .await
        .unwrap();
    assert_eq!(json_refusal.status(), 400);

    // a login begun before the org turns sso off ends on the same refusal a
    // fresh start now gives at once (#2339)
    let in_flight = client
        .get(format!("{base}/auth/sso/browser/start"))
        .send()
        .await
        .unwrap();
    let state = url_param(&location_of(&in_flight), "state");
    let disabled = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .bearer_auth(admin_token())
        .json(&json!({"allow_password_login": true, "allow_sso": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(disabled.status(), 200);
    let off = client
        .get(format!(
            "{base}/auth/sso/browser/callback?code=abc&state={state}"
        ))
        .header("accept", "text/html,application/xhtml+xml;q=0.9,*/*;q=0.8")
        .send()
        .await
        .unwrap();
    assert_eq!(
        location_of(&off),
        format!("{base}/login?sso_error=sso_disabled&sso=browser")
    );
}

/// #2558: an SSO login whose groups no longer imply the org's last admin grant
/// keeps that grant and audits it rather than failing the sign-in; once the org
/// has another admin, the next login revokes it.
#[tokio::test]
async fn sso_group_sync_keeps_an_orgs_last_admin_grant() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve_with_public_url(pool.clone(), Some(admin_token().to_string())).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "SyncOrg", "slug": "sync-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let org_uuid: uuid::Uuid = org_id.parse().unwrap();
    let (issuer, stub) = stub_idp::serve_stub().await;
    let provider: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Stub IdP", "slug": "mixed", "issuer": issuer, "client_id": "rolter"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let provider_id = provider["id"].as_str().unwrap().to_string();
    for (group, role) in [("admins", "admin"), ("devs", "member")] {
        let res = client
            .post(format!(
                "{base}/api/v1/sso-providers/{provider_id}/group-mappings"
            ))
            .bearer_auth(admin_token())
            .json(&json!({"group_name": group, "role": role, "org_id": org_id}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
    }

    sso_login(&client, &base, &stub, &issuer, json!(["admins"]))
        .await
        .expect("first sign-in");
    let ada: uuid::Uuid = sqlx::query_scalar("select id from users where email = $1")
        .bind("ada@example.com")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(org_admin_rows(&pool, ada, org_uuid).await, vec!["sso"]);

    // the IdP moves ada out of the admin group while nobody else is admin: the
    // sign-in still succeeds, the admin grant stays, and the audit says why
    sso_login(&client, &base, &stub, &issuer, json!(["devs"]))
        .await
        .expect("a kept admin grant must not fail the sign-in");
    assert_eq!(org_admin_rows(&pool, ada, org_uuid).await, vec!["sso"]);
    assert_eq!(last_admin_kept_audits(&pool, org_uuid).await, 1);
    let granted: Value = sqlx::query_scalar(
        "select detail->'granted_roles' from audit_log where action = 'auth.sso_login' \
         order by at desc limit 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(granted, json!(["admin", "member"]));

    // with another active admin the next sign-in revokes it
    let bob = seed_user(&pool, "bob@example.com", false).await;
    seed_membership(&pool, bob, Some(org_uuid), None, None, "admin").await;
    sso_login(&client, &base, &stub, &issuer, json!(["devs"]))
        .await
        .expect("sign-in");
    assert!(org_admin_rows(&pool, ada, org_uuid).await.is_empty());
    assert_eq!(last_admin_kept_audits(&pool, org_uuid).await, 1);
}

/// #2435: the first OIDC sign-in gives an account with no name the IdP's, as a
/// default the user still owns. A name already on the account is never
/// replaced, and a later sign-in never reverts the user's own edit.
#[tokio::test]
async fn first_sso_sign_in_defaults_a_missing_display_name_without_managing_it() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve_with_public_url(pool.clone(), Some(admin_token().to_string())).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("http://{addr}");
    let (issuer, stub) = stub_idp::serve_stub().await;

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "SsoNameOrg", "slug": "sso-name-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let created = client
        .post(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Stub IdP", "slug": "names", "issuer": issuer,
            "client_id": "rolter", "client_secret": "s3cret",
            "default_role": "member"
        }))
        .send()
        .await
        .unwrap();
    assert!(created.status().is_success());

    let sign_in = |email: &'static str, name: &'static str| {
        let client = client.clone();
        let base = base.clone();
        let issuer = issuer.clone();
        let stub = stub.clone();
        async move {
            let start = client
                .get(format!("{base}/auth/sso/names/start"))
                .send()
                .await
                .unwrap();
            let location = start
                .headers()
                .get("location")
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            let state = url_param(&location, "state");
            let nonce = url_param(&location, "nonce");
            let mut claims = stub_idp::claims(&issuer, "rolter", &nonce, json!([]));
            claims["email"] = json!(email);
            claims["preferred_username"] = json!(name);
            *stub.next_claims.lock().unwrap() = claims;
            let body: Value = client
                .get(format!(
                    "{base}/auth/sso/names/callback?code=abc&state={state}"
                ))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            body["token"].as_str().unwrap().to_string()
        }
    };
    let me = |token: String| {
        let client = client.clone();
        let base = base.clone();
        async move {
            client
                .get(format!("{base}/api/v1/auth/me"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };

    // a new account takes the IdP's name, unmanaged
    let token = sign_in("ada@example.com", "Ada Lovelace").await;
    let seen = me(token.clone()).await;
    assert_eq!(seen["user"]["display_name"], "Ada Lovelace");
    assert_eq!(seen["display_name_managed"], false);

    // the user can still change it, and the next sign-in leaves their edit be
    let renamed = client
        .patch(format!("{base}/api/v1/me/profile"))
        .bearer_auth(&token)
        .json(&json!({"display_name": "Countess Ada"}))
        .send()
        .await
        .unwrap();
    assert_eq!(renamed.status(), 200);
    let token = sign_in("ada@example.com", "Augusta Ada King").await;
    assert_eq!(me(token).await["user"]["display_name"], "Countess Ada");

    // an existing account that already has a name keeps it
    let named = seed_user(&pool, "grace@example.com", false).await;
    sqlx::query("update users set display_name = 'Grace Hopper' where id = $1")
        .bind(named)
        .execute(&pool)
        .await
        .unwrap();
    let token = sign_in("grace@example.com", "grace").await;
    assert_eq!(me(token).await["user"]["display_name"], "Grace Hopper");

    // an existing account with no name adopts the IdP's, sanitised
    seed_user(&pool, "linus@example.com", false).await;
    let token = sign_in("linus@example.com", " Linus\u{7} T ").await;
    assert_eq!(me(token).await["user"]["display_name"], "Linus T");
}

/// An org's provider signs in as an existing account only when every role that
/// account holds sits inside the provider's org, and never as a superadmin:
/// whoever registers a provider chooses the emails it asserts, so anything
/// wider would hand one org's admins another org's member, or the break-glass
/// account. The account's roles are left exactly as they were.
#[tokio::test]
async fn sso_never_adopts_a_superadmin_or_an_account_with_roles_in_another_org() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve_with_public_url(pool.clone(), Some(admin_token().to_string())).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("http://{addr}");
    let (issuer, stub) = stub_idp::serve_stub().await;

    let mut org_ids = Vec::new();
    for (name, slug) in [("Acme", "acme-adopt"), ("Globex", "globex-adopt")] {
        let org: Value = client
            .post(format!("{base}/api/v1/orgs"))
            .bearer_auth(admin_token())
            .json(&json!({"name": name, "slug": slug}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        org_ids.push(org["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap());
    }
    let (acme, globex) = (org_ids[0], org_ids[1]);
    let created = client
        .post(format!("{base}/api/v1/orgs/{acme}/sso-providers"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Acme IdP", "slug": "acme-idp", "issuer": issuer,
            "client_id": "rolter", "client_secret": "s3cret",
            "default_role": "member"
        }))
        .send()
        .await
        .unwrap();
    assert!(created.status().is_success());

    let ada = seed_user(&pool, "ada@example.com", false).await;
    seed_membership(&pool, ada, Some(globex), None, None, "admin").await;
    seed_user(&pool, "root@example.com", true).await;
    let grace = seed_user(&pool, "grace@example.com", false).await;
    seed_membership(&pool, grace, Some(acme), None, None, "viewer").await;

    let sign_in_as = |email: &'static str, accept: &'static str| {
        let client = client.clone();
        let base = base.clone();
        let issuer = issuer.clone();
        let stub = stub.clone();
        async move {
            let start = client
                .get(format!("{base}/auth/sso/acme-idp/start"))
                .send()
                .await
                .unwrap();
            let location = start
                .headers()
                .get("location")
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            let state = url_param(&location, "state");
            let nonce = url_param(&location, "nonce");
            let mut claims = stub_idp::claims(&issuer, "rolter", &nonce, json!([]));
            claims["email"] = json!(email);
            *stub.next_claims.lock().unwrap() = claims;
            client
                .get(format!(
                    "{base}/auth/sso/acme-idp/callback?code=abc&state={state}"
                ))
                .header("accept", accept)
                .send()
                .await
                .unwrap()
        }
    };
    let sign_in = |email: &'static str| sign_in_as(email, "text/html");
    let refused = format!("{base}/login?sso_error=account_outside_org&sso=acme-idp");
    let roles = |user: uuid::Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (Option<uuid::Uuid>, String, String)>(
                "select org_id, role, source from memberships where user_id = $1 order by org_id",
            )
            .bind(user)
            .fetch_all(&pool)
            .await
            .unwrap()
        }
    };

    // another org's admin: refused, and nothing is granted in acme
    let response = sign_in("ada@example.com").await;
    assert_eq!(response.status(), 303);
    assert_eq!(redirect_of(&response), refused);
    assert_eq!(
        roles(ada).await,
        [(Some(globex), "admin".to_string(), "manual".to_string())]
    );

    // the break-glass account: refused
    let response = sign_in("root@example.com").await;
    assert_eq!(redirect_of(&response), refused);

    // a client that does not ask for html is told the same code, with the
    // status it always had and nothing that names the account (#3468)
    for email in ["ada@example.com", "root@example.com"] {
        let response = sign_in_as(email, "application/json").await;
        assert_eq!(response.status(), 403);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"]["code"], "account_outside_org", "{body}");
        let message = body["error"]["message"].as_str().unwrap();
        assert!(!message.contains(email), "{message}");
        assert!(!message.contains("example.com"), "{message}");
        assert_ne!(message, "insufficient role for this resource");
    }

    // an account whose roles are all in acme is adopted, as before
    let response = sign_in("grace@example.com").await;
    assert_eq!(response.status(), 303);
    let location = redirect_of(&response);
    assert!(
        !url_param(&location, "sso_code").is_empty(),
        "grace is adopted: {location}"
    );

    // a new address is created in acme, as before
    let response = sign_in("new@example.com").await;
    assert!(!url_param(&redirect_of(&response), "sso_code").is_empty());
}

/// An account holding a deployment role (`users.deployment_role`) carries
/// deployment-wide authority on the account itself, so no org's provider may
/// sign in as it, even with no memberships: refused like a superadmin, and no
/// session is issued.
#[tokio::test]
async fn sso_never_adopts_an_account_holding_a_deployment_role() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve_with_public_url(pool.clone(), Some(admin_token().to_string())).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("http://{addr}");
    let (issuer, stub) = stub_idp::serve_stub().await;

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "Acme", "slug": "acme-role"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let acme = org["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap();
    let created = client
        .post(format!("{base}/api/v1/orgs/{acme}/sso-providers"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Acme IdP", "slug": "acme-idp", "issuer": issuer,
            "client_id": "rolter", "client_secret": "s3cret",
            "default_role": "member"
        }))
        .send()
        .await
        .unwrap();
    assert!(created.status().is_success());

    let ops = seed_user(&pool, "ops@example.com", false).await;
    sqlx::query("update users set deployment_role = 'operator' where id = $1")
        .bind(ops)
        .execute(&pool)
        .await
        .unwrap();

    let start = client
        .get(format!("{base}/auth/sso/acme-idp/start"))
        .send()
        .await
        .unwrap();
    let location = redirect_of(&start);
    let state = url_param(&location, "state");
    let nonce = url_param(&location, "nonce");
    let mut claims = stub_idp::claims(&issuer, "rolter", &nonce, json!([]));
    claims["email"] = json!("ops@example.com");
    *stub.next_claims.lock().unwrap() = claims;
    let response = client
        .get(format!(
            "{base}/auth/sso/acme-idp/callback?code=abc&state={state}"
        ))
        .header("accept", "text/html")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 303);
    assert_eq!(
        redirect_of(&response),
        format!("{base}/login?sso_error=account_outside_org&sso=acme-idp")
    );
    let memberships: i64 =
        sqlx::query_scalar("select count(*) from memberships where user_id = $1")
            .bind(ops)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(memberships, 0, "the refused sign-in granted nothing");
    let sessions: i64 = sqlx::query_scalar("select count(*) from sessions where user_id = $1")
        .bind(ops)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(sessions, 0, "no session is issued");
}

fn redirect_of(response: &reqwest::Response) -> String {
    response
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}
