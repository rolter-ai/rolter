//! A served OpenAPI 3.1 description of the control-plane management API.
//!
//! Hand-authored in the same style as `rolter-gateway`'s `openapi` module
//! (ADR-0030) rather than macro-derived, so the two documents stay
//! dependency-free and describe the wire contract instead of rolter's internal
//! Rust types. Served as JSON at `GET /openapi.json` and rendered interactively
//! by Scalar at `GET /docs`, with the Scalar bundle embedded in the binary so
//! an air-gapped deployment still gets the reference.
//!
//! `operations()` is the single table the document is built from, and the
//! `every_registered_route_is_documented` test walks every `.route(...)` call
//! under `src/` and fails when one is missing from it — the same shape of drift
//! guard as `the_matrix_lists_every_capability_exactly_once` in `rbac_matrix`.
//! Adding an endpoint without a row here is a test failure, not a silently
//! incomplete schema.
//!
//! The table describes the surface this binary can serve, not the subset a
//! given process has mounted: the CRUD routes only appear with a postgres pool,
//! but a schema that changed shape with deployment configuration would be
//! useless to generate a client from.
//!
//! Bodies are typed against `components/schemas` wherever the handler already
//! has a stable `serde` shape (tenancy, users, providers, routes, virtual keys,
//! budgets and prices). Surfaces whose payload is genuinely dynamic — settings
//! blobs, guardrail configs, SCIM envelopes, analytics rows — are documented as
//! open objects: imprecise, but present, so a generated SDK has the call even
//! where it cannot yet have the struct.

// spec-ref: openapi
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Map, Value};
use std::sync::OnceLock;

use crate::ControlState;

/// Mount the document and its interactive reference.
///
/// Unauthenticated on purpose: the schema names endpoints and shapes, never a
/// tenant's data, and a client that cannot read it before holding a credential
/// cannot generate against it either.
pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route("/openapi.json", get(openapi_json))
        .route("/docs", get(docs))
        .route("/docs/scalar.js", get(docs_bundle))
}

