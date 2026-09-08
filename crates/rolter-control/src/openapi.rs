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
    /// reachable without a credential
    public: bool,
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
            public: false,
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

    fn public(mut self) -> Self {
        self.public = true;
        self
    }

    fn to_json(self) -> Value {
        let mut op = Map::new();
        op.insert("summary".into(), json!(self.summary));
        op.insert("operationId".into(), json!(self.id));
        op.insert("tags".into(), json!([self.tag]));
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

/// Stamp one tag across a group of operations, so the table reads as the
/// grouping the dashboard and the SDKs use rather than repeating a literal.
fn tagged(tag: &'static str, ops: Vec<Op>) -> Vec<Op> {
    ops.into_iter().map(|op| Op { tag, ..op }).collect()
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
            Op::get("/api/v1/ping", "ping", "Round-trip check for the dashboard"),
            Op::get(
                "/api/v1/version",
                "getVersion",
                "Running version and any available update",
            ),
            Op::get(
                "/api/v1/config",
                "getConfig",
                "The assembled gateway configuration",
            ),
            Op::get(
                "/api/v1/config/problems",
                "getConfigProblems",
                "Configuration problems detected in the assembled config",
            ),
            Op::get(
                "/api/v1/config/export",
                "exportConfig",
                "The live configuration as an importable rolter.toml",
            ),
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
            ),
            Op::post(
                "/internal/adaptive-telemetry",
                "ingestAdaptiveTelemetry",
                "Accept adaptive-routing samples pushed by a gateway",
            ),
        ],
    ));

    ops.extend(tagged(
        "auth",
        vec![
            Op::post(
                "/api/v1/auth/login",
                "login",
                "Exchange email and password for a session token, or for a \
                 second-factor challenge when the account has one armed",
            )
            .public(),
            Op::post(
                "/api/v1/auth/mfa/verify",
                "verifyMfaChallenge",
                "Redeem a second-factor challenge for a session token",
            )
            .public(),
            Op::post(
                "/api/v1/auth/logout",
                "logout",
                "Revoke the current session",
            ),
            Op::get(
                "/api/v1/auth/me",
                "authMe",
                "The account behind the current session",
            ),
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
            ),
            Op::get(
                "/api/v1/orgs/{org_id}/teams",
                "listTeams",
                "List an organization's teams",
            )
            .ok(Payload::List("Team")),
            Op::post("/api/v1/orgs/{org_id}/teams", "createTeam", "Create a team")
                .body(Payload::Ref("CreateTeam"))
                .ok(Payload::Ref("Team")),
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
            Op::delete("/api/v1/projects/{id}", "deleteProject", "Delete a project"),
            Op::get(
                "/api/v1/orgs/{org_id}/business-units",
                "listBusinessUnits",
                "List business units",
            )
            .ok(Payload::List("BusinessUnit")),
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
            .ok(Payload::List("Customer")),
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
                .ok(Payload::Ref("User")),
            Op::delete("/api/v1/users/{id}", "deleteUser", "Delete an account"),
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
            Op::delete(
                "/api/v1/memberships/{id}",
                "deleteMembership",
                "Revoke a role grant",
            ),
        ],
    ));

    ops.extend(tagged(
        "access-control",
        vec![
            Op::get(
                "/api/v1/rbac/matrix",
                "getRbacMatrix",
                "The capability matrix every guard enforces",
            ),
            Op::get(
                "/api/v1/rbac/effective",
                "getEffectiveRbac",
                "What the calling principal may do at a scope",
            ),
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
                "List access profiles",
            ),
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
            .body(Payload::Ref("CreateProvider"))
            .ok(Payload::Ref("Provider")),
            Op::put(
                "/api/v1/providers/{id}",
                "updateProvider",
                "Update an upstream provider",
            )
            .body(Payload::Ref("UpdateProvider"))
            .ok(Payload::Ref("Provider")),
            Op::delete(
                "/api/v1/providers/{id}",
                "deleteProvider",
                "Delete an upstream provider",
            ),
            Op::post(
                "/api/v1/providers/{id}/test",
                "testProvider",
                "Probe a provider's credentials and reachability",
            )
            .body(Payload::Empty),
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
                "List every public model name served",
            ),
            Op::delete(
                "/api/v1/models/{model}",
                "deleteModel",
                "Delete every route publishing a model name",
            ),
            Op::get(
                "/api/v1/projects/{project_id}/routes",
                "listRoutes",
                "List a project's routes",
            )
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
                "setRouteEnabled",
                "Enable or disable a route",
            )
            .body(Payload::Ref("SetRouteEnabled"))
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
                "Replace a route's catalog metadata and execution policy",
            )
            .ok(Payload::Ref("Route")),
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
            ),
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
                .ok(Payload::List("Budget")),
            Op::post("/api/v1/budgets", "createBudget", "Create a spend budget")
                .body(Payload::Ref("CreateBudget"))
                .ok(Payload::Ref("Budget")),
            Op::delete(
                "/api/v1/budgets/{id}",
                "deleteBudget",
                "Delete a spend budget",
            ),
            Op::get("/api/v1/rate-limits", "listRateLimits", "List rate limits")
                .ok(Payload::List("RateLimit")),
            Op::post(
                "/api/v1/rate-limits",
                "createRateLimit",
                "Create a rate limit",
            )
            .body(Payload::Ref("CreateRateLimit"))
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
                "/api/v1/guardrails/providers",
                "listGuardrailProviders",
                "List guardrail providers",
            ),
            Op::post(
                "/api/v1/guardrails/providers",
                "createGuardrailProvider",
                "Register a guardrail provider",
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
            ),
            Op::patch(
                "/api/v1/mcp-servers/{id}",
                "updateMcpServer",
                "Update a registered MCP server",
            ),
            Op::delete(
                "/api/v1/mcp-servers/{id}",
                "deleteMcpServer",
                "Delete a registered MCP server",
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
            ),
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
            .public(),
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
            ),
            Op::get(
                "/api/v1/mcp/logs/summary",
                "getMcpLogSummary",
                "Aggregate MCP tool-call activity",
            ),
            Op::get(
                "/api/v1/mcp/logs/{event_id}",
                "getMcpLogEvent",
                "Read one MCP tool-call event",
            ),
        ],
    ));

    ops.extend(tagged(
        "alerting",
        vec![
            Op::get(
                "/api/v1/alert-channels",
                "listAlertChannels",
                "List alert channels",
            ),
            Op::post(
                "/api/v1/alert-channels",
                "createAlertChannel",
                "Create an alert channel",
            ),
            Op::put(
                "/api/v1/alert-channels/{id}",
                "updateAlertChannel",
                "Update an alert channel",
            ),
            Op::delete(
                "/api/v1/alert-channels/{id}",
                "deleteAlertChannel",
                "Delete an alert channel",
            ),
            Op::get("/api/v1/alert-rules", "listAlertRules", "List alert rules"),
            Op::post(
                "/api/v1/alert-rules",
                "createAlertRule",
                "Create an alert rule",
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
                "Page delivered alert notifications",
            ),
        ],
    ));

    ops.extend(tagged(
        "analytics",
        vec![
            Op::get(
                "/api/v1/analytics/summary",
                "getAnalyticsSummary",
                "Spend, tokens and request counts over a window",
            ),
            Op::get(
                "/api/v1/analytics/timeseries",
                "getAnalyticsTimeseries",
                "Bucketed spend and usage over a window",
            ),
            Op::get(
                "/api/v1/analytics/by-model",
                "getAnalyticsByModel",
                "Spend and usage grouped by model",
            ),
            Op::get(
                "/api/v1/analytics/by-attribution",
                "getAnalyticsByAttribution",
                "Spend and usage grouped by business unit or customer",
            ),
            Op::get(
                "/api/v1/analytics/invocations",
                "listInvocations",
                "Page individual request records",
            ),
        ],
    ));

    ops.extend(tagged(
        "health",
        vec![
            Op::get(
                "/api/v1/health/uptime",
                "getUptime",
                "Per-provider uptime over a window",
            ),
            Op::get(
                "/api/v1/health/timeline",
                "getHealthTimeline",
                "Per-provider health transitions over a window",
            ),
            Op::get(
                "/api/v1/health/mttr",
                "getMttr",
                "Mean time to recovery per provider",
            ),
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
        ],
    ));

    ops.extend(tagged(
        "cluster",
        vec![
            Op::get(
                "/api/v1/cluster/nodes",
                "listClusterNodes",
                "List gateway nodes that have registered",
            ),
            Op::put(
                "/api/v1/cluster/nodes/{id}/drain",
                "setClusterNodeDrain",
                "Drain or un-drain a gateway node",
            ),
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
            ),
            Op::put(
                "/api/v1/connectors/{id}",
                "updateConnector",
                "Update an observability connector",
            ),
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
                "Read the request-logging and retention settings",
            ),
            Op::put(
                "/api/v1/logging-settings",
                "updateLoggingSettings",
                "Replace the request-logging and retention settings",
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
            ),
            Op::put(
                "/api/v1/sso-providers/{id}",
                "updateSsoProvider",
                "Update an SSO provider in place",
            ),
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
            .public(),
            Op::get(
                "/auth/sso/{slug}/callback",
                "ssoCallback",
                "OAuth/OIDC redirect target for an SSO login",
            )
            .public(),
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

    json!({
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
    })
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
    ] {
        if let Value::Object(entries) = group {
            out.extend(entries);
        }
    }
    Value::Object(out)
}

