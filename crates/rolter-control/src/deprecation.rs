//! The deprecation machinery ADR-0032 promises (#1423).
//!
//! `/api/v1/*` is additive within `v1`. Removing or incompatibly changing
//! anything the control-plane OpenAPI document publishes needs a deprecation
//! window of at least two minor releases **and** 90 days, announced three ways:
//! `Deprecation` and `Sunset` response headers (RFC 9745, RFC 8594),
//! `deprecated: true` in `GET /openapi.json`, and a section in the release
//! notes. [`DEPRECATIONS`] is the one place a deprecation is written down, and
//! all three are derived from it, so they cannot drift apart:
//!
//! - [`apply`] is the middleware that adds the headers to a response;
//! - `openapi.rs` asks [`deprecates_operation`] and [`mark_fields`] while it
//!   builds the document;
//! - [`render_notes`] renders the release-notes section, checked in as
//!   `deprecations.md` and appended to the release by
//!   `scripts/release-deprecations.py`.
//!
//! [`problems`] holds every entry to the ADR's window, so shortening one has to
//! say so (`security_fix`) rather than slip through a review.
//!
//! A superseded `/internal/snapshot` format is deprecated here too, as
//! [`Target::SnapshotFormat`] (ADR-0038). Its headers are not added by
//! [`apply`]: the snapshot handler asks [`for_snapshot_format`], since which
//! format a poll is served depends on the gateway rather than on the path.
//! [`snapshot_format_problems`] holds `MIN_SERVED_FORMAT` to this registry, so
//! a format cannot be retired before its window has run.
//!
//! To deprecate something, add an entry to [`DEPRECATIONS`], run
//! `ROLTER_TEST_UPDATE_DEPRECATIONS=1 cargo test -p rolter-control --lib
//! deprecation`, and commit `deprecations.md` with it. The walkthrough is in
//! `docs/dev-docs/development/deprecations.md`.

// spec-ref: deprecation-header, sunset-header
use axum::extract::{MatchedPath, Request};
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use chrono::{Duration, NaiveDate};

/// Fewest days between the announcement and the sunset (ADR-0032 rule 3).
pub const MIN_WINDOW_DAYS: i64 = 90;

/// Fewest minor releases a deprecation must survive (ADR-0032 rule 3).
pub const MIN_WINDOW_MINORS: u64 = 2;

/// What is being deprecated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// a whole operation, as the OpenAPI document spells it: the method and
    /// the path template (`{id}`, not axum's `{*id}`)
    Operation {
        method: &'static str,
        path: &'static str,
    },
    /// one property of a schema in `components/schemas`. The headers go on the
    /// operations that serve it, since a header cannot name a field
    Field {
        schema: &'static str,
        field: &'static str,
        served_by: &'static [(&'static str, &'static str)],
    },
    /// a superseded `/internal/snapshot` format (ADR-0038), deprecated by the
    /// release that bumps `SNAPSHOT_FORMAT` past it. The snapshot handler sends
    /// the headers with every snapshot served in this format
    SnapshotFormat(u32),
}

/// One deprecation. Every date is `YYYY-MM-DD`, read as UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deprecation {
    pub target: Target,
    /// the release that publishes the deprecation, as the workspace version.
    /// The two-minor half of the window counts from here
    pub since: &'static str,
    /// the date of that release; the 90-day half counts from here, and it is
    /// the `Deprecation` header's value
    pub announced: &'static str,
    /// the date after which the target may be removed; the `Sunset` header
    pub sunset: &'static str,
    /// what to use instead, one sentence for the release notes
    pub replacement: &'static str,
    /// the window was shortened to fix a vulnerability, which ADR-0032 allows.
    /// The release notes say so
    pub security_fix: bool,
}

/// Everything currently deprecated. Empty: nothing has been deprecated yet.
pub const DEPRECATIONS: &[Deprecation] = &[];

impl Target {
    /// Every `(method, path)` whose responses carry the headers.
    fn operations(&self) -> Vec<(&'static str, &'static str)> {
        match self {
            Target::Operation { method, path } => vec![(*method, *path)],
            Target::Field { served_by, .. } => served_by.to_vec(),
            // answered by the snapshot handler, not by the route middleware
            Target::SnapshotFormat(_) => Vec::new(),
        }
    }