/// What a request or response carries.
#[derive(Clone, Copy)]
enum Payload {
    /// no body at all: a `204`, or a request that takes none
    Empty,
    /// a JSON object whose shape is dynamic or not yet modelled
    Open,
    /// a named entry under `components/schemas`
    Ref(&'static str),
    /// an array of a named entry under `components/schemas`
    List(&'static str),
}

impl Payload {
    fn schema(self) -> Option<Value> {
        match self {
            Payload::Empty => None,
            Payload::Open => Some(json!({"type": "object"})),
            Payload::Ref(name) => Some(json!({"$ref": format!("#/components/schemas/{name}")})),
            Payload::List(name) => Some(json!({
                "type": "array",
                "items": {"$ref": format!("#/components/schemas/{name}")}
            })),
        }
    }
}

/// One documented query parameter.
///
/// Path parameters are derived from the path template, but a query parameter
/// exists nowhere in the route, so an undocumented one is unreachable from the
/// reference: a caller cannot page a list whose cursor is not written down.
#[derive(Clone, Copy)]
struct QueryParam {
    name: &'static str,
    /// the JSON Schema type of the value
    ty: &'static str,
    description: &'static str,
    /// the request fails without it. `/api/v1/budgets` and
    /// `/api/v1/rate-limits` are the reason this exists: their handlers
    /// deserialize `scope_type` and `scope_id` as bare fields rather than
    /// `Option`, so a caller following a reference that called them optional
    /// gets a 400 and no clue which parameter it forgot (#1412)
    required: bool,
}

impl QueryParam {
    const fn new(name: &'static str, ty: &'static str, description: &'static str) -> Self {
        Self {
            name,
            ty,
            description,
            required: false,
        }
    }

    /// A parameter the route refuses to run without.
    const fn required(name: &'static str, ty: &'static str, description: &'static str) -> Self {
        Self {
            name,
            ty,
            description,
            required: true,
        }
    }

    fn to_json(self) -> Value {
        json!({
            "name": self.name,
            "in": "query",
            "required": self.required,
            "description": self.description,
            "schema": {"type": self.ty}
        })
    }
}

/// the `409` an account write answers when it would leave no active superadmin
const LAST_SUPERADMIN_409: &str =
    "error.code `last_superadmin`: the write would demote, deactivate or delete the last active superadmin";

/// the `409` a token rotation answers for a token that no longer works
const API_TOKEN_INACTIVE_409: &str =
    "error.code `api_token_inactive`: the token is revoked or past its expiry, so there is nothing to rotate; mint a new one";

/// the `409` a password change answers for an account with no local password
const NO_LOCAL_PASSWORD_409: &str =
    "error.code `no_local_password`: the account signs in through single sign-on only, so there is no local password to change";

/// the `409` a provider delete answers while something still references it
const PROVIDER_IN_USE_409: &str = "a route target or a provider group member still references the provider; the message names each route and group, and the provider is left in place";

/// the `409` revoking an org's last admin grant answers (#2311)
const LAST_ORG_ADMIN_409: &str =
    "error.code `last_org_admin`: the revoke would leave the org without an admin; a superadmin is exempt";

/// One documented operation: a path, a method, and what crosses the wire.
#[derive(Clone, Copy)]
struct Op {
    method: &'static str,
    /// the OpenAPI path template; axum's `{*wildcard}` is written `{wildcard}`
    path: &'static str,
    id: &'static str,
    summary: &'static str,
    tag: &'static str,
    request: Payload,
    ok: Payload,
    /// query parameters worth documenting; empty for an operation whose query
    /// string only narrows what it returns
    query: &'static [QueryParam],
    /// reachable without a credential
    public: bool,
    /// what a `303 See Other` from this operation points at, for an endpoint
    /// a browser lands on and is sent onwards from
    see_other: Option<&'static str>,
    /// when this operation can answer `409`, what that refusal means, naming
    /// the stable `error.code` where it carries one
    conflict: Option<&'static str>,
    /// when this operation can answer `406`, what that refusal means
    not_acceptable: Option<&'static str>,
    /// when this operation can answer `429`, what bounds it
    too_many_requests: Option<&'static str>,
    /// a ClickHouse-backed read, which can answer `502` or `504`
    clickhouse: bool,
    /// cuts its window into buckets, so it can answer `400`
    /// `too_many_buckets` (#3506)
    bucketed: bool,
    /// seals a credential with `ROLTER_KEK`, so it can answer `503`
    /// `kek_unavailable` on a control plane that has none
    needs_kek: bool,
}

impl Op {
    fn new(
        method: &'static str,
        path: &'static str,
        id: &'static str,
        summary: &'static str,
    ) -> Self {
        Self {
            method,
            path,
            id,
            summary,
            tag: "",
            request: Payload::Empty,
            ok: Payload::Open,
            query: &[],
            public: false,
            see_other: None,
            conflict: None,
            not_acceptable: None,
            too_many_requests: None,
            clickhouse: false,
            bucketed: false,
            needs_kek: false,
        }
    }

    fn get(path: &'static str, id: &'static str, summary: &'static str) -> Self {
        Self::new("get", path, id, summary)
    }

    fn post(path: &'static str, id: &'static str, summary: &'static str) -> Self {
        Self::new("post", path, id, summary).body(Payload::Open)
    }

    fn put(path: &'static str, id: &'static str, summary: &'static str) -> Self {
        Self::new("put", path, id, summary).body(Payload::Open)
    }

    fn patch(path: &'static str, id: &'static str, summary: &'static str) -> Self {
        Self::new("patch", path, id, summary).body(Payload::Open)
    }

    /// A delete answers `204` with no body unless [`Op::ok`] says otherwise.
    fn delete(path: &'static str, id: &'static str, summary: &'static str) -> Self {
        Self::new("delete", path, id, summary).ok(Payload::Empty)
    }

    fn body(mut self, payload: Payload) -> Self {
        self.request = payload;
        self
    }

    fn ok(mut self, payload: Payload) -> Self {
        self.ok = payload;
        self
    }

    fn query(mut self, params: &'static [QueryParam]) -> Self {
        self.query = params;
        self
    }

    fn public(mut self) -> Self {
        self.public = true;
        self
    }

    fn see_other(mut self, description: &'static str) -> Self {
        self.see_other = Some(description);
        self
    }

    fn conflict(mut self, description: &'static str) -> Self {
        self.conflict = Some(description);
        self
    }

    fn not_acceptable(mut self, description: &'static str) -> Self {
        self.not_acceptable = Some(description);
        self
    }

    /// Mark an operation bounded in how many may run at once: past the bound it
    /// answers `429` with a `Retry-After` header.
    fn too_many_requests(mut self, description: &'static str) -> Self {
        self.too_many_requests = Some(description);
        self
    }

    /// Mark an operation that seals a credential with `ROLTER_KEK`: without the
    /// key it answers `503` `kek_unavailable` rather than a field-validation
    /// `400` (#3218).
    fn needs_kek(mut self) -> Self {
        self.needs_kek = true;
        self
    }

    /// Mark a read that goes to ClickHouse: it answers `502`
    /// `analytics_query_failed`, or `504` `analytics_query_timeout` when the
    /// query ran past the control plane's request timeout.
    fn clickhouse_read(mut self) -> Self {
        self.clickhouse = true;
        self
    }

    /// Mark a read that cuts its window into buckets: one that would answer
    /// more than `MAX_BUCKETS` points is a `400` `too_many_buckets` (#3506).
    fn bucketed(mut self) -> Self {
        self.bucketed = true;
        self
    }

    fn to_json(self) -> Value {
        let mut op = Map::new();
        op.insert("summary".into(), json!(self.summary));
        op.insert("operationId".into(), json!(self.id));
        // the one registry that also drives the `Deprecation` and `Sunset`
        // headers (ADR-0032, #1423)
        if crate::deprecation::deprecates_operation(self.method, self.path) {
            op.insert("deprecated".into(), json!(true));
        }
        op.insert("tags".into(), json!([self.tag]));
        if !self.query.is_empty() {
            // operation-level parameters, so they merge with the path item's
            // own path parameters rather than replacing them
            op.insert(
                "parameters".into(),
                Value::Array(self.query.iter().map(|p| p.to_json()).collect()),
            );
        }
        if let Some(schema) = self.request.schema() {
            op.insert(
                "requestBody".into(),
                json!({"required": true, "content": {"application/json": {"schema": schema}}}),
            );
        }
        let mut responses = Map::new();
        let (code, success) = match self.ok.schema() {
            Some(schema) => (
                "200",
                json!({
                    "description": "success",
                    "content": {"application/json": {"schema": schema}}
                }),
            ),
            None => ("204", json!({"description": "deleted"})),
        };
        responses.insert(code.to_string(), success);
        if let Some(description) = self.see_other {
            responses.insert(
                "303".into(),
                json!({
                    "description": description,
                    "headers": {"Location": {"schema": {"type": "string"}}}
                }),
            );
        }
        if let Some(description) = self.conflict {
            responses.insert(
                "409".into(),
                json!({"$ref": "#/components/responses/Error", "description": description}),
            );
        }
        if let Some(description) = self.not_acceptable {
            responses.insert(
                "406".into(),
                json!({"$ref": "#/components/responses/Error", "description": description}),
            );
        }
        if let Some(description) = self.too_many_requests {
            responses.insert(
                "429".into(),
                json!({
                    "$ref": "#/components/responses/Error",
                    "description": description,
                    "headers": {"Retry-After": {"schema": {"type": "integer"}}}
                }),
            );
        }
        if self.needs_kek {
            responses.insert(
                "503".into(),
                json!({
                    "$ref": "#/components/responses/Error",
                    "description": "`ROLTER_KEK` is not configured on the control plane, so the credential cannot be sealed (`kek_unavailable`)"
                }),
            );
        }
        // every read that takes a window refuses a bound it cannot read or that
        // names a year outside the ones a window may (#1192, #3506)
        let windowed = self.query.iter().any(|p| p.name == "since");
        if windowed || self.bucketed {
            let mut description = String::from(
                "a `since` or `until` that does not parse (`invalid_time_bound`) or lies before \
                 2000-01-01 or more than a day past now (`time_bound_out_of_range`)",
            );
            if self.bucketed {
                description.push_str(
                    ", or a `bucket` that would cut the window into more than 20000 points \
                     (`too_many_buckets`)",
                );
            }
            responses.insert(
                "400".into(),
                json!({"$ref": "#/components/responses/Error", "description": description}),
            );
        }
        if self.clickhouse {
            responses.insert(
                "502".into(),
                json!({
                    "$ref": "#/components/responses/Error",
                    "description": "ClickHouse did not answer the query (`analytics_query_failed`)"
                }),
            );
            responses.insert(
                "504".into(),
                json!({
                    "$ref": "#/components/responses/Error",
                    "description": "the query ran past the request timeout (`analytics_query_timeout`); narrow the time window"
                }),
            );
        }
        responses.insert(
            "default".into(),
            json!({"$ref": "#/components/responses/Error"}),
        );
        op.insert("responses".into(), Value::Object(responses));
        if self.public {
            op.insert("security".into(), json!([]));
        }
        Value::Object(op)
    }
}

/// The inclusive lower bound every ClickHouse-backed read shares. The format is
/// spelled out because a value outside it is a `400`, not a wider window: the
/// bound reaches a best-effort parse that turns anything it cannot read into
/// the epoch, so the control plane checks it first (#1192).
const SINCE: QueryParam = QueryParam::new(
    "since",
    "string",
    "inclusive lower bound, RFC 3339 (`2026-07-01T00:00:00Z`; percent-encode a `+` offset as \
     `%2B`), `YYYY-MM-DD hh:mm:ss[.fff]` or `YYYY-MM-DD`; defaults to 7 days ago, and a value \
     that does not parse is a 400",
);

/// The exclusive upper bound beside [`SINCE`], checked the same way.
const UNTIL: QueryParam = QueryParam::new(
    "until",
    "string",
    "exclusive upper bound, in the same forms as `since`; defaults to now, and a value that \
     does not parse is a 400",
);

/// What `GET /api/v1/config/export` narrows by.
const CONFIG_EXPORT_QUERY: &[QueryParam] = &[
    QueryParam::new(
        "org",
        "string",
        "the slug of one organization: export its rows only (and those no organization owns), \
         which is how a deployment whose organizations share a route or provider name is \
         exported (without it that deployment answers a 400 with the code \
         `config_export_needs_org`). A slug no organization has is a 400",
    ),
    QueryParam::new(
        "without_person_caps",
        "boolean",
        "`true` leaves caps on a person out, which name an account by its email, and counts \
         them in the trailing comment; defaults to `false`",
    ),
];

/// What a gateway sends on every snapshot poll (ADR-0038).
const SNAPSHOT_QUERY: &[QueryParam] = &[
    QueryParam::new(
        "version",
        "integer",
        "the config version the caller already holds; when it is current the answer is a \
         bodyless `304`",
    ),
    QueryParam::new(
        "formats",
        "string",
        "the snapshot formats the caller reads, as one format (`1`) or an inclusive range \
         (`1-2`); absent means format 1 only. The reply's `format` names the one served, and \
         a value that does not parse is a 400 `invalid_query`",
    ),
];

/// the `406` the snapshot answers when it cannot serve the caller's formats
const SNAPSHOT_406: &str = "error.code `snapshot_format_unsupported`: the control plane serves no snapshot format the caller reads, also when its `version` is current; or `snapshot_format_inexpressible`: the formats overlap, but the config holds a setting the older format cannot carry. `served` and `requested` give both ranges";

/// The org-wide access profile assignment list: narrow it to one subject.
const ASSIGNMENT_SUBJECT_QUERY: &[QueryParam] = &[
    QueryParam::new(
        "user_id",
        "string",
        "only the assignments this user holds; send at most one of `user_id` and `team_id`",
    ),
    QueryParam::new(
        "team_id",
        "string",
        "only the assignments this team holds; sending both is a 400 `invalid_field`",
    ),
];

/// The routes list's query string: what to embed beside each route.
const ROUTE_INCLUDE_QUERY: &[QueryParam] = &[QueryParam::new(
    "include",
    "string",
    "comma-separated extras to embed in each route: `targets`, `variants` and `complexity`; \
     omitted, the list carries none of them, and a value outside those three is a 400",
)];

/// The model delete's query string: whose model it removes (#1857).
const DELETE_MODEL_QUERY: &[QueryParam] = &[QueryParam::new(
    "org_id",
    "string",
    "the organization whose routes of that name are deleted, across its projects; may be \
     omitted only while one organization has the name, otherwise a 400",
)];

/// The access-profile list's query string: what to embed beside each profile.
const PROFILE_INCLUDE_QUERY: &[QueryParam] = &[QueryParam::new(
    "include",
    "string",
    "comma-separated extras to embed in each profile, as the detail returns them: `roles`, \
     `assignments` and `policy`; omitted, the list carries none, and a value outside those \
     three is a 400",
)];

/// The provider test's query string: the model to look up.
const PROVIDER_TEST_QUERY: &[QueryParam] = &[QueryParam::new(
    "model",
    "string",
    "an upstream model name to look up in the provider's catalogue; the response then carries \
     `model_listed`: true or false, or null when the provider has no catalogue to check",
)];

const PROVIDER_CACHE_PROBE_QUERY: &[QueryParam] = &[QueryParam::new(
    "model",
    "string",
    "the upstream model to ask for one token (required); the probe is a completion, so it costs a token",
)];

const PRICE_CATALOGUE_QUERY: &[QueryParam] = &[
    QueryParam::new(
        "q",
        "string",
        "a substring of the model id; every model when omitted",
    ),
    QueryParam::new(
        "limit",
        "integer",
        "at most this many entries (default and ceiling 200)",
    ),
];

/// The MCP call log's query string. The same keyset-paging problem as the
/// invocation log below, and the one #1412 names first: `next_cursor` comes
/// back in the body, and a caller who does not know to send it as `cursor`
/// sees page one forever.
const MCP_LOGS_QUERY: &[QueryParam] = &[
    SINCE,
    UNTIL,
    QueryParam::new(
        "server",
        "string",
        "exact MCP server id; omit for every server",
    ),
    QueryParam::new("tool", "string", "exact tool name; omit for every tool"),
    QueryParam::new(
        "transport",
        "string",
        "exact transport; omit for every transport",
    ),
    QueryParam::new(
        "status",
        "string",
        "exact call status; omit for every status",
    ),
    QueryParam::new("key", "string", "exact virtual key id; omit for every key"),
    QueryParam::new("user", "string", "exact user id; omit for every user"),
    QueryParam::new("limit", "integer", "page size; the server caps it"),
    QueryParam::new(
        "cursor",
        "string",
        "opaque keyset cursor; send back the previous page's `next_cursor` unchanged. one \
         whose timestamp does not parse is a 400",
    ),
];

/// The subject lookup behind the Log erasure picker (#3385).
const LOG_ERASURE_SUBJECT_QUERY: &[QueryParam] = &[
    QueryParam::required("kind", "string", "virtual_key, user or org"),
    QueryParam::new(
        "q",
        "string",
        "part of a name or email, case-insensitive; omit for the first few",
    ),
    QueryParam::new(
        "id",
        "string",
        "one subject by id, to name an id already in hand",
    ),
];

/// The audit log's query string. Keyset-paged like the two logs above — it was
/// equally undiscoverable — and it pages in both directions, which is the part
/// a caller cannot guess at all.
const AUDIT_LOG_QUERY: &[QueryParam] = &[
    QueryParam::new(
        "actor",
        "string",
        "exact actor user id; omit for every actor",
    ),
    QueryParam::new(
        "action",
        "string",
        "exact action name; omit for every action",
    ),
    QueryParam::new(
        "target_type",
        "string",
        "exact target type; omit for every type",
    ),
    QueryParam::new(
        "start_at",
        "string",
        "inclusive lower bound, RFC 3339; omit for no lower bound",
    ),
    QueryParam::new(
        "end_at",
        "string",
        "exclusive upper bound, RFC 3339; omit for no upper bound",
    ),
    QueryParam::new("limit", "integer", "page size; the server caps it"),
    QueryParam::new(
        "cursor",
        "string",
        "opaque keyset cursor; send back the previous page's `next_cursor`",
    ),
    QueryParam::new(
        "direction",
        "string",
        "which way to page from `cursor`: `forward` (older) or `backward` (newer)",
    ),
    QueryParam::new(
        "include_total",
        "boolean",
        "also count every matching row; off by default because the count is the expensive half",
    ),
];

/// The filters of the audit-log page read, minus the paging, plus the format:
/// an export streams the whole filtered log (#3051).
const AUDIT_EXPORT_QUERY: &[QueryParam] = &[
    QueryParam::required(
        "format",
        "string",
        "`csv` or `json`; the body streams as an attachment, newest first, with no row cap",
    ),
    QueryParam::new(
        "actor",
        "string",
        "exact actor user id; omit for every actor",
    ),
    QueryParam::new(
        "action",
        "string",
        "exact action name; omit for every action",
    ),
    QueryParam::new(
        "target_type",
        "string",
        "exact target type; omit for every type",
    ),
    QueryParam::new(
        "start_at",
        "string",
        "inclusive lower bound, RFC 3339; omit for no lower bound",
    ),
    QueryParam::new(
        "end_at",
        "string",
        "inclusive upper bound, RFC 3339; omit for no upper bound",
    ),
];

/// The scope selector both budget and rate-limit lists require. Not optional:
/// the handlers deserialize these as bare fields, so omitting either is a 400.
const SCOPE_QUERY: &[QueryParam] = &[
    QueryParam::required(
        "scope_type",
        "string",
        "what the limit is attached to: `org`, `team`, `project`, `virtual_key`, `business_unit` or `customer`",
    ),
    QueryParam::required("scope_id", "string", "uuid of that scope"),
];

/// The time window the analytics, health and usage summaries share.
const WINDOW_QUERY: &[QueryParam] = &[SINCE, UNTIL];

/// what the users list can be widened by (#2804)
const USERS_QUERY: &[QueryParam] = &[QueryParam::new(
    "include_unassigned",
    "boolean",
    "also list the accounts that hold no membership anywhere, such as the superadmin \
     `rolter-seed --admin-email` creates. Only a superadmin is shown them; for any other \
     caller the flag changes nothing. Defaults to false",
)];

const SAVED_VIEW_QUERY: &[QueryParam] = &[QueryParam::new(
    "surface",
    "string",
    "only presets of one screen: llm_logs or dashboard",
)];

/// The row filters the dashboard's reads and the invocation log share, with
/// the same shapes on both (#2453). Each narrows inside the caller's row
/// visibility and an omitted one applies no filter.
const MODEL_FILTER: QueryParam =
    QueryParam::new("model", "string", "exact model name; omit for every model");
const KEY_FILTER: QueryParam =
    QueryParam::new("key", "string", "exact virtual key id; omit for every key");
const BUSINESS_UNIT_FILTER: QueryParam = QueryParam::new(
    "business_unit",
    "string",
    "comma-separated business unit ids; omit for every unit",
);
const CUSTOMER_FILTER: QueryParam = QueryParam::new(
    "customer",
    "string",
    "comma-separated customer ids; omit for every customer",
);

const PROJECT_FILTER: QueryParam = QueryParam::new(
    "project",
    "string",
    "exact project id; omit for every project the caller may read",
);

/// The dashboard's summary and by-model reads: the window plus the row filters.
const DASHBOARD_QUERY: &[QueryParam] = &[
    SINCE,
    UNTIL,
    MODEL_FILTER,
    KEY_FILTER,
    BUSINESS_UNIT_FILTER,
    CUSTOMER_FILTER,
    PROJECT_FILTER,
];

/// The savings advisor's window, scope and thresholds (#1581).
const SAVINGS_QUERY: &[QueryParam] = &[
    SINCE,
    UNTIL,
    QueryParam::new(
        "org",
        "string",
        "exact org id; omit for every org the caller may read",
    ),
    PROJECT_FILTER,
    QueryParam::new(
        "route",
        "string",
        "exact route (its public model name); omit for every route",
    ),
    KEY_FILTER,
    QueryParam::new(
        "max_prompt_tokens",
        "integer",
        "a request with a longer prompt is not substitutable; defaults to 4000",
    ),
    QueryParam::new(
        "max_completion_tokens",
        "integer",
        "a request with a longer output is not substitutable; defaults to 1000",
    ),
    QueryParam::new(
        "output_margin",
        "number",
        "how much longer a candidate's answers are assumed to run at the high end of the \
         range, as a fraction; defaults to 0.25 and is clamped to 0..4",
    ),
];

/// The window plus the bucket only the timeseries endpoint reads, and the
/// dashboard's row filters.
const TIMESERIES_QUERY: &[QueryParam] = &[
    SINCE,
    UNTIL,
    QueryParam::new(
        "bucket",
        "string",
        "time bucket: `hour`, `day` (the default), `week` or `month`; one that cuts the \
         window into more than 20000 points is a 400",
    ),
    MODEL_FILTER,
    KEY_FILTER,
    BUSINESS_UNIT_FILTER,
    CUSTOMER_FILTER,
    PROJECT_FILTER,
];

/// The health timeline's window plus the bucket it cuts it into.
const HEALTH_TIMELINE_QUERY: &[QueryParam] = &[
    SINCE,
    UNTIL,
    QueryParam::new(
        "bucket",
        "string",
        "time bucket: `hour` (the default), `day`, `week` or `month`; one that cuts the window \
         into more than 20000 points is a 400",
    ),
];

/// The uptime endpoint's window plus the target it measures against.
const UPTIME_QUERY: &[QueryParam] = &[
    SINCE,
    UNTIL,
    QueryParam::new(
        "sla",
        "number",
        "uptime target to compare against, as a fraction (e.g. 0.999)",
    ),
];

/// The alert history's filters.
const ALERT_HISTORY_QUERY: &[QueryParam] = &[
    QueryParam::new(
        "rule_id",
        "string",
        "exact alert rule id; omit for every rule",
    ),
    QueryParam::new(
        "state",
        "string",
        "`firing` or `resolved`; omit for both",
    ),
    QueryParam::new(
        "delivery_status",
        "string",
        "`pending`, `delivered`, `failed` or `skipped`; omit for all",
    ),
    QueryParam::new(
        "before",
        "string",
        "keyset cursor `<sent_at>,<id>` of the last row of the previous page; returns the rows strictly older",
    ),
    QueryParam::new("limit", "integer", "page size; the server caps it"),
];

/// The label index's filters. Every one narrows; none is required.
const LABEL_QUERY: &[QueryParam] = &[
    QueryParam::new(
        "subject_type",
        "string",
        "what the label is attached to; omit for every kind",
    ),
    QueryParam::new(
        "subject_id",
        "string",
        "exact subject id; omit for every subject",
    ),
    QueryParam::new("key", "string", "exact label key; omit for every key"),
    QueryParam::new("value", "string", "exact label value; omit for every value"),
    QueryParam::new(
        "source",
        "string",
        "how the label was set; omit for every source",
    ),
];

/// The model-label listing: [`LABEL_QUERY`] plus the organization whose models
/// to list the labels of (#3476).
const MODEL_LABEL_QUERY: &[QueryParam] = &[
    QueryParam::new(
        "subject_type",
        "string",
        "what the label is attached to; omit for every kind",
    ),
    QueryParam::new(
        "subject_id",
        "string",
        "exact subject id; omit for every subject",
    ),
    QueryParam::new("key", "string", "exact label key; omit for every key"),
    QueryParam::new("value", "string", "exact label value; omit for every value"),
    QueryParam::new(
        "source",
        "string",
        "how the label was set; omit for every source",
    ),
    QueryParam::new(
        "org_id",
        "string",
        "the labels that apply to this organization's models: its own, and each \
         deployment-wide one whose model, source and key it has not set itself; omit for \
         every model label",
    ),
];

/// The RBAC matrix's optional org filter.
const MATRIX_QUERY: &[QueryParam] = &[QueryParam::new(
    "org_id",
    "string",
    "resolve against this organization; omit for the deployment-wide matrix",
)];

/// The scope an effective-permission lookup is resolved against.
const EFFECTIVE_QUERY: &[QueryParam] = &[
    QueryParam::new("org_id", "string", "organization to resolve against"),
    QueryParam::new("team_id", "string", "team to resolve against"),
    QueryParam::new("project_id", "string", "project to resolve against"),
];

/// The attribution report's window plus the two knobs that change which rows
/// come back at all — not merely how many.
const ATTRIBUTION_QUERY: &[QueryParam] = &[
    SINCE,
    UNTIL,
    QueryParam::new(
        "dimension",
        "string",
        "group by `business_unit` (default) or `customer`",
    ),
    QueryParam::new(
        "include_unattributed",
        "boolean",
        "also return rows the key left unattributed; off by default so a chargeback report is not skewed by an empty bucket",
    ),
];

/// The invocation log's query string. Written down because it is keyset-paged:
/// a caller that does not know to send back `next_cursor` as `cursor` cannot
/// reach the second page at all (#1394).
const INVOCATIONS_QUERY: &[QueryParam] = &[
    SINCE,
    UNTIL,
    MODEL_FILTER,
    KEY_FILTER,
    BUSINESS_UNIT_FILTER,
    CUSTOMER_FILTER,
    PROJECT_FILTER,
    QueryParam::new("status", "string", "all|error|success; defaults to all"),
    QueryParam::new(
        "request_id",
        "string",
        "exact request id — the one the gateway returned in `x-request-id`, or in \
         the header `request_id_header` names; with no `since`, searches every \
         retained row rather than the last 7 days",
    ),
    QueryParam::new(
        "trace_id",
        "string",
        "exact W3C trace id; with no `since`, searches every retained row",
    ),
    QueryParam::new(
        "unpriced",
        "boolean",
        "`true` returns only requests recorded as unpriced; omit for every request",
    ),
    QueryParam::new(
        "withheld",
        "boolean",
        "`true` returns only requests whose response a policy withheld after the provider billed \
         them; omit for every request",
    ),
    QueryParam::new(
        "usage_unknown",
        "boolean",
        "`true` returns only requests the upstream answered without reporting usage, so their \
         tokens and cost are unknown; omit for every request",
    ),
    QueryParam::new(
        "guardrail",
        "string",
        "only requests a guardrail acted on, named as the guardrails rollup does: \
         `blocked:<rule>`, `redacted:<rule>`, `annotated:<rule>`, `provider_blocked:<provider>`, \
         `provider_transformed:<provider>` (the bare forms for unnamed rows); omit for every request",
    ),
    QueryParam::new(
        "plugin",
        "string",
        "only requests a plugin acted on, named as the plugins rollup does: \
         `blocked:<slug>`, `transformed:<slug>`, `annotated:<slug>` or `error:<slug>:<kind>`; \
         omit for every request",
    ),
    QueryParam::new(
        "outcome",
        "string",
        "only requests the gateway recorded with this outcome: `served`, `upstream_error`, \
         `withheld`, `cancelled`, `refused_budget`, `refused_rate_limit`, `refused_guardrail`, \
         `refused_plugin`, `refused_access`, `refused_invalid`, `refused`, or `unknown` for rows \
         written before the outcome was recorded; any other value is a 400; omit for every request",
    ),
    QueryParam::new("limit", "integer", "page size, 1..=200; defaults to 50"),
    QueryParam::new(
        "cursor",
        "string",
        "the preceding page's `next_cursor`, unchanged; omit for the first page. \
         paging is a keyset over `(ts, request_id)`, so rows logged between two \
         pages cannot repeat or hide a row. one whose timestamp does not parse \
         is a 400",
    ),
];

/// Stamp one tag across a group of operations, so the table reads as the
/// grouping the dashboard and the SDKs use rather than repeating a literal.
fn tagged(tag: &'static str, ops: Vec<Op>) -> Vec<Op> {
    ops.into_iter().map(|op| Op { tag, ..op }).collect()
}

/// Whether the document publishes `method path` (the OpenAPI path template).
/// The deprecation registry is held to this so it cannot name an operation that
/// does not exist, and the seed lint so a renamed route it names fails a test.
#[cfg(test)]
pub(crate) fn documents_operation(method: &str, path: &str) -> bool {
    operations()
        .iter()
        .any(|op| op.method.eq_ignore_ascii_case(method) && op.path == path)
}

/// Whether `components/schemas` has `schema` with a property `field`.
#[cfg(test)]
pub(crate) fn schema_has_property(schema: &str, field: &str) -> bool {
    schemas()
        .get(schema)
        .and_then(|s| s.get("properties"))
        .is_some_and(|p| p.get(field).is_some())
}

/// Every operation the control plane serves.
///
/// Grouped by tag in the order the reference renders them. A route registered
/// in a `router()` but absent here fails `every_registered_route_is_documented`.
fn operations() -> Vec<Op> {
    let mut ops = Vec::new();

    ops.extend(tagged(
        "system",
        vec![
            Op::get("/healthz", "controlHealthz", "Liveness probe").public(),
            Op::get(
                "/readyz",
                "readyz",
                "Readiness probe: database, migrations and KEK",
            )
            .public(),
            Op::get("/openapi.json", "openapiJson", "This OpenAPI document").public(),
            Op::get("/docs", "docs", "Interactive API reference").public(),
            Op::get(
                "/docs/scalar.js",
                "docsBundle",
                "Embedded Scalar bundle backing /docs",
            )
            .public(),
            Op::get("/api/v1/ping", "ping", "Round-trip check for the dashboard").public(),
            Op::get(
                "/api/v1/version",
                "getVersion",
                "Running version and any available update",
            ),
            Op::get(
                "/api/v1/config",
                "getConfig",
                "The assembled gateway configuration, with every secret redacted. Providers, routes and groups are limited to the organizations and projects the caller holds a role in; a superadmin sees all",
            ),
            Op::get(
                "/api/v1/config/problems",
                "getConfigProblems",
                "Configuration problems detected in the assembled config, and the providers, routes and file keys the gateway snapshot leaves out",
            )
            .ok(Payload::Ref("ConfigProblems")),
            Op::get(
                "/api/v1/config/export",
                "exportConfig",
                "The live configuration as an importable rolter.toml",
            )
            .query(CONFIG_EXPORT_QUERY),
            Op::get(
                "/api/v1/currency",
                "getCurrency",
                "Supported currencies and their conversion rates",
            ),
            Op::get(
                "/api/v1/provider-kinds",
                "getProviderKinds",
                "Provider kinds this build can talk to",
            ),
            Op::get("/api/v1/roles", "listRoles", "The built-in role catalog"),
            Op::get(
                "/api/v1/stability",
                "getStability",
                "Subsystems this build marks experimental; absence means stable",
            )
            .ok(Payload::List("SubsystemStability")),
            Op::get(
                "/api/v1/public-url",
                "getPublicUrl",
                "The control plane's public base URL, and whether it was configured",
            ),
            Op::post(
                "/api/v1/ui-events",
                "ingestUiEvent",
                "Record a dashboard telemetry event",
            ),
        ],
    ));

    ops.extend(tagged(
        "internal",
        vec![
            Op::get(
                "/internal/snapshot",
                "getSnapshot",
                "The config snapshot the gateway polls",
            )
            .query(SNAPSHOT_QUERY)
            .not_acceptable(SNAPSHOT_406),
            Op::post(
                "/internal/adaptive-telemetry",
                "ingestAdaptiveTelemetry",
                "Accept adaptive-routing samples pushed by a gateway",
            ),
            Op::post(
                "/internal/breaker-state",
                "ingestBreakerState",
                "Accept circuit-breaker state pushed by a gateway",
            ),
        ],
    ));

    ops.extend(tagged(
        "auth",
        vec![
            Op::post(
                "/api/v1/auth/login",
                "login",
                "Exchange email and password for a session token, for a \
                 second-factor challenge when the account has one armed, or \
                 for an enrolment challenge when its org requires one it lacks",
            )
            .public(),
            Op::post(
                "/api/v1/auth/mfa/verify",
                "verifyMfaChallenge",
                "Redeem a second-factor challenge for a session token",
            )
            .public(),
            Op::post(
                "/api/v1/auth/mfa/enroll",
                "beginSignInMfaEnrolment",
                "Issue a TOTP secret against an enrolment challenge (shown once)",
            )
            .needs_kek()
            .public(),
            Op::post(
                "/api/v1/auth/mfa/confirm",
                "confirmSignInMfaEnrolment",
                "Arm the factor against an enrolment challenge, returning a \
                 session token and recovery codes",
            )
            .public(),
            Op::post(
                "/api/v1/auth/logout",
                "logout",
                "Revoke the current session",
            )
            // idempotent: a missing or already-dead token is a quiet `204`
            .public(),
            Op::get(
                "/api/v1/auth/me",
                "authMe",
                "The account behind the current session, with `has_local_password` \
                 (false for an account that signs in through single sign-on only) and \
                 `gateway_base_url`, the saved public base URL of the gateway (null when \
                 none is saved), readable by every signed-in caller",
            ),
            Op::post(
                "/api/v1/auth/password",
                "changeMyPassword",
                "Change the calling account's own password: proves the current one \
                 (a wrong one counts as a failed sign-in and is throttled with it, \
                 `400` `invalid_field` on `current_password`), ends the account's \
                 other sessions and answers how many (any role)",
            )
            .body(Payload::Ref("ChangePassword"))
            .ok(Payload::Ref("PasswordChanged"))
            .conflict(NO_LOCAL_PASSWORD_409),
            Op::get(
                "/api/v1/auth/methods",
                "authMethods",
                "Login methods this deployment offers",
            )
            .public(),
            Op::get(
                "/api/v1/orgs/{org_id}/auth-policy",
                "getAuthPolicy",
                "Read an organization's authentication policy",
            ),
            Op::put(
                "/api/v1/orgs/{org_id}/auth-policy",
                "setAuthPolicy",
                "Replace an organization's authentication policy",
            ),
        ],
    ));

    ops.extend(tagged(
        "tenancy",
        vec![
            Op::get("/api/v1/orgs", "listOrgs", "List organizations").ok(Payload::List("Org")),
            Op::post("/api/v1/orgs", "createOrg", "Create an organization")
                .body(Payload::Ref("CreateOrg"))
                .ok(Payload::Ref("Org")),
            Op::delete("/api/v1/orgs/{id}", "deleteOrg", "Delete an organization"),
            Op::get(
                "/api/v1/orgs/{org_id}/audit-log",
                "listAuditLog",
                "Page an organization's audit log",
            )
            .query(AUDIT_LOG_QUERY),
            Op::get(
                "/api/v1/audit-log",
                "listDeploymentAuditLog",
                "Page the deployment-wide audit log (superadmin)",
            )
            .query(AUDIT_LOG_QUERY),
            Op::get(
                "/api/v1/orgs/{org_id}/audit-log/export",
                "exportAuditLog",
                "Stream an organization's filtered audit log as CSV or JSON",
            )
            .query(AUDIT_EXPORT_QUERY),
            Op::get(
                "/api/v1/audit-log/export",
                "exportDeploymentAuditLog",
                "Stream the deployment-wide audit log as CSV or JSON (superadmin)",
            )
            .query(AUDIT_EXPORT_QUERY),
            Op::get(
                "/api/v1/orgs/{org_id}/teams",
                "listTeams",
                "List an organization's teams",
            )
            .ok(Payload::List("Team")),
            Op::post("/api/v1/orgs/{org_id}/teams", "createTeam", "Create a team")
                .body(Payload::Ref("CreateTeam"))
                .ok(Payload::Ref("Team")),
            Op::put("/api/v1/teams/{id}", "updateTeam", "Rename a team")
                .body(Payload::Ref("UpdateTeam"))
                .ok(Payload::Ref("Team"))
                .conflict("a team of the organization already has that name"),
            Op::delete("/api/v1/teams/{id}", "deleteTeam", "Delete a team"),
            Op::get(
                "/api/v1/teams/{team_id}/projects",
                "listProjects",
                "List a team's projects",
            )
            .ok(Payload::List("Project")),
            Op::post(
                "/api/v1/teams/{team_id}/projects",
                "createProject",
                "Create a project",
            )
            .body(Payload::Ref("CreateProject"))
            .ok(Payload::Ref("Project")),
            Op::get(
                "/api/v1/orgs/{org_id}/projects",
                "listOrgProjects",
                "List every project in an organization, across all of its teams",
            )
            .ok(Payload::List("OrgProject")),
            Op::delete("/api/v1/projects/{id}", "deleteProject", "Delete a project"),
            Op::get(
                "/api/v1/projects/{id}/settings",
                "getProjectSettings",
                "A project's settings: who may read its captured payloads",
            )
            .ok(Payload::Ref("ProjectSettings")),
            Op::put(
                "/api/v1/projects/{id}/settings",
                "updateProjectSettings",
                "Change a project's settings (project admin)",
            )
            .body(Payload::Ref("ProjectSettings"))
            .ok(Payload::Ref("ProjectSettings")),
            Op::get(
                "/api/v1/orgs/{org_id}/business-units",
                "listBusinessUnits",
                "List business units",
            )
            .ok(Payload::List("BusinessUnitListing")),
            Op::post(
                "/api/v1/orgs/{org_id}/business-units",
                "createBusinessUnit",
                "Create a business unit",
            )
            .body(Payload::Ref("CreateBusinessUnit"))
            .ok(Payload::Ref("BusinessUnit")),
            Op::put(
                "/api/v1/business-units/{id}",
                "updateBusinessUnit",
                "Update a business unit",
            )
            .body(Payload::Ref("UpdateBusinessUnit"))
            .ok(Payload::Ref("BusinessUnit")),
            Op::delete(
                "/api/v1/business-units/{id}",
                "deleteBusinessUnit",
                "Delete a business unit",
            ),
            Op::get(
                "/api/v1/orgs/{org_id}/customers",
                "listCustomers",
                "List customers",
            )
            .ok(Payload::List("CustomerListing")),
            Op::post(
                "/api/v1/orgs/{org_id}/customers",
                "createCustomer",
                "Create a customer",
            )
            .body(Payload::Ref("CreateCustomer"))
            .ok(Payload::Ref("Customer")),
            Op::put(
                "/api/v1/customers/{id}",
                "updateCustomer",
                "Update a customer",
            )
            .body(Payload::Ref("UpdateCustomer"))
            .ok(Payload::Ref("Customer")),
            Op::delete(
                "/api/v1/customers/{id}",
                "deleteCustomer",
                "Delete a customer",
            ),
        ],
    ));

    ops.extend(tagged(
        "users",
        vec![
            Op::get(
                "/api/v1/orgs/{org_id}/users",
                "listUsers",
                "List accounts with a membership in this organization",
            )
            .query(USERS_QUERY)
            .ok(Payload::List("User")),
            Op::post(
                "/api/v1/orgs/{org_id}/users",
                "createUser",
                "Create an account and grant it a role here",
            )
            .body(Payload::Ref("CreateUser"))
            .ok(Payload::Ref("CreatedUser")),
            Op::put("/api/v1/users/{id}", "updateUser", "Edit a global account")
                .body(Payload::Ref("UpdateUser"))
                .ok(Payload::Ref("User"))
                .conflict(LAST_SUPERADMIN_409),
            Op::put(
                "/api/v1/users/{id}/deployment-role",
                "setUserDeploymentRole",
                "Give an account a deployment role (operator or security auditor), or clear it (superadmin)",
            )
            .body(Payload::Ref("SetDeploymentRole"))
            .ok(Payload::Ref("User")),
            Op::delete("/api/v1/users/{id}", "deleteUser", "Delete an account")
                .conflict(LAST_SUPERADMIN_409),
            Op::get(
                "/api/v1/users",
                "listDeploymentUsers",
                "List every account in the deployment, ordered by e-mail (superadmin)",
            )
            .ok(Payload::List("User")),
            Op::get(
                "/api/v1/orgs/{org_id}/memberships",
                "listMemberships",
                "List role grants in this organization",
            )
            .ok(Payload::List("Membership")),
            Op::post(
                "/api/v1/orgs/{org_id}/memberships",
                "createMembership",
                "Grant a role at a scope",
            )
            .body(Payload::Ref("CreateMembership"))
            .ok(Payload::Ref("Membership")),
            Op::put(
                "/api/v1/memberships/{id}",
                "updateMembership",
                "Change a role grant's role in one request",
            )
            .body(Payload::Ref("UpdateMembership"))
            .ok(Payload::Ref("Membership"))
            .conflict(LAST_ORG_ADMIN_409),
            Op::delete(
                "/api/v1/memberships/{id}",
                "deleteMembership",
                "Revoke a role grant",
            )
            .conflict(LAST_ORG_ADMIN_409),
            Op::post(
                "/api/v1/orgs/{org_id}/users/{id}/password-reset",
                "issuePasswordReset",
                "Issue a one-time password reset link for a member and end their sessions",
            )
            .body(Payload::Empty)
            .ok(Payload::Ref("CreatedPasswordReset")),
        ],
    ));

    ops.extend(tagged(
        "access-control",
        vec![
            Op::get(
                "/api/v1/rbac/matrix",
                "getRbacMatrix",
                "The capability matrix every guard enforces",
            )
            .query(MATRIX_QUERY),
            Op::get(
                "/api/v1/rbac/effective",
                "getEffectiveRbac",
                "What the calling principal may do at a scope",
            )
            .query(EFFECTIVE_QUERY),
            Op::get(
                "/api/v1/orgs/{org_id}/custom-roles",
                "listCustomRoles",
                "List custom roles",
            ),
            Op::post(
                "/api/v1/orgs/{org_id}/custom-roles",
                "createCustomRole",
                "Create a custom role",
            ),
            Op::get(
                "/api/v1/custom-roles/{id}",
                "getCustomRole",
                "Read a custom role",
            ),
            Op::put(
                "/api/v1/custom-roles/{id}",
                "updateCustomRole",
                "Update a custom role",
            ),
            Op::delete(
                "/api/v1/custom-roles/{id}",
                "deleteCustomRole",
                "Delete a custom role",
            ),
            Op::get(
                "/api/v1/orgs/{org_id}/access-profiles",
                "listAccessProfiles",
                "List access profiles; `include` embeds each profile's roles, assignments and policy so a screen needs one request, not one per profile",
            )
            .query(PROFILE_INCLUDE_QUERY),
            Op::post(
                "/api/v1/orgs/{org_id}/access-profiles",
                "createAccessProfile",
                "Create an access profile",
            ),
            Op::get(
                "/api/v1/access-profiles/{id}",
                "getAccessProfile",
                "Read an access profile",
            ),
            Op::put(
                "/api/v1/access-profiles/{id}",
                "updateAccessProfile",
                "Update an access profile",
            ),
            Op::delete(
                "/api/v1/access-profiles/{id}",
                "deleteAccessProfile",
                "Delete an access profile",
            ),
            Op::put(
                "/api/v1/access-profiles/{id}/policy",
                "setAccessProfilePolicy",
                "Replace an access profile's policy",
            ),
            Op::get(
                "/api/v1/access-profiles/{id}/assignments",
                "listAccessProfileAssignments",
                "List an access profile's assignments",
            ),
            Op::get(
                "/api/v1/orgs/{org_id}/access-profile-assignments",
                "listSubjectAccessProfileAssignments",
                "List an org's access profile assignments, optionally for one user or team",
            )
            .query(ASSIGNMENT_SUBJECT_QUERY),
            Op::post(
                "/api/v1/access-profiles/{id}/assignments",
                "createAccessProfileAssignment",
                "Assign an access profile",
            ),
            Op::delete(
                "/api/v1/access-profile-assignments/{id}",
                "deleteAccessProfileAssignment",
                "Remove an access profile assignment",
            ),
        ],
    ));

    ops.extend(tagged(
        "providers",
        vec![
            Op::get(
                "/api/v1/orgs/{org_id}/providers",
                "listProviders",
                "List upstream providers",
            )
            .ok(Payload::List("Provider")),
            Op::post(
                "/api/v1/orgs/{org_id}/providers",
                "createProvider",
                "Register an upstream provider",
            )
            .needs_kek()
            .body(Payload::Ref("CreateProvider"))
            .ok(Payload::Ref("Provider")),
            Op::put(
                "/api/v1/providers/{id}",
                "updateProvider",
                "Update an upstream provider",
            )
            .needs_kek()
            .body(Payload::Ref("UpdateProvider"))
            .ok(Payload::Ref("Provider")),
            Op::delete(
                "/api/v1/providers/{id}",
                "deleteProvider",
                "Delete an upstream provider",
            )
            .conflict(PROVIDER_IN_USE_409),
            Op::post(
                "/api/v1/providers/{id}/test",
                "testProvider",
                "Probe a provider's credentials and reachability; with `?model=`, also whether its catalogue lists that upstream model",
            )
            .query(PROVIDER_TEST_QUERY)
            .body(Payload::Empty),
            Op::post(
                "/api/v1/providers/{id}/probe-cache",
                "probeProviderCache",
                "Ask a provider for one token and record whether the answer's usage carries prompt-cache metrics (needs provider:update)",
            )
            .query(PROVIDER_CACHE_PROBE_QUERY)
            .body(Payload::Empty)
            .ok(Payload::Open),
            Op::get(
                "/api/v1/orgs/{org_id}/provider-cache-probes",
                "listProviderCacheProbes",
                "List the stored prompt-cache reporting verdicts of an org's providers; a provider with no entry was never probed",
            )
            .ok(Payload::Open),
            Op::get(
                "/api/v1/providers/{id}/models",
                "listProviderModels",
                "List the models a provider's upstream serves (needs provider:update, like the test)",
            )
            .ok(Payload::Open),
            Op::get(
                "/api/v1/orgs/{org_id}/provider-groups",
                "listProviderGroups",
                "List provider groups",
            )
            .ok(Payload::List("ProviderGroupView")),
            Op::post(
                "/api/v1/orgs/{org_id}/provider-groups",
                "createProviderGroup",
                "Create a provider group",
            )
            .body(Payload::Ref("CreateProviderGroup"))
            .ok(Payload::Ref("ProviderGroupView")),
            Op::put(
                "/api/v1/provider-groups/{id}",
                "updateProviderGroup",
                "Update a provider group",
            )
            .body(Payload::Ref("UpdateProviderGroup"))
            .ok(Payload::Ref("ProviderGroupView")),
            Op::delete(
                "/api/v1/provider-groups/{id}",
                "deleteProviderGroup",
                "Delete a provider group",
            ),
        ],
    ));

    ops.extend(tagged(
        "routes",
        vec![
            Op::get(
                "/api/v1/models",
                "listModels",
                "List the public model names served in the caller's organizations and projects (all for a superadmin), with each entry's targets and owning organization",
            )
            .ok(Payload::List("EffectiveModel")),
            Op::delete(
                "/api/v1/models/{model}",
                "deleteModel",
                "Delete the routes publishing a model name in one organization; `org_id` names it, and is required once two organizations share the name",
            )
            .query(DELETE_MODEL_QUERY),
            Op::get(
                "/api/v1/projects/{project_id}/routes",
                "listRoutes",
                "List a project's routes; `include` embeds each route's targets, variants and complexity policy so a screen needs one request, not one per route",
            )
            .query(ROUTE_INCLUDE_QUERY)
            .ok(Payload::List("Route")),
            Op::post(
                "/api/v1/projects/{project_id}/routes",
                "createRoute",
                "Create a route",
            )
            .body(Payload::Ref("CreateRoute"))
            .ok(Payload::Ref("Route")),
            Op::put(
                "/api/v1/routes/{id}",
                "updateRoute",
                "Change a route's enabled flag, strategy or targets; an omitted field keeps its value",
            )
            .body(Payload::Ref("UpdateRoute"))
            .ok(Payload::Ref("Route")),
            Op::delete("/api/v1/routes/{id}", "deleteRoute", "Delete a route"),
            Op::put(
                "/api/v1/routes/{id}/params",
                "setRouteParams",
                "Replace a route's default inference params",
            )
            .ok(Payload::Ref("Route")),
            Op::put(
                "/api/v1/routes/{id}/advanced",
                "setRouteAdvanced",
                "Replace a route's catalog metadata and execution policy (headers, timeout, retries, output-token cap, visibility, guardrails)",
            )
            .ok(Payload::Ref("Route")),
            Op::get(
                "/api/v1/routes/{id}/variants",
                "listRouteVariants",
                "List a route's weighted variants in fallback order, each with its targets in routing order",
            )
            .ok(Payload::List("RouteVariant")),
            Op::put(
                "/api/v1/routes/{id}/variants",
                "setRouteVariants",
                "Replace a route's weighted variants; an empty list puts the route back on its own targets",
            )
            .body(Payload::Ref("SetRouteVariants"))
            .ok(Payload::List("RouteVariant")),
            Op::get(
                "/api/v1/routes/{id}/complexity",
                "getRouteComplexity",
                "Read a route's complexity-routing tiers",
            ),
            Op::put(
                "/api/v1/routes/{id}/complexity",
                "setRouteComplexity",
                "Replace a route's complexity-routing tiers",
            ),
            Op::delete(
                "/api/v1/routes/{id}/complexity",
                "disableRouteComplexity",
                "Disable complexity routing while preserving ordinary route parameters",
            ).ok(Payload::Ref("Route")),
            Op::get(
                "/api/v1/routes/{route_id}/targets",
                "listRouteTargets",
                "List a route's upstream targets",
            )
            .ok(Payload::List("RouteTarget")),
            Op::post(
                "/api/v1/routes/{route_id}/targets",
                "createRouteTarget",
                "Add an upstream target to a route",
            )
            .body(Payload::Ref("CreateRouteTarget"))
            .ok(Payload::Ref("RouteTarget")),
            Op::delete(
                "/api/v1/route-targets/{id}",
                "deleteRouteTarget",
                "Remove an upstream target from a route",
            )
            .conflict(
                "removing the target would leave the route only targets that set `takes_over` \
                 to dial (the variants' targets when it has variants, its own otherwise), which \
                 could never be dialled first (`all_targets_restricted`)",
            ),
        ],
    ));

    ops.extend(tagged(
        "virtual-keys",
        vec![
            Op::get(
                "/api/v1/projects/{project_id}/virtual-keys",
                "listVirtualKeys",
                "List a project's virtual keys",
            )
            .ok(Payload::List("VirtualKey")),
            Op::get(
                "/api/v1/orgs/{org_id}/virtual-keys",
                "listOrgVirtualKeys",
                "List the virtual keys of every project of an organization the caller may read",
            )
            .ok(Payload::List("OrgVirtualKey")),
            Op::post(
                "/api/v1/projects/{project_id}/virtual-keys",
                "createVirtualKey",
                "Mint a virtual key; the plaintext is returned once",
            )
            .body(Payload::Ref("CreateVirtualKey"))
            .ok(Payload::Ref("CreatedVirtualKey")),
            Op::put(
                "/api/v1/virtual-keys/{id}",
                "setVirtualKeyDisabled",
                "Disable or re-enable a virtual key",
            )
            .body(Payload::Ref("SetVirtualKeyDisabled"))
            .ok(Payload::Ref("VirtualKey")),
            Op::delete(
                "/api/v1/virtual-keys/{id}",
                "deleteVirtualKey",
                "Delete a virtual key",
            ),
            Op::put(
                "/api/v1/virtual-keys/{id}/providers",
                "setVirtualKeyProviders",
                "Restrict which providers a key may reach",
            )
            .body(Payload::Ref("SetVirtualKeyProviders"))
            .ok(Payload::Ref("VirtualKey")),
            Op::put(
                "/api/v1/virtual-keys/{id}/attribution",
                "setVirtualKeyAttribution",
                "Point a key's spend at a business unit or customer",
            )
            .body(Payload::Ref("SetVirtualKeyAttribution"))
            .ok(Payload::Ref("VirtualKey")),
            Op::put(
                "/api/v1/virtual-keys/{id}/cache",
                "setVirtualKeyCache",
                "Override a key's response-cache decision",
            )
            .body(Payload::Ref("SetVirtualKeyCache"))
            .ok(Payload::Ref("VirtualKey")),
            Op::post(
                "/api/v1/virtual-keys/{id}/rotate",
                "rotateVirtualKey",
                "Issue a new secret for a shared key, optionally keeping the old one valid for an overlap",
            )
            .body(Payload::Ref("RotateVirtualKey"))
            .ok(Payload::Ref("CreatedVirtualKey"))
            .conflict("the key is personal: the member who minted it rotates it from Account"),
            Op::delete(
                "/api/v1/virtual-keys/{id}/previous-secret",
                "revokeVirtualKeyPreviousSecret",
                "End a rotation's overlap now so the old secret stops authenticating",
            )
            .ok(Payload::Ref("VirtualKey"))
            .conflict("the key is personal: the member who minted it manages its secrets"),
        ],
    ));

    ops.extend(tagged(
        "me",
        vec![
            Op::get(
                "/api/v1/me/virtual-keys",
                "listMyVirtualKeys",
                "List the calling account's own virtual keys",
            ),
            Op::post(
                "/api/v1/me/projects/{project_id}/virtual-keys",
                "mintMyVirtualKey",
                "Mint a virtual key for the calling account",
            ),
            Op::post(
                "/api/v1/me/projects/{project_id}/playground-key",
                "mintPlaygroundKey",
                "Mint a short-lived key the server scopes to the project's routes",
            ),
            Op::post(
                "/api/v1/me/virtual-keys/{id}/rotate",
                "rotateMyVirtualKey",
                "Rotate one of the calling account's keys",
            ),
            Op::delete(
                "/api/v1/me/virtual-keys/{id}",
                "deleteMyVirtualKey",
                "Delete one of the calling account's keys",
            ),
            Op::get(
                "/api/v1/me/usage",
                "getMyUsage",
                "Spend and usage for the calling account's keys",
            )
            .clickhouse_read()
            .query(WINDOW_QUERY),
            Op::get(
                "/api/v1/me/budgets",
                "getMyBudgets",
                "The per-user budgets and rate limits that count the calling account's keys, with the spend so far",
            ),
            Op::patch(
                "/api/v1/me/profile",
                "updateMyProfile",
                "Change the calling account's own display name and bio (any role)",
            ),
            Op::get(
                "/api/v1/me/preferences",
                "getMyPreferences",
                "The calling account's preferences, with the default scope it can still read",
            ),
            Op::put(
                "/api/v1/me/preferences",
                "putMyPreferences",
                "Replace the calling account's whole preferences document (any role)",
            ),
            Op::get(
                "/api/v1/me/saved-views",
                "listMySavedViews",
                "The calling account's saved filter presets, with what it can no longer read (any role)",
            )
            .query(SAVED_VIEW_QUERY),
            Op::post(
                "/api/v1/me/saved-views",
                "createMySavedView",
                "Save a named filter preset for LLM Logs or the Dashboard (any role)",
            ),
            Op::get(
                "/api/v1/me/saved-views/{id}",
                "getMySavedView",
                "One of the calling account's saved filter presets",
            ),
            Op::patch(
                "/api/v1/me/saved-views/{id}",
                "updateMySavedView",
                "Rename a saved filter preset and/or replace its filters",
            ),
            Op::delete(
                "/api/v1/me/saved-views/{id}",
                "deleteMySavedView",
                "Delete a saved filter preset",
            ),
            Op::get(
                "/api/v1/orgs/{org_id}/mfa-status",
                "getOrgMfaStatus",
                "Which members of an organization have an armed second factor",
            ),
            Op::get(
                "/api/v1/me/mfa",
                "getMyMfa",
                "Second-factor state and policy for the calling account",
            ),
            Op::post(
                "/api/v1/me/mfa/enroll",
                "beginMyMfaEnrolment",
                "Issue a TOTP secret for the calling account (shown once)",
            )
            .needs_kek(),
            Op::post(
                "/api/v1/me/mfa/confirm",
                "confirmMyMfaEnrolment",
                "Arm the second factor with a code, returning recovery codes",
            ),
            Op::post(
                "/api/v1/me/mfa/recovery-codes",
                "regenerateMyRecoveryCodes",
                "Replace the calling account's recovery codes",
            ),
            Op::delete(
                "/api/v1/me/mfa",
                "disableMyMfa",
                "Remove the calling account's second factor",
            ),
        ],
    ));

    ops.extend(tagged(
        "governance",
        vec![
            Op::get("/api/v1/budgets", "listBudgets", "List spend budgets")
                .query(SCOPE_QUERY)
                .ok(Payload::List("Budget")),
            Op::get(
                "/api/v1/budgets/spend",
                "getBudgetSpend",
                "Spend so far against each budget on a scope, from the gateway's Redis counters",
            )
            .query(SCOPE_QUERY)
            .ok(Payload::Ref("BudgetSpendReport")),
            Op::get(
                "/api/v1/orgs/{org_id}/budgets",
                "listOrgBudgets",
                "List every spend budget in this organization the caller may read, at any scope; a key's budget carries `scope_name`",
            )
            .ok(Payload::List("Budget")),
            Op::get(
                "/api/v1/orgs/{org_id}/budgets/spend",
                "getOrgBudgetSpend",
                "Spend so far against every budget in this organization the caller may read, from the gateway's Redis counters",
            )
            .ok(Payload::Ref("BudgetSpendReport")),
            Op::post("/api/v1/budgets", "createBudget", "Create a spend budget")
                .body(Payload::Ref("CreateBudget"))
                .ok(Payload::Ref("Budget")),
            Op::patch(
                "/api/v1/budgets/{id}",
                "updateBudget",
                "Change a spend budget's limit, period or unpriced policy in place",
            )
            .body(Payload::Ref("UpdateBudget"))
            .ok(Payload::Ref("Budget")),
            Op::delete(
                "/api/v1/budgets/{id}",
                "deleteBudget",
                "Delete a spend budget",
            ),
            Op::get("/api/v1/rate-limits", "listRateLimits", "List rate limits")
                .query(SCOPE_QUERY)
                .ok(Payload::List("RateLimit")),
            Op::get(
                "/api/v1/orgs/{org_id}/rate-limits",
                "listOrgRateLimits",
                "List every rate limit in this organization the caller may read, at any scope",
            )
            .ok(Payload::List("RateLimit")),
            Op::post(
                "/api/v1/rate-limits",
                "createRateLimit",
                "Create a rate limit",
            )
            .body(Payload::Ref("CreateRateLimit"))
            .ok(Payload::Ref("RateLimit")),
            Op::patch(
                "/api/v1/rate-limits/{id}",
                "updateRateLimit",
                "Change a rate limit's rpm or tpm cap in place",
            )
            .body(Payload::Ref("UpdateRateLimit"))
            .ok(Payload::Ref("RateLimit")),
            Op::delete(
                "/api/v1/rate-limits/{id}",
                "deleteRateLimit",
                "Delete a rate limit",
            ),
            Op::get(
                "/api/v1/model-prices",
                "listModelPrices",
                "List per-model token prices",
            )
            .ok(Payload::List("ModelPrice")),
            Op::put(
                "/api/v1/model-prices",
                "upsertModelPrice",
                "Create or replace a model's price",
            )
            .body(Payload::Ref("UpsertModelPrice"))
            .ok(Payload::Ref("ModelPrice")),
            Op::delete(
                "/api/v1/model-prices/{model}",
                "deleteModelPrice",
                "Delete a model's price",
            ),
            Op::get(
                "/api/v1/model-prices/catalogue",
                "listPriceCatalogue",
                "Search the vendored price catalogue the importer reads (offline), with its source, licence and snapshot date",
            )
            .query(PRICE_CATALOGUE_QUERY)
            .ok(Payload::Open),
            Op::post(
                "/api/v1/model-prices/import",
                "importModelPrices",
                "Import the named models' prices from the vendored catalogue; an operator-set price is never replaced",
            )
            .body(Payload::Open)
            .ok(Payload::Open),
        ],
    ));

    ops.extend(tagged(
        "prompt-templates",
        vec![
            Op::get(
                "/api/v1/orgs/{org_id}/prompt-templates",
                "listPromptTemplates",
                "List prompt templates",
            ),
            Op::post(
                "/api/v1/orgs/{org_id}/prompt-templates",
                "createPromptTemplate",
                "Create a prompt template",
            ),
            Op::put(
                "/api/v1/prompt-templates/{id}",
                "updatePromptTemplate",
                "Update a prompt template",
            ),
            Op::delete(
                "/api/v1/prompt-templates/{id}",
                "deletePromptTemplate",
                "Delete a prompt template",
            ),
            Op::get(
                "/api/v1/prompt-templates/{id}/versions",
                "listPromptTemplateVersions",
                "List a template's versions",
            ),
            Op::post(
                "/api/v1/prompt-templates/{id}/versions",
                "createPromptTemplateVersion",
                "Draft a new template version",
            ),
            Op::put(
                "/api/v1/prompt-templates/{id}/publish",
                "publishPromptTemplateVersion",
                "Publish a template version",
            ),
            Op::put(
                "/api/v1/prompt-templates/{id}/rollback",
                "rollbackPromptTemplateVersion",
                "Roll a template back to an earlier version",
            ),
            Op::get(
                "/api/v1/prompt-templates/{id}/versions/{version}/scopes",
                "listPromptTemplateScopes",
                "List the scopes a template version is bound to",
            ),
            Op::put(
                "/api/v1/prompt-templates/{id}/versions/{version}/scopes",
                "setPromptTemplateScopes",
                "Replace the scopes a template version is bound to",
            ),
        ],
    ));

    ops.extend(tagged(
        "skills",
        vec![
            Op::get("/api/v1/orgs/{org_id}/skills", "listSkills", "List skills"),
            Op::post(
                "/api/v1/orgs/{org_id}/skills",
                "createSkill",
                "Create a skill",
            ),
            Op::get(
                "/api/v1/orgs/{org_id}/skills/resolve/{slug}",
                "resolvePublishedSkill",
                "Resolve a slug to its published skill version",
            ),
            Op::put("/api/v1/skills/{id}", "updateSkill", "Update a skill"),
            Op::delete("/api/v1/skills/{id}", "deleteSkill", "Delete a skill"),
            Op::get(
                "/api/v1/skills/{id}/versions",
                "listSkillVersions",
                "List a skill's versions",
            ),
            Op::post(
                "/api/v1/skills/{id}/versions",
                "createSkillVersion",
                "Draft a new skill version",
            ),
            Op::put(
                "/api/v1/skills/{id}/publish",
                "publishSkillVersion",
                "Publish a skill version",
            ),
            Op::put(
                "/api/v1/skills/{id}/rollback",
                "rollbackSkillVersion",
                "Roll a skill back to an earlier version",
            ),
        ],
    ));

    ops.extend(tagged(
        "guardrails",
        vec![
            Op::get(
                "/api/v1/guardrails/effective",
                "getEffectiveGuardrails",
                "Read the guardrail rules, semantic guardrail providers and policies the gateways run, each with where it comes from (config file or dashboard row)",
            ),
            Op::get(
                "/api/v1/guardrails/providers",
                "listGuardrailProviders",
                "List guardrail providers",
            ),
            Op::post(
                "/api/v1/guardrails/providers",
                "createGuardrailProvider",
                "Register a guardrail provider: a webhook, or a local classifier the gateways run in process",
            ),
            Op::put(
                "/api/v1/guardrails/providers/{id}",
                "updateGuardrailProvider",
                "Update a guardrail provider",
            ),
            Op::delete(
                "/api/v1/guardrails/providers/{id}",
                "deleteGuardrailProvider",
                "Delete a guardrail provider",
            )
            .conflict("a guardrail policy still lists the provider"),
            Op::get(
                "/api/v1/guardrails/policies",
                "listGuardrailPolicies",
                "List scoped semantic guardrail policies, narrowest scope first",
            ),
            Op::post(
                "/api/v1/guardrails/policies",
                "createGuardrailPolicy",
                "Create a guardrail policy scoped to the deployment, an org, a project, a route or a virtual key",
            )
            .conflict("a policy of that name, or an enabled policy for the same scope, already exists"),
            Op::put(
                "/api/v1/guardrails/policies/{id}",
                "updateGuardrailPolicy",
                "Replace a guardrail policy and its ordered provider list",
            )
            .conflict("a policy of that name, or an enabled policy for the same scope, already exists"),
            Op::delete(
                "/api/v1/guardrails/policies/{id}",
                "deleteGuardrailPolicy",
                "Delete a guardrail policy",
            ),
            Op::post(
                "/api/v1/guardrails/providers/test",
                "testGuardrailProvider",
                "Send a sample envelope to an unsaved guardrail provider (needs guardrail_provider:update)",
            ),
            Op::post(
                "/api/v1/guardrails/rules/test",
                "testGuardrailRule",
                "Run an unsaved guardrail rule against sample text (needs guardrail_rule:update)",
            ),
            Op::post(
                "/api/v1/guardrails/policy/test",
                "testGuardrailPolicy",
                "Run the whole guardrail policy, with an optional unsaved draft rule, against sample text (needs guardrail_rule:update)",
            ),
            Op::get(
                "/api/v1/guardrails/rules",
                "listGuardrailRules",
                "List guardrail rules",
            ),
            Op::post(
                "/api/v1/guardrails/rules",
                "createGuardrailRule",
                "Create a guardrail rule",
            ),
            Op::put(
                "/api/v1/guardrails/rules/{id}",
                "updateGuardrailRule",
                "Update a guardrail rule",
            ),
            Op::delete(
                "/api/v1/guardrails/rules/{id}",
                "deleteGuardrailRule",
                "Delete a guardrail rule",
            ),
        ],
    ));

    ops.extend(tagged(
        "labels",
        vec![
            Op::get(
                "/api/v1/orgs/{org_id}/labels",
                "listOrgLabels",
                "List labels on an org's providers, provider groups and routes",
            )
            .query(LABEL_QUERY),
            Op::post(
                "/api/v1/orgs/{org_id}/labels",
                "createOrgLabel",
                "Attach a custom label to a provider, provider group or route",
            ),
            Op::put(
                "/api/v1/orgs/{org_id}/labels/{id}",
                "updateOrgLabel",
                "Change a custom label's value",
            ),
            Op::delete(
                "/api/v1/orgs/{org_id}/labels/{id}",
                "deleteOrgLabel",
                "Remove a custom label",
            ),
            Op::get(
                "/api/v1/model-labels",
                "listModelLabels",
                "List model labels. A label carries `org_id`, the organization whose model it is \
                 on, or `null` for a deployment-wide one; an organization's labels are listed \
                 only to a caller who reaches that organization",
            )
            .query(MODEL_LABEL_QUERY),
            Op::post(
                "/api/v1/model-labels",
                "createModelLabel",
                "Attach a custom label to a model: `model`, `key`, an optional `value`, and an \
                 optional `org_id` naming the organization whose model it is (a 404 when that \
                 organization serves no route of that name); without `org_id` the label is \
                 deployment-wide. A label naming an organization takes that organization's \
                 `label:create`; a deployment-wide one is superadmin-only",
            ),
            Op::put(
                "/api/v1/model-labels/{id}",
                "updateModelLabel",
                "Change a custom model label's value. Takes `label:update` in the label's \
                 organization, or superadmin for a deployment-wide label",
            ),
            Op::delete(
                "/api/v1/model-labels/{id}",
                "deleteModelLabel",
                "Remove a custom model label. Takes `label:delete` in the label's organization, \
                 or superadmin for a deployment-wide label",
            ),
        ],
    ));

    ops.extend(tagged(
        "plugins",
        vec![
            Op::get(
                "/api/v1/orgs/{org_id}/plugins",
                "listPlugins",
                "List plugins",
            ),
            Op::post(
                "/api/v1/orgs/{org_id}/plugins",
                "createPlugin",
                "Install a plugin",
            ),
            Op::put("/api/v1/plugins/{id}", "updatePlugin", "Update a plugin"),
            Op::delete("/api/v1/plugins/{id}", "deletePlugin", "Remove a plugin"),
        ],
    ));

    ops.extend(tagged(
        "mcp",
        vec![
            Op::get(
                "/api/v1/orgs/{org_id}/mcp-servers",
                "listMcpServers",
                "List registered MCP servers",
            ),
            Op::post(
                "/api/v1/orgs/{org_id}/mcp-servers",
                "createMcpServer",
                "Register an MCP server",
            )
            .needs_kek(),
            Op::patch(
                "/api/v1/mcp-servers/{id}",
                "updateMcpServer",
                "Update a registered MCP server",
            )
            .needs_kek(),
            Op::delete(
                "/api/v1/mcp-servers/{id}",
                "deleteMcpServer",
                "Delete a registered MCP server",
            ),
            Op::put(
                "/api/v1/mcp-servers/{id}/auth",
                "setMcpServerAuth",
                "Set how rolter authenticates to an MCP server",
            )
            .needs_kek(),
            Op::post(
                "/api/v1/mcp-servers/{id}/discover",
                "discoverMcpServerTools",
                "Ask an MCP server for its tools with tools/list",
            ),
            Op::post(
                "/api/v1/mcp-servers/{id}/test",
                "testMcpServerConnection",
                "Run the MCP handshake against a server and report its status",
            ),
            Op::post(
                "/api/v1/mcp-servers/{id}/tools/call",
                "callMcpServerTool",
                "Run one of an MCP server's tools and return the raw exchange",
            ),
            Op::get(
                "/api/v1/mcp-servers/{id}/tools",
                "listMcpServerTools",
                "List an MCP server's tools with their input schemas",
            ),
            Op::get(
                "/api/v1/mcp-servers/{id}/oauth-client",
                "getMcpOauthClient",
                "Read an MCP server's OAuth client",
            ),
            Op::put(
                "/api/v1/mcp-servers/{id}/oauth-client",
                "setMcpOauthClient",
                "Replace an MCP server's OAuth client",
            )
            .needs_kek(),
            Op::post(
                "/api/v1/mcp-servers/{id}/oauth/authorize",
                "startMcpAuthorize",
                "Begin the MCP OAuth authorization flow",
            ),
            Op::get(
                "/auth/mcp/callback",
                "mcpOauthCallback",
                "OAuth redirect target for the MCP consent flow",
            )
            .public()
            .see_other(
                "sent to a browser (`Accept: text/html`): the dashboard's Auth Sessions screen, \
                 with `consent=completed&session=&server=` on success or \
                 `consent=failed&reason=` (plus `server=` once known) on a failure. Any other \
                 caller gets the JSON body or the error",
            ),
            Op::get(
                "/api/v1/orgs/{org_id}/mcp/library",
                "listMcpLibrary",
                "The catalog of installable MCP servers",
            ),
            Op::get(
                "/api/v1/orgs/{org_id}/mcp/settings",
                "getMcpSettings",
                "Read an organization's MCP settings",
            ),
            Op::put(
                "/api/v1/orgs/{org_id}/mcp/settings",
                "updateMcpSettings",
                "Replace an organization's MCP settings",
            ),
            Op::get(
                "/api/v1/orgs/{org_id}/mcp/tool-groups",
                "listMcpToolGroups",
                "List MCP tool groups",
            ),
            Op::post(
                "/api/v1/orgs/{org_id}/mcp/tool-groups",
                "createMcpToolGroup",
                "Create an MCP tool group",
            ),
            Op::put(
                "/api/v1/mcp/tool-groups/{id}",
                "updateMcpToolGroup",
                "Update an MCP tool group",
            ),
            Op::delete(
                "/api/v1/mcp/tool-groups/{id}",
                "deleteMcpToolGroup",
                "Delete an MCP tool group",
            ),
            Op::get(
                "/api/v1/orgs/{org_id}/mcp/tool-group-assignments",
                "listMcpToolGroupAssignments",
                "List the keys, projects and teams MCP tool groups are assigned to",
            ),
            Op::put(
                "/api/v1/mcp/tool-groups/{id}/assignments",
                "replaceMcpToolGroupAssignments",
                "Replace the keys, projects and teams an MCP tool group is assigned to",
            ),
            Op::get(
                "/api/v1/orgs/{org_id}/mcp/grants",
                "listMcpGrants",
                "List MCP consent grants",
            ),
            Op::delete(
                "/api/v1/mcp/grants/{id}",
                "revokeMcpGrant",
                "Revoke an MCP consent grant",
            )
            .ok(Payload::Open),
            Op::get(
                "/api/v1/orgs/{org_id}/mcp/sessions",
                "listMcpSessions",
                "List MCP OAuth sessions",
            ),
            Op::delete(
                "/api/v1/mcp/sessions/{id}",
                "revokeMcpSession",
                "Revoke an MCP OAuth session",
            )
            .ok(Payload::Open),
            Op::post(
                "/api/v1/mcp/sessions/{id}/exchange",
                "exchangeMcpSession",
                "Exchange an authorization code for MCP tokens",
            ),
            Op::post(
                "/api/v1/mcp/sessions/{id}/refresh",
                "refreshMcpSession",
                "Refresh an MCP session's tokens",
            ),
            Op::post(
                "/api/v1/mcp/events",
                "ingestMcpEvent",
                "Record an MCP tool-call event",
            ),
            Op::get(
                "/api/v1/mcp/logs",
                "listMcpLogs",
                "Page MCP tool-call events",
            )
            .clickhouse_read()
            .query(MCP_LOGS_QUERY),
            Op::get(
                "/api/v1/mcp/logs/summary",
                "getMcpLogSummary",
                "Aggregate MCP tool-call activity",
            )
            .clickhouse_read()
            .query(WINDOW_QUERY),
            Op::get(
                "/api/v1/mcp/logs/{event_id}",
                "getMcpLogEvent",
                "Read one MCP tool-call event",
            )
            .clickhouse_read(),
        ],
    ));

    ops.extend(tagged(
        "alerting",
        vec![
            Op::get(
                "/api/v1/alert-channels",
                "listAlertChannels",
                "List the alert channels the caller may use: their orgs' and the deployment's",
            ),
            Op::post(
                "/api/v1/alert-channels",
                "createAlertChannel",
                "Create an alert channel, owned by an org (org admin) or the deployment (superadmin)",
            )
            .needs_kek(),
            Op::put(
                "/api/v1/alert-channels/{id}",
                "updateAlertChannel",
                "Update an alert channel",
            )
            .needs_kek(),
            Op::delete(
                "/api/v1/alert-channels/{id}",
                "deleteAlertChannel",
                "Delete an alert channel",
            ),
            Op::get(
                "/api/v1/alert-rules",
                "listAlertRules",
                "List the alert rules the caller owns",
            ),
            Op::post(
                "/api/v1/alert-rules",
                "createAlertRule",
                "Create an alert rule scoped to an org, team, project, key, business unit or customer",
            ),
            Op::put(
                "/api/v1/alert-rules/{id}",
                "updateAlertRule",
                "Update an alert rule",
            ),
            Op::delete(
                "/api/v1/alert-rules/{id}",
                "deleteAlertRule",
                "Delete an alert rule",
            ),
            Op::post(
                "/api/v1/alert-rules/{id}/evaluate",
                "evaluateAlertRule",
                "Evaluate an alert rule now",
            )
            .body(Payload::Empty),
            Op::get(
                "/api/v1/alert-notifications",
                "listAlertNotifications",
                "Page alert transitions and their delivery",
            )
            .query(ALERT_HISTORY_QUERY)
            .ok(Payload::List("AlertNotification")),
        ],
    ));

    ops.extend(tagged(
        "analytics",
        vec![
            Op::get(
                "/api/v1/analytics/summary",
                "getAnalyticsSummary",
                "Spend, tokens and request counts over a window",
            )
            .clickhouse_read()
            .query(DASHBOARD_QUERY),
            Op::get(
                "/api/v1/analytics/timeseries",
                "getAnalyticsTimeseries",
                "Bucketed spend and usage over a window",
            )
            .clickhouse_read()
            .bucketed()
            .query(TIMESERIES_QUERY),
            Op::get(
                "/api/v1/analytics/by-model",
                "getAnalyticsByModel",
                "Spend and usage grouped by organization and model: a row carries `org_id` (an empty string for a request no organization owns) beside `model`, since two organizations may each have a route of one name",
            )
            .clickhouse_read()
            .query(DASHBOARD_QUERY),
            Op::get(
                "/api/v1/analytics/by-variant",
                "getAnalyticsByVariant",
                "Spend and usage grouped by organization, route and the weighted variant that served it \
                 (`model` narrows to one route): a row carries `org_id` (an empty string for a request no \
                 organization owns) beside `model` and `variant`, since two organizations may each have a \
                 route of one name; requests served without a variant come back under the empty string. \
                 `requests` counts every request the variant answered, failovers included; \
                 `first_attempt_requests` counts those it served on the first attempt (no \
                 `fallback_reason`, `clickhouse/023`), the closer comparison with the configured \
                 weights; a request that failed over from another variant counts for neither. It is `null` on a ClickHouse that has not applied \
                 `023`, which cannot tell the two apart. `assigned_requests` counts the requests each \
                 variant was dialled to first (`first_variant`, `clickhouse/048`), so a request that \
                 failed over counts for the variant it was dialled to first and a variant that \
                 answered nothing still has a row. It is not the sampled split: a variant skipped \
                 while its breaker is open or its targets are parked is dialled first by none of the \
                 requests sampled for it; requests \
                 served without a variant count under the empty string. A row written before `048` \
                 counts for the variant that answered it when it has no `fallback_reason`, and for no \
                 variant otherwise. It is `null` on a ClickHouse that has not applied `048`. \
                 `sampled_requests` counts the requests the weighted split sampled each variant for \
                 (`sampled_variant`, `clickhouse/050`), before the gateway skips a variant whose \
                 breaker is open or whose targets are parked or unhealthy, so it is the comparison \
                 with the configured weights; a variant row written before `050` counts for no \
                 variant, requests served without a variant count under the empty string, and it is \
                 `null` on a ClickHouse that has not applied `050`",
            )
            .clickhouse_read()
            .query(DASHBOARD_QUERY),
            Op::get(
                "/api/v1/analytics/by-attribution",
                "getAnalyticsByAttribution",
                "Spend and usage grouped by business unit or customer",
            )
            .clickhouse_read()
            .query(ATTRIBUTION_QUERY),
            Op::get(
                "/api/v1/analytics/invocations",
                "listInvocations",
                "Page the request records the caller's roles reach, bodies withheld below the payload floor. \
                 A row's `lifecycle_operation` names the call on a stored response it records \
                 (`retrieve`, `delete`, `cancel`, `input_items`, `compact`, `input_tokens`) and is an \
                 empty string for a model request and for a row older than the column. A row's \
                 `cache_write_1h_tokens` (a number) is the part of `cache_write_tokens` written to the \
                 provider's 1 hour prompt cache, so the 5 minute share is the difference; it is `0` for a \
                 provider that reports no split. `upstream_status`, `attempts`, `lifecycle_operation`, \
                 `cache_write_1h_tokens`, `guardrail_events` (an array of strings, one per guardrail \
                 action) and `plugin_events` (the same for plugin calls) come from ClickHouse files an operator applies by hand \
                 (`clickhouse/015` to `019`): on a table that lacks one the field is still on every row, \
                 as `0` (an empty string for `lifecycle_operation`), and the list does not fail. \
                 `fallback_reason` (`clickhouse/023`) is the failure class that sent the request on to \
                 the target that answered it (`throttled`, `timeout`, `transport`, `server_error`, \
                 `refusal` or `refusal:<category>`), and an empty string when the first dial answered, \
                 for a row older than the column, or on a table that lacks it. \
                 `first_variant` (`clickhouse/048`) is the weighted variant the request was first \
                 dialled to, which differs from `variant` when it failed over to another variant, and \
                 an empty string on the classic pool, for a row older than the column, or on a table \
                 that lacks it. \
                 `sampled_variant` (`clickhouse/050`) is the variant the route's weighted split picked \
                 as primary before the gateway skipped one whose breaker is open or whose targets are \
                 parked or unhealthy, which differs from `first_variant` when the request was routed \
                 around it; on a realtime turn row it is the variant the session's dial walk led with, \
                 the route's first variant with a non-zero weight (#3553). An empty string on the \
                 classic pool, for a row older than the column, or on a table that lacks it. \
                 `outcome` (a string, `clickhouse/030`) names what happened to the request: `served`, \
                 `upstream_error`, `withheld`, `cancelled` or a `refused_*` reason, and `unknown` for a \
                 row written before the gateway recorded one or on a table without the column. \
                 `has_tools`, `has_image_input`, `has_audio_input`, `has_response_schema` (each `0` or \
                 `1`) and `turns` (`clickhouse/031`) say what the request asked the model to do beyond \
                 its tokens, `0` on an older row or a table without the columns. \
                 `param_adjustments` (an array of `<disposition>:<param>` strings, `clickhouse/022`) \
                 lists the request parameters dialect translation dropped, clamped or translated, \
                 empty when every parameter went through",
            )
            .clickhouse_read()
            .query(INVOCATIONS_QUERY),
            Op::get(
                "/api/v1/analytics/by-attribution/export",
                "exportAnalyticsByAttribution",
                "The attribution rollup as CSV for a window, flagging totals that hold unpriced requests; the file name carries the window and currency",
            )
            .clickhouse_read()
            .query(ATTRIBUTION_QUERY),
            Op::get(
                "/api/v1/analytics/timeseries/export",
                "exportAnalyticsTimeseries",
                "Requests, tokens and cost per bucket as CSV, flagging buckets that hold unpriced requests",
            )
            .clickhouse_read()
            .bucketed()
            .query(TIMESERIES_QUERY),
            Op::get(
                "/api/v1/analytics/invocations/export",
                "exportInvocations",
                "Stream the request log as CSV, metadata only (never bodies or error text), under the same row scoping as the JSON list; `limit` and `cursor` are ignored. An export is bounded in rows and in time: past either the body ends in an error rather than passing for a complete file",
            )
            .too_many_requests(
                "too many request log exports are running, in this process or for this caller; wait for `Retry-After` seconds",
            )
            .clickhouse_read()
            .query(INVOCATIONS_QUERY),
            Op::post(
                "/api/v1/analytics/exports/tickets",
                "mintExportTicket",
                "Mint a single-use download ticket for one CSV export, for a browser that cannot stream the file to disk itself (#3504). The body names `path` (one of the three `.../export` routes above; any other is a `400`) and `query`, the query string that export would be sent, which is checked as the export checks it. The export's own credential and scope checks run here, and the request log export is refused with `429` at once when its concurrency bound is full. Answers `id`, `download_url` (the secret is in it: hand it to the browser, never log it), `status_url` and `expires_at`, 60 seconds out",
            )
            .body(Payload::Ref("ExportTicketRequest"))
            .ok(Payload::Ref("ExportTicket"))
            .too_many_requests(
                "the request log export's concurrency bound is already full, in this process or for this caller; wait for `Retry-After` seconds",
            ),
            Op::get(
                "/api/v1/analytics/exports/tickets/{id}",
                "getExportTicket",
                "How a ticket's export is going, for the principal that minted it only (anyone else gets `404`). `state` is `pending`, `streaming` (with `rows` so far), `complete` (`rows`), `truncated` (`rows` and a `reason`: `row_limit`, `time_limit`, `read_error`, `client_closed`, or `interrupted` when the replica serving it stopped reporting), `refused` (the download's HTTP `status`, and `retry_after` for a `429`) or `expired` (never redeemed); `done` is `true` once it can no longer change",
            )
            .ok(Payload::Ref("ExportTicketStatus")),
            Op::get(
                "/api/v1/analytics/exports/download/{ticket}",
                "downloadExport",
                "Redeem a download ticket and send its export as the bearer route would, under the principal that minted it, re-checked now. The ticket in the path is the credential, and it is spent by the first request whatever its outcome; an unknown, used or expired ticket is one uniform `410`, and any method but `GET` (a `HEAD` from a link checker included) is a `405` that leaves the ticket unspent. Answers `Cache-Control: no-store` and `Referrer-Policy: no-referrer`. The body is CSV, not JSON",
            )
            .public()
            .clickhouse_read(),
            Op::get(
                "/api/v1/analytics/guardrails",
                "getAnalyticsGuardrails",
                "What each guardrail did to requests in the window: one row per `action` and `rule` \
                 with the number of `requests` it acted on. `action` is `blocked`, `redacted` or \
                 `annotated` for a rule (named in `rule`), and `provider_blocked` or \
                 `provider_transformed` for a guardrail provider, with its name in `rule` (empty \
                 for rows written before the name was recorded). \
                 `recorded` is `false` when the ClickHouse table lacks \
                 `clickhouse/018_guardrail_events.sql`, in which case `data` is empty",
            )
            .clickhouse_read()
            .query(WINDOW_QUERY),
            Op::get(
                "/api/v1/analytics/plugins",
                "getAnalyticsPlugins",
                "What each plugin did to requests in the window: one row per `org_id` and `plugin` (its slug) with \
                 `calls`, and the `blocked`, `transformed`, `annotated` and `errors` among them (the \
                 rest were allowed). An `errors` call gave no verdict, so a fail-closed plugin that \
                 refused traffic because its endpoint was down counts there, not under `blocked`. \
                 `last_error_at` and `last_error_kind` (`refused`, `timeout`, `connect`, \
                 `status_<code>` or `body`) name the newest failure and are `null` and empty when `errors` is \
                 `0`. `recorded` is `false` when the ClickHouse table lacks \
                 `clickhouse/019_plugin_events.sql`, in which case `data` is empty",
            )
            .clickhouse_read()
            .query(WINDOW_QUERY),
            Op::get(
                "/api/v1/analytics/param-adjustments",
                "getAnalyticsParamAdjustments",
                "Which request parameters dialect translation did not forward as sent, per route \
                 and provider, in the window: one row per `route` (the public model), `provider`, \
                 `disposition` (`dropped`, `clamped` or `translated`) and `param`, with the \
                 `requests` that had it, the `route_requests` that route sent that provider in all \
                 and their ratio as `share`. `recorded` is `false` when the ClickHouse table lacks \
                 `clickhouse/022_param_adjustments.sql`, in which case `data` is empty",
            )
            .clickhouse_read()
            .query(WINDOW_QUERY),
            Op::get(
                "/api/v1/analytics/savings",
                "getAnalyticsSavings",
                "The savings advisor: where the window's spend went (by route, upstream model and \
                 caller, with retries and failovers, failed requests and response-cache misses \
                 called out), which requests were substitutable, and what they would have cost at \
                 each cheaper model already in the same route's pool, as a `low`/`high` range. It \
                 re-prices and never evaluates quality, so it is a shortlist to test, not a \
                 recommendation; `limits` says so on every answer. A request or a candidate whose \
                 price is unknown is excluded and counted under `excluded`, never priced at zero",
            )
            .clickhouse_read()
            .query(SAVINGS_QUERY)
            .ok(Payload::Ref("SavingsReport")),
            Op::get(
                "/api/v1/analytics/sampling",
                "getAnalyticsSampling",
                "The request log's sample rate, for every caller the analytics reads serve. Below \
                 `1` the gateway logs one request in about `1 / sample_rate` and the counts, \
                 tokens and spend the other analytics reads return are scaled up from that \
                 sample, so they are estimates. Read from the store rather than ClickHouse, and \
                 only the rate: the rest of the logging policy stays behind \
                 `GET /api/v1/logging-settings`",
            )
            .ok(Payload::Ref("LogSampling")),
        ],
    ));

    ops.extend(tagged(
        "health",
        vec![
            Op::get(
                "/api/v1/health/uptime",
                "getUptime",
                "Per-provider uptime over a window",
            )
            .clickhouse_read()
            .query(UPTIME_QUERY),
            Op::get(
                "/api/v1/health/timeline",
                "getHealthTimeline",
                "Per-provider health transitions over a window",
            )
            .clickhouse_read()
            .bucketed()
            .query(HEALTH_TIMELINE_QUERY),
            Op::get(
                "/api/v1/health/mttr",
                "getMttr",
                "Mean time to recovery per provider",
            )
            .clickhouse_read()
            .query(WINDOW_QUERY),
        ],
    ));

    ops.extend(tagged(
        "adaptive-routing",
        vec![
            Op::get(
                "/api/v1/adaptive-routing-policy",
                "getAdaptivePolicy",
                "Read the adaptive-routing policy",
            ),
            Op::put(
                "/api/v1/adaptive-routing-policy",
                "updateAdaptivePolicy",
                "Replace the adaptive-routing policy",
            ),
            Op::get(
                "/api/v1/adaptive-routing-telemetry",
                "getAdaptiveTelemetry",
                "Samples the gateways have pushed back",
            ),
            Op::get(
                "/api/v1/breaker-state",
                "getBreakerState",
                "Circuit-breaker state of every target, across the gateway fleet",
            ),
        ],
    ));

    ops.extend(tagged(
        "cluster",
        vec![
            Op::get(
                "/api/v1/cluster/nodes",
                "listClusterNodes",
                "List gateway nodes that have registered",
            )
            .ok(Payload::List("ClusterNode")),
            Op::get(
                "/api/v1/cluster/summary",
                "getClusterSummary",
                "Counts of live, stale and lagging gateways, with no node identities",
            ),
            Op::put(
                "/api/v1/cluster/nodes/{id}/drain",
                "setClusterNodeDrain",
                "Drain or un-drain a gateway node",
            )
            .ok(Payload::Ref("ClusterNode")),
            Op::delete(
                "/api/v1/cluster/nodes/{id}",
                "forgetClusterNode",
                "Forget a gateway node",
            ),
        ],
    ));

    ops.extend(tagged(
        "connectors",
        vec![
            Op::get(
                "/api/v1/connectors",
                "listConnectors",
                "List observability connectors",
            ),
            Op::post(
                "/api/v1/connectors",
                "createConnector",
                "Create an observability connector",
            )
            .needs_kek(),
            Op::put(
                "/api/v1/connectors/{id}",
                "updateConnector",
                "Update an observability connector (managed_auth_secret: omit to keep, null or blank to remove)",
            )
            .needs_kek(),
            Op::delete(
                "/api/v1/connectors/{id}",
                "deleteConnector",
                "Delete an observability connector",
            ),
            Op::post(
                "/api/v1/connectors/{id}/test",
                "testConnector",
                "Send a test delivery through a connector",
            )
            .body(Payload::Empty),
            Op::get(
                "/api/v1/connectors/collector-config",
                "renderCollectorConfig",
                "Render an OpenTelemetry collector config for the connectors",
            ),
        ],
    ));

    ops.extend(tagged(
        "settings",
        vec![
            Op::get(
                "/api/v1/feature-flags",
                "getFeatureFlags",
                "Read the persisted feature flags",
            ),
            Op::put(
                "/api/v1/feature-flags",
                "updateFeatureFlags",
                "Replace the persisted feature flags",
            ),
            Op::get(
                "/api/v1/runtime-policy",
                "getRuntimePolicy",
                "Read the retry, timeout and queue policy",
            ),
            Op::put(
                "/api/v1/runtime-policy",
                "updateRuntimePolicy",
                "Replace the retry, timeout and queue policy",
            ),
            Op::get(
                "/api/v1/compatibility-policy",
                "getCompatibilityPolicy",
                "Read the dialect-compatibility policy",
            ),
            Op::put(
                "/api/v1/compatibility-policy",
                "updateCompatibilityPolicy",
                "Replace the dialect-compatibility policy",
            ),
            Op::get(
                "/api/v1/logging-settings",
                "getLoggingSettings",
                "Read the request-logging, retention and UX-event settings",
            ),
            Op::put(
                "/api/v1/logging-settings",
                "updateLoggingSettings",
                "Replace the request-logging, retention and UX-event settings",
            ),
            Op::get(
                "/api/v1/log-erasures",
                "listLogErasures",
                "List recent subject-scoped log erasures",
            ),
            Op::post(
                "/api/v1/log-erasures",
                "createLogErasure",
                "Erase one virtual key's, user's, organization's or single request's logs and captured payloads",
            ),
            Op::get(
                "/api/v1/log-erasures/subjects",
                "findLogErasureSubjects",
                "Find the named virtual keys, users or organizations the erasure form offers (superadmin)",
            )
            .query(LOG_ERASURE_SUBJECT_QUERY),
            Op::get(
                "/api/v1/log-erasures/{id}",
                "getLogErasure",
                "Read an erasure, settling it once ClickHouse reports its mutations done",
            ),
            Op::post(
                "/api/v1/logging-settings/redaction-preview",
                "previewPayloadRedaction",
                "Dry-run the payload redaction policy against a pasted sample; stores and logs nothing",
            ),
            Op::get(
                "/api/v1/client-settings",
                "getClientSettings",
                "Read the upstream HTTP client settings",
            ),
            Op::put(
                "/api/v1/client-settings",
                "updateClientSettings",
                "Replace the upstream HTTP client settings",
            ),
            Op::get(
                "/api/v1/model-defaults",
                "getModelDefaults",
                "Read the deployment-wide model defaults",
            ),
            Op::put(
                "/api/v1/model-defaults",
                "updateModelDefaults",
                "Replace the deployment-wide model defaults",
            ),
            Op::get(
                "/api/v1/security-settings",
                "getSecuritySettings",
                "Read the deployment security settings",
            ),
            Op::put(
                "/api/v1/security-settings",
                "updateSecuritySettings",
                "Replace the deployment security settings",
            ),
        ],
    ));

    ops.extend(tagged(
        "invitations",
        vec![
            Op::get(
                "/api/v1/orgs/{org_id}/invitations",
                "listInvitations",
                "List an organization's invitations",
            )
            .ok(Payload::List("Invitation")),
            Op::post(
                "/api/v1/orgs/{org_id}/invitations",
                "createInvitation",
                "Invite an email address to an organization",
            )
            .ok(Payload::Open),
            Op::delete(
                "/api/v1/invitations/{id}",
                "revokeInvitation",
                "Revoke an invitation",
            )
            .ok(Payload::Ref("Invitation")),
            Op::post(
                "/api/v1/invitations/{id}/resend",
                "resendInvitation",
                "Email a pending invitation's link again (at most once a minute; `429` with `Retry-After` otherwise)",
            )
            .body(Payload::Empty)
            .ok(Payload::Ref("Invitation"))
            .conflict(
                "the invitation is not pending, has expired, has no kept link, or mail is not configured",
            ),
            Op::post(
                "/api/v1/orgs/{org_id}/invitations/mail-check",
                "checkInvitationMailRelay",
                "Check the mail relay without sending an invitation",
            )
            .body(Payload::Empty)
            .ok(Payload::Open)
            .too_many_requests(
                "the relay was checked a moment ago, by this caller (every five seconds) or by anyone (every two); `Retry-After` says how long to wait",
            )
            .conflict("mail is not configured on this control plane"),
            Op::get(
                "/api/v1/invitations/accept/{token}",
                "previewInvitation",
                "Preview an invitation from its one-time token",
            )
            .public(),
            Op::post(
                "/api/v1/invitations/accept/{token}/accept",
                "acceptInvitation",
                "Accept an invitation",
            )
            .public(),
            Op::get(
                "/api/v1/password-reset/{token}",
                "previewPasswordReset",
                "Preview a password reset from its one-time token",
            )
            .public(),
            Op::post(
                "/api/v1/password-reset/{token}",
                "redeemPasswordReset",
                "Choose a new password with a reset token",
            )
            .public(),
        ],
    ));

    ops.extend(tagged(
        "ldap",
        vec![
            Op::get(
                "/api/v1/orgs/{org_id}/ldap-directories",
                "listLdapDirectories",
                "Read an organization's LDAP directory",
            ),
            Op::post(
                "/api/v1/orgs/{org_id}/ldap-directories",
                "createLdapDirectory",
                "Register an LDAP directory",
            ),
            Op::put(
                "/api/v1/ldap-directories/{id}",
                "updateLdapDirectory",
                "Update an LDAP directory in place",
            ),
            Op::delete(
                "/api/v1/ldap-directories/{id}",
                "deleteLdapDirectory",
                "Delete an LDAP directory",
            ),
            Op::get(
                "/api/v1/ldap-directories/{id}/group-mappings",
                "listLdapGroupMappings",
                "List an LDAP directory's group mappings",
            ),
            Op::post(
                "/api/v1/ldap-directories/{id}/group-mappings",
                "createLdapGroupMapping",
                "Map a directory group to a role at a scope",
            ),
            Op::delete(
                "/api/v1/ldap-group-mappings/{id}",
                "deleteLdapGroupMapping",
                "Delete an LDAP group mapping",
            ),
        ],
    ));

    ops.extend(tagged(
        "sso",
        vec![
            Op::get(
                "/api/v1/orgs/{org_id}/sso-providers",
                "listSsoProviders",
                "List an organization's SSO providers",
            ),
            Op::post(
                "/api/v1/orgs/{org_id}/sso-providers",
                "createSsoProvider",
                "Register an SSO provider",
            )
            .needs_kek(),
            Op::put(
                "/api/v1/sso-providers/{id}",
                "updateSsoProvider",
                "Update an SSO provider in place",
            )
            .needs_kek(),
            Op::delete(
                "/api/v1/sso-providers/{id}",
                "deleteSsoProvider",
                "Delete an SSO provider",
            ),
            Op::get(
                "/api/v1/sso-providers/{id}/group-mappings",
                "listSsoGroupMappings",
                "List an SSO provider's group mappings",
            ),
            Op::post(
                "/api/v1/sso-providers/{id}/group-mappings",
                "createSsoGroupMapping",
                "Map an IdP group to a role at a scope",
            ),
            Op::delete(
                "/api/v1/sso-group-mappings/{id}",
                "deleteSsoGroupMapping",
                "Delete an SSO group mapping",
            ),
            Op::get(
                "/auth/sso/{slug}/start",
                "startSsoLogin",
                "Begin an SSO login",
            )
            .public()
            .see_other(
                "the identity provider's authorization endpoint. While the provider's org has \
                 single sign-on turned off, a browser (`Accept: text/html`) is sent to the \
                 dashboard's `/login` screen with `sso_error=sso_disabled&sso=` instead, and any \
                 other caller gets a `403` with `error.code` `sso_disabled`. A slug no enabled provider \
                 answers to sends a browser to `/login` with `sso_error=unknown_provider`, and any \
                 other caller a `400` with `error.code` `unknown_provider`",
            ),
            Op::get(
                "/auth/sso/{slug}/callback",
                "ssoCallback",
                "OAuth/OIDC redirect target for an SSO login",
            )
            .public()
            .see_other(
                "sent to a browser (`Accept: text/html`): the dashboard's `/login` screen, with \
                 `sso_code=` (a one-time code for `POST /auth/sso/exchange`) on success or \
                 `sso_error=` (a stable code, plus `sso=` once the provider is known) on a \
                 refusal. Any other caller gets the session JSON, or on a refusal the status it \
                 always had with the same code as `error.code` and a fixed message that names \
                 no account",
            ),
            Op::post(
                "/auth/sso/exchange",
                "exchangeSsoCode",
                "Redeem the one-time code a browser SSO sign-in ended with for its session",
            )
            .public(),
        ],
    ));

    ops.extend(tagged(
        "api-tokens",
        vec![
            Op::get(
                "/api/v1/orgs/{org_id}/api-tokens",
                "listApiTokens",
                "List an organization's scoped API tokens",
            )
            .ok(Payload::List("ApiToken")),
            Op::post(
                "/api/v1/orgs/{org_id}/api-tokens",
                "createApiToken",
                "Mint a scoped API token; the secret is returned once",
            )
            .body(Payload::Ref("CreateApiToken"))
            .ok(Payload::Ref("SecretApiToken")),
            Op::post(
                "/api/v1/api-tokens/{id}/rotate",
                "rotateApiToken",
                "Replace a token's secret; the old one stops working at once",
            )
            .ok(Payload::Ref("SecretApiToken"))
            .conflict(API_TOKEN_INACTIVE_409),
            Op::delete(
                "/api/v1/api-tokens/{id}",
                "revokeApiToken",
                "Revoke a scoped API token",
            )
            .ok(Payload::Ref("ApiToken")),
        ],
    ));

    ops.extend(tagged(
        "scim",
        vec![
            Op::get(
                "/api/v1/orgs/{org_id}/scim-tokens",
                "listScimTokens",
                "List an organization's SCIM tokens",
            ),
            Op::post(
                "/api/v1/orgs/{org_id}/scim-tokens",
                "createScimToken",
                "Mint a SCIM token",
            ),
            Op::delete(
                "/api/v1/scim-tokens/{id}",
                "revokeScimToken",
                "Revoke a SCIM token",
            )
            .ok(Payload::Open),
            Op::get(
                "/api/v1/orgs/{org_id}/scim-group-mappings",
                "listScimGroupMappings",
                "List SCIM group mappings",
            ),
            Op::post(
                "/api/v1/orgs/{org_id}/scim-group-mappings",
                "createScimGroupMapping",
                "Map a SCIM group to a role at a scope",
            ),
            Op::delete(
                "/api/v1/scim-group-mappings/{id}",
                "deleteScimGroupMapping",
                "Delete a SCIM group mapping",
            ),
            Op::get("/scim/v2/Users", "scimListUsers", "SCIM 2.0: list users"),
            Op::post(
                "/scim/v2/Users",
                "scimCreateUser",
                "SCIM 2.0: create a user",
            ),
            Op::get(
                "/scim/v2/Users/{id}",
                "scimGetUser",
                "SCIM 2.0: read a user",
            ),
            Op::put(
                "/scim/v2/Users/{id}",
                "scimReplaceUser",
                "SCIM 2.0: replace a user",
            ),
            Op::patch(
                "/scim/v2/Users/{id}",
                "scimPatchUser",
                "SCIM 2.0: patch a user",
            ),
            Op::delete(
                "/scim/v2/Users/{id}",
                "scimDeleteUser",
                "SCIM 2.0: deprovision a user",
            ),
            Op::get("/scim/v2/Groups", "scimListGroups", "SCIM 2.0: list groups"),
            Op::post(
                "/scim/v2/Groups",
                "scimCreateGroup",
                "SCIM 2.0: create a group",
            ),
            Op::get(
                "/scim/v2/Groups/{id}",
                "scimGetGroup",
                "SCIM 2.0: read a group",
            ),
            Op::put(
                "/scim/v2/Groups/{id}",
                "scimReplaceGroup",
                "SCIM 2.0: replace a group",
            ),
            Op::patch(
                "/scim/v2/Groups/{id}",
                "scimPatchGroup",
                "SCIM 2.0: patch a group",
            ),
            Op::delete(
                "/scim/v2/Groups/{id}",
                "scimDeleteGroup",
                "SCIM 2.0: delete a group",
            ),
        ],
    ));

    ops.extend(tagged(
        "proxy",
        vec![
            Op::get(
                "/gw/{path}",
                "proxyGet",
                "Reverse-proxy a GET to the gateway data plane",
            ),
            Op::post(
                "/gw/{path}",
                "proxyPost",
                "Reverse-proxy a POST to the gateway data plane",
            ),
            Op::put(
                "/gw/{path}",
                "proxyPut",
                "Reverse-proxy a PUT to the gateway data plane",
            ),
            Op::patch(
                "/gw/{path}",
                "proxyPatch",
                "Reverse-proxy a PATCH to the gateway data plane",
            ),
            Op::delete(
                "/gw/{path}",
                "proxyDelete",
                "Reverse-proxy a DELETE to the gateway data plane",
            )
            .ok(Payload::Open),
        ],
    ));

    ops
}

/// The tag groups, in the order the reference renders them.
fn tags() -> Value {
    json!([
        {"name": "system", "description": "probes, version, the assembled config and this document"},
        {"name": "internal", "description": "the control↔data-plane channel; carries decrypted credentials and sits behind the internal token"},
        {"name": "auth", "description": "session login and per-organization authentication policy"},
        {"name": "tenancy", "description": "organizations, teams, projects and the governance dimensions spend rolls up to"},
        {"name": "users", "description": "accounts and the role grants that scope them"},
        {"name": "access-control", "description": "the RBAC matrix, custom roles and access profiles"},
        {"name": "providers", "description": "upstream providers and the groups that balance across them"},
        {"name": "routes", "description": "published model names, their strategy and their upstream targets"},
        {"name": "virtual-keys", "description": "the credentials clients present to the gateway"},
        {"name": "me", "description": "self-service surface for the calling account"},
        {"name": "governance", "description": "budgets, rate limits and model prices"},
        {"name": "prompt-templates", "description": "versioned prompt templates and their scope bindings"},
        {"name": "skills", "description": "versioned skills and their published slugs"},
        {"name": "guardrails", "description": "guardrail providers and the rules that apply them"},
        {"name": "labels", "description": "custom and automatic labels on providers, provider groups, routes and models"},
        {"name": "plugins", "description": "installed request/response plugins"},
        {"name": "mcp", "description": "MCP servers, tool groups, OAuth sessions and the tool-call log"},
        {"name": "alerting", "description": "alert channels, rules and delivery history"},
        {"name": "analytics", "description": "spend and usage aggregates over a time window"},
        {"name": "health", "description": "provider uptime, transitions and recovery time"},
        {"name": "adaptive-routing", "description": "the adaptive-routing policy and the samples feeding it"},
        {"name": "cluster", "description": "registered gateway nodes and their drain state"},
        {"name": "connectors", "description": "observability connectors and the collector config they render"},
        {"name": "settings", "description": "deployment-wide singletons the gateway reads from the snapshot"},
        {"name": "invitations", "description": "email invitations into an organization"},
        {"name": "sso", "description": "SSO providers, group mappings and the login redirect"},
        {"name": "api-tokens", "description": "scoped control-plane API tokens for automation"},
        {"name": "ldap", "description": "LDAP directories and the group mappings that turn directory groups into roles"},
        {"name": "scim", "description": "SCIM 2.0 provisioning and the tokens that authenticate it"},
        {"name": "proxy", "description": "the dashboard Playground's authenticated passthrough to the data plane"}
    ])
}

/// A path template's parameters, derived from its `{name}` segments so the
/// table never has to restate what the path already says.
fn path_parameters(path: &str) -> Vec<Value> {
    path.split('/')
        .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
        .map(|name| {
            let schema = if name == "id" || name.ends_with("_id") {
                json!({"type": "string", "format": "uuid"})
            } else if name == "version" {
                json!({"type": "integer"})
            } else {
                json!({"type": "string"})
            };
            json!({"name": name, "in": "path", "required": true, "schema": schema})
        })
        .collect()
}

/// Build the OpenAPI document. The version tracks the crate version so a served
/// spec always matches the running binary.
fn build_document() -> Value {
    // group first so a path item is built once, with its parameters, rather
    // than re-entered per method
    let mut grouped: std::collections::BTreeMap<&'static str, Vec<Op>> =
        std::collections::BTreeMap::new();
    for op in operations() {
        grouped.entry(op.path).or_default().push(op);
    }
    let mut paths: Map<String, Value> = Map::new();
    for (path, ops) in grouped {
        let mut item = Map::new();
        let params = path_parameters(path);
        if !params.is_empty() {
            item.insert("parameters".into(), Value::Array(params));
        }
        for op in ops {
            item.insert(op.method.to_string(), op.to_json());
        }
        paths.insert(path.to_string(), Value::Object(item));
    }

    let mut doc = json!({
        "openapi": "3.1.0",
        "info": {
            "title": "rolter control plane",
            "description": "Management API for a rolter deployment: tenancy, providers, routes, \
                virtual keys, governance and the settings the gateway reads back through \
                `/internal/snapshot`. Authenticate with a session token from \
                `POST /api/v1/auth/login`, or with the deployment admin token.",
            "version": env!("CARGO_PKG_VERSION"),
            "license": {"name": "Apache-2.0"}
        },
        "servers": [{"url": "/", "description": "this control plane"}],
        "security": [{"bearerAuth": []}],
        "tags": tags(),
        "paths": Value::Object(paths),
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "description": "a session token from `POST /api/v1/auth/login`, the deployment \
                        admin token, a SCIM token on `/scim/v2/*`, or the internal token on \
                        `/internal/*`"
                }
            },
            "responses": {
                "Error": {
                    "description": "an error, in the shape every rolter surface renders",
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Error"}}}
                }
            },
            "schemas": schemas()
        }
    });
    // `externalDocs` and `x-rolter-*` provenance from the specification
    // registry (#907)
    rolter_core::spec_registry::annotate_openapi(
        &mut doc,
        rolter_core::spec_registry::Plane::Control,
    );
    doc
}