fn error_schemas(p: &Prim) -> Value {
    let string = &p.string;
    json!({
        "Error": {
            "type": "object",
            "properties": {
                "error": {
                    "type": "object",
                    "properties": {
                        "message": string,
                        "type": string,
                        "code": string,
                        "param": string
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

        "Project": {
            "type": "object",
            "required": ["id", "team_id", "name", "created_at"],
            "properties": {"id": uuid, "team_id": uuid, "name": string, "created_at": timestamp}
        },
        "CreateProject": {
            "type": "object",
            "required": ["name"],
            "properties": {"name": string},
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
                "created_at": timestamp
            }
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

        "Invitation": {
            "type": "object",
            "required": ["id", "org_id", "email", "role", "expires_at"],
            "properties": {
                "id": uuid, "org_id": uuid, "email": {"type": "string", "format": "email"},
                "role": string, "team_id": nullable_uuid, "project_id": nullable_uuid,
                "invited_by": nullable_uuid,
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
            "required": ["id", "org_id", "name", "slug", "kind", "api_base", "created_at"],
            "properties": {
                "id": uuid, "org_id": uuid, "name": string,
                "slug": {"type": "string", "description": "stable identity for `provider-slug/model` addressing"},
                "kind": {"type": "string", "description": "a kind listed by `GET /api/v1/provider-kinds`"},
                "api_base": string,
                "api_key_env": nullable_string,
                "egress_proxy": nullable_string,
                "egress_proxies": string_list,
                "created_at": timestamp
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
                "egress_proxies": string_list
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
                "egress_proxies": {"type": ["array", "null"], "items": {"type": "string"}}
            },
            "additionalProperties": false
        },

        "ProviderGroup": {
            "type": "object",
            "required": ["id", "org_id", "name", "slug", "strategy", "created_at"],
            "properties": {
                "id": uuid, "org_id": uuid, "name": string, "slug": string,
                "strategy": string, "created_at": timestamp
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
                "members": {"type": "array", "items": {"$ref": "#/components/schemas/ProviderGroupMemberInput"}}
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
                }
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
                "created_at": timestamp
            }
        },
        "CreateRoute": {
            "type": "object",
            "required": ["model"],
            "properties": {"model": string, "strategy": {"type": "string", "default": "round_robin"}},
            "additionalProperties": false
        },
        "SetRouteEnabled": {
            "type": "object",
            "required": ["enabled"],
            "properties": {"enabled": {"type": "boolean"}},
            "additionalProperties": false
        },

        "RouteTarget": {
            "type": "object",
            "required": ["id", "route_id", "provider_id", "weight", "created_at"],
            "properties": {
                "id": uuid, "route_id": uuid, "provider_id": uuid,
                "upstream_model": nullable_string,
                "weight": {"type": "integer"},
                "created_at": timestamp
            }
        },
        "CreateRouteTarget": {
            "type": "object",
            "required": ["provider_id"],
            "properties": {
                "provider_id": uuid,
                "upstream_model": nullable_string,
                "weight": {"type": "integer", "default": 1, "minimum": 1}
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
                "key_hash": {"type": "string", "description": "peppered digest; never the plaintext"},
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
                "expires_in_days": {"type": ["integer", "null"], "minimum": 1}
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
    json!({
        "Budget": {
            "type": "object",
            "required": ["id", "scope_type", "scope_id", "limit_usd", "period", "created_at"],
            "properties": {
                "id": uuid,
                "scope_type": {"type": "string", "enum": ["org", "team", "project", "virtual_key"]},
                "scope_id": uuid,
                "limit_usd": {"type": "string", "description": "decimal(12,4) as text"},
                "period": string,
                "unpriced_policy": {"type": ["string", "null"], "enum": ["ignore", "warn", "block", null]},
                "created_at": timestamp
            }
        },
        "CreateBudget": {
            "type": "object",
            "required": ["scope_type", "scope_id", "limit_usd"],
            "properties": {
                "scope_type": {"type": "string", "enum": ["org", "team", "project", "virtual_key"]},
                "scope_id": uuid,
                "limit_usd": string,
                "period": {"type": "string", "default": "30d"},
                "unpriced_policy": {"type": ["string", "null"], "enum": ["ignore", "warn", "block", null]}
            },
            "additionalProperties": false
        },

        "RateLimit": {
            "type": "object",
            "required": ["id", "scope_type", "scope_id", "created_at"],
            "properties": {
                "id": uuid,
                "scope_type": {"type": "string", "enum": ["org", "team", "project", "virtual_key"]},
                "scope_id": uuid,
                "rpm": {"type": ["integer", "null"]},
                "tpm": {"type": ["integer", "null"]},
                "created_at": timestamp
            }
        },
        "CreateRateLimit": {
            "type": "object",
            "required": ["scope_type", "scope_id"],
            "properties": {
                "scope_type": {"type": "string", "enum": ["org", "team", "project", "virtual_key"]},
                "scope_id": uuid,
                "rpm": {"type": ["integer", "null"]},
                "tpm": {"type": ["integer", "null"]}
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
                "currency": string,
                "created_at": timestamp
            }
        },
        "UpsertModelPrice": {
            "type": "object",
            "required": ["model", "input_per_mtok", "output_per_mtok"],
            "properties": {
                "model": string,
                "input_per_mtok": string,
                "output_per_mtok": string,
                "cached_input_per_mtok": nullable_string,
                "currency": {"type": "string", "default": "USD"}
            },
            "additionalProperties": false
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

    #[test]
    fn docs_page_is_self_contained() {
        let config = json!({"url": "/openapi.json", "withDefaultFonts": false});
        let html = scalar_api_reference::scalar_html(&config, Some(DOCS_BUNDLE_PATH));
        // the bundle is loaded from this control plane, never a cdn (air-gapped)
        assert!(html.contains(DOCS_BUNDLE_PATH));
        assert!(!html.contains("cdn.jsdelivr.net"));
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
}
