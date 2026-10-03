//! The one list of routes this control plane answers without a credential
//! (#1840).
//!
//! The router has no blanket auth layer: a handler is protected only because
//! it takes an extractor (`Principal`, `CurrentUser`, `AnySession`, a SCIM
//! bearer, an internal token). Forgetting one is silent, and it is exactly how
//! `GET /api/v1/config` shipped the deployment's topology to anyone who asked.
//! So the list of routes that are *meant* to be open lives here, each with the
//! reason it has to be, and the tests below fail on any route that answers an
//! anonymous caller and is not on it.
//!
//! Adding an entry is a decision to publish something to the internet, so the
//! reason must say what the route reveals and why that is acceptable. The
//! OpenAPI document's `security: []` marking (`Op::public`) must agree with
//! this list; `openapi_public_marking_matches_the_allowlist` checks both ways.

/// One route the control plane answers without a session or token.
pub(crate) struct PublicRoute {
    /// lowercase http method, as the OpenAPI document spells it
    pub(crate) method: &'static str,
    /// the OpenAPI path template
    pub(crate) path: &'static str,
    /// why this route has to be reachable anonymously, and what it reveals
    pub(crate) reason: &'static str,
}

const fn route(method: &'static str, path: &'static str, reason: &'static str) -> PublicRoute {
    PublicRoute {
        method,
        path,
        reason,
    }
}