    /// How the release notes and error messages name the target.
    fn describe(&self) -> String {
        match self {
            Target::Operation { method, path } => format!("{} {path}", method.to_uppercase()),
            Target::Field { schema, field, .. } => format!("{schema}.{field}"),
            Target::SnapshotFormat(format) => format!("/internal/snapshot format {format}"),
        }
    }
}

impl Deprecation {
    fn announced_date(&self) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(self.announced, "%Y-%m-%d").ok()
    }

    fn sunset_date(&self) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(self.sunset, "%Y-%m-%d").ok()
    }

    /// The `Deprecation` header: an RFC 9745 structured-field date, `@` and
    /// the Unix time of the announcement at 00:00 UTC.
    pub fn deprecation_header(&self) -> Option<String> {
        let at = self.announced_date()?.and_hms_opt(0, 0, 0)?;
        Some(format!("@{}", at.and_utc().timestamp()))
    }

    /// The `Sunset` header: an RFC 8594 HTTP-date (IMF-fixdate) for 00:00 UTC
    /// on the sunset date.
    pub fn sunset_header(&self) -> Option<String> {
        let at = self.sunset_date()?.and_hms_opt(0, 0, 0)?;
        Some(at.and_utc().format("%a, %d %b %Y %H:%M:%S GMT").to_string())
    }

    /// The first release the target may be removed in: two minors after the
    /// one that announced it. Removal still waits for the sunset date.
    pub fn removable_from(&self) -> Option<String> {
        let (major, minor, _) = parse_version(self.since)?;
        Some(format!("{major}.{}.0", minor + MIN_WINDOW_MINORS))
    }
}

/// `major.minor.patch`, ignoring any pre-release or build suffix.
fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

/// axum writes a wildcard `{*rest}`; the OpenAPI document writes `{rest}`.
fn openapi_path(matched: &str) -> String {
    matched.replace("{*", "{")
}

/// The deprecation covering `method path`, or the soonest-sunsetting one when
/// several do.
pub fn for_operation<'a>(
    list: &'a [Deprecation],
    method: &str,
    path: &str,
) -> Option<&'a Deprecation> {
    list.iter()
        .filter(|d| {
            d.target
                .operations()
                .iter()
                .any(|(m, p)| m.eq_ignore_ascii_case(method) && *p == path)
        })
        .min_by_key(|d| d.sunset)
}

/// The deprecation of snapshot format `format`, if it is deprecated.
pub fn for_snapshot_format(list: &[Deprecation], format: u32) -> Option<&Deprecation> {
    list.iter()
        .find(|d| d.target == Target::SnapshotFormat(format))
}

/// Whether `GET /openapi.json` marks `method path` `deprecated: true`. A field
/// deprecation does not deprecate the operation that serves it.
pub fn deprecates_operation(method: &str, path: &str) -> bool {
    deprecates_operation_in(DEPRECATIONS, method, path)
}

fn deprecates_operation_in(list: &[Deprecation], method: &str, path: &str) -> bool {
    list.iter().any(|d| {
        matches!(d.target, Target::Operation { method: m, path: p }
            if m.eq_ignore_ascii_case(method) && p == path)
    })
}

/// Mark every deprecated field `deprecated: true` in a `components/schemas`
/// object.
pub fn mark_fields(schemas: &mut serde_json::Value) {
    mark_fields_in(DEPRECATIONS, schemas);
}

fn mark_fields_in(list: &[Deprecation], schemas: &mut serde_json::Value) {
    for d in list {
        let Target::Field { schema, field, .. } = d.target else {
            continue;
        };
        if let Some(property) = schemas
            .get_mut(schema)
            .and_then(|s| s.get_mut("properties"))
            .and_then(|p| p.get_mut(field))
            .and_then(|p| p.as_object_mut())
        {
            property.insert("deprecated".to_string(), serde_json::Value::Bool(true));
        }
    }
}

/// Add `Deprecation` and `Sunset` to the response of a deprecated operation.
pub async fn apply(request: Request, next: Next) -> Response {
    apply_with(DEPRECATIONS, request, next).await
}