/// The primitive schemas the entity definitions reuse, so a `uuid` is spelled
/// the same way in every one of them.
struct Prim {
    uuid: Value,
    timestamp: Value,
    string: Value,
    nullable_string: Value,
    nullable_uuid: Value,
    nullable_timestamp: Value,
    string_list: Value,
}

impl Prim {
    fn new() -> Self {
        Self {
            uuid: json!({"type": "string", "format": "uuid"}),
            timestamp: json!({"type": "string", "format": "date-time"}),
            string: json!({"type": "string"}),
            nullable_string: json!({"type": ["string", "null"]}),
            nullable_uuid: json!({"type": ["string", "null"], "format": "uuid"}),
            nullable_timestamp: json!({"type": ["string", "null"], "format": "date-time"}),
            string_list: json!({"type": "array", "items": {"type": "string"}}),
        }
    }
}

/// The typed entries `components/schemas` carries.
///
/// Only the surfaces with a stable `serde` shape are modelled; everything else
/// is an open object at the call site rather than a guess here. Split by
/// subject rather than written as one literal because `json!` hits the macro
/// recursion limit long before this many entries.
fn schemas() -> Value {
    let p = Prim::new();
    let mut out = Map::new();
    for group in [
        error_schemas(&p),
        tenancy_schemas(&p),
        identity_schemas(&p),
        provider_schemas(&p),
        routing_schemas(&p),
        virtual_key_schemas(&p),
        governance_schemas(&p),
        stability_schemas(&p),
        alerting_schemas(&p),
        config_problem_schemas(&p),
        cluster_schemas(&p),
        export_ticket_schemas(&p),
    ] {
        if let Value::Object(entries) = group {
            out.extend(entries);
        }
    }
    let mut schemas = Value::Object(out);
    crate::deprecation::mark_fields(&mut schemas);
    schemas
}