/// Every anonymous route. Keep it sorted by path, then method.
pub(crate) const PUBLIC_ROUTES: &[PublicRoute] = &[
    route(
        "get",
        "/auth/mcp/callback",
        "the MCP server's OAuth redirect lands in a browser that holds no bearer header; the \
         flow is bound by a single-use `state` value the control plane issued",
    ),
    route(
        "get",
        "/auth/sso/{slug}/callback",
        "the identity provider's redirect lands in a browser with no session yet; the flow is \
         bound by a single-use `state` and the provider's signed assertion",
    ),
    route(
        "get",
        "/auth/sso/{slug}/start",
        "begins a sign-in, so it runs before any session exists; reveals only a redirect to \
         the identity provider the operator already exposes on the login screen",
    ),
    route(
        "post",
        "/auth/sso/exchange",
        "redeems the one-time code a browser sign-in ended with; the code is the credential",
    ),
    route(
        "get",
        "/docs",
        "the API reference page; static, describes this build's surface and no deployment data",
    ),
    route(
        "get",
        "/docs/scalar.js",
        "the embedded script backing /docs; static",
    ),
    route(
        "get",
        "/healthz",
        "liveness probe for the orchestrator; answers a constant `ok`",
    ),
    route(
        "get",
        "/openapi.json",
        "the API schema; describes this build's surface and no deployment data",
    ),
    route(
        "get",
        "/api/v1/auth/methods",
        "the login screen asks which sign-in methods exist before anyone can sign in; \
         reveals only method names and the SSO providers shown as login buttons",
    ),
    route(
        "post",
        "/api/v1/auth/login",
        "exchanges an email and password for a session; authenticated by its own body and \
         throttled",
    ),
    route(
        "post",
        "/api/v1/auth/logout",
        "idempotent by design: a dashboard whose session already lapsed still signs out \
         cleanly. With no live token it revokes nothing and reveals nothing",
    ),
    route(
        "post",
        "/api/v1/auth/mfa/confirm",
        "completes a forced enrolment mid-sign-in; authenticated by the single-use challenge \
         token login issued",
    ),
    route(
        "post",
        "/api/v1/auth/mfa/enroll",
        "starts a forced enrolment mid-sign-in; authenticated by the single-use challenge \
         token login issued",
    ),
    route(
        "post",
        "/api/v1/auth/mfa/verify",
        "redeems a second-factor challenge mid-sign-in; authenticated by the challenge token \
         and the code",
    ),
    route(
        "get",
        "/api/v1/invitations/accept/{token}",
        "an invitee has no account yet; the unguessable one-time token is the credential and \
         the preview reveals only what the invitation already tells its recipient",
    ),
    route(
        "post",
        "/api/v1/invitations/accept/{token}/accept",
        "an invitee has no account yet; the unguessable one-time token is the credential",
    ),
    route(
        "get",
        "/api/v1/ping",
        "round-trip check the login screen uses to tell a reachable control plane from a \
         dead one; answers a constant",
    ),
    route(
        "get",
        "/readyz",
        "readiness probe for the orchestrator; answers fixed status words and never a driver \
         error, host or version",
    ),
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use tower::ServiceExt;

    use super::*;

    const METHODS: [&str; 5] = ["get", "post", "put", "patch", "delete"];

    /// every `(method, path)` the served OpenAPI document lists. The document is
    /// itself pinned to the source by `every_registered_route_is_documented`,
    /// so this is the real route set rather than a hand-copied one
    fn documented() -> Vec<(String, String, bool)> {
        let document = crate::openapi::document();
        let mut out = Vec::new();
        let paths = document["paths"].as_object().expect("paths object");
        for (path, item) in paths {
            for method in METHODS {
                if let Some(op) = item.get(method) {
                    let marked_public = op["security"]
                        .as_array()
                        .is_some_and(|security| security.is_empty());
                    out.push((method.to_string(), path.clone(), marked_public));
                }
            }
        }
        out
    }

    fn allowlisted(method: &str, path: &str) -> bool {
        PUBLIC_ROUTES
            .iter()
            .any(|route| route.method == method && route.path == path)
    }

    #[test]
    fn the_allowlist_is_sorted_unique_and_explained() {
        let mut seen = BTreeSet::new();
        for route in PUBLIC_ROUTES {
            assert!(
                seen.insert((route.path, route.method)),
                "{} {} is listed twice",
                route.method,
                route.path
            );
            assert!(
                METHODS.contains(&route.method),
                "{} is not a lowercase method",
                route.method
            );
            assert!(
                route.reason.trim().len() >= 20,
                "{} {} needs a real reason it is public",
                route.method,
                route.path
            );
        }
    }

    /// `security: []` in the served document is what API clients read as "no
    /// credential needed", so it must say the same thing as the allowlist
    #[test]
    fn openapi_public_marking_matches_the_allowlist() {
        let ops = documented();
        let mut problems = Vec::new();
        for (method, path, marked_public) in &ops {
            match (*marked_public, allowlisted(method, path)) {
                (true, false) => problems.push(format!(
                    "{} {path} is marked `.public()` in openapi.rs but is not in PUBLIC_ROUTES",
                    method.to_uppercase()
                )),
                (false, true) => problems.push(format!(
                    "{} {path} is in PUBLIC_ROUTES but openapi.rs does not mark it `.public()`",
                    method.to_uppercase()
                )),
                _ => {}
            }
        }
        for route in PUBLIC_ROUTES {
            if !ops
                .iter()
                .any(|(m, p, _)| m == route.method && p == route.path)
            {
                problems.push(format!(
                    "{} {} is in PUBLIC_ROUTES but no such operation is documented",
                    route.method.to_uppercase(),
                    route.path
                ));
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    #[cfg(feature = "postgres")]
    fn app() -> axum::Router {
        // a pool that never connects: an anonymous request is refused before
        // any handler reaches for it, and one that is not refused and does
        // reach for it fails loudly instead of quietly passing
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(200))
            .connect_lazy("postgres://nobody:nothing@127.0.0.1:1/none")
            .expect("a lazy pool never connects, so it cannot fail to build");
        crate::build_app_with(
            crate::test_state(pool, Some("guard-test-token".into()), None),
            true,
        )
    }

    #[cfg(not(feature = "postgres"))]
    fn app() -> axum::Router {
        crate::build_app_with(
            crate::tests::state_with_token(Some("guard-test-token")),
            true,
        )
    }

    async fn anonymous_status(app: &axum::Router, method: &str, path: &str) -> StatusCode {
        let concrete = path
            .split('/')
            .map(|segment| {
                if segment.starts_with('{') {
                    "00000000-0000-0000-0000-000000000000"
                } else {
                    segment
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        let request = Request::builder()
            .method(method.to_uppercase().as_str())
            .uri(concrete)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .expect("a well-formed request");
        app.clone()
            .oneshot(request)
            .await
            .expect("the router is infallible")
            .status()
    }

    /// The guard. It drives the real router, with an admin token set so the
    /// control plane is not in open mode, and sends every documented operation
    /// with no credential at all. Anything that is not turned away with `401`
    /// has to be on [`PUBLIC_ROUTES`], which is where a person wrote down why.
    ///
    /// A route added to the open router without an extractor, or with one that
    /// lets an anonymous caller through, answers something other than `401` and
    /// fails here by name. That is the whole defence: the router has no layer
    /// that would catch it (#1840).
    #[tokio::test]
    async fn no_route_answers_an_anonymous_caller_unless_it_is_allowlisted() {
        let app = app();
        let mut offenders = Vec::new();
        let mut stale = Vec::new();
        for (method, path, _) in documented() {
            let status = anonymous_status(&app, &method, &path).await;
            if allowlisted(&method, &path) {
                // a `401` is a legitimate answer here (a login with the wrong
                // password), a `404` is not: it means the route is gone or was
                // never mounted, and the entry would outlive it unnoticed
                if status == StatusCode::NOT_FOUND && cfg!(feature = "postgres") {
                    stale.push(format!("{} {path} answered 404", method.to_uppercase()));
                }
                continue;
            }
            // without a database the routes that need one are not mounted at all
            if cfg!(not(feature = "postgres")) && status == StatusCode::NOT_FOUND {
                continue;
            }
            if status != StatusCode::UNAUTHORIZED {
                offenders.push(format!(
                    "{} {path} answered {status} to an anonymous caller",
                    method.to_uppercase()
                ));
            }
        }
        assert!(
            offenders.is_empty(),
            "routes reachable without a session that are not in PUBLIC_ROUTES \
             (require a session, or add an entry with a reason):\n{}",
            offenders.join("\n")
        );
        assert!(
            stale.is_empty(),
            "PUBLIC_ROUTES entries for routes the router does not serve:\n{}",
            stale.join("\n")
        );
    }
}