async fn apply_with(list: &[Deprecation], request: Request, next: Next) -> Response {
    // nothing is deprecated for most of a project's life; skip the lookup
    if list.is_empty() {
        return next.run(request).await;
    }
    let found = request
        .extensions()
        .get::<MatchedPath>()
        .and_then(|matched| {
            for_operation(
                list,
                request.method().as_str(),
                &openapi_path(matched.as_str()),
            )
        })
        .copied();
    let mut response = next.run(request).await;
    if let Some(d) = found {
        for (name, value) in [
            ("deprecation", d.deprecation_header()),
            ("sunset", d.sunset_header()),
        ] {
            if let Some(value) = value.and_then(|v| HeaderValue::from_str(&v).ok()) {
                response
                    .headers_mut()
                    .insert(HeaderName::from_static(name), value);
            }
        }
    }
    response
}

/// Every way `list` falls short of ADR-0032, one sentence each.
///
/// `documented` says whether the OpenAPI document publishes an operation, and
/// `has_field` whether a schema has a property, so a typo in the registry
/// cannot silently deprecate nothing.
pub fn problems(
    list: &[Deprecation],
    documented: impl Fn(&str, &str) -> bool,
    has_field: impl Fn(&str, &str) -> bool,
) -> Vec<String> {
    let mut problems = Vec::new();
    for (i, d) in list.iter().enumerate() {
        let name = d.target.describe();
        let here = |what: String| problems_line(&name, what);
        if list[..i].iter().any(|earlier| earlier.target == d.target) {
            problems.push(here("is listed twice".to_string()));
        }
        for (method, path) in d.target.operations() {
            if !documented(method, path) {
                problems.push(here(format!(
                    "names {} {path}, which the OpenAPI document does not publish",
                    method.to_uppercase()
                )));
            }
        }
        if let Target::Field {
            schema,
            field,
            served_by,
        } = d.target
        {
            if !has_field(schema, field) {
                problems.push(here(format!(
                    "names a property the {schema} schema does not have"
                )));
            }
            if served_by.is_empty() {
                problems.push(here(
                    "lists no operation in served_by, so no response carries the headers"
                        .to_string(),
                ));
            }
        }
        if let Target::SnapshotFormat(format) = d.target {
            if format == 0 || format >= rolter_core::snapshot_format::SNAPSHOT_FORMAT {
                problems.push(here(format!(
                    "deprecates a format this build does not supersede; it writes format {}",
                    rolter_core::snapshot_format::SNAPSHOT_FORMAT
                )));
            }
        }
        if parse_version(d.since).is_none() {
            problems.push(here(format!(
                "has since = {:?}, which is not major.minor.patch",
                d.since
            )));
        }
        if d.replacement.trim().is_empty() {
            problems.push(here("says nothing about what to use instead".to_string()));
        }
        match (d.announced_date(), d.sunset_date()) {
            (Some(announced), Some(sunset)) => {
                if !d.security_fix && sunset < announced + Duration::days(MIN_WINDOW_DAYS) {
                    problems.push(here(format!(
                        "sunsets {} days after it is announced; ADR-0032 requires at least \
                         {MIN_WINDOW_DAYS} unless it fixes a vulnerability (security_fix)",
                        (sunset - announced).num_days()
                    )));
                }
            }
            _ => problems.push(here(format!(
                "has an announced ({:?}) or sunset ({:?}) that is not YYYY-MM-DD",
                d.announced, d.sunset
            ))),
        }
    }
    problems
}