/// The download tickets of the analytics CSV exports (#3504).
fn export_ticket_schemas(p: &Prim) -> Value {
    let uuid = &p.uuid;
    let timestamp = &p.timestamp;
    json!({
        "ExportTicketRequest": {
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "enum": [
                        "/api/v1/analytics/by-attribution/export",
                        "/api/v1/analytics/timeseries/export",
                        "/api/v1/analytics/invocations/export"
                    ],
                    "description": "the export, by the exact path of its bearer route"
                },
                "query": {
                    "type": "string",
                    "description": "the query string that export would be sent, without the `?`; checked as the export checks it"
                }
            }
        },
        "ExportTicket": {
            "type": "object",
            "required": ["id", "download_url", "status_url", "expires_at"],
            "properties": {
                "id": uuid,
                "download_url": {
                    "type": "string",
                    "description": "the same-origin link that downloads the export once; it carries the secret, so hand it to the browser and never log it"
                },
                "status_url": {"type": "string"},
                "expires_at": timestamp
            }
        },
        "ExportTicketStatus": {
            "type": "object",
            "required": ["id", "export", "expires_at", "done", "state"],
            "properties": {
                "id": uuid,
                "export": {"type": "string", "enum": ["attribution", "timeseries", "invocations"]},
                "expires_at": timestamp,
                "done": {"type": "boolean", "description": "the state can no longer change"},
                "state": {
                    "type": "string",
                    "enum": ["pending", "streaming", "complete", "truncated", "refused", "expired"]
                },
                "rows": {
                    "type": "integer",
                    "description": "rows written so far, on `streaming`, `complete` and `truncated`"
                },
                "reason": {
                    "type": "string",
                    "enum": ["row_limit", "time_limit", "read_error", "client_closed", "interrupted"],
                    "description": "why a `truncated` export stopped short"
                },
                "status": {
                    "type": "integer",
                    "description": "the HTTP status a `refused` download was answered with"
                },
                "retry_after": {
                    "type": "integer",
                    "description": "seconds to wait, on a `refused` `429`"
                }
            }
        }
    })
}