/// Every way `min_served`, the oldest snapshot format a control plane serves,
/// falls short of `list` (ADR-0038 section 5).
///
/// A format a release supersedes (`min_served <= k < current`) is still served
/// and must be announced as deprecated. A format it no longer serves
/// (`k < min_served`) was removed, which ADR-0032 rule 3 allows only once its
/// entry is past its sunset on `today` and `version` has reached the entry's
/// [`Deprecation::removable_from`].
pub fn snapshot_format_problems(
    list: &[Deprecation],
    min_served: u32,
    current: u32,
    today: NaiveDate,
    version: &str,
) -> Vec<String> {
    let mut problems = Vec::new();
    if min_served == 0 || min_served > current {
        problems.push(format!(
            "MIN_SERVED_FORMAT is {min_served}, outside 1..={current}, so this control plane \
             would not serve its own format"
        ));
        return problems;
    }
    for format in 1..current {
        let entry = for_snapshot_format(list, format);
        let served = format >= min_served;
        match entry {
            None if served => problems.push(format!(
                "snapshot format {format} is superseded by format {current} but has no \
                 DEPRECATIONS entry, so nothing announces when it goes"
            )),
            None => problems.push(format!(
                "snapshot format {format} is no longer served, but no DEPRECATIONS entry \
                 shows that its window ran"
            )),
            Some(_) if served => {}
            Some(d) => {
                if d.sunset_date().is_none_or(|sunset| today < sunset) {
                    problems.push(format!(
                        "snapshot format {format} is no longer served before its sunset ({}); \
                         MIN_SERVED_FORMAT may pass it only after that date",
                        d.sunset
                    ));
                }
                let removable = d.removable_from();
                let reached = match (
                    removable.as_deref().and_then(parse_version),
                    parse_version(version),
                ) {
                    (Some(from), Some(now)) => now >= from,
                    _ => false,
                };
                if !reached {
                    problems.push(format!(
                        "snapshot format {format} is no longer served in {version}, but it is \
                         removable only from {} onwards",
                        removable.unwrap_or_else(|| "?".to_string())
                    ));
                }
            }
        }
    }
    problems
}

fn problems_line(name: &str, what: String) -> String {
    format!("deprecation of {name} {what}")
}