/// Alert history rows and rules, where the stable `detail_code` and
/// `last_error_code` values are listed (#2334) and the recorded reading
/// (#2301) is typed.
fn alerting_schemas(p: &Prim) -> Value {
    let uuid = &p.uuid;
    let timestamp = &p.timestamp;
    let nullable_string = &p.nullable_string;
    let nullable_number = json!({"type": ["number", "null"]});
    let nullable_object = json!({"type": ["object", "null"]});
    json!({
        "AlertNotification": {
            "type": "object",
            "required": ["id", "rule_id", "state", "delivery_status", "sent_at"],
            "properties": {
                "id": uuid, "rule_id": uuid,
                "channel_id": p.nullable_uuid,
                "state": {"type": "string", "enum": ["firing", "resolved"]},
                "delivery_status": {"type": "string", "enum": ["pending", "delivered", "failed", "skipped"]},
                "detail": {"type": ["string", "null"], "description": "English fallback for detail_code"},
                "detail_code": {
                    "type": ["string", "null"],
                    "enum": ["http_ok", "http_error", "timeout", "connect_failed", "delivery_failed",
                             "egress_denied", "secret_unsealable", "secret_invalid",
                             "client_unavailable", "no_channel", "channel_disabled", null],
                    "description": "stable code of detail; null on rows written before codes existed"
                },
                "detail_params": {
                    "type": ["object", "null"],
                    "description": "the numbers detail_code interpolates (`status`, `seconds`) and `no_data: true` for a no-data transition"
                },
                "value": {
                    "type": ["number", "null"],
                    "description": "the reading the row was written for, in the signal's API unit; null for a no-data transition and on older rows"
                },
                "threshold": nullable_number,
                "sent_at": timestamp
            }
        },
        "AlertRuleError": {
            "type": "object",
            "description": "the error fields of an alert rule",
            "properties": {
                "last_error": nullable_string,
                "last_error_code": {
                    "type": ["string", "null"],
                    "enum": ["clickhouse_not_configured", "unknown_signal", "query_timeout",
                             "clickhouse_timeout", "clickhouse_unreachable",
                             "clickhouse_bad_response", "query_failed", "clickhouse_error", null],
                    "description": "stable code of last_error, which is its English fallback"
                },
                "last_error_params": nullable_object
            }
        }
    })
}

/// What `GET /api/v1/config/problems` answers (#1876).
fn config_problem_schemas(p: &Prim) -> Value {
    let string = &p.string;
    json!({
        "ConfigProblems": {
            "type": "object",
            "required": ["problems", "omitted"],
            "properties": {
                "problems": {
                    "type": "array",
                    "items": string,
                    "description": "one English sentence per finding, including ones that drop nothing; for a person to read, not a client to parse"
                },
                "omitted": {
                    "type": "array",
                    "items": {"$ref": "#/components/schemas/OmittedEntry"},
                    "description": "the providers, routes and config-file virtual keys the snapshot leaves out whole, so they are not served"
                }
            }
        },
        "OmittedEntry": {
            "type": "object",
            "required": ["kind", "name", "reason", "index"],
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["provider", "route", "virtual_key", "public_example_key"]
                },
                "name": {
                    "type": "string",
                    "description": "a provider name, a route model or a key's name (<unnamed> without one)"
                },
                "reason": string,
                "index": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "the row's position in the matching list of GET /api/v1/config (providers, routes or virtual_keys); a name alone can match several rows, as tenancy is not in that response"
                }
            }
        }
    })
}

/// One node of the cluster inventory (#543), with the snapshot formats it reads
/// and was last served (ADR-0038, #3351).
fn cluster_schemas(p: &Prim) -> Value {
    let string = &p.string;
    let timestamp = &p.timestamp;
    json!({
        "ClusterNode": {
            "type": "object",
            "required": [
                "id", "role", "build_version", "config_version", "desired_state",
                "state_changed_at", "first_seen_at", "last_seen_at", "formats_min",
                "formats_max", "served_format", "live", "converged"
            ],
            "properties": {
                "id": string,
                "role": {"type": "string", "enum": ["gateway", "control"]},
                "build_version": string,
                "config_version": {"type": "integer", "format": "int64"},
                "desired_state": {"type": "string", "enum": ["active", "draining"]},
                "state_changed_at": timestamp,
                "first_seen_at": timestamp,
                "last_seen_at": timestamp,
                "formats_min": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "the oldest snapshot format the node stated it reads; 1 for a node that states none"
                },
                "formats_max": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "the newest snapshot format the node stated it reads"
                },
                "served_format": {
                    "type": ["integer", "null"],
                    "minimum": 1,
                    "description": "the format the node's last poll was answered in; null when that poll was refused for its format"
                },
                "live": {"type": "boolean"},
                "converged": {
                    "type": "boolean",
                    "description": "the node reported the current config version and was not last served an older snapshot format"
                }
            }
        }
    })
}

/// The stability marker (#1385). `stable` is in the enum for completeness, but
/// the endpoint never emits it: absence from the array is what "stable" means.
fn stability_schemas(p: &Prim) -> Value {
    let string = &p.string;
    json!({
        "SubsystemStability": {
            "type": "object",
            "required": ["subsystem", "stability", "note"],
            "properties": {
                "subsystem": string,
                "stability": {"type": "string", "enum": ["stable", "experimental"]},
                "note": string
            }
        }
    })
}

fn error_schemas(p: &Prim) -> Value {
    let string = &p.string;
    // `param` is null when a refusal has no single parameter to name, as the
    // OpenAI envelope does for a query string that does not deserialize
    let nullable_string = &p.nullable_string;
    json!({
        "Error": {
            "type": "object",
            "properties": {
                "error": {
                    "type": "object",
                    "properties": {
                        "message": string,
                        "type": string,
                        "code": {
                            "type": "string",
                            "description": "stable, never renamed; a client branches on it \
                                rather than on `message`. The common ones are `name_taken`, \
                                `invalid_field`, `invalid_body`, `referenced` and `scope_mismatch` (#2567, #2729)"
                        },
                        "param": nullable_string,
                        // the field (or list row, `injected_headers.x-team`) an `invalid_field` refusal is about
                        "field": string
                    }
                }
            }
        }
    })
}