/// The release-notes section for `list`, ordered by sunset. Empty when nothing
/// is deprecated, so a release with nothing to announce carries no section.
pub fn render_notes(list: &[Deprecation]) -> String {
    if list.is_empty() {
        return String::new();
    }
    let mut sorted: Vec<&Deprecation> = list.iter().collect();
    sorted.sort_by_key(|d| (d.sunset, d.target.describe()));
    let mut out = String::from("### Deprecations\n\n");
    for d in sorted {
        out.push_str(&format!(
            "- `{}`: deprecated in {} ({}), sunset {}, removable from {} onwards. {}{}\n",
            d.target.describe(),
            d.since,
            d.announced,
            d.sunset,
            d.removable_from().unwrap_or_else(|| "?".to_string()),
            d.replacement.trim(),
            if d.security_fix {
                " The window is shortened to fix a vulnerability."
            } else {
                ""
            },
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    const OLD: Deprecation = Deprecation {
        target: Target::Operation {
            method: "get",
            path: "/api/v1/old/{id}",
        },
        since: "1.2.3",
        announced: "2027-01-15",
        sunset: "2027-04-20",
        replacement: "Use `GET /api/v1/new/{id}`.",
        security_fix: false,
    };

    const FIELD: Deprecation = Deprecation {
        target: Target::Field {
            schema: "Thing",
            field: "legacy",
            served_by: &[("get", "/api/v1/things")],
        },
        since: "1.3.0",
        announced: "2027-02-01",
        sunset: "2027-06-01",
        replacement: "Read `modern`.",
        security_fix: false,
    };

    fn always(_: &str, _: &str) -> bool {
        true
    }

    #[test]
    fn the_registry_meets_the_adr_and_names_real_things() {
        let found = problems(
            DEPRECATIONS,
            crate::openapi::documents_operation,
            crate::openapi::schema_has_property,
        );
        assert!(found.is_empty(), "{found:#?}");
    }

    /// ADR-0038 section 5: `MIN_SERVED_FORMAT` cannot pass a format before its
    /// window has run, and every superseded format still served is announced.
    #[test]
    fn min_served_format_is_held_to_the_registry() {
        use rolter_core::snapshot_format::{MIN_SERVED_FORMAT, SNAPSHOT_FORMAT};
        let found = snapshot_format_problems(
            DEPRECATIONS,
            MIN_SERVED_FORMAT,
            SNAPSHOT_FORMAT,
            chrono::Utc::now().date_naive(),
            env!("CARGO_PKG_VERSION"),
        );
        assert!(found.is_empty(), "{found:#?}");
    }

    const FORMAT_ONE: Deprecation = Deprecation {
        target: Target::SnapshotFormat(1),
        since: "1.2.0",
        announced: "2027-01-15",
        sunset: "2027-04-20",
        replacement: "Upgrade the gateways to a release that reads format 2.",
        security_fix: false,
    };

    fn day(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn a_superseded_format_still_served_must_be_announced() {
        let found = snapshot_format_problems(&[], 1, 2, day("2027-01-01"), "1.2.0");
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("no DEPRECATIONS entry"), "{found:?}");
        assert!(
            snapshot_format_problems(&[FORMAT_ONE], 1, 2, day("2027-01-01"), "1.2.0").is_empty()
        );
    }

    #[test]
    fn a_format_is_retired_only_after_its_sunset_and_two_minors() {
        let retire = |today: &str, version: &str| {
            snapshot_format_problems(&[FORMAT_ONE], 2, 2, day(today), version)
        };
        // too early on both counts
        assert_eq!(retire("2027-02-01", "1.3.0").len(), 2);
        // the date has passed, the release has not
        let early = retire("2027-05-01", "1.3.5");
        assert_eq!(early.len(), 1, "{early:?}");
        assert!(early[0].contains("removable only from 1.4.0"), "{early:?}");
        // the release has, the date has not
        let soon = retire("2027-04-19", "1.4.0");
        assert_eq!(soon.len(), 1, "{soon:?}");
        assert!(soon[0].contains("sunset"), "{soon:?}");
        assert!(retire("2027-04-20", "1.4.0").is_empty());
        // and retiring with no record of the window at all is a problem
        assert_eq!(
            snapshot_format_problems(&[], 2, 2, day("2030-01-01"), "9.0.0").len(),
            1
        );
        assert!(!snapshot_format_problems(&[], 3, 2, day("2030-01-01"), "9.0.0").is_empty());
    }

    #[test]
    fn a_snapshot_format_entry_names_a_superseded_format_and_no_route() {
        // this build writes format 1, so deprecating 1 (or 0) is a problem
        let own = Deprecation {
            target: Target::SnapshotFormat(rolter_core::snapshot_format::SNAPSHOT_FORMAT),
            ..FORMAT_ONE
        };
        let found = problems(&[own], |_, _| false, |_, _| false);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("does not supersede"), "{found:?}");
        // the headers come from the snapshot handler, never the middleware
        assert!(own.target.operations().is_empty());
        assert_eq!(for_snapshot_format(&[own], own_format()), Some(&own));
        assert_eq!(for_snapshot_format(&[own], own_format() + 1), None);
        assert!(render_notes(&[FORMAT_ONE])
            .contains("`/internal/snapshot format 1`: deprecated in 1.2.0"));
    }

    fn own_format() -> u32 {
        rolter_core::snapshot_format::SNAPSHOT_FORMAT
    }

    /// RFC 9745 wants a structured-field Date (`@` and Unix seconds); RFC 8594
    /// wants an HTTP-date.
    #[test]
    fn headers_follow_rfc_9745_and_rfc_8594() {
        // 2027-01-15T00:00:00Z
        assert_eq!(OLD.deprecation_header().as_deref(), Some("@1799971200"));
        assert_eq!(
            OLD.sunset_header().as_deref(),
            Some("Tue, 20 Apr 2027 00:00:00 GMT")
        );
        let value = OLD.deprecation_header().unwrap();
        assert!(value.starts_with('@') && value[1..].bytes().all(|b| b.is_ascii_digit()));
    }

    #[test]
    fn removal_waits_two_minors_after_the_announcing_release() {
        assert_eq!(OLD.removable_from().as_deref(), Some("1.4.0"));
        assert_eq!(FIELD.removable_from().as_deref(), Some("1.5.0"));
    }

    #[test]
    fn a_window_shorter_than_ninety_days_is_a_problem_unless_it_is_a_security_fix() {
        let short = Deprecation {
            sunset: "2027-03-01",
            ..OLD
        };
        let found = problems(&[short], always, always);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("at least 90"), "{found:?}");

        let urgent = Deprecation {
            security_fix: true,
            ..short
        };
        assert!(problems(&[urgent], always, always).is_empty());
        assert!(render_notes(&[urgent]).contains("shortened to fix a vulnerability"));
        // exactly ninety days is enough
        let exact = Deprecation {
            sunset: "2027-04-15",
            ..OLD
        };
        assert!(problems(&[exact], always, always).is_empty());
    }

    #[test]
    fn an_entry_naming_nothing_real_is_a_problem() {
        let found = problems(&[OLD, FIELD], |_, _| false, |_, _| false);
        assert!(
            found.iter().any(|p| p.contains("does not publish")),
            "{found:?}"
        );
        assert!(found.iter().any(|p| p.contains("property")), "{found:?}");
        let bad = Deprecation {
            since: "soon",
            announced: "yesterday",
            replacement: " ",
            ..OLD
        };
        assert_eq!(problems(&[bad], always, always).len(), 3);
        assert!(problems(&[OLD, OLD], always, always)
            .iter()
            .any(|p| p.contains("twice")));
    }

    #[test]
    fn the_release_notes_list_each_sunset_soonest_first() {
        assert_eq!(render_notes(&[]), "");
        let notes = render_notes(&[FIELD, OLD]);
        assert!(notes.starts_with("### Deprecations\n"), "{notes}");
        let old = notes.find("GET /api/v1/old/{id}").unwrap();
        let field = notes.find("Thing.legacy").unwrap();
        assert!(old < field, "{notes}");
        assert!(notes.contains("sunset 2027-04-20, removable from 1.4.0 onwards"));
    }

    #[test]
    fn a_field_is_marked_deprecated_in_its_schema_only() {
        let mut schemas = serde_json::json!({
            "Thing": {"properties": {"legacy": {"type": "string"}, "modern": {"type": "string"}}}
        });
        mark_fields_in(&[FIELD], &mut schemas);
        assert_eq!(schemas["Thing"]["properties"]["legacy"]["deprecated"], true);
        assert!(schemas["Thing"]["properties"]["modern"]
            .get("deprecated")
            .is_none());
        // a field deprecation does not deprecate its operation
        assert!(for_operation(&[FIELD], "GET", "/api/v1/things").is_some());
        assert!(!deprecates_operation_in(&[FIELD], "get", "/api/v1/things"));
        assert!(deprecates_operation_in(&[OLD], "GET", "/api/v1/old/{id}"));
        assert!(!deprecates_operation_in(&[OLD], "POST", "/api/v1/old/{id}"));
    }

    async fn call(list: &'static [Deprecation], method: &str, uri: &str) -> Response {
        let app = Router::new()
            .route(
                "/api/v1/old/{id}",
                get(|| async { "old" }).post(|| async { "old" }),
            )
            .route("/api/v1/other", get(|| async { "other" }))
            .layer(axum::middleware::from_fn(move |request, next| {
                apply_with(list, request, next)
            }));
        app.oneshot(
            HttpRequest::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_deprecated_operation_answers_with_both_headers() {
        static LIST: [Deprecation; 1] = [OLD];
        let response = call(&LIST, "GET", "/api/v1/old/42").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["deprecation"], "@1799971200");
        assert_eq!(
            response.headers()["sunset"],
            "Tue, 20 Apr 2027 00:00:00 GMT"
        );
    }

    #[tokio::test]
    async fn other_operations_and_methods_carry_no_headers() {
        static LIST: [Deprecation; 1] = [OLD];
        for (method, uri) in [("GET", "/api/v1/other"), ("POST", "/api/v1/old/42")] {
            let response = call(&LIST, method, uri).await;
            assert!(
                response.headers().get("deprecation").is_none(),
                "{method} {uri}"
            );
            assert!(response.headers().get("sunset").is_none(), "{method} {uri}");
        }
    }

    /// The registry, the document and the checked-in notes agree.
    #[test]
    fn the_checked_in_notes_match_the_registry() {
        let expected = render_notes(DEPRECATIONS);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("deprecations.md");
        if std::env::var_os("ROLTER_TEST_UPDATE_DEPRECATIONS").is_some() {
            std::fs::write(&path, &expected).unwrap();
        }
        let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            on_disk, expected,
            "deprecations.md is stale; rerun with ROLTER_TEST_UPDATE_DEPRECATIONS=1"
        );
    }
}