fn tenancy_schemas(p: &Prim) -> Value {
    let uuid = &p.uuid;
    let timestamp = &p.timestamp;
    let string = &p.string;
    let nullable_string = &p.nullable_string;
    let nullable_uuid = &p.nullable_uuid;
    let nullable_timestamp = &p.nullable_timestamp;
    json!({
        "Org": {
            "type": "object",
            "required": ["id", "name", "slug", "created_at"],
            "properties": {"id": uuid, "name": string, "slug": string, "created_at": timestamp}
        },
        "CreateOrg": {
            "type": "object",
            "required": ["name", "slug"],
            "properties": {"name": string, "slug": string},
            "additionalProperties": false
        },

        "Team": {
            "type": "object",
            "required": ["id", "org_id", "name", "created_at"],
            "properties": {"id": uuid, "org_id": uuid, "name": string, "created_at": timestamp}
        },
        "CreateTeam": {
            "type": "object",
            "required": ["name"],
            "properties": {"name": string},
            "additionalProperties": false
        },

        "UpdateTeam": {
            "type": "object",
            "required": ["name"],
            "properties": {"name": string},
            "additionalProperties": false
        },

        "Project": {
            "type": "object",
            "required": ["id", "team_id", "name", "created_at"],
            "properties": {"id": uuid, "team_id": uuid, "name": string, "created_at": timestamp}
        },
        "OrgProject": {
            "type": "object",
            "description": "A project listed org-wide, carrying the name of the team that owns it so callers can group without a request per team.",
            "required": ["id", "team_id", "team_name", "name", "created_at"],
            "properties": {
                "id": uuid, "team_id": uuid, "team_name": string,
                "name": string, "created_at": timestamp
            }
        },
        "CreateProject": {
            "type": "object",
            "required": ["name"],
            "properties": {"name": string},
            "additionalProperties": false
        },
        "ProjectSettings": {
            "type": "object",
            "description": "A project's own settings. `payload_min_role` is the lowest built-in role that may read the request and response bodies payload capture stored for the project's traffic; an admin always may.",
            "required": ["payload_min_role"],
            "properties": {
                "payload_min_role": {"type": "string", "enum": ["member", "viewer"], "default": "member"}
            },
            "additionalProperties": false
        },

        "BusinessUnit": {
            "type": "object",
            "required": ["id", "org_id", "name", "slug", "created_at"],
            "properties": {
                "id": uuid, "org_id": uuid, "name": string, "slug": string,
                "retired_at": nullable_timestamp,
                "created_at": timestamp
            }
        },
        "BusinessUnitListing": {
            "description": "A business unit as the org-wide listing returns it, with the count of live virtual keys attributed to it, computed in the same query.",
            "allOf": [
                {"$ref": "#/components/schemas/BusinessUnit"},
                {
                    "type": "object",
                    "required": ["live_key_count"],
                    "properties": {
                        "live_key_count": {
                            "type": "integer",
                            "minimum": 0,
                            "description": "virtual keys attributed to this business unit that are live: not disabled and not past their expiry. Zero on a card with no spend means no key could have produced any"
                        }
                    }
                }
            ]
        },
        "CreateBusinessUnit": {
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": string,
                "slug": {"type": ["string", "null"], "description": "derived from `name` when omitted"}
            },
            "additionalProperties": false
        },
        "UpdateBusinessUnit": {
            "type": "object",
            "properties": {
                "name": nullable_string,
                "slug": nullable_string,
                "allow_slug_change": {"type": "boolean", "default": false},
                "retired": {"type": ["boolean", "null"]}
            },
            "additionalProperties": false
        },

        "Customer": {
            "type": "object",
            "required": ["id", "org_id", "name", "slug", "created_at"],
            "properties": {
                "id": uuid, "org_id": uuid, "business_unit_id": nullable_uuid,
                "name": string, "slug": string,
                "retired_at": nullable_timestamp,
                "created_at": timestamp
            }
        },
        "CustomerListing": {
            "description": "A customer as the org-wide listing returns it, with the count of live virtual keys attributed to it, computed in the same query.",
            "allOf": [
                {"$ref": "#/components/schemas/Customer"},
                {
                    "type": "object",
                    "required": ["live_key_count"],
                    "properties": {
                        "live_key_count": {
                            "type": "integer",
                            "minimum": 0,
                            "description": "virtual keys attributed to this customer that are live: not disabled and not past their expiry. Zero on a card with no spend means no key could have produced any"
                        }
                    }
                }
            ]
        },
        "CreateCustomer": {
            "type": "object",
            "required": ["name"],
            "properties": {"name": string, "slug": nullable_string, "business_unit_id": nullable_uuid},
            "additionalProperties": false
        },
        "UpdateCustomer": {
            "type": "object",
            "properties": {
                "name": nullable_string,
                "slug": nullable_string,
                "allow_slug_change": {"type": "boolean", "default": false},
                "business_unit_id": nullable_uuid,
                "retired": {"type": ["boolean", "null"]}
            },
            "additionalProperties": false
        }
    })
}

fn identity_schemas(p: &Prim) -> Value {
    let uuid = &p.uuid;
    let timestamp = &p.timestamp;
    let string = &p.string;
    let nullable_string = &p.nullable_string;
    let nullable_uuid = &p.nullable_uuid;
    let nullable_timestamp = &p.nullable_timestamp;
    json!({
        "User": {
            "type": "object",
            "required": ["id", "email", "is_superadmin", "created_at"],
            "properties": {
                "id": uuid, "email": {"type": "string", "format": "email"},
                "is_superadmin": {"type": "boolean"},
                "deactivated_at": nullable_timestamp,
                "created_at": timestamp,
                "deployment_role": {"type": ["string", "null"], "enum": ["operator", "security_auditor", null]}
            }
        },
        "SetDeploymentRole": {
            "type": "object",
            "required": ["role"],
            "properties": {
                "role": {"type": ["string", "null"], "enum": ["operator", "security_auditor", null],
                    "description": "null clears the account's deployment role"}
            },
            "additionalProperties": false
        },
        "ApiToken": {
            "type": "object",
            "required": ["id", "org_id", "name", "token_prefix", "created_at"],
            "properties": {
                "id": uuid, "org_id": uuid,
                "team_id": {"type": ["string", "null"], "format": "uuid"},
                "project_id": {"type": ["string", "null"], "format": "uuid"},
                "name": {"type": "string"},
                "role": {"type": ["string", "null"], "enum": ["admin", "member", "viewer", null]},
                "custom_role_id": {"type": ["string", "null"], "format": "uuid"},
                "token_prefix": {"type": "string", "description": "the first characters of the value, to recognise it in a list"},
                "expires_at": nullable_timestamp,
                "created_by": {"type": ["string", "null"], "format": "uuid"},
                "created_at": timestamp,
                "rotated_at": nullable_timestamp,
                "last_used_at": nullable_timestamp,
                "revoked_at": nullable_timestamp
            }
        },
        "SecretApiToken": {
            "type": "object",
            "required": ["id", "org_id", "name", "token_prefix", "created_at", "secret"],
            "description": "an ApiToken plus its secret, returned this once and never again",
            "properties": {
                "id": uuid, "org_id": uuid,
                "name": {"type": "string"},
                "token_prefix": {"type": "string"},
                "created_at": timestamp,
                "secret": {"type": "string", "description": "the bearer value, `rolter_api_…`"}
            }
        },
        "CreateApiToken": {
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {"type": "string", "maxLength": 80},
                "team_id": {"type": ["string", "null"], "format": "uuid", "description": "omit team_id and project_id for an org-wide token"},
                "project_id": {"type": ["string", "null"], "format": "uuid"},
                "role": {"type": ["string", "null"], "enum": ["admin", "member", "viewer", null]},
                "custom_role_id": {"type": ["string", "null"], "format": "uuid", "description": "give exactly one of role and custom_role_id"},
                "expires_in_days": {"type": ["integer", "null"], "minimum": 1, "maximum": 730, "default": 90},
                "never_expires": {"type": "boolean", "default": false}
            },
            "additionalProperties": false
        },
        "CreateUser": {
            "type": "object",
            "required": ["email"],
            "properties": {
                "email": {"type": "string", "format": "email"},
                "password": {"type": ["string", "null"], "description": "omit for an SSO-only shell account"},
                "role": {"type": ["string", "null"], "enum": ["admin", "member", "viewer", null], "default": "member"}
            },
            "additionalProperties": false
        },
        "UpdateUser": {
            "type": "object",
            "properties": {
                "email": nullable_string,
                "password": nullable_string,
                "is_superadmin": {"type": ["boolean", "null"]},
                "deactivated": {"type": ["boolean", "null"]}
            },
            "additionalProperties": false
        },
        "ChangePassword": {
            "type": "object",
            "required": ["current_password", "new_password"],
            "properties": {
                "current_password": {"type": "string"},
                "new_password": {"type": "string", "minLength": 8}
            },
            "additionalProperties": false
        },
        "PasswordChanged": {
            "type": "object",
            "required": ["sessions_revoked"],
            "properties": {
                "sessions_revoked": {
                    "type": "integer",
                    "description": "the account's other sessions that ended; the caller's own is kept"
                }
            }
        },
        "CreatedUser": {
            "type": "object",
            "required": ["user", "membership"],
            "properties": {
                "user": {"$ref": "#/components/schemas/User"},
                "membership": {"$ref": "#/components/schemas/Membership"}
            }
        },

        "Membership": {
            "type": "object",
            "required": ["id", "user_id", "role", "source", "created_at"],
            "properties": {
                "id": uuid, "user_id": uuid,
                "org_id": nullable_uuid, "team_id": nullable_uuid, "project_id": nullable_uuid,
                "role": {"type": "string", "enum": ["admin", "member", "viewer"]},
                "source": {"type": "string", "enum": ["manual", "sso"]},
                "created_at": timestamp
            }
        },
        "CreateMembership": {
            "type": "object",
            "required": ["user_id", "scope_type", "scope_id", "role"],
            "properties": {
                "user_id": uuid,
                "scope_type": {"type": "string", "enum": ["org", "team", "project"]},
                "scope_id": uuid,
                "role": {"type": "string", "enum": ["admin", "member", "viewer"]}
            },
            "additionalProperties": false
        },

        "UpdateMembership": {
            "type": "object",
            "required": ["role"],
            "properties": {
                "role": {"type": "string", "enum": ["admin", "member", "viewer"]}
            },
            "additionalProperties": false
        },

        "CreatedPasswordReset": {
            "type": "object",
            "required": ["reset", "token", "reset_url", "sessions_revoked"],
            "properties": {
                "reset": {"type": "object", "description": "the reset row: id, user_id, created_by, expires_at, created_at"},
                "token": {"type": "string", "description": "shown once; hand it to the member, it is not recoverable"},
                "reset_url": {"type": "string", "description": "ready-made link carrying the token"},
                "sessions_revoked": {"type": "integer"}
            }
        },

        "Invitation": {
            "type": "object",
            "required": ["id", "org_id", "email", "role", "expires_at"],
            "properties": {
                "id": uuid, "org_id": uuid, "email": {"type": "string", "format": "email"},
                "role": string, "team_id": nullable_uuid, "project_id": nullable_uuid,
                "invited_by": nullable_uuid,
                "invited_by_email": {"type": ["string", "null"], "description": "email of the sender, resolved server-side whatever users the caller may list; null when the invitation came from the admin token or its sender was deleted. Set on the list only"},
                "expires_at": timestamp,
                "accepted_at": nullable_timestamp,
                "revoked_at": nullable_timestamp
            }
        }
    })
}

fn provider_schemas(p: &Prim) -> Value {
    let uuid = &p.uuid;
    let timestamp = &p.timestamp;
    let string = &p.string;
    let nullable_string = &p.nullable_string;
    let string_list = &p.string_list;
    json!({
        "Provider": {
            "type": "object",
            "required": ["id", "org_id", "name", "slug", "kind", "api_base", "created_at", "has_stored_key"],
            "properties": {
                "id": uuid, "org_id": uuid, "name": string,
                "slug": {"type": "string", "description": "stable identity for `provider-slug/model` addressing"},
                "kind": {"type": "string", "description": "a kind listed by `GET /api/v1/provider-kinds`"},
                "api_base": string,
                "api_key_env": nullable_string,
                "egress_proxy": nullable_string,
                "egress_proxies": string_list,
                "kv_events": {"oneOf": [{"$ref": "#/components/schemas/KvEventsConfig"}, {"type": "null"}], "description": "vLLM kv-event stream for `precise_cache_aware`; null has none"},
                "lmcache": {"oneOf": [{"$ref": "#/components/schemas/LmCacheConfig"}, {"type": "null"}], "description": "LMCache controller for `lmcache_aware`; null has none"},
                "project_id": {"type": ["string", "null"], "format": "uuid", "description": "the project the provider is scoped to; null is org-wide. Only keys minted in that project may reach it, through a route or by `slug/model`"},
                "allow_custom_api_base": {"type": "boolean", "description": "opts out of the host pin a hosted kind such as `openrouter` carries, so `api_base` may name another host (ADR-0029)"},
                "created_at": timestamp,
                "has_stored_key": {"type": "boolean", "description": "whether a sealed key is stored for the provider (a row in `provider_keys`). An `api_key_env` does not count. The key, its ciphertext and its nonce are never returned"}
            }
        },
        "CreateProvider": {
            "type": "object",
            "required": ["name", "kind", "api_base"],
            "properties": {
                "name": string,
                "slug": {"type": ["string", "null"], "description": "derived from `name` when omitted"},
                "kind": string,
                "api_base": string,
                "api_key": {"type": ["string", "null"], "description": "sealed with the KEK before storage; never returned"},
                "api_key_env": nullable_string,
                "egress_proxy": nullable_string,
                "egress_proxies": string_list,
                "kv_events": {"oneOf": [{"$ref": "#/components/schemas/KvEventsConfig"}, {"type": "null"}], "description": "omit for none"},
                "lmcache": {"oneOf": [{"$ref": "#/components/schemas/LmCacheConfig"}, {"type": "null"}], "description": "omit for none"},
                "project_id": {"type": ["string", "null"], "format": "uuid", "description": "scope the provider to one project of the org; omit for an org-wide provider. Needs the provider create capability at that project (an environment-variable credential needs it at the org)"},
                "allow_custom_api_base": {"type": "boolean", "description": "opt out of the host pin a hosted kind such as `openrouter` carries, so `api_base` may name another host the key then travels to; default false (ADR-0029)"}
            },
            "additionalProperties": false
        },
        "UpdateProvider": {
            "type": "object",
            "description": "every field is optional; omit to leave unchanged, send an empty string to clear",
            "properties": {
                "slug": nullable_string,
                "allow_slug_change": {"type": "boolean", "default": false},
                "kind": nullable_string,
                "api_base": nullable_string,
                "api_key": nullable_string,
                "api_key_env": nullable_string,
                "egress_proxy": nullable_string,
                "egress_proxies": {"type": ["array", "null"], "items": {"type": "string"}},
                "kv_events": {"oneOf": [{"$ref": "#/components/schemas/KvEventsConfig"}, {"type": "null"}], "description": "omit to leave unchanged, null to clear, an object to replace the whole setting"},
                "lmcache": {"oneOf": [{"$ref": "#/components/schemas/LmCacheConfig"}, {"type": "null"}], "description": "omit to leave unchanged, null to clear, an object to replace the whole setting"},
                "project_id": {"type": ["string", "null"], "format": "uuid", "description": "omit to leave the scope unchanged, a project id to scope the provider to it, null to make it org-wide. Refused with 409 while a route or group of another project uses the provider"},
                "allow_custom_api_base": {"type": "boolean", "description": "omit to leave unchanged; see the create body"}
            },
            "additionalProperties": false
        },

        "KvEventsConfig": {
            "type": "object",
            "required": ["endpoint"],
            "properties": {
                "endpoint": {"type": "string", "description": "ZMQ subscriber endpoint, a `tcp://` URL such as `tcp://vllm-0:5557`; checked against the egress policy"},
                "topic": {"type": "string", "default": "kv-events"},
                "max_blocks": {"type": "integer", "minimum": 1, "default": 1000000, "description": "resident block identities kept for the provider"},
                "stale_secs": {"type": "integer", "minimum": 1, "default": 30, "description": "telemetry older than this is ignored"}
            }
        },
        "LmCacheConfig": {
            "type": "object",
            "required": ["endpoint"],
            "properties": {
                "endpoint": {"type": "string", "description": "http(s) URL returning `{\"occupancy\": 0..1, \"cache_available\": bool}`; checked against the egress policy"},
                "refresh_secs": {"type": "integer", "minimum": 1, "default": 2},
                "stale_secs": {"type": "integer", "minimum": 1, "default": 10}
            }
        },

        "ProviderGroup": {
            "type": "object",
            "required": ["id", "org_id", "name", "slug", "strategy", "created_at"],
            "properties": {
                "id": uuid, "org_id": uuid, "name": string, "slug": string,
                "strategy": string,
                "project_id": {"type": ["string", "null"], "format": "uuid", "description": "the project the group is scoped to; null is org-wide. Only keys minted in that project may reach it, through a route or by `slug/model`"},
                "created_at": timestamp
            }
        },
        "ProviderGroupMember": {
            "type": "object",
            "required": ["group_id", "provider_id", "provider_name", "weight", "position"],
            "properties": {
                "group_id": uuid, "provider_id": uuid, "provider_name": string,
                "upstream_model": nullable_string,
                "weight": {"type": "integer"},
                "position": {"type": "integer"}
            }
        },
        "ProviderGroupView": {
            "allOf": [
                {"$ref": "#/components/schemas/ProviderGroup"},
                {
                    "type": "object",
                    "required": ["members"],
                    "properties": {
                        "members": {"type": "array", "items": {"$ref": "#/components/schemas/ProviderGroupMember"}}
                    }
                }
            ]
        },
        "ProviderGroupMemberInput": {
            "type": "object",
            "required": ["provider_id"],
            "properties": {
                "provider_id": uuid,
                "upstream_model": nullable_string,
                "weight": {"type": "integer", "default": 1}
            },
            "additionalProperties": false
        },
        "CreateProviderGroup": {
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": string,
                "slug": nullable_string,
                "strategy": {"type": "string", "default": "round_robin"},
                "members": {"type": "array", "items": {"$ref": "#/components/schemas/ProviderGroupMemberInput"}},
                "project_id": {"type": ["string", "null"], "format": "uuid", "description": "scope the group to one project of the org; omit for an org-wide group. A scoped group may hold that project's providers and org-wide ones; an org-wide group only org-wide ones (409 otherwise)"}
            },
            "additionalProperties": false
        },
        "UpdateProviderGroup": {
            "type": "object",
            "properties": {
                "name": nullable_string,
                "slug": nullable_string,
                "allow_slug_change": {"type": "boolean", "default": false},
                "strategy": nullable_string,
                "members": {
                    "type": ["array", "null"],
                    "description": "when present, replaces the entire membership",
                    "items": {"$ref": "#/components/schemas/ProviderGroupMemberInput"}
                },
                "project_id": {"type": ["string", "null"], "format": "uuid", "description": "omit to leave the scope unchanged, a project id to scope the group to it, null to make it org-wide"}
            },
            "additionalProperties": false
        }
    })
}

fn routing_schemas(p: &Prim) -> Value {
    let uuid = &p.uuid;
    let timestamp = &p.timestamp;
    let string = &p.string;
    let nullable_string = &p.nullable_string;
    // a variant target's classes (#3373), built apart: the schema below is
    // already as deep as `json!` expands
    let variant_takes_over = json!({
        "type": "array",
        "default": [],
        "items": {
            "type": "string",
            "pattern": "^(throttled|timeout|transport|server_error|refusal(:[a-z0-9_-]{1,64})?)$"
        },
        "description": "the failure classes this target takes over, as on a route target. A route with variants dials only the variants' targets, and one without dials its own: a set that leaves the route only such targets to dial is refused naming `all_targets_restricted`"
    });
    json!({
        "Route": {
            "type": "object",
            "required": ["id", "project_id", "model", "strategy", "enabled", "created_at"],
            "properties": {
                "id": uuid, "project_id": uuid,
                "model": {"type": "string", "description": "the public model name clients request"},
                "strategy": string,
                "enabled": {"type": "boolean"},
                "params": {"type": "object", "description": "admin default inference params"},
                "param_policy": {"type": "object", "description": "`{mode, allow, deny}` override policy"},
                "advanced": {"type": "object", "description": "catalog metadata and per-model execution policy"},
                "created_at": timestamp,
                "targets": {
                    "type": "array",
                    "description": "the route's targets, as GET /api/v1/routes/{route_id}/targets lists them; present only when the list was asked for `include=targets`",
                    "items": {"$ref": "#/components/schemas/RouteTarget"}
                },
                "variants": {
                    "type": "array",
                    "description": "the route's weighted variants, as GET /api/v1/routes/{id}/variants lists them; present only when the list was asked for `include=variants`",
                    "items": {"$ref": "#/components/schemas/RouteVariant"}
                },
                "complexity": {
                    "type": "object",
                    "description": "the route's complexity policy, as GET /api/v1/routes/{id}/complexity returns it (`{\"tiers\": []}` when none is set); present only when the list was asked for `include=complexity`"
                }
            }
        },
        "EffectiveModel": {
            "type": "object",
            "required": ["model", "strategy", "targets", "route_targets", "source"],
            "properties": {
                "model": {"type": "string", "description": "the public model name"},
                "strategy": string,
                "targets": {"type": "integer", "description": "how many targets the route has"},
                "route_targets": {
                    "type": "array",
                    "description": "the targets themselves: provider name, upstream model and weight",
                    "items": {
                        "type": "object",
                        "required": ["provider", "weight"],
                        "properties": {
                            "provider": string,
                            "model": nullable_string,
                            "weight": {"type": "integer"},
                            "takes_over": {"type": "array", "items": string, "description": "the failure classes the target is a fallback for; absent for an ordinary target"}
                        }
                    }
                },
                "org_id": {"type": "string", "description": "the organization the route belongs to; absent for a route from the bootstrap file. Two organizations can serve the same model name, and this tells their entries apart"},
                "project_id": {"type": "string", "description": "the project the route belongs to; absent for a route from the bootstrap file"},
                "source": {"type": "string", "enum": ["config", "db"]}
            }
        },
        "CreateRoute": {
            "type": "object",
            "required": ["model"],
            "properties": {"model": string, "strategy": {"type": "string", "default": "round_robin"}},
            "additionalProperties": false
        },
        "UpdateRoute": {
            "type": "object",
            "description": "every field is optional; one that is omitted or null keeps the stored value, and an empty body changes nothing",
            "properties": {
                "enabled": {"type": ["boolean", "null"]},
                "strategy": {
                    "type": ["string", "null"],
                    "description": "one of the strategies route creation accepts"
                },
                "targets": {
                    "type": ["array", "null"],
                    "minItems": 1,
                    "description": "the route's complete target list. Entries are matched to the stored targets by provider_id and upstream_model: a match keeps its id and takes the new weight, a stored target the list leaves out is removed, and an entry with no match is added, all in one transaction. Each entry is checked as POST /api/v1/routes/{route_id}/targets checks one",
                    "items": {"$ref": "#/components/schemas/CreateRouteTarget"}
                }
            },
            "additionalProperties": false
        },

        "RouteTarget": {
            "type": "object",
            "required": ["id", "route_id", "provider_id", "weight", "created_at"],
            "properties": {
                "id": uuid, "route_id": uuid, "provider_id": uuid,
                "upstream_model": nullable_string,
                "weight": {"type": "integer"},
                "takes_over": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "the failure classes this target is a fallback for; empty for an ordinary member of the pool. A target with classes is never dialled first and is tried only after another target failed with one of them"
                },
                "created_at": timestamp
            }
        },
        "RouteVariant": {
            "type": "object",
            "required": ["id", "route_id", "name", "weight", "position", "params", "targets", "created_at"],
            "properties": {
                "id": uuid, "route_id": uuid,
                "name": {"type": "string", "description": "the variant's name, unique in its route; request logs and metrics carry it"},
                "weight": {"type": "integer", "minimum": 0, "description": "relative share of the requests that sample this variant as their primary. A weight of 0 is accepted: the variant stays and takes no first attempts while another variant has a positive weight, and is still tried in fallback order (a set of only zeroes is sampled uniformly)"},
                "position": {"type": "integer", "description": "the declared order, which is the fallback chain after the sampled primary"},
                "params": {"type": "object", "description": "variant-scoped default inference params, layered over the route's"},
                "targets": {
                    "type": "array",
                    "description": "the variant's targets in routing order",
                    "items": {"$ref": "#/components/schemas/RouteVariantTarget"}
                },
                "created_at": timestamp
            }
        },
        "RouteVariantTarget": {
            "type": "object",
            "required": ["id", "variant_id", "provider_id", "weight", "position", "takes_over"],
            "properties": {
                "id": uuid, "variant_id": uuid, "provider_id": uuid,
                "upstream_model": nullable_string,
                "weight": {"type": "integer", "minimum": 0},
                "position": {"type": "integer"},
                "takes_over": {"type": "array", "items": string, "description": "the failure classes this target takes over; empty for an ordinary target"}
            }
        },
        "SetRouteVariants": {
            "type": "object",
            "required": ["variants"],
            "properties": {
                "variants": {
                    "type": "array",
                    "maxItems": 32,
                    "description": "the route's whole variant set in fallback order. A variant's weight is 0 or more (0 takes no first attempts while another variant has a positive weight and is still tried in fallback order; a set of only zeroes is sampled uniformly); names are unique and not blank; each variant names 1 to 32 targets, each a provider of the route's org its project may use",
                    "items": {
                        "type": "object",
                        "required": ["name", "targets"],
                        "properties": {
                            "name": string,
                            "weight": {"type": "integer", "default": 1, "minimum": 0, "description": "relative share of first attempts; 0 is accepted and takes none while another variant has a positive weight (the variant stays and is still tried in fallback order), and a set of only zeroes is sampled uniformly"},
                            "params": {"type": ["object", "null"], "description": "variant-scoped default inference params; omitted or null is none"},
                            "targets": {
                                "type": "array",
                                "minItems": 1,
                                "maxItems": 32,
                                "description": "in routing order; at least one needs a weight above 0",
                                "items": {
                                    "type": "object",
                                    "required": ["provider_id"],
                                    "properties": {
                                        "provider_id": uuid,
                                        "upstream_model": {"type": ["string", "null"], "description": "the model name sent upstream; omitted, null or blank forwards the route's public name"},
                                        "weight": {"type": "integer", "default": 1, "minimum": 0},
                                        "takes_over": variant_takes_over
                                    },
                                    "additionalProperties": false
                                }
                            }
                        },
                        "additionalProperties": false
                    }
                }
            },
            "additionalProperties": false
        },
        "CreateRouteTarget": {
            "type": "object",
            "required": ["provider_id"],
            "properties": {
                "provider_id": uuid,
                "upstream_model": {"type": ["string", "null"], "description": "the model name sent upstream; omitted, null or blank forwards the route's public name"},
                "weight": {"type": "integer", "default": 1, "minimum": 1},
                "takes_over": {
                    "type": "array",
                    "default": [],
                    "items": {
                        "type": "string",
                        "pattern": "^(throttled|timeout|transport|server_error|refusal(:[a-z0-9_-]{1,64})?)$"
                    },
                    "description": "the failure classes this target takes over (`throttled`, `timeout`, `transport`, `server_error`, `refusal` or `refusal:<category>`), each at most once. A target with classes is never dialled first and is tried only after another target failed with one of them. A route may not be left with only such targets to dial (the variants' targets when it has variants, its own otherwise): that is refused naming `all_targets_restricted`"
                }
            },
            "additionalProperties": false
        }
    })
}

fn virtual_key_schemas(p: &Prim) -> Value {
    let uuid = &p.uuid;
    let timestamp = &p.timestamp;
    let string = &p.string;
    let nullable_string = &p.nullable_string;
    let nullable_uuid = &p.nullable_uuid;
    let nullable_timestamp = &p.nullable_timestamp;
    let string_list = &p.string_list;
    json!({
        "VirtualKey": {
            "type": "object",
            "required": ["id", "project_id", "key_prefix", "models", "providers", "disabled", "created_at"],
            "properties": {
                "id": uuid, "project_id": uuid,
                "key_prefix": string,
                "name": nullable_string,
                "models": string_list,
                "providers": string_list,
                "disabled": {"type": "boolean"},
                "expires_at": nullable_timestamp,
                "cache_enabled": {"type": ["boolean", "null"]},
                "created_by": nullable_uuid,
                "business_unit_id": nullable_uuid,
                "customer_id": nullable_uuid,
                "created_at": timestamp,
                "previous_key_prefix": {"type": ["string", "null"], "description": "first characters of the secret an admin rotation replaced, while its overlap is open"},
                "previous_expires_at": {"type": ["string", "null"], "format": "date-time", "description": "when the replaced secret stops authenticating; null when no overlap is running"}
            }
        },
        "OrgVirtualKey": {
            "type": "object",
            "description": "a virtual key as the organization-wide listing returns it: no digest of any secret",
            "required": ["id", "project_id", "key_prefix", "models", "providers", "disabled", "created_at"],
            "properties": {
                "id": uuid, "project_id": uuid,
                "key_prefix": string,
                "name": nullable_string,
                "models": string_list,
                "providers": string_list,
                "disabled": {"type": "boolean"},
                "expires_at": nullable_timestamp,
                "cache_enabled": {"type": ["boolean", "null"]},
                "created_by": nullable_uuid,
                "business_unit_id": nullable_uuid,
                "customer_id": nullable_uuid,
                "purpose": {"type": ["string", "null"]},
                "created_at": timestamp
            }
        },
        "CreatedVirtualKey": {
            "allOf": [
                {"$ref": "#/components/schemas/VirtualKey"},
                {
                    "type": "object",
                    "required": ["key"],
                    "properties": {
                        "key": {"type": "string", "description": "the plaintext key; returned once and never again"}
                    }
                }
            ]
        },
        "CreateVirtualKey": {
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": string,
                "models": string_list,
                "providers": string_list,
                "cache": {"type": ["boolean", "null"]},
                "expires_in_days": {"type": ["integer", "null"], "minimum": 1},
                "budget": {
                    "type": ["object", "null"],
                    "description": "a spend cap minted with the key: a budget scoped to the key, written in the same transaction (#1171)",
                    "required": ["limit_usd"],
                    "properties": {
                        "limit_usd": {"type": "string", "description": "decimal, 0 to 99999999.9999"},
                        "period": {"type": "string", "description": "daily, monthly or total (default monthly)"}
                    },
                    "additionalProperties": false
                },
                "rate_limit": {
                    "type": ["object", "null"],
                    "description": "a throughput cap minted with the key: a rate limit scoped to the key (#1171). At least one of rpm and tpm, each at least 1",
                    "properties": {
                        "rpm": {"type": ["integer", "null"], "minimum": 1},
                        "tpm": {"type": ["integer", "null"], "minimum": 1}
                    },
                    "additionalProperties": false
                }
            },
            "additionalProperties": false
        },
        "SetVirtualKeyProviders": {
            "type": "object",
            "properties": {"providers": string_list},
            "additionalProperties": false
        },
        "SetVirtualKeyAttribution": {
            "type": "object",
            "description": "omit a field to leave it unchanged, send null to clear it",
            "properties": {"business_unit_id": nullable_uuid, "customer_id": nullable_uuid},
            "additionalProperties": false
        },
        "SetVirtualKeyDisabled": {
            "type": "object",
            "required": ["disabled"],
            "properties": {"disabled": {"type": "boolean"}},
            "additionalProperties": false
        },
        "RotateVirtualKey": {
            "type": "object",
            "description": "the body may be omitted, which rotates with no overlap",
            "properties": {
                "overlap_seconds": {"type": "integer", "minimum": 0, "maximum": 2592000, "description": "how long the replaced secret keeps authenticating; 0 or omitted ends it at once"}
            },
            "additionalProperties": false
        },
        "SetVirtualKeyCache": {
            "type": "object",
            "properties": {
                "cache": {"type": ["boolean", "null"], "description": "null inherits the route's decision"}
            },
            "additionalProperties": false
        }
    })
}

fn governance_schemas(p: &Prim) -> Value {
    let uuid = &p.uuid;
    let timestamp = &p.timestamp;
    let string = &p.string;
    let nullable_string = &p.nullable_string;
    // the `SCOPE_TYPES` allowlist in `crud/`: business unit and customer are
    // the governance dimensions a key's spend rolls up to (#539)
    let scope_type = json!({
        "type": "string",
        "enum": ["org", "team", "project", "virtual_key", "business_unit", "customer", "user"]
    });
    // what `validate_period` in `crud/` accepts, read off the same table the
    // snapshot loader uses, so the documented set cannot drift from it (#1902)
    let periods: Vec<&str> = rolter_core::BudgetPeriod::SPELLINGS
        .iter()
        .map(|(spelling, _)| *spelling)
        .collect();
    let period = json!({
        "type": "string",
        "enum": periods,
        "description": "daily, 1d and 24h reset at 00:00 UTC; monthly and 30d on the first of the calendar month, UTC; total, lifetime and all never reset. Case-insensitive. There are no rolling windows such as 7d"
    });
    json!({
        "Budget": {
            "type": "object",
            "required": ["id", "scope_type", "scope_id", "limit_usd", "period", "created_at"],
            "properties": {
                "id": uuid,
                "scope_type": scope_type,
                "scope_id": uuid,
                "limit_usd": {"type": "string", "description": "decimal(12,4) as text"},
                "period": {
                    "type": "string",
                    "description": "as stored. A budget written before the period was checked may hold a value the gateway does not recognise; it is enforced as monthly and listed by GET /api/v1/config/problems"
                },
                "unpriced_policy": {"type": ["string", "null"], "enum": ["ignore", "warn", "block", null]},
                "created_at": timestamp,
                "within_org_id": {"type": ["string", "null"], "format": "uuid", "description": "a user cap only: counts the keys the person minted in this org"},
                "within_project_id": {"type": ["string", "null"], "format": "uuid", "description": "a user cap only: counts the keys the person minted in this project"},
                "scope_name": {"type": ["string", "null"], "description": "the org-wide list only: the name (else the prefix) of the key a virtual_key budget caps"}
            }
        },
        "BudgetSpendReport": {
            "type": "object",
            "required": ["status", "budgets"],
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["ok", "not_configured", "unavailable"],
                    "description": "not_configured: the control plane has no Redis url. unavailable: Redis did not answer. In both, every spent_usd is null"
                },
                "budgets": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["id", "spent_usd", "resets_at"],
                        "properties": {
                            "id": uuid,
                            "spent_usd": {"type": ["string", "null"], "description": "the gateway's counter for the current window, four decimal places. null when it could not be read; never zero in its place"},
                            "resets_at": {"type": ["string", "null"], "format": "date-time", "description": "start of the next window, UTC; null for a budget that never resets"}
                        }
                    }
                }
            }
        },
        "CreateBudget": {
            "type": "object",
            "required": ["scope_type", "scope_id", "limit_usd"],
            "properties": {
                "scope_type": scope_type,
                "scope_id": uuid,
                "limit_usd": {"type": "string", "description": "a decimal from 0 to 99999999.9999, the most the numeric(12,4) column holds"},
                "period": {
                    "type": "string",
                    "enum": period["enum"],
                    "description": period["description"],
                    "default": "30d"
                },
                "unpriced_policy": {"type": ["string", "null"], "enum": ["ignore", "warn", "block", null]},
                "within_org_id": {"type": ["string", "null"], "format": "uuid", "description": "a user cap only: counts the keys the person minted in this org"},
                "within_project_id": {"type": ["string", "null"], "format": "uuid", "description": "a user cap only: counts the keys the person minted in this project"}
            },
            "additionalProperties": false
        },
        "UpdateBudget": {
            "type": "object",
            "description": "every field is optional; omit one to leave it unchanged. The scope is not editable. An edit that changes nothing writes nothing",
            "properties": {
                "limit_usd": {"type": "string", "description": "a decimal from 0 to 99999999.9999, the most the numeric(12,4) column holds"},
                "period": period,
                "unpriced_policy": {
                    "type": ["string", "null"],
                    "enum": ["ignore", "warn", "block", null],
                    "description": "null drops the override and inherits the deployment-wide setting"
                }
            },
            "additionalProperties": false
        },

        "RateLimit": {
            "type": "object",
            "required": ["id", "scope_type", "scope_id", "created_at"],
            "properties": {
                "id": uuid,
                "scope_type": scope_type,
                "scope_id": uuid,
                "rpm": {"type": ["integer", "null"]},
                "tpm": {"type": ["integer", "null"]},
                "created_at": timestamp,
                "within_org_id": {"type": ["string", "null"], "format": "uuid", "description": "a user cap only: counts the keys the person minted in this org"},
                "within_project_id": {"type": ["string", "null"], "format": "uuid", "description": "a user cap only: counts the keys the person minted in this project"},
                "scope_name": {"type": ["string", "null"], "description": "the org-wide list only: the name (else the prefix) of the key a virtual_key limit caps"}
            }
        },
        "CreateRateLimit": {
            "type": "object",
            "description": "at least one of rpm and tpm must be set; omitted or null leaves that dimension uncapped",
            "required": ["scope_type", "scope_id"],
            "properties": {
                "scope_type": scope_type,
                "scope_id": uuid,
                "rpm": {"type": ["integer", "null"], "minimum": 1},
                "tpm": {"type": ["integer", "null"], "minimum": 1},
                "within_org_id": {"type": ["string", "null"], "format": "uuid", "description": "a user cap only: counts the keys the person minted in this org"},
                "within_project_id": {"type": ["string", "null"], "format": "uuid", "description": "a user cap only: counts the keys the person minted in this project"}
            },
            "additionalProperties": false
        },
        "UpdateRateLimit": {
            "type": "object",
            "description": "omit a cap to leave it unchanged, send null to lift it; at least one cap must remain",
            "properties": {
                "rpm": {"type": ["integer", "null"], "minimum": 1},
                "tpm": {"type": ["integer", "null"], "minimum": 1}
            },
            "additionalProperties": false
        },

        "ModelPrice": {
            "type": "object",
            "required": ["id", "model", "input_per_mtok", "output_per_mtok", "currency", "created_at"],
            "properties": {
                "id": uuid, "model": string,
                "input_per_mtok": {"type": "string", "description": "decimal(12,6) as text"},
                "output_per_mtok": string,
                "cached_input_per_mtok": nullable_string,
                "cache_write_per_mtok": {
                    "type": ["string", "null"],
                    "description": "rate for input tokens written to the prompt cache; null prices them at input_per_mtok"
                },
                "cache_write_1h_per_mtok": {
                    "type": ["string", "null"],
                    "description": "rate for input tokens written to the provider's 1 hour prompt cache, for a provider that reports the split (Anthropic's usage.cache_creation); null prices them at cache_write_per_mtok, and from there at input_per_mtok"
                },
                "image_per_unit": {
                    "type": ["string", "null"],
                    "description": "price of one generated image, for a model billed per image; null charges nothing for images (a request that reports no tokens either is then logged unpriced)"
                },
                "audio_input_per_minute": {
                    "type": ["string", "null"],
                    "description": "price of one minute of input audio (transcriptions and translations), read from the duration the response states; null charges nothing for audio"
                },
                "speech_per_mchar": {
                    "type": ["string", "null"],
                    "description": "price per million characters of text sent to a text-to-speech model, counted from the request's input; null charges nothing for speech"
                },
                "audio_input_per_mtok": {
                    "type": ["string", "null"],
                    "description": "price per million audio input tokens, for a realtime or audio model that bills audio apart from text; null charges them at input_per_mtok"
                },
                "audio_output_per_mtok": {
                    "type": ["string", "null"],
                    "description": "price per million audio output tokens; null charges them at output_per_mtok"
                },
                "cached_audio_input_per_mtok": {
                    "type": ["string", "null"],
                    "description": "price per million cached audio input tokens; null charges them at cached_input_per_mtok when that is set, otherwise at the audio input rate"
                },
                "currency": string,
                "created_at": timestamp,
                "source": {
                    "type": ["string", "null"],
                    "description": "the catalogue this price was imported from (e.g. litellm), a value subject to change; null is a price an operator set, which is an override that an import never replaces"
                },
                "source_imported_at": {
                    "type": ["string", "null"],
                    "format": "date-time",
                    "description": "the day the catalogue snapshot behind an imported price was taken; null for an operator-set price"
                }
            }
        },
        "UpsertModelPrice": {
            "type": "object",
            "required": ["model", "input_per_mtok", "output_per_mtok"],
            "properties": {
                "model": string,
                "input_per_mtok": {
                    "type": "string",
                    "description": "a number from 0 up to 999999.999999, as text; a negative value, NaN, infinity or anything the decimal(12,6) column cannot hold is a 400 naming the field"
                },
                "output_per_mtok": {
                    "type": "string",
                    "description": "a number from 0 up to 999999.999999, as text; a negative value, NaN, infinity or anything the decimal(12,6) column cannot hold is a 400 naming the field"
                },
                "cached_input_per_mtok": {
                    "type": ["string", "null"],
                    "description": "a number from 0 up to 999999.999999, as text, or null to price cache hits at input_per_mtok; a negative value, NaN, infinity or anything the decimal(12,6) column cannot hold is a 400 naming the field"
                },
                "cache_write_per_mtok": {
                    "type": ["string", "null"],
                    "description": "rate for input tokens written to the prompt cache, a number from 0 up to 999999.999999. Omit to leave the stored rate unchanged, send null to clear it (writes are then priced at input_per_mtok). Unlike the other rates, which a PUT replaces, so a client that predates the field cannot reset it"
                },
                "cache_write_1h_per_mtok": {
                    "type": ["string", "null"],
                    "description": "rate for input tokens written to the provider's 1 hour prompt cache, a number from 0 up to 999999.999999. Omit to leave the stored rate unchanged, send null to clear it (those writes are then priced at cache_write_per_mtok). Same rule as cache_write_per_mtok, so a client that predates the field cannot reset it"
                },
                "image_per_unit": {
                    "type": ["string", "null"],
                    "description": "price of one generated image, a number from 0 up to 999999.999999. Omit to leave the stored rate unchanged, send null to clear it. Same rule as cache_write_per_mtok, so a client that predates the field cannot reset it"
                },
                "audio_input_per_minute": {
                    "type": ["string", "null"],
                    "description": "price of one minute of input audio, a number from 0 up to 999999.999999. Omit to leave the stored rate unchanged, send null to clear it"
                },
                "speech_per_mchar": {
                    "type": ["string", "null"],
                    "description": "price per million characters of text-to-speech input, a number from 0 up to 999999.999999. Omit to leave the stored rate unchanged, send null to clear it"
                },
                "audio_input_per_mtok": {
                    "type": ["string", "null"],
                    "description": "price per million audio input tokens, a number from 0 up to 999999.999999. Omit to leave the stored rate unchanged, send null to clear it (those tokens are then priced at input_per_mtok). Same rule as cache_write_per_mtok, so a client that predates the field cannot reset it"
                },
                "audio_output_per_mtok": {
                    "type": ["string", "null"],
                    "description": "price per million audio output tokens, a number from 0 up to 999999.999999. Omit to leave the stored rate unchanged, send null to clear it (those tokens are then priced at output_per_mtok)"
                },
                "cached_audio_input_per_mtok": {
                    "type": ["string", "null"],
                    "description": "price per million cached audio input tokens, a number from 0 up to 999999.999999. Omit to leave the stored rate unchanged, send null to clear it (those tokens are then priced at cached_input_per_mtok, or the audio input rate when that is unset)"
                },
                "currency": {"type": "string", "default": "USD"}
            },
            "additionalProperties": false
        },
        "SavingsReport": {
            "type": "object",
            "description": "money is in the deployment's base `currency`; counts and costs are scaled by the sampling weight",
            "required": ["currency", "limits", "assumptions", "spend", "substitutable", "candidates", "excluded"],
            "properties": {
                "currency": {"type": "string"},
                "limits": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "what the report does not claim, in prose"
                },
                "assumptions": {
                    "type": "object",
                    "description": "the thresholds and margin the report was built with, after defaults and clamping",
                    "properties": {
                        "max_prompt_tokens": {"type": "integer"},
                        "max_completion_tokens": {"type": "integer"},
                        "output_margin": {"type": "number"}
                    }
                },
                "spend": {
                    "type": "object",
                    "description": "where the money went: totals, `multi_attempt` (requests that took more than one upstream attempt), `failed`, `cache` (hits, and misses on routes that cache), and `by_route`, `by_model` (upstream model) and `by_caller` (project and virtual key, the 50 most expensive, each with the name given to its key as `key_name`, `null` for a key with no name, one that is gone, or a deployment without a store)",
                    "properties": {
                        "requests": {"type": "number"},
                        "cost_usd": {"type": "number"},
                        "unpriced_requests": {"type": "number"},
                        "multi_attempt": {"type": "object"},
                        "failed": {"type": "object"},
                        "cache": {"type": "object"},
                        "by_route": {"type": "array", "items": {"type": "object"}},
                        "by_model": {"type": "array", "items": {"type": "object"}},
                        "by_caller": {"type": "array", "items": {"type": "object"}}
                    }
                },
                "substitutable": {
                    "type": "object",
                    "description": "the requests with at least one cheaper candidate, their recorded cost, and the saving at the best candidate of each as `{low, high}`",
                    "properties": {
                        "requests": {"type": "number"},
                        "current_cost_usd": {"type": "number"},
                        "projected_savings_usd": {"type": "object"}
                    }
                },
                "candidates": {
                    "type": "array",
                    "description": "one entry per route and current upstream model, biggest conservative saving first; each lists its `candidates` with `projected_cost_usd` and `savings_usd` as `{low, high}`",
                    "items": {"type": "object"}
                },
                "excluded": {
                    "type": "object",
                    "description": "`requests` counts what was not re-priced by reason (`unpriced`, `failed`, `cache_hit`, `usage_unknown`, `not_a_model_call`, `uses_tools`, `image_input`, `audio_input`, `structured_output`, `multi_turn`, `no_token_usage`, `prompt_too_large`, `output_too_large`, `route_not_configured`, `no_cheaper_candidate`); `unknown_candidate_prices` names pool models with no price",
                    "properties": {
                        "requests": {"type": "object", "additionalProperties": {"type": "number"}},
                        "unknown_candidate_prices": {"type": "array", "items": {"type": "string"}}
                    }
                }
            }
        },
        "LogSampling": {
            "type": "object",
            "required": ["sample_rate"],
            "properties": {
                "sample_rate": {
                    "type": "number",
                    "minimum": 0,
                    "maximum": 1,
                    "description": "share of requests the gateway writes to the request log; 1 logs every request"
                }
            }
        }
    })
}

/// The document, built once. Nothing in it depends on request state, so a
/// served spec is a clone-free borrow rather than a rebuild per call.
pub(crate) fn document() -> &'static Value {
    static DOCUMENT: OnceLock<Value> = OnceLock::new();
    DOCUMENT.get_or_init(build_document)
}

/// `GET /openapi.json` — serve the OpenAPI document.
async fn openapi_json() -> Response {
    Json(document()).into_response()
}

/// Where the embedded Scalar bundle is mounted; referenced by the `/docs` page.
const DOCS_BUNDLE_PATH: &str = "/docs/scalar.js";

/// `GET /docs` — interactive Scalar API reference rendering [`document`].
///
/// Self-contained for air-gapped deployments, exactly as the gateway's is: the
/// Scalar bundle is embedded in the binary and `withDefaultFonts: false` keeps
/// the page from reaching for `fonts.scalar.com`.
async fn docs() -> Response {
    let config = json!({
        "url": "/openapi.json",
        "withDefaultFonts": false,
    });
    let html = scalar_api_reference::scalar_html(&config, Some(DOCS_BUNDLE_PATH));
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
}

/// `GET /docs/scalar.js` — the Scalar JS bundle, embedded at compile time.
async fn docs_bundle() -> Response {
    match scalar_api_reference::get_asset_with_mime("scalar.js") {
        Some((mime, bytes)) => ([(header::CONTENT_TYPE, mime)], bytes).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    /// The axum method-router constructors a `.route(...)` call can name.
    const METHODS: [&str; 8] = [
        "get", "post", "put", "patch", "delete", "head", "options", "any",
    ];

    /// Every `.rs` file under the crate's `src/`.
    fn sources() -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("src/ is readable") {
                let path = entry.expect("readable dir entry").path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let mut out = Vec::new();
        walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
        out.sort();
        out
    }

    /// Reduce a source file to the lines the scan may read: comment lines go,
    /// because prose about a route registration is not one, and `#[cfg(test)]`
    /// items go, because a router stood up inside a fixture is not shipped.
    fn scannable(text: &str) -> String {
        let mut out = String::new();
        let mut lines = text.lines();
        while let Some(line) = lines.next() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") {
                continue;
            }
            let is_test_attr = trimmed == "#[cfg(test)]" || trimmed.starts_with("#[cfg(all(test");
            if !is_test_attr {
                out.push_str(line);
                out.push('\n');
                continue;
            }
            // consume the annotated item by brace-counting from its first `{`
            let mut depth = 0usize;
            let mut opened = false;
            for inner in lines.by_ref() {
                if inner.trim().starts_with("//") {
                    continue;
                }
                depth += inner.matches('{').count();
                if depth > 0 {
                    opened = true;
                }
                depth = depth.saturating_sub(inner.matches('}').count());
                if opened && depth == 0 {
                    break;
                }
            }
        }
        out
    }

    /// Rewrite an axum path template as the OpenAPI one: a `{*rest}` wildcard
    /// captures the same parameter, it just spells the capture differently.
    fn to_openapi_path(path: &str) -> String {
        path.replace("{*", "{")
    }

    /// Collect the method routers named inside one `.route(...)` call.
    fn methods_in(call: &str) -> Vec<String> {
        let bytes = call.as_bytes();
        let mut found = Vec::new();
        for method in METHODS {
            let mut from = 0;
            while let Some(pos) = call[from..].find(method) {
                let at = from + pos;
                from = at + method.len();
                // an ident boundary on the left, an opening paren on the right:
                // `get(list_x)` is a method router, `get_config` is a handler
                let left_ok =
                    at == 0 || !(bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_');
                let right_ok = bytes.get(from) == Some(&b'(');
                if left_ok && right_ok {
                    found.push(method.to_string());
                }
            }
        }
        found
    }

    /// Every `(path, method)` the crate registers on a router, read straight
    /// out of the source so the guard cannot drift from what is mounted.
    fn registered_routes() -> BTreeSet<(String, String)> {
        let mut out = BTreeSet::new();
        for file in sources() {
            let text = scannable(&std::fs::read_to_string(&file).expect("source is utf-8"));
            for (at, _) in text.match_indices(".route(") {
                let open = at + ".route(".len() - 1;
                let after = &text[open + 1..];
                let Some(quote) = after.find('"') else {
                    continue;
                };
                let Some(end) = after[quote + 1..].find('"') else {
                    continue;
                };
                let path = &after[quote + 1..quote + 1 + end];
                // the call's own parentheses, so a following `.route(...)` on
                // the same chain is not read as part of this one
                let mut depth = 0usize;
                let mut close = open;
                for (i, c) in text[open..].char_indices() {
                    match c {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                close = open + i;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                for method in methods_in(&text[open + 1 + quote + 1 + end..close]) {
                    out.insert((to_openapi_path(path), method));
                }
            }
        }
        out
    }

    /// Every `(path, method)` the document describes.
    fn documented_routes() -> BTreeSet<(String, String)> {
        operations()
            .into_iter()
            .map(|op| (op.path.to_string(), op.method.to_string()))
            .collect()
    }

    #[test]
    fn every_registered_route_is_documented() {
        let documented = documented_routes();
        let mut missing: Vec<String> = registered_routes()
            .into_iter()
            .filter(|(path, method)| {
                // `any(...)` accepts every method, so the document satisfies it
                // by describing the path at all
                if method == "any" {
                    return !documented.iter().any(|(p, _)| p == path);
                }
                !documented.contains(&(path.clone(), method.clone()))
            })
            .map(|(path, method)| format!("{} {path}", method.to_uppercase()))
            .collect();
        missing.sort();
        assert!(
            missing.is_empty(),
            "these routes are mounted but missing from the control-plane OpenAPI document; \
             add a row to `operations()` in src/openapi.rs:\n{}",
            missing.join("\n")
        );
    }

    #[test]
    fn every_documented_route_is_registered() {
        let registered = registered_routes();
        let wildcards: BTreeSet<&String> = registered
            .iter()
            .filter(|(_, method)| method == "any")
            .map(|(path, _)| path)
            .collect();
        let mut stale: Vec<String> = documented_routes()
            .into_iter()
            .filter(|(path, method)| {
                !registered.contains(&(path.clone(), method.clone())) && !wildcards.contains(path)
            })
            .map(|(path, method)| format!("{} {path}", method.to_uppercase()))
            .collect();
        stale.sort();
        assert!(
            stale.is_empty(),
            "these operations are documented but no router mounts them; \
             drop them from `operations()` in src/openapi.rs:\n{}",
            stale.join("\n")
        );
    }

    // #3427: a variant weight of 0 is accepted, so the published schema must not
    // refuse it to a client that validates against it
    #[test]
    fn a_variant_weight_of_zero_is_within_the_published_schema() {
        let doc = document();
        let schemas = &doc["components"]["schemas"];
        assert_eq!(
            schemas["SetRouteVariants"]["properties"]["variants"]["items"]["properties"]["weight"]
                ["minimum"],
            0
        );
        assert_eq!(
            schemas["RouteVariant"]["properties"]["weight"]["minimum"],
            0
        );
    }

    // #907: a control-plane endpoint the specification registry names must
    // exist, and must carry the registry's provenance in the served document
    #[test]
    fn registry_endpoints_are_documented_with_their_provenance() {
        let registry = rolter_core::spec_registry::registry().expect("the registry parses");
        let doc = document();
        assert_eq!(
            doc["externalDocs"]["url"],
            rolter_core::spec_registry::REGISTRY_URL
        );
        let mut checked = 0;
        for spec in &registry.specs {
            for endpoint in &spec.control_endpoints {
                let (method, path) =
                    rolter_core::spec_registry::parse_endpoint(endpoint).expect("validated");
                let method = method.to_ascii_lowercase();
                assert!(
                    documents_operation(&method, path),
                    "the registry names {endpoint}, which the document does not have"
                );
                let refs = doc["paths"][path][&method]["x-rolter-spec-refs"]
                    .as_array()
                    .unwrap_or_else(|| panic!("{endpoint} carries no x-rolter-spec-refs"));
                assert!(refs.iter().any(|id| id == spec.id.as_str()), "{endpoint}");
                checked += 1;
            }
        }
        assert!(checked > 0);
        assert_eq!(
            doc["paths"]["/scim/v2/Users"]["get"]["externalDocs"]["url"],
            "https://www.rfc-editor.org/rfc/rfc7644"
        );
    }

    #[test]
    fn operation_ids_are_unique() {
        let mut seen = BTreeSet::new();
        for op in operations() {
            assert!(seen.insert(op.id), "duplicate operationId {}", op.id);
        }
    }

    #[test]
    fn every_operation_carries_a_declared_tag() {
        let declared: BTreeSet<String> = tags()
            .as_array()
            .expect("tags() is an array")
            .iter()
            .map(|t| t["name"].as_str().expect("a tag has a name").to_string())
            .collect();
        for op in operations() {
            assert!(
                declared.contains(op.tag),
                "operation {} carries tag `{}`, which the root `tags` list does not declare",
                op.id,
                op.tag
            );
        }
    }

    #[test]
    fn every_schema_reference_resolves() {
        let doc = document();
        let names: BTreeSet<String> = doc["components"]["schemas"]
            .as_object()
            .expect("components/schemas is an object")
            .keys()
            .cloned()
            .collect();
        fn walk(value: &Value, names: &BTreeSet<String>, dangling: &mut Vec<String>) {
            match value {
                Value::Object(map) => {
                    if let Some(Value::String(reference)) = map.get("$ref") {
                        if let Some(name) = reference.strip_prefix("#/components/schemas/") {
                            if !names.contains(name) {
                                dangling.push(reference.clone());
                            }
                        }
                    }
                    for nested in map.values() {
                        walk(nested, names, dangling);
                    }
                }
                Value::Array(items) => {
                    for nested in items {
                        walk(nested, names, dangling);
                    }
                }
                _ => {}
            }
        }
        let mut dangling = Vec::new();
        walk(doc, &names, &mut dangling);
        assert!(dangling.is_empty(), "dangling $ref: {dangling:?}");
    }

    #[test]
    fn document_is_well_formed() {
        let doc = document();
        assert_eq!(doc["openapi"], "3.1.0");
        assert_eq!(doc["info"]["version"], env!("CARGO_PKG_VERSION"));
        assert!(doc["components"]["securitySchemes"]["bearerAuth"].is_object());
        assert!(doc["components"]["responses"]["Error"].is_object());
        // `time_bounds` answers `param: null` for a query string with no one
        // parameter to blame, so the schema has to admit it
        assert_eq!(
            doc["components"]["schemas"]["Error"]["properties"]["error"]["properties"]["param"]
                ["type"],
            json!(["string", "null"])
        );
        // a representative operation is fully described, path parameter included
        let create = &doc["paths"]["/api/v1/projects/{project_id}/routes"]["post"];
        assert_eq!(create["operationId"], "createRoute");
        assert_eq!(
            create["requestBody"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/CreateRoute"
        );
        let params = doc["paths"]["/api/v1/projects/{project_id}/routes"]["parameters"]
            .as_array()
            .expect("the path item declares its parameters");
        assert_eq!(params[0]["name"], "project_id");
        assert_eq!(params[0]["schema"]["format"], "uuid");
        // a delete answers 204 with no body
        assert!(doc["paths"]["/api/v1/routes/{id}"]["delete"]["responses"]["204"].is_object());
        // the probes are reachable without a credential
        assert_eq!(doc["paths"]["/healthz"]["get"]["security"], json!([]));
    }

    /// #2581: the business-unit and customer listings carry a live key count
    /// beside each row, and the create and update answers do not claim one.
    #[test]
    fn attribution_listings_document_their_live_key_count() {
        let doc = document();
        for (path, listing, row) in [
            (
                "/api/v1/orgs/{org_id}/business-units",
                "BusinessUnitListing",
                "BusinessUnit",
            ),
            (
                "/api/v1/orgs/{org_id}/customers",
                "CustomerListing",
                "Customer",
            ),
        ] {
            let items = &doc["paths"][path]["get"]["responses"]["200"]["content"]
                ["application/json"]["schema"]["items"]["$ref"];
            assert_eq!(items, &json!(format!("#/components/schemas/{listing}")));
            let schema = &doc["components"]["schemas"][listing]["allOf"];
            assert_eq!(schema[0]["$ref"], format!("#/components/schemas/{row}"));
            assert_eq!(schema[1]["required"], json!(["live_key_count"]));
            assert_eq!(schema[1]["properties"]["live_key_count"]["type"], "integer");
            let created = &doc["paths"][path]["post"]["responses"]["200"]["content"]
                ["application/json"]["schema"]["$ref"];
            assert_eq!(created, &json!(format!("#/components/schemas/{row}")));
        }
    }

    /// #2166: a browser landing on the MCP consent callback is sent to the
    /// dashboard, and the reference says so beside the JSON answer it keeps.
    #[test]
    fn the_mcp_consent_callback_documents_its_redirect() {
        let doc = document();
        let responses = &doc["paths"]["/auth/mcp/callback"]["get"]["responses"];
        assert!(responses["200"].is_object());
        assert!(responses["303"]["headers"]["Location"].is_object());
        assert!(responses["303"]["description"]
            .as_str()
            .is_some_and(|d| d.contains("reason=")));
        // the SSO callback is another (#2297)
        let sso = &doc["paths"]["/auth/sso/{slug}/callback"]["get"]["responses"];
        assert!(sso["303"]["description"]
            .as_str()
            .is_some_and(|d| d.contains("sso_code=") && d.contains("sso_error=")));
        // the start of an sso login redirects too, and names the refusal it
        // can end in (#2339)
        let start = &doc["paths"]["/auth/sso/{slug}/start"]["get"]["responses"];
        assert!(start["303"]["description"]
            .as_str()
            .is_some_and(|d| d.contains("sso_disabled")));
        // and nothing else grew one
        let redirects = doc["paths"]
            .as_object()
            .expect("paths")
            .values()
            .flat_map(|item| item.as_object().expect("path item").values())
            .filter(|op| op["responses"]["303"].is_object())
            .count();
        assert_eq!(redirects, 3);
    }

    #[test]
    fn the_dashboard_reads_document_the_invocation_logs_row_filters() {
        let doc = document();
        let names_of = |path: &str| -> BTreeSet<String> {
            doc["paths"][path]["get"]["parameters"]
                .as_array()
                .unwrap_or_else(|| panic!("{path} declares its query parameters"))
                .iter()
                .map(|p| {
                    p["name"]
                        .as_str()
                        .expect("a parameter has a name")
                        .to_string()
                })
                .collect()
        };
        let invocations = names_of("/api/v1/analytics/invocations");
        for path in [
            "/api/v1/analytics/summary",
            "/api/v1/analytics/timeseries",
            "/api/v1/analytics/by-model",
            "/api/v1/analytics/by-variant",
        ] {
            let names = names_of(path);
            for filter in [
                "since",
                "until",
                "model",
                "key",
                "business_unit",
                "customer",
                "project",
            ] {
                assert!(names.contains(filter), "{path} lacks {filter}: {names:?}");
                assert!(invocations.contains(filter), "invocations lacks {filter}");
            }
            // the list's own paging and lookups are not the rollups' to take
            for absent in ["status", "cursor", "limit", "request_id"] {
                assert!(!names.contains(absent), "{path} documents {absent}");
            }
        }
    }

    #[test]
    fn the_invocation_log_documents_its_cursor_and_no_offset() {
        let doc = document();
        let params = doc["paths"]["/api/v1/analytics/invocations"]["get"]["parameters"]
            .as_array()
            .expect("the invocation log declares its query parameters")
            .clone();
        let names: BTreeSet<&str> = params
            .iter()
            .map(|p| p["name"].as_str().expect("a parameter has a name"))
            .collect();
        assert!(names.contains("cursor"), "{names:?}");
        assert!(names.contains("limit"), "{names:?}");
        // a request is found by the id its client was handed (#1849)
        assert!(names.contains("request_id"), "{names:?}");
        assert!(names.contains("trace_id"), "{names:?}");
        // and the unpriced ones can be asked for by themselves
        assert!(names.contains("unpriced"), "{names:?}");
        // and so can the withheld and the unknown-usage ones (#3302)
        assert!(names.contains("withheld"), "{names:?}");
        assert!(names.contains("usage_unknown"), "{names:?}");
        // and by the outcome the gateway recorded (#3307)
        assert!(names.contains("outcome"), "{names:?}");
        // the list no longer pages on an offset, so documenting one would send
        // a caller down a path that silently returns the same page (#1394)
        assert!(!names.contains("offset"), "{names:?}");
        let cursor = params
            .iter()
            .find(|p| p["name"] == "cursor")
            .expect("cursor is documented");
        assert_eq!(cursor["in"], "query");
        assert_eq!(cursor["schema"]["type"], "string");
    }

    #[test]
    fn docs_page_is_self_contained() {
        let config = json!({"url": "/openapi.json", "withDefaultFonts": false});
        let html = scalar_api_reference::scalar_html(&config, Some(DOCS_BUNDLE_PATH));
        // the bundle is loaded from this control plane, never a cdn (air-gapped)
        assert!(html.contains(DOCS_BUNDLE_PATH));
        assert!(!html.contains("cdn.jsdelivr.net"));
    }

    /// Handler names in one file whose signature takes an axum `Query<T>`.
    ///
    /// Scoped to a single file on purpose. Keying by bare function name across
    /// the crate collides: `list_users` exists in both `crud/` (no query
    /// string) and `scim.rs` (RFC 7644's), and `callback` in both
    /// `mcp_oauth_flow.rs` and `sso.rs`. A crate-wide map reported the
    /// `crud/` route as taking a query string it does not take. Handlers and
    /// the `router()` that mounts them live in the same module here, so
    /// matching within the file is both correct and stricter.
    fn handlers_taking_query(text: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for (at, _) in text.match_indices("async fn ") {
            let rest = &text[at + "async fn ".len()..];
            let Some(open) = rest.find('(') else { continue };
            let name: String = rest[..open]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if name.is_empty() {
                continue;
            }
            // the parameter list only, so a `Query<..>` built in the body does
            // not count as an extractor
            let mut depth = 0usize;
            let mut close = open;
            for (i, c) in rest[open..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            close = open + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if rest[open..close].contains("Query<") {
                out.insert(name);
            }
        }
        out
    }

    /// Every `(path, method)` whose handler takes a `Query<T>`.
    ///
    /// Read out of the source for the same reason `registered_routes` is: a
    /// hand-kept list of "routes with a query string" is exactly the thing that
    /// drifts, which is how every route but one ended up undocumented (#1412).
    fn routes_taking_query() -> BTreeSet<(String, String)> {
        let mut out = BTreeSet::new();
        for file in sources() {
            let text = scannable(&std::fs::read_to_string(&file).expect("source is utf-8"));
            let handlers = handlers_taking_query(&text);
            if handlers.is_empty() {
                continue;
            }
            for (at, _) in text.match_indices(".route(") {
                let open = at + ".route(".len() - 1;
                let after = &text[open + 1..];
                let Some(quote) = after.find('"') else {
                    continue;
                };
                let Some(end) = after[quote + 1..].find('"') else {
                    continue;
                };
                let path = &after[quote + 1..quote + 1 + end];
                let mut depth = 0usize;
                let mut close = open;
                for (i, c) in text[open..].char_indices() {
                    match c {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                close = open + i;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let call = &text[open + 1 + quote + 1 + end..close];
                for method in methods_in(call) {
                    // the handler named inside this method's own parentheses,
                    // not any other on the same chain
                    let Some(m_at) = call.find(&format!("{method}(")) else {
                        continue;
                    };
                    let args = &call[m_at + method.len() + 1..];
                    let handler: String = args
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect();
                    if handlers.contains(&handler) {
                        out.insert((to_openapi_path(path), method));
                    }
                }
            }
        }
        out
    }

    /// Routes whose handler takes a `Query<T>` that the document deliberately
    /// says nothing about. Each one needs a reason, because the default is that
    /// a query string a caller cannot see is a query string they cannot use.
    const QUERY_UNDOCUMENTED: &[(&str, &str, &str)] = &[
        (
            "/scim/v2/Users",
            "get",
            "SCIM's filter/startIndex/count are RFC 7644, not ours to redefine; \
             the SCIM service-provider config advertises them",
        ),
        (
            "/scim/v2/Groups",
            "get",
            "same: RFC 7644 defines this query string",
        ),
        (
            "/auth/mcp/callback",
            "get",
            "an OAuth redirect target: the authorization server fills the query \
             string in, and a caller never builds one",
        ),
        (
            "/auth/sso/{slug}/callback",
            "get",
            "same: the identity provider fills this in",
        ),
    ];

    /// #1412: the mechanism to document a query string existed but was used on
    /// exactly one route, and nothing noticed the other twenty. A route whose
    /// handler takes a `Query<T>` now either documents its parameters or names
    /// itself in `QUERY_UNDOCUMENTED` with a reason.
    ///
    /// This is deliberately not "every `Query<T>` route must have parameters":
    /// a few genuinely should not, and an allowlist that has to be edited on
    /// purpose is what keeps that a decision rather than an oversight.
    #[test]
    fn every_route_with_a_query_string_documents_it_or_says_why_not() {
        let doc = document();
        let excused: BTreeSet<(String, String)> = QUERY_UNDOCUMENTED
            .iter()
            .map(|(path, method, _)| (path.to_string(), method.to_string()))
            .collect();
        let documented = documented_routes();
        let mut missing: Vec<String> = Vec::new();
        for (path, method) in routes_taking_query() {
            if method == "any" || excused.contains(&(path.clone(), method.clone())) {
                continue;
            }
            // a route the document does not describe at all is the other test's
            // to report; this one only judges routes that are described
            if !documented.contains(&(path.clone(), method.clone())) {
                continue;
            }
            let params = doc["paths"][&path][&method]["parameters"].as_array();
            let has_query = params.is_some_and(|ps| ps.iter().any(|p| p["in"] == "query"));
            if !has_query {
                missing.push(format!("{} {path}", method.to_uppercase()));
            }
        }
        missing.sort();
        assert!(
            missing.is_empty(),
            "these routes take a query string the served OpenAPI document never mentions; \
             give the operation a `.query(...)` in `operations()`, or add it to \
             `QUERY_UNDOCUMENTED` with the reason:\n{}",
            missing.join("\n")
        );
    }

    /// The scan behind that guard has to actually find things, or it passes by
    /// finding nothing — the same failure mode `the_source_scan_finds_the_routes_it_is_meant_to`
    /// guards for the route scan.
    #[test]
    fn the_query_scan_finds_the_handlers_it_is_meant_to() {
        let routes = routes_taking_query();
        assert!(
            routes.len() > 15,
            "only found {} routes taking Query<>",
            routes.len()
        );
        assert!(
            routes.contains(&("/api/v1/mcp/logs".into(), "get".into())),
            "{routes:?}"
        );
        assert!(
            routes.contains(&("/api/v1/budgets".into(), "get".into())),
            "{routes:?}"
        );
        // a handler with no Query<> extractor must not be swept in
        assert!(
            !routes.contains(&("/api/v1/orgs".into(), "post".into())),
            "{routes:?}"
        );
    }

    /// The two lists that make a caller's request fail rather than merely
    /// return more rows than they wanted.
    #[test]
    fn the_scope_filtered_lists_document_their_required_parameters() {
        let doc = document();
        for path in ["/api/v1/budgets", "/api/v1/rate-limits"] {
            let params = doc["paths"][path]["get"]["parameters"]
                .as_array()
                .unwrap_or_else(|| panic!("{path} declares its query parameters"));
            for name in ["scope_type", "scope_id"] {
                let param = params
                    .iter()
                    .find(|p| p["name"] == name)
                    .unwrap_or_else(|| panic!("{path} documents {name}"));
                assert_eq!(
                    param["required"], true,
                    "{path}'s {name} is not optional: the handler deserializes it as a bare \
                     field, so omitting it is a 400"
                );
            }
        }
    }

    /// Both remaining keyset-paged logs, which #1412 names as the reason it
    /// was filed: without the cursor written down, page two is unreachable.
    #[test]
    fn the_keyset_paged_logs_document_their_cursors() {
        let doc = document();
        for path in [
            "/api/v1/mcp/logs",
            "/api/v1/orgs/{org_id}/audit-log",
            "/api/v1/audit-log",
        ] {
            let params = doc["paths"][path]["get"]["parameters"]
                .as_array()
                .unwrap_or_else(|| panic!("{path} declares its query parameters"));
            let names: BTreeSet<&str> = params
                .iter()
                .map(|p| p["name"].as_str().expect("a parameter has a name"))
                .collect();
            assert!(names.contains("cursor"), "{path}: {names:?}");
            assert!(names.contains("limit"), "{path}: {names:?}");
        }
    }

    /// The operations that seal a credential with `ROLTER_KEK` document the
    /// `503` `kek_unavailable` refusal, and no other operation claims it (#3218).
    #[test]
    fn the_credential_sealing_operations_document_the_kek_refusal() {
        let doc = document();
        let sealing = [
            ("post", "/api/v1/orgs/{org_id}/providers"),
            ("put", "/api/v1/providers/{id}"),
            ("post", "/api/v1/orgs/{org_id}/mcp-servers"),
            ("patch", "/api/v1/mcp-servers/{id}"),
            ("put", "/api/v1/mcp-servers/{id}/auth"),
            ("put", "/api/v1/mcp-servers/{id}/oauth-client"),
            ("post", "/api/v1/alert-channels"),
            ("put", "/api/v1/alert-channels/{id}"),
            ("post", "/api/v1/connectors"),
            ("put", "/api/v1/connectors/{id}"),
            ("post", "/api/v1/orgs/{org_id}/sso-providers"),
            ("put", "/api/v1/sso-providers/{id}"),
            ("post", "/api/v1/me/mfa/enroll"),
            ("post", "/api/v1/auth/mfa/enroll"),
        ];
        for (method, path) in sealing {
            assert!(
                doc["paths"][path][method]["responses"]["503"].is_object(),
                "{method} {path} seals a credential, so it must document the 503"
            );
        }
        let mut documented = 0;
        for (path, item) in doc["paths"].as_object().expect("paths") {
            for (method, op) in item.as_object().into_iter().flatten() {
                if op["responses"]["503"].is_object() {
                    documented += 1;
                    assert!(
                        sealing.contains(&(method.as_str(), path.as_str())),
                        "{method} {path} documents a 503 but is not a sealing operation"
                    );
                }
            }
        }
        assert_eq!(documented, sealing.len());
    }

    #[test]
    fn the_source_scan_finds_the_routes_it_is_meant_to() {
        let routes = registered_routes();
        // a sanity floor: the scan is the whole guard, so a regression that
        // made it find nothing would silently pass every other test here
        assert!(routes.len() > 150, "only found {} routes", routes.len());
        assert!(routes.contains(&("/api/v1/orgs".into(), "get".into())));
        assert!(routes.contains(&("/api/v1/orgs".into(), "post".into())));
        assert!(routes.contains(&("/gw/{path}".into(), "any".into())));
        // `#[cfg(test)]` fixtures stand up their own routers; none of them count
        assert!(!routes.contains(&("/v1/ping".into(), "get".into())));
    }

    #[test]
    fn every_windowed_read_documents_its_400() {
        // #3506: a bound outside the years a window may name is a coded 400,
        // and so is a bucket that cuts the window too fine
        let doc = document();
        let mut windowed = 0;
        for (path, item) in doc["paths"].as_object().expect("paths") {
            for (method, op) in item.as_object().expect("an item") {
                let takes_since = op["parameters"].as_array().is_some_and(|ps| {
                    ps.iter()
                        .any(|p| p["in"] == "query" && p["name"] == "since")
                });
                if !takes_since {
                    continue;
                }
                windowed += 1;
                let description = op["responses"]["400"]["description"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{method} {path} declares no 400"));
                assert!(
                    description.contains("time_bound_out_of_range"),
                    "{method} {path}: {description}"
                );
            }
        }
        assert!(windowed >= 15, "{windowed}");
        // the reads the extractor holds to the bucket limit are the ones the
        // document says it holds, and each documents the parameter it names
        let mut bucketed: Vec<&str> = operations()
            .iter()
            .filter(|op| op.bucketed)
            .map(|op| op.path)
            .collect();
        bucketed.sort_unstable();
        let mut checked = crate::time_bounds::BUCKETED_READS.to_vec();
        checked.sort_unstable();
        assert_eq!(bucketed, checked);
        for path in checked {
            let params = doc["paths"][path]["get"]["parameters"]
                .as_array()
                .unwrap_or_else(|| panic!("{path} documents no parameters"));
            assert!(
                params.iter().any(|p| p["name"] == "bucket"),
                "{path} documents no bucket"
            );
        }
        for path in [
            "/api/v1/analytics/timeseries",
            "/api/v1/analytics/timeseries/export",
            "/api/v1/health/timeline",
        ] {
            assert!(
                doc["paths"][path]["get"]["responses"]["400"]["description"]
                    .as_str()
                    .is_some_and(|d| d.contains("too_many_buckets")
                        && d.contains(&crate::time_bounds::MAX_BUCKETS.to_string())),
                "{path}"
            );
        }
    }

    #[test]
    fn clickhouse_reads_document_the_timeout_code() {
        let doc = document();
        let responses = &doc["paths"]["/api/v1/analytics/summary"]["get"]["responses"];
        assert!(responses["504"]["description"]
            .as_str()
            .is_some_and(|d| d.contains("analytics_query_timeout")));
        assert!(responses["502"].is_object());
        let mcp = &doc["paths"]["/api/v1/mcp/logs/{event_id}"]["get"]["responses"];
        assert!(mcp["504"].is_object());
    }
}
