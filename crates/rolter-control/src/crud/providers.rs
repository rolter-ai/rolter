//! Providers: upstream credentials and endpoints, plus the connection probe
//! and model listing.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use rolter_core::Error;
use rolter_store::postgres::crypto::{Kek, KEK_ENV};
use rolter_store::postgres::models::Provider;
use rolter_store::postgres::repo::{
    NewProvider, ProjectRepo, ProviderCacheProbeRepo, ProviderDeletion, ProviderKeyRepo,
    ProviderRepo, ProviderUpdate,
};

use super::{
    explicit_null, invalid_field, log_audit, pool, publish_config_change, require_allowed_egress,
    require_non_empty, require_not_config_owned, require_row_not_config_owned, resolve_new_slug,
    resolve_slug_change, ApiError, ApiResult, Query, SafeJson, SCOPE_MISMATCH,
};
use crate::rbac::{authorize, reaches_org, Principal, ScopeChain, ScopeFilter};
use crate::rbac_matrix::{cap, Requirement};
use crate::ControlState;

use super::provider_groups::require_address_slug_free;

use super::provider_groups::taken_in_org;

pub(super) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/orgs/{org_id}/providers",
            get(list_providers).post(create_provider),
        )
        .route(
            "/api/v1/providers/{id}",
            put(update_provider).delete(delete_provider),
        )
        .route("/api/v1/providers/{id}/test", post(test_provider))
        .route("/api/v1/providers/{id}/models", get(list_provider_models))
        .route(
            "/api/v1/providers/{id}/probe-cache",
            post(probe_provider_cache),
        )
        .route(
            "/api/v1/orgs/{org_id}/provider-cache-probes",
            get(list_provider_cache_probes),
        )
}

// --- providers ---

/// What a "Test connection" attempt found.
#[derive(Serialize)]
struct ProviderTestResult {
    /// whether the probe demonstrated a working provider API — for kinds whose
    /// probe is a catalogue that means a parseable model list, not merely a
    /// 2xx. `false` with a `status` in the 2xx range is the "answered, but not
    /// with a model list" case, and `error` says so (#980)
    reachable: bool,
    /// the URL actually probed, so an operator can see what was tried rather
    /// than guessing how `api_base` was turned into an endpoint
    probed_url: String,
    /// HTTP status the upstream returned; absent when the request never
    /// completed (DNS, TLS, connection refused, timeout)
    status: Option<u16>,
    latency_ms: u64,
    /// how the credential was resolved, so "401" and "no key configured" are
    /// distinguishable without reading the provider row
    credential: &'static str,
    /// how many models the upstream listed, when it answered with a catalogue
    models_found: Option<usize>,
    /// whether the catalogue lists the `model` the caller asked about: `true`
    /// or `false` when the provider answered with a catalogue, `null` when no
    /// model was asked about or the provider cannot tell (a liveness-only
    /// probe, an unreachable provider, a body that is not a catalogue)
    model_listed: Option<bool>,
    error: Option<String>,
}

/// Query of the provider test.
#[derive(Deserialize, Default)]
struct ProviderTestQuery {
    /// an upstream model name to look up in the catalogue
    model: Option<String>,
}

/// Whether a catalogue lists `model`, compared the way
/// [`rolter_core::probe::catalogue_names`] reduces an id (last path segment), so
/// `publishers/google/models/gemini-2.5-pro` and `gemini-2.5-pro` agree.
///
/// `None` when there is nothing to check against: no model asked about, a
/// probe that failed, or a body that is not a catalogue.
fn model_listed(
    kind: rolter_core::ProviderKind,
    verdict: &ProbeVerdict,
    body: Option<&serde_json::Value>,
    model: Option<&str>,
) -> Option<bool> {
    let wanted = model?.trim();
    let wanted = wanted.rsplit('/').next().unwrap_or(wanted).trim();
    // a liveness probe's body is not evidence about models, whatever it holds
    let catalogue =
        rolter_core::probe_expectation(kind, "/") == rolter_core::ProbeExpectation::Catalogue;
    if wanted.is_empty() || !verdict.reachable || !catalogue {
        return None;
    }
    let ids = rolter_core::probe::catalogue_names(body?)?;
    Some(ids.iter().any(|id| id == wanted))
}

/// The `credential` a test reports when `api_key_env` names a variable this
/// process does not have.
///
/// The control plane resolves the variable from its own environment, while the
/// gateway resolves it from the gateway's, so this is not proof that the
/// gateway lacks it, only that the test ran without a key (#2811).
const ENV_UNSET: &str = "env (unset)";

/// What a probe response amounts to, once the body has been read.
struct ProbeVerdict {
    reachable: bool,
    models_found: Option<usize>,
    error: Option<String>,
}

/// Decide what a probe proved.
///
/// `reachable` used to be `status.is_success()` alone, so any endpoint
/// answering 2xx at the probed URL — a catch-all route, an SPA index, a reverse
/// proxy, an auth portal — reported green for a provider that could not serve a
/// single completion (#980). It is what made the doubled base in #947 look
/// healthy: the upstream answered 200 at `/v1/v1/models` with something that
/// was not a catalogue.
///
/// So for kinds whose probe *is* a model catalogue, the catalogue is the
/// evidence and the status is only the envelope. Kinds whose probe is a
/// liveness endpoint (Tei's `/health`) keep 2xx as the criterion — explicitly,
/// via [`rolter_core::probe_expectation`], rather than incidentally.
///
/// The three outcomes stay distinguishable: success, a flat failure, and
/// "answered, but not with a model list" — which is neither, and is the one an
/// operator most needs named, because the host is up and it is the URL or the
/// service behind it that is wrong.
fn judge_probe(
    kind: rolter_core::ProviderKind,
    status: u16,
    body: Option<&serde_json::Value>,
    url: &str,
    credential: &str,
    api_key_env: Option<&str>,
) -> ProbeVerdict {
    // a provider that answers 200 with zero models is reachable but not yet
    // useful, and the operator should be able to see that difference
    let models_found = body.and_then(rolter_core::count_catalogue);
    let answered = (200..300).contains(&status);
    let proved = answered
        && match rolter_core::probe_expectation(kind, "/") {
            rolter_core::ProbeExpectation::Catalogue => models_found.is_some(),
            rolter_core::ProbeExpectation::Liveness => true,
        };
    ProbeVerdict {
        reachable: proved,
        models_found: proved.then_some(models_found).flatten(),
        error: (!proved).then(|| {
            if answered {
                format!(
                    "{status}: {url} answered, but not with a model list. Something other than \
                     the provider's API is serving that URL — check api_base for a duplicated \
                     path segment, a catch-all route or a login portal."
                )
            } else {
                match status {
                    401 | 403 => match (credential, api_key_env) {
                        (ENV_UNSET, Some(var)) => format!(
                            "{status}: the upstream rejected the request, and the key's \
                             environment variable {var} is not set in the control plane's \
                             environment (resolved from: {credential}). This test runs in the \
                             control plane, so set {var} there; the gateway reads it from its \
                             own environment, so set it there too."
                        ),
                        _ => format!(
                            "{status}: the upstream rejected the credential (resolved from: \
                             {credential})"
                        ),
                    },
                    404 => format!("{status}: reached the host, but {url} is not served there"),
                    other => format!("{other}: the upstream refused the probe"),
                }
            }
        }),
    }
}

/// How a stored provider's credential is resolved for a probe, and the secret
/// itself when there is one.
///
/// The same precedence the snapshot uses: a sealed key wins over the env var.
/// The second half names where the credential came from, so a refused probe is
/// distinguishable from a missing key without reading the provider row.
async fn resolve_probe_credential(
    state: &ControlState,
    id: Uuid,
    api_key_env: Option<&str>,
) -> ApiResult<(Option<String>, &'static str)> {
    // a row saved before the refusal existed, or written by an import, may name
    // a control-plane secret; it is checked here, before any read, because the
    // probe sends the value to the row's own api_base
    if let Some(name) = api_key_env {
        crate::secret_env::refuse("api_key_env", name, "as a provider credential")?;
    }
    let sealed: Option<(Vec<u8>, Vec<u8>)> =
        sqlx::query_as("select ciphertext, nonce from provider_keys where provider_id=$1")
            .bind(id)
            .fetch_optional(pool(state))
            .await
            .map_err(rolter_store::postgres::store_err)?;
    Ok(match sealed {
        Some((ciphertext, nonce)) => match Kek::from_env() {
            Some(kek) => match kek.decrypt(&ciphertext, &nonce) {
                Ok(plaintext) => (Some(plaintext), "stored"),
                // the operator needs to know this is a KEK problem, not a
                // provider problem — the upstream is never even contacted
                Err(_) => (None, "stored (undecryptable)"),
            },
            None => (None, "stored (KEK unset)"),
        },
        None => match api_key_env.map(std::env::var) {
            Some(Ok(value)) => (Some(value), "env"),
            Some(Err(_)) => (None, ENV_UNSET),
            None => (None, "none"),
        },
    })
}

/// The model ids a stored provider's upstream lists.
#[derive(Serialize)]
struct ProviderModelList {
    /// sorted and without duplicates; empty when the upstream is down, answered
    /// with something other than a catalogue, or has no catalogue to read
    models: Vec<String>,
}

/// Most ids one listing returns. An aggregator can list thousands, and the
/// caller offers these as suggestions, not as the catalogue.
const MAX_LISTED_MODELS: usize = 2000;

/// Largest probe body read for a listing, the cap the gateway's own catalogue
/// read holds, so an upstream answering with something enormous costs a bounded
/// amount of memory.
const MAX_LISTING_BYTES: usize = 1 << 20;

/// Sorted, de-duplicated and capped ids, the shape a listing is returned in.
fn tidy_model_ids(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids.dedup();
    ids.truncate(MAX_LISTED_MODELS);
    ids
}

/// Ask a stored provider's upstream which models it serves.
///
/// The connection test counts the same catalogue and drops the ids, so the
/// route sheet had nothing to suggest for a target's upstream model and the id
/// had to be retyped to the letter (#2810). The ids are what the gateway lists
/// under `provider-slug/model` (`rolter_core::probe::catalogue_ids`).
///
/// Gated like the test, not like the provider listing: it sends the provider's
/// own credential to the upstream, authenticated the way the test does and
/// through the same egress policy, so a role that may not spend the credential
/// on a test may not spend it here either. That is `provider:update`, in the
/// provider's own scope. A provider the caller cannot see at all is answered as
/// one that does not exist. A failure at the upstream is an empty list — the
/// list only ever suggests, and "Test connection" is the call that says why a
/// provider does not answer.
async fn list_provider_models(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ProviderModelList>> {
    let provider = ProviderRepo(pool(&state)).get(id).await?;
    let org_id = provider.org_id;
    // the listing's own rule: an org-wide provider is visible to anyone with a
    // role in the org, a project-scoped one to those holding a role there. one
    // that is not visible is answered as one that does not exist
    let visible = visible_in_scope(
        &state,
        &principal,
        cap!("provider", Read),
        org_id,
        vec![provider.clone()],
        |row| row.project_id,
    )
    .await?;
    if visible.is_empty() {
        return Err(Error::NotFound(format!("provider {id}")).into());
    }
    // seeing it is not enough: the credential is spent on the call. a provider
    // scoped to a project is its project admin's to change, an org-wide one the
    // org admin's, as for an edit of the row (#1919)
    let chain = match provider.project_id {
        Some(project_id) => ScopeChain::from_project(pool(&state), project_id).await?,
        None => ScopeChain::org(org_id),
    };
    authorize(&state, &principal, chain, cap!("provider", Update)).await?;

    // the base was checked when it was stored, but the egress policy may have
    // been tightened since; a stored row is not a standing permission to egress
    require_allowed_egress(&state, &provider.api_base, "api_base")?;
    let kind = known_kind(&provider.kind)?;
    let (secret, credential) =
        resolve_probe_credential(&state, id, provider.api_key_env.as_deref()).await?;
    if credential == "stored (undecryptable)" || credential == "stored (KEK unset)" {
        return Ok(Json(ProviderModelList { models: Vec::new() }));
    }

    let (url, headers) = rolter_core::probe_request(kind, &provider.api_base, "/");
    let models =
        match send_provider_probe(&state.egress, kind, &url, headers, secret.as_deref()).await {
            Ok(resp) if resp.status().is_success() => read_listing(resp).await,
            _ => Vec::new(),
        };
    Ok(Json(ProviderModelList {
        models: tidy_model_ids(models),
    }))
}

/// The model ids in a probe response, chunk by chunk against
/// [`MAX_LISTING_BYTES`]; none when it is not a catalogue or is too large.
async fn read_listing(mut resp: reqwest::Response) -> Vec<String> {
    let mut body: Vec<u8> = Vec::new();
    while let Ok(Some(chunk)) = resp.chunk().await {
        if body.len() + chunk.len() > MAX_LISTING_BYTES {
            return Vec::new();
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|parsed| rolter_core::probe::catalogue_ids(&parsed))
        .unwrap_or_default()
}

/// Probe a stored provider and report whether it actually answers.
///
/// Configuring a provider is otherwise a write with no feedback: the row saves,
/// and the first sign that the base URL is wrong or the key is stale is a failed
/// request through the gateway, attributed to whatever route happened to select
/// it. This closes that loop at the moment the operator is looking at the form.
///
/// Probes the same free, non-inference endpoint the gateway's health sweep uses
/// (`rolter_core::probe`), so a green test means the sweep will agree. Costs
/// nothing to run: it is a model-list call, not a completion.
async fn test_provider(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Query(query): Query<ProviderTestQuery>,
) -> ApiResult<Json<ProviderTestResult>> {
    // reveals whether a credential works, so it is gated like a mutation even
    // though it writes nothing
    let row: (Uuid, Option<Uuid>, String, String, String, Option<String>) = sqlx::query_as(
        "select org_id, project_id, name, kind, api_base, api_key_env from providers where id=$1",
    )
    .bind(id)
    .fetch_optional(pool(&state))
    .await
    .map_err(rolter_store::postgres::store_err)?
    .ok_or_else(|| Error::NotFound(format!("provider {id}")))?;
    let (org_id, project_id, name, kind, api_base, api_key_env) = row;

    // in the provider's own scope, as for an edit of the row (#1919): a
    // project-scoped provider is its project admin's to test, which is what
    // lets the model sheet check an upstream model without an org admin (#2008)
    let chain = match project_id {
        Some(project_id) => ScopeChain::from_project(pool(&state), project_id).await?,
        None => ScopeChain::org(org_id),
    };
    authorize(&state, &principal, chain, cap!("provider", Update)).await?;

    // the base was checked when it was stored, but the egress policy may have
    // been tightened since; a stored row is not a standing permission to egress
    require_allowed_egress(&state, &api_base, "api_base")?;

    let parsed_kind = known_kind(&kind)?;

    let (secret, credential) = resolve_probe_credential(&state, id, api_key_env.as_deref()).await?;

    if credential == "stored (undecryptable)" || credential == "stored (KEK unset)" {
        return Ok(Json(ProviderTestResult {
            reachable: false,
            probed_url: String::new(),
            status: None,
            latency_ms: 0,
            credential,
            models_found: None,
            model_listed: None,
            error: Some(format!(
                "the stored credential for '{name}' could not be read: {KEK_ENV} is unset or \
                 does not match the key it was sealed with. The upstream was not contacted."
            )),
        }));
    }

    let (url, headers) = rolter_core::probe_request(parsed_kind, &api_base, "/");
    let started = std::time::Instant::now();
    let outcome =
        send_provider_probe(&state.egress, parsed_kind, &url, headers, secret.as_deref()).await;
    let latency_ms = started.elapsed().as_millis() as u64;

    let result = match outcome {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.json::<serde_json::Value>().await.ok();
            let verdict = judge_probe(
                parsed_kind,
                status,
                body.as_ref(),
                &url,
                credential,
                api_key_env.as_deref(),
            );
            let model_listed =
                model_listed(parsed_kind, &verdict, body.as_ref(), query.model.as_deref());
            ProviderTestResult {
                reachable: verdict.reachable,
                probed_url: url.clone(),
                status: Some(status),
                latency_ms,
                credential,
                models_found: verdict.models_found,
                model_listed,
                error: verdict.error,
            }
        }
        // never surface the raw reqwest error: it can carry the full URL
        // including a query-string credential for kinds that authenticate that way
        Err(e) => ProviderTestResult {
            reachable: false,
            probed_url: url,
            status: None,
            latency_ms,
            credential,
            models_found: None,
            model_listed: None,
            error: Some(if e.is_timeout() {
                "no response within 10s".to_string()
            } else if e.is_connect() {
                "could not connect — check the host, port and TLS".to_string()
            } else {
                "the request could not be completed".to_string()
            }),
        },
    };

    Ok(Json(result))
}

/// Send the probe request through the connect-time egress client.
///
/// Not `ControlState::http`: that client classifies only IP literals and
/// follows redirects, so a name that resolves to link-local, or an upstream
/// answering `302 Location: http://169.254.169.254/`, would reach a denied
/// address. The probe has never used the provider's `egress_proxy`, and still
/// does not, so there is no proxy path to keep.
///
/// The credential goes out the way `kind`'s API reads it (`x-api-key` for
/// Anthropic, `api-key` for Azure, bearer for most), through the same function
/// the proxy authenticates with. A bearer token to an API that ignores it reads
/// as a rejected key, and every Anthropic provider failed its first test that
/// way (#2806).
async fn send_provider_probe(
    egress: &std::sync::Arc<rolter_core::EgressPolicy>,
    kind: rolter_core::ProviderKind,
    url: &str,
    headers: impl IntoIterator<Item = (String, String)>,
    secret: Option<&str>,
) -> Result<reqwest::Response, reqwest::Error> {
    let client = crate::egress_client::builder(egress)
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let mut req = client.get(url);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    if let Some(secret) = secret {
        let (name, value) = kind.auth_header(secret);
        req = req.header(name, value.as_ref());
    }
    req.send().await
}

/// A provider as the API returns it: the row plus whether a sealed key is
/// stored for it.
///
/// The flag is derived from `provider_keys` and says nothing else about the
/// credential; the key, its ciphertext and its nonce never leave the store.
#[derive(Serialize)]
struct ProviderView {
    #[serde(flatten)]
    provider: Provider,
    /// a row exists in `provider_keys`; an `api_key_env` is not a stored key
    has_stored_key: bool,
}

async fn list_providers(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<ProviderView>>> {
    let rows = ProviderRepo(pool(&state)).list(org_id).await?;
    let visible = visible_in_scope(
        &state,
        &principal,
        cap!("provider", Read),
        org_id,
        rows,
        |row| row.project_id,
    )
    .await?;
    // after the scope filter so only rows the caller may see are looked up
    let ids: Vec<Uuid> = visible.iter().map(|row| row.id).collect();
    let stored = ProviderKeyRepo(pool(&state)).stored_among(&ids).await?;
    let views = visible
        .into_iter()
        .map(|provider| ProviderView {
            has_stored_key: stored.contains(&provider.id),
            provider,
        })
        .collect();
    Ok(Json(views))
}

/// The rows of an org listing the caller may see, where a row may be scoped to
/// one project (#1919).
///
/// Holding the read role at the org shows every row. Below it, a caller who
/// holds a role somewhere in the org sees the org-wide rows, which every
/// project may use, plus the rows scoped to a project they hold the role in;
/// a caller with no role in the org at all is refused, as the listing always
/// did.
pub(super) async fn visible_in_scope<T>(
    state: &ControlState,
    principal: &Principal,
    requirement: Requirement,
    org_id: Uuid,
    rows: Vec<T>,
    project_of: impl Fn(&T) -> Option<Uuid>,
) -> ApiResult<Vec<T>> {
    let filter = ScopeFilter::load(state, principal, requirement).await?;
    if filter.allows(ScopeChain::org(org_id)) {
        return Ok(rows);
    }
    let reach = filter.reach(pool(state)).await?;
    if !reaches_org(&reach, org_id) {
        return Err(ApiError::Forbidden);
    }
    let mut chains: HashMap<Uuid, ScopeChain> = HashMap::new();
    let mut visible = Vec::new();
    for row in rows {
        let Some(project) = project_of(&row) else {
            visible.push(row);
            continue;
        };
        let chain = match chains.get(&project) {
            Some(chain) => *chain,
            None => {
                let chain = ScopeChain::from_project(pool(state), project).await?;
                chains.insert(project, chain);
                chain
            }
        };
        if filter.allows(chain) {
            visible.push(row);
        }
    }
    Ok(visible)
}

/// The project a provider or group is being scoped to, checked against the org
/// it lives in: a project of another org is refused, and so is an unknown one,
/// with the same message so neither confirms what exists elsewhere (#1919).
pub(super) async fn require_scope_project(
    state: &ControlState,
    org_id: Uuid,
    project_id: Uuid,
) -> ApiResult<()> {
    if ProjectRepo(pool(state)).in_org(project_id, org_id).await? {
        return Ok(());
    }
    Err(invalid_field(
        "project_id",
        "project_id must name a project of this organization",
    ))
}

/// Refuse a route or group owned by `owner` (a route's project, a group's
/// scope, `None` for an org-wide group) using a provider scoped to a different
/// project, since the provider would then be reachable from outside its
/// project through it (#1919).
pub(super) async fn require_providers_usable_from(
    state: &ControlState,
    provider_ids: &[Uuid],
    owner: Option<Uuid>,
    what: &str,
) -> ApiResult<()> {
    let outside = ProviderRepo(pool(state))
        .scoped_outside(provider_ids, owner)
        .await?;
    if outside.is_empty() {
        return Ok(());
    }
    let owner_note = if owner.is_some() {
        "a different project"
    } else {
        "a project, and an org-wide group would expose it to every project"
    };
    Err(ApiError::CodedConflict {
        code: SCOPE_MISMATCH,
        message: format!(
            "{what} cannot use provider{} {}: scoped to {owner_note}; use org-wide providers \
             or ones scoped to the same project",
            if outside.len() == 1 { "" } else { "s" },
            outside
                .iter()
                .map(|name| format!("'{name}'"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateProvider {
    name: String,
    /// stable URL-safe identity; when omitted it is derived from `name`
    slug: Option<String>,
    kind: String,
    api_base: String,
    /// upstream credential, sealed with AES-256-GCM (`ROLTER_KEK`) before it
    /// reaches the database; never returned by the API
    api_key: Option<String>,
    api_key_env: Option<String>,
    egress_proxy: Option<String>,
    #[serde(default)]
    egress_proxies: Vec<String>,
    /// vLLM kv-event stream `precise_cache_aware` reads; omit for none
    #[serde(default)]
    kv_events: Option<rolter_core::KvEventsConfig>,
    /// LMCache controller `lmcache_aware` polls; omit for none
    #[serde(default)]
    lmcache: Option<rolter_core::LmCacheConfig>,
    /// scope the provider to one project of the org: only keys minted in it may
    /// reach the provider, through a route or `provider-slug/model`. Omit for an
    /// org-wide provider, usable by every project
    #[serde(default)]
    project_id: Option<Uuid>,
    /// opt out of the host pin a hosted kind carries, so `api_base` may point
    /// at an egress proxy, a vendor-compatible endpoint or a local fake
    /// (ADR-0029). The provider's key then travels to that host
    #[serde(default)]
    allow_custom_api_base: bool,
}

const PROVIDER_KINDS: [&str; 50] = [
    "openai",
    "anthropic",
    "openai_compatible",
    "ollama",
    "ollama_cloud",
    "llama_cpp",
    "openrouter",
    "tei",
    "azure_openai",
    "bedrock",
    "vertex",
    "gemini",
    "gemini_native",
    "gemini_interactions",
    "mistral",
    "groq",
    "xai",
    "meta_llama_api",
    "cohere",
    "perplexity",
    "together",
    "fireworks",
    "databricks",
    "aleph_alpha",
    "nebius",
    "ovhcloud",
    "scaleway",
    "deepseek",
    "qwen",
    "zhipu",
    "kimi",
    "ernie",
    "doubao",
    "hunyuan",
    "yi",
    "minimax",
    "baichuan",
    "gigachat",
    "yandex_gpt",
    "cloud_ru",
    "mts_ai",
    "naver",
    "upstage",
    "rinna",
    "rakuten",
    "sarvam",
    "krutrim",
    "falcon",
    "system_one",
    "infinity",
];

/// Seal `api_key` with the deployment KEK for at-rest storage. An empty or
/// whitespace-only key is rejected; a missing `ROLTER_KEK` is a client-visible
/// configuration error rather than a silent plaintext fallback.
fn seal_api_key(api_key: &str) -> ApiResult<(Vec<u8>, Vec<u8>)> {
    use rolter_store::postgres::crypto::{Kek, KEK_ENV};
    require_non_empty(api_key, "api_key")?;
    let Some(kek) = Kek::from_env() else {
        return Err(ApiError::KekUnavailable(format!(
            // only the control plane holds the KEK: it unseals provider keys
            // when it builds the snapshot, and gateways receive them over the
            // token-guarded /internal/snapshot, so naming the gateway here
            // would send operators to copy the KEK where it is not needed
            "storing provider keys requires the {KEK_ENV} environment variable on the \
             control plane to seal them at rest"
        )));
    };
    Ok(kek.encrypt(api_key)?)
}

/// A stored kind as a [`rolter_core::ProviderKind`] this build can dial.
///
/// A kind from a newer release decodes as `Unknown` rather than failing
/// (#3347); a probe must not dial it, so it is refused here like a kind that
/// does not parse.
fn known_kind(kind: &str) -> ApiResult<rolter_core::ProviderKind> {
    match serde_json::from_value(serde_json::Value::String(kind.to_string())) {
        Ok(rolter_core::ProviderKind::Unknown) | Err(_) => {
            Err(ApiError::Curated(format!("unknown provider kind '{kind}'")))
        }
        Ok(parsed) => Ok(parsed),
    }
}

fn validate_kind(kind: &str) -> ApiResult<()> {
    if !PROVIDER_KINDS.contains(&kind) {
        return Err(invalid_field(
            "kind",
            format!("kind must be one of {PROVIDER_KINDS:?}"),
        ));
    }
    Ok(())
}

/// Reject an `api_base` that would double the version prefix.
///
/// For openai-shaped kinds the gateway appends `/v1/chat/completions` itself, so
/// a base that already ends in `/v1` resolves to `/v1/v1/chat/completions` and
/// every request 404s. The dashboard used to actively teach this mistake, its
/// base-URL placeholder being `https://api.openai.com/v1` for all kinds (#947).
///
/// Refused rather than silently trimmed: the operator pasted that URL from
/// somewhere, and quietly rewriting it hides the fact that this provider kind
/// wants a different base than the one they were reading about.
/// Refuse an `api_base` that breaks the host pin of a hosted kind unless the
/// provider opts out, as `provider_problems` refuses it in a file (#1133).
/// Without this the row saves and the snapshot then omits the provider.
fn require_pinned_api_base(kind: &str, api_base: &str, allow_custom: bool) -> ApiResult<()> {
    let Ok(parsed) = known_kind(kind) else {
        // an unknown kind is validate_kind's error to report, not this one's
        return Ok(());
    };
    match parsed.host_pin_problem(api_base, allow_custom) {
        Some(problem) => Err(invalid_field("api_base", problem)),
        None => Ok(()),
    }
}

fn require_wellformed_api_base(kind: &str, api_base: &str) -> ApiResult<()> {
    let Ok(parsed) = known_kind(kind) else {
        // an unknown kind is validate_kind's error to report, not this one's
        return Ok(());
    };
    match parsed.api_base_problem(api_base) {
        Some(problem) => Err(invalid_field("api_base", format!("api_base: {problem}"))),
        None => Ok(()),
    }
}

async fn create_provider(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateProvider>,
) -> ApiResult<Json<ProviderView>> {
    // a project-scoped provider is authorised at its project, so a project admin
    // may create one for their own project. Naming an environment variable is
    // a read of the control plane's environment, which is an org admin's call
    // whatever the scope, so that half is checked at the org
    if let Some(project_id) = body.project_id {
        require_scope_project(&state, org_id, project_id).await?;
        let chain = ScopeChain::from_project(pool(&state), project_id).await?;
        authorize(&state, &principal, chain, cap!("provider", Create)).await?;
    }
    if body.project_id.is_none() || has_env_name(&body.api_key_env) {
        authorize(
            &state,
            &principal,
            ScopeChain::org(org_id),
            cap!("provider", Create),
        )
        .await?;
    }
    require_non_empty(&body.name, "name")?;
    require_not_config_owned(&state.config_owned.providers, &body.name, "provider")?;
    require_non_empty(&body.api_base, "api_base")?;
    require_allowed_egress(&state, &body.api_base, "api_base")?;
    if let Some(name) = body.api_key_env.as_deref() {
        crate::secret_env::refuse("api_key_env", name, "as a provider credential")?;
    }
    validate_kind(&body.kind)?;
    require_wellformed_api_base(&body.kind, &body.api_base)?;
    require_pinned_api_base(&body.kind, &body.api_base, body.allow_custom_api_base)?;
    require_valid_telemetry(
        &state,
        &body.name,
        body.kv_events.as_ref(),
        body.lmcache.as_ref(),
    )?;
    let slug = resolve_new_slug(&body.name, body.slug.as_deref())?;
    let providers = ProviderRepo(pool(&state));
    if providers.name_in_use(&body.name, Some(org_id)).await? {
        return Err(taken_in_org("provider name", &body.name));
    }
    require_address_slug_free(&state, "provider slug", &slug, org_id, None).await?;
    // seal before touching the database so a missing KEK leaves no row behind
    let sealed = body.api_key.as_deref().map(seal_api_key).transpose()?;
    let row = ProviderRepo(pool(&state))
        .create(
            org_id,
            NewProvider {
                name: &body.name,
                slug: &slug,
                kind: &body.kind,
                api_base: &body.api_base,
                api_key_env: body.api_key_env.as_deref(),
                egress_proxy: body.egress_proxy.as_deref(),
                egress_proxies: &body.egress_proxies,
                kv_events: body.kv_events.as_ref(),
                lmcache: body.lmcache.as_ref(),
                project_id: body.project_id,
                allow_custom_api_base: body.allow_custom_api_base,
            },
        )
        .await?;
    let has_stored_key = sealed.is_some();
    if let Some((ciphertext, nonce)) = sealed {
        ProviderKeyRepo(pool(&state))
            .set(row.id, &ciphertext, &nonce)
            .await?;
    }
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "provider.create",
        "provider",
        row.id,
        serde_json::json!({
            "name": row.name, "slug": row.slug, "kind": row.kind, "project_id": row.project_id
        }),
    )
    .await;
    Ok(Json(ProviderView {
        provider: row,
        has_stored_key,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateProvider {
    /// new slug; rejected unless `allow_slug_change` is true, since the slug is
    /// a stable identity that addresses (`provider-slug/model`) depend on
    slug: Option<String>,
    /// explicit opt-in to rename an immutable slug
    #[serde(default)]
    allow_slug_change: bool,
    /// omit to leave unchanged
    kind: Option<String>,
    /// omit to leave unchanged
    api_base: Option<String>,
    /// omit to leave the stored credential unchanged; empty string deletes
    /// it; anything else rotates it
    api_key: Option<String>,
    /// omit to leave unchanged; empty string clears
    api_key_env: Option<String>,
    /// omit to leave unchanged; empty string clears
    egress_proxy: Option<String>,
    /// omit to leave unchanged; an empty array clears
    egress_proxies: Option<Vec<String>>,
    /// omit to leave unchanged; `null` clears; an object replaces the whole
    /// setting (omitted fields inside it take their defaults)
    #[serde(default, deserialize_with = "explicit_null")]
    kv_events: Option<Option<rolter_core::KvEventsConfig>>,
    /// omit to leave unchanged; `null` clears; see `kv_events`
    #[serde(default, deserialize_with = "explicit_null")]
    lmcache: Option<Option<rolter_core::LmCacheConfig>>,
    /// omit to leave the scope unchanged; a project id scopes the provider to
    /// that project; `null` makes it org-wide again
    #[serde(default, deserialize_with = "explicit_null")]
    project_id: Option<Option<Uuid>>,
    /// omit to leave unchanged; see [`CreateProvider`]
    allow_custom_api_base: Option<bool>,
}

/// Hold a provider's `kv_events` and `lmcache` to the rules the file config
/// applies, and to the egress policy: the gateway dials both endpoints, so an
/// SSRF target must be refused at write time like `api_base`.
fn require_valid_telemetry(
    state: &ControlState,
    provider: &str,
    kv_events: Option<&rolter_core::KvEventsConfig>,
    lmcache: Option<&rolter_core::LmCacheConfig>,
) -> ApiResult<()> {
    if let Some(kv) = kv_events {
        require_allowed_egress(state, &kv.endpoint, "kv_events.endpoint")?;
        if let Some(problem) = kv.shape_problems(provider).into_iter().next() {
            return Err(ApiError::Core(Error::Config(problem)));
        }
    }
    if let Some(lmcache) = lmcache {
        require_allowed_egress(state, &lmcache.endpoint, "lmcache.endpoint")?;
        if let Some(problem) = lmcache.shape_problems(provider).into_iter().next() {
            return Err(ApiError::Core(Error::Config(problem)));
        }
    }
    Ok(())
}

/// Whether a request names an environment variable to read a credential from.
fn has_env_name(field: &Option<String>) -> bool {
    field.as_deref().is_some_and(|name| !name.trim().is_empty())
}

/// Map an optional string field to the repo's tri-state: omitted = unchanged,
/// empty = clear, otherwise = set.
fn tri_state(field: &Option<String>) -> Option<Option<&str>> {
    field
        .as_deref()
        .map(|v| Some(v.trim()).filter(|v| !v.is_empty()))
}

async fn update_provider(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateProvider>,
) -> ApiResult<Json<ProviderView>> {
    let existing = ProviderRepo(pool(&state)).get(id).await?;
    let org_id = existing.org_id;
    // an org-wide provider is an org admin's, and so is any change of scope or
    // of the environment variable a credential is read from. Only edits inside
    // a provider already scoped to a project are the project admin's (#1919)
    match existing.project_id {
        Some(project_id) if body.project_id.is_none() && !has_env_name(&body.api_key_env) => {
            let chain = ScopeChain::from_project(pool(&state), project_id).await?;
            authorize(&state, &principal, chain, cap!("provider", Update)).await?;
        }
        _ => {
            authorize(
                &state,
                &principal,
                ScopeChain::org(existing.org_id),
                cap!("provider", Update),
            )
            .await?;
        }
    }
    require_row_not_config_owned(
        &state.config_owned.provider_rows,
        existing.org_id,
        &existing.name,
        "provider",
    )?;
    if let Some(Some(project_id)) = body.project_id {
        require_scope_project(&state, org_id, project_id).await?;
    }
    if let Some(scope) = body.project_id {
        let dependents = ProviderRepo(pool(&state))
            .dependents_outside(id, scope)
            .await?;
        if !dependents.is_empty() {
            return Err(ApiError::CodedConflict {
                code: SCOPE_MISMATCH,
                message: format!(
                    "provider '{}' is used by {}, which belong to other projects; remove it \
                     from them before scoping it to one project",
                    existing.name,
                    dependents.join(", ")
                ),
            });
        }
    }
    if let Some(kind) = &body.kind {
        validate_kind(kind)?;
    }
    if let Some(name) = body.api_key_env.as_deref() {
        crate::secret_env::refuse("api_key_env", name, "as a provider credential")?;
    }
    if let Some(api_base) = &body.api_base {
        require_non_empty(api_base, "api_base")?;
        require_allowed_egress(&state, api_base, "api_base")?;
        // an edit may change either half of the pair, so check the base
        // against the kind this provider will have once the update lands
        require_wellformed_api_base(body.kind.as_deref().unwrap_or(&existing.kind), api_base)?;
    }
    // only an edit that touches the pin's inputs is held to it, so a row saved
    // before the check existed can still take an unrelated change
    if body.kind.is_some() || body.api_base.is_some() || body.allow_custom_api_base.is_some() {
        require_pinned_api_base(
            body.kind.as_deref().unwrap_or(&existing.kind),
            body.api_base.as_deref().unwrap_or(&existing.api_base),
            body.allow_custom_api_base
                .unwrap_or(existing.allow_custom_api_base),
        )?;
    }
    require_valid_telemetry(
        &state,
        &existing.name,
        body.kv_events.as_ref().and_then(Option::as_ref),
        body.lmcache.as_ref().and_then(Option::as_ref),
    )?;
    let slug_change =
        resolve_slug_change(body.slug.as_deref(), &existing.slug, body.allow_slug_change)?;
    if let Some(slug) = slug_change.as_deref() {
        require_address_slug_free(&state, "provider slug", slug, existing.org_id, Some(id)).await?;
    }
    // seal before writing anything so a missing KEK changes nothing
    let sealed = match body.api_key.as_deref().map(str::trim) {
        None => None,
        Some("") => Some(None),
        Some(key) => Some(Some(seal_api_key(key)?)),
    };
    let row = ProviderRepo(pool(&state))
        .update(
            id,
            ProviderUpdate {
                slug: slug_change.as_deref(),
                kind: body.kind.as_deref(),
                api_base: body.api_base.as_deref(),
                api_key_env: tri_state(&body.api_key_env),
                egress_proxy: tri_state(&body.egress_proxy),
                egress_proxies: body.egress_proxies.as_deref(),
                kv_events: body.kv_events.as_ref().map(Option::as_ref),
                lmcache: body.lmcache.as_ref().map(Option::as_ref),
                project_id: body.project_id,
                allow_custom_api_base: body.allow_custom_api_base,
            },
        )
        .await?;
    match sealed {
        None => {}
        Some(None) => ProviderKeyRepo(pool(&state)).clear(id).await?,
        Some(Some((ciphertext, nonce))) => {
            ProviderKeyRepo(pool(&state))
                .set(id, &ciphertext, &nonce)
                .await?
        }
    }
    // read back after the write so an omitted `api_key` reports what is stored
    let has_stored_key = ProviderKeyRepo(pool(&state)).exists(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "provider.update",
        "provider",
        id,
        serde_json::json!({"slug": row.slug, "project_id": row.project_id}),
    )
    .await;
    Ok(Json(ProviderView {
        provider: row,
        has_stored_key,
    }))
}

async fn delete_provider(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let existing = ProviderRepo(pool(&state)).get(id).await?;
    let chain = match existing.project_id {
        Some(project_id) => ScopeChain::from_project(pool(&state), project_id).await?,
        None => ScopeChain::org(existing.org_id),
    };
    authorize(&state, &principal, chain, cap!("provider", Delete)).await?;
    if let ProviderDeletion::InUse(dependents) = ProviderRepo(pool(&state)).delete(id).await? {
        return Err(ApiError::Conflict(format!(
            "provider '{}' is used by {}; remove it from them before deleting it",
            existing.name,
            dependents.join(", ")
        )));
    }
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "provider.delete",
        "provider",
        id,
        serde_json::json!({"name": existing.name}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod provider_model_listing_tests {
    use super::*;

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    /// A catalogue arrives in the upstream's order and may repeat an id; the
    /// route sheet offers them as suggestions, so each shows once and in a
    /// stable place (#2810).
    #[test]
    fn a_listing_is_sorted_and_without_duplicates() {
        let tidy = tidy_model_ids(ids(&["llama-3.1-8b", "gemma-2-9b", "llama-3.1-8b"]));
        assert_eq!(tidy, ids(&["gemma-2-9b", "llama-3.1-8b"]));
    }

    /// An aggregator lists thousands of models, and a suggestion list that long
    /// is not one: the cap is a bound on the response, not a ranking.
    #[test]
    fn a_listing_is_capped() {
        let many: Vec<String> = (0..MAX_LISTED_MODELS + 500)
            .map(|n| format!("model-{n:05}"))
            .collect();
        let tidy = tidy_model_ids(many);
        assert_eq!(tidy.len(), MAX_LISTED_MODELS);
        assert_eq!(tidy.first().map(String::as_str), Some("model-00000"));
    }

    #[test]
    fn an_empty_catalogue_is_an_empty_listing() {
        assert!(tidy_model_ids(Vec::new()).is_empty());
    }
}

#[cfg(test)]
mod probe_verdict_tests {
    use super::*;
    use rolter_core::ProviderKind;
    use serde_json::json;

    fn judge(kind: ProviderKind, status: u16, body: Option<serde_json::Value>) -> ProbeVerdict {
        judge_probe(
            kind,
            status,
            body.as_ref(),
            "https://x.test/v1/models",
            "stored",
            None,
        )
    }

    fn listed(
        kind: ProviderKind,
        status: u16,
        body: serde_json::Value,
        model: &str,
    ) -> Option<bool> {
        let verdict = judge(kind, status, Some(body.clone()));
        model_listed(kind, &verdict, Some(&body), Some(model))
    }

    /// #2008: the catalogue answers "listed" and "not listed" separately.
    #[test]
    fn a_catalogue_says_whether_it_lists_the_model() {
        let body = json!({"data": [{"id": "gpt-4o"}, {"id": "gpt-4o-mini"}]});
        assert_eq!(
            listed(ProviderKind::Openai, 200, body.clone(), "gpt-4o"),
            Some(true)
        );
        assert_eq!(
            listed(ProviderKind::Openai, 200, body, "gpt-5"),
            Some(false)
        );
    }

    /// A resource path and a bare name agree, as in `catalogue_names`.
    #[test]
    fn a_resource_path_and_a_bare_name_agree() {
        let body =
            json!({"publisherModels": [{"name": "publishers/google/models/gemini-2.5-pro"}]});
        assert_eq!(
            listed(ProviderKind::Vertex, 200, body.clone(), "gemini-2.5-pro"),
            Some(true)
        );
        assert_eq!(
            listed(
                ProviderKind::Vertex,
                200,
                body,
                "publishers/google/models/gemini-2.5-pro"
            ),
            Some(true)
        );
    }

    /// A liveness probe has no catalogue, a failed probe proves nothing, and
    /// no model asked about means nothing to answer.
    #[test]
    fn nothing_is_claimed_without_a_catalogue_to_check() {
        assert_eq!(
            listed(ProviderKind::Tei, 200, json!({"data": [{"id": "x"}]}), "x"),
            None,
            "tei is a liveness probe"
        );
        assert_eq!(
            listed(
                ProviderKind::Openai,
                401,
                json!({"data": [{"id": "x"}]}),
                "x"
            ),
            None,
            "a refused probe cannot say"
        );
        let body = json!({"data": [{"id": "x"}]});
        let verdict = judge(ProviderKind::Openai, 200, Some(body.clone()));
        assert_eq!(
            model_listed(ProviderKind::Openai, &verdict, Some(&body), None),
            None
        );
        assert_eq!(
            model_listed(ProviderKind::Openai, &verdict, Some(&body), Some("  ")),
            None
        );
    }

    /// #2811: `401 … (resolved from: env (unset))` said a key was missing and
    /// not whose environment it was missing from. The test runs in the control
    /// plane, the gateway has its own environment, and the operator has to fix
    /// the right one.
    #[test]
    fn an_unset_env_var_names_the_process_it_is_missing_from() {
        let verdict = judge_probe(
            ProviderKind::Openai,
            401,
            None,
            "https://x.test/v1/models",
            ENV_UNSET,
            Some("OPENAI_API_KEY"),
        );
        let error = verdict.error.expect("a rejection explains itself");
        assert!(
            error.contains("OPENAI_API_KEY"),
            "the message names the variable"
        );
        assert!(
            error.contains("control plane's environment"),
            "the message says the control plane is missing it"
        );
        assert!(
            error.contains("gateway"),
            "the message says the gateway needs it too"
        );
        assert!(
            error.contains("resolved from: env (unset)"),
            "the message keeps naming the credential source"
        );
    }

    /// The extra explanation is for the one credential source it is true of.
    #[test]
    fn a_rejected_stored_or_set_key_keeps_the_plain_message() {
        for credential in ["stored", "env", "none"] {
            let verdict = judge_probe(
                ProviderKind::Openai,
                401,
                None,
                "https://x.test/v1/models",
                credential,
                Some("OPENAI_API_KEY"),
            );
            let error = verdict.error.expect("a rejection explains itself");
            assert!(
                error.contains("rejected the credential"),
                "the plain message is kept for {credential}"
            );
            assert!(
                !error.contains("control plane's environment"),
                "the process explanation is only for an unset variable"
            );
        }
    }

    /// The case from #980: an upstream answering 200 with an HTML page at the
    /// probed URL must not read as Reachable. A test that passes where
    /// inference fails is worse than no test.
    #[test]
    fn a_2xx_carrying_a_non_catalogue_body_is_not_reachable() {
        // reqwest fails to parse html as json, so the body arrives as None
        let verdict = judge(ProviderKind::Openai, 200, None);
        assert!(!verdict.reachable, "html answered as a green result");
        assert_eq!(verdict.models_found, None);
        let error = verdict.error.expect("the operator must be told why");
        assert!(
            error.contains("not with a model list"),
            "the failure says the answer was not a model list"
        );
        // distinct from a flat failure: it must not claim the host refused us
        assert!(
            !error.contains("refused the probe"),
            "it must not claim the host refused the probe"
        );
    }

    /// Valid JSON that simply is not a catalogue — an auth portal's `{"error":…}`
    /// or a proxy's status document.
    #[test]
    fn a_2xx_json_body_with_no_model_array_is_not_reachable() {
        let verdict = judge(ProviderKind::Openai, 200, Some(json!({"status": "ok"})));
        assert!(!verdict.reachable);
        assert!(verdict
            .error
            .is_some_and(|e| e.contains("not with a model list")));
    }

    #[test]
    fn a_real_catalogue_is_reachable_and_counted() {
        let verdict = judge(
            ProviderKind::Openai,
            200,
            Some(json!({"data": [{"id": "gpt-4o"}, {"id": "gpt-4o-mini"}]})),
        );
        assert!(verdict.reachable);
        assert_eq!(verdict.models_found, Some(2));
        assert!(verdict.error.is_none());
    }

    /// Reachable but empty is a real state, and a different one from "not a
    /// catalogue" — the operator can see it in `models_found`.
    #[test]
    fn an_empty_catalogue_is_still_reachable() {
        let verdict = judge(ProviderKind::Openai, 200, Some(json!({"data": []})));
        assert!(verdict.reachable);
        assert_eq!(verdict.models_found, Some(0));
    }

    /// Tei probes `/health`, which has no `data` array. A blanket
    /// "require a catalogue" would report a healthy embedder as down.
    #[test]
    fn a_liveness_probe_keeps_2xx_as_the_criterion() {
        assert!(judge(ProviderKind::Tei, 200, None).reachable);
        assert!(judge(ProviderKind::Tei, 200, Some(json!("OK"))).reachable);
    }

    /// Bedrock and Vertex answer their own catalogue shapes rather than `data`.
    #[test]
    fn the_control_plane_catalogue_shapes_are_recognised() {
        let bedrock = judge(
            ProviderKind::Bedrock,
            200,
            Some(json!({"modelSummaries": [{"modelId": "anthropic.claude"}]})),
        );
        assert!(bedrock.reachable);
        assert_eq!(bedrock.models_found, Some(1));

        let vertex = judge(
            ProviderKind::Vertex,
            200,
            Some(json!({"publisherModels": [{"name": "gemini"}]})),
        );
        assert!(vertex.reachable);
    }

    /// Non-2xx keeps the messages it always had.
    #[test]
    fn a_failing_status_still_explains_itself() {
        let unauthorized = judge(ProviderKind::Openai, 401, None);
        assert!(!unauthorized.reachable);
        assert!(unauthorized
            .error
            .is_some_and(|e| e.contains("rejected the credential")));

        let missing = judge(ProviderKind::Openai, 404, None);
        assert!(missing
            .error
            .is_some_and(|e| e.contains("not served there")));

        let broken = judge(ProviderKind::Openai, 503, None);
        assert!(broken
            .error
            .is_some_and(|e| e.contains("refused the probe")));
    }
}

#[cfg(test)]
mod api_base_tests {
    use super::*;

    #[test]
    fn a_doubled_v1_is_refused_for_openai_shaped_kinds() {
        // the exact base from #947, hit against a real GPUStack instance
        for kind in ["openai", "openai_compatible", "ollama", "llama_cpp"] {
            let err = require_wellformed_api_base(kind, "https://gpustack.localhost/v1")
                .expect_err("{kind} must refuse a doubled base");
            let message = format!("{err:?}");
            assert!(message.contains("/v1/v1/chat/completions"), "{message}");
        }
    }

    #[test]
    fn a_v1_base_is_required_for_stripping_kinds() {
        // the same spelling is correct here, so it must pass untouched
        for (kind, base) in [
            ("mistral", "https://api.mistral.ai/v1"),
            ("groq", "https://api.groq.com/openai/v1"),
            ("xai", "https://api.x.ai/v1"),
        ] {
            assert!(require_wellformed_api_base(kind, base).is_ok(), "{kind}");
        }
    }

    #[test]
    fn a_wellformed_openai_base_passes() {
        assert!(require_wellformed_api_base("openai", "https://api.openai.com").is_ok());
        assert!(require_wellformed_api_base("ollama", "http://localhost:11434").is_ok());
    }

    #[test]
    fn an_unknown_kind_defers_to_validate_kind() {
        // reporting "your base is malformed" for a kind that does not exist
        // would bury the real error
        assert!(require_wellformed_api_base("not_a_kind", "https://host/v1").is_ok());
    }

    /// Every kind the CRUD API accepts must be a kind core can parse, or
    /// `require_wellformed_api_base` silently skips it and the guard is a no-op.
    #[test]
    fn every_accepted_kind_is_known_to_core() {
        for kind in PROVIDER_KINDS {
            let parsed = serde_json::from_value::<rolter_core::ProviderKind>(
                serde_json::Value::String(kind.to_string()),
            );
            assert!(parsed.is_ok(), "core cannot parse accepted kind '{kind}'");
        }
        assert_eq!(PROVIDER_KINDS.len(), rolter_core::ProviderKind::ALL.len());
    }
}

#[cfg(test)]
mod probe_egress_tests {
    use super::*;

    /// #2392: a name that resolves only to a denied address is never dialled.
    #[tokio::test]
    async fn a_provider_resolving_to_a_denied_address_is_never_contacted() {
        let listener = crate::egress_client::testing::Counter::start().await;
        let outcome = send_provider_probe(
            &crate::egress_client::testing::deny_loopback(),
            rolter_core::ProviderKind::Openai,
            &listener.url("/v1/models"),
            Vec::new(),
            None,
        )
        .await;
        assert!(outcome.is_err_and(|e| e.is_connect()));
        assert_eq!(listener.accepted(), 0);
    }

    /// a stub that answers a model list only to a request presenting `secret`
    /// in the header `header`, and 401 to everything else, as the real APIs do
    async fn serve_keyed_catalogue(header: &'static str, secret: String) -> String {
        let app = axum::Router::new().fallback(move |headers: axum::http::HeaderMap| {
            let secret = secret.clone();
            async move {
                let presented = headers.get(header).and_then(|v| v.to_str().ok());
                // anthropic rejects a version-less request even when the key is good
                let versioned = header != "x-api-key" || headers.contains_key("anthropic-version");
                if presented == Some(secret.as_str()) && versioned {
                    (
                        StatusCode::OK,
                        axum::Json(serde_json::json!({"data": [{"id": "m"}]})),
                    )
                } else {
                    (
                        StatusCode::UNAUTHORIZED,
                        axum::Json(serde_json::json!({"error": "unauthorized"})),
                    )
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local port");
        let base = format!("http://{}", listener.local_addr().expect("an address"));
        tokio::spawn(async move { axum::serve(listener, app).await });
        base
    }

    /// #2806: Anthropic reads the key from `x-api-key` and ignores a bearer
    /// token, so a probe that sent the bearer form reported every valid key as
    /// rejected.
    #[tokio::test]
    async fn an_anthropic_probe_presents_its_key_as_x_api_key() {
        let secret = format!("sk-{}", uuid::Uuid::new_v4());
        let base = serve_keyed_catalogue("x-api-key", secret.clone()).await;
        let kind = rolter_core::ProviderKind::Anthropic;
        let (url, headers) = rolter_core::probe_request(kind, &base, "/");

        let response = send_provider_probe(&Default::default(), kind, &url, headers, Some(&secret))
            .await
            .expect("the upstream answers");
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// every kind presents its key in the header its own API reads: the stub
    /// only opens for the header the shared mapping names
    #[tokio::test]
    async fn every_kind_is_probed_with_the_credential_its_api_reads() {
        for kind in rolter_core::ProviderKind::ALL {
            let secret = format!("sk-{}", uuid::Uuid::new_v4());
            let (name, value) = kind.auth_header(&secret);
            let base = serve_keyed_catalogue(name, value.into_owned()).await;
            let (url, headers) = rolter_core::probe_request(kind, &base, "/");
            let response =
                send_provider_probe(&Default::default(), kind, &url, headers, Some(&secret))
                    .await
                    .expect("the upstream answers");
            // the stub opens for the exact header and value auth_header gives
            // this kind, so it is that header that has to be on the wire
            assert_eq!(response.status(), StatusCode::OK, "{kind:?}");
        }
    }

    /// #2392: an upstream that passes the check cannot bounce the probe on.
    #[tokio::test]
    async fn a_redirect_from_the_provider_is_not_followed() {
        let target = crate::egress_client::testing::Counter::start().await;
        let location = format!("http://127.0.0.1:{}/", target.port);
        let app = axum::Router::new().fallback(move || {
            let location = location.clone();
            async move {
                (
                    StatusCode::FOUND,
                    [(axum::http::header::LOCATION, location)],
                )
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local port");
        let base = format!(
            "http://{}/v1/models",
            listener.local_addr().expect("an address")
        );
        tokio::spawn(async move { axum::serve(listener, app).await });

        let response = send_provider_probe(
            &Default::default(),
            rolter_core::ProviderKind::Openai,
            &base,
            Vec::new(),
            None,
        )
        .await
        .expect("the upstream answers");
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(target.accepted(), 0);
    }
}

// --- prompt-cache reporting probe (#984) ---

/// Query of the prompt-cache probe.
#[derive(Deserialize)]
struct CacheProbeQuery {
    /// the upstream model to ask; required, since the probe is a completion
    model: String,
}

/// What a prompt-cache probe found.
#[derive(Serialize)]
struct CacheProbeResult {
    /// the verdict was stored. `false` when the provider did not answer with a
    /// completion, which says nothing about its cache reporting, so any earlier
    /// verdict is left alone
    recorded: bool,
    verdict: Option<rolter_store::postgres::models::ProviderCacheProbe>,
    /// the URL probed, as for the connection test
    probed_url: String,
    status: Option<u16>,
    latency_ms: u64,
    error: Option<String>,
}

/// The kinds whose chat completion can be asked from here with a bearer or
/// key header and a fixed path. Bedrock and Vertex sign or mint tokens, Azure
/// addresses a deployment, Gemini native embeds the model in the URL, and the
/// embedding and decision servers do not complete chat.
fn cache_probe_supported(kind: rolter_core::ProviderKind) -> bool {
    use rolter_core::ProviderKind as K;
    kind.primary_request() == rolter_core::upstream::RequestKind::Chat
        && !matches!(
            kind,
            K::Bedrock | K::Vertex | K::AzureOpenai | K::GeminiNative | K::GeminiInteractions
        )
}

/// The one-token completion the probe sends, in the dialect of `kind`.
fn cache_probe_body(kind: rolter_core::ProviderKind, model: &str) -> serde_json::Value {
    let messages = serde_json::json!([{"role": "user", "content": "ping"}]);
    match kind {
        rolter_core::ProviderKind::Openai => serde_json::json!({
            "model": model, "messages": messages, "max_completion_tokens": 1
        }),
        _ => serde_json::json!({"model": model, "messages": messages, "max_tokens": 1}),
    }
}

/// Whether the response `usage` carries a prompt-cache field, and under which
/// spelling: the same list the gateway reads (`rolter_proxy::cached_prompt_tokens`),
/// so "reports" means the gateway will see a hit if there is one.
fn cache_verdict(body: &serde_json::Value) -> Option<(bool, Option<&'static str>)> {
    let usage = body.get("usage").filter(|usage| usage.is_object())?;
    let spelling = rolter_proxy::cached_prompt_tokens_spelling(usage);
    Some((spelling.is_some(), spelling))
}

/// Ask a provider for one token and record whether the answer's `usage`
/// carries prompt-cache metrics (#984).
///
/// Unlike the connection test this is a completion, so it costs a token and
/// needs a model; it runs only when the operator asks. The verdict is what
/// one response showed, not a promise: a provider that adds the field only on
/// a hit would read as "does not report", which the stored model and time let
/// an operator weigh.
async fn probe_provider_cache(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Query(query): Query<CacheProbeQuery>,
) -> ApiResult<Json<CacheProbeResult>> {
    let provider = ProviderRepo(pool(&state)).get(id).await?;
    let chain = match provider.project_id {
        Some(project_id) => ScopeChain::from_project(pool(&state), project_id).await?,
        None => ScopeChain::org(provider.org_id),
    };
    authorize(&state, &principal, chain, cap!("provider", Update)).await?;
    require_allowed_egress(&state, &provider.api_base, "api_base")?;

    let model = query.model.trim();
    if model.is_empty() {
        return Err(invalid_field("model", "name the upstream model to ask"));
    }
    let kind = known_kind(&provider.kind)?;
    if !cache_probe_supported(kind) {
        return Err(ApiError::Curated(format!(
            "cache reporting cannot be probed for a '{}' provider",
            provider.kind
        )));
    }
    let (secret, credential) =
        resolve_probe_credential(&state, id, provider.api_key_env.as_deref()).await?;
    let unreadable = |error: String| CacheProbeResult {
        recorded: false,
        verdict: None,
        probed_url: String::new(),
        status: None,
        latency_ms: 0,
        error: Some(error),
    };
    if credential == "stored (undecryptable)" || credential == "stored (KEK unset)" {
        return Ok(Json(unreadable(format!(
            "the stored credential for '{}' could not be read: {KEK_ENV} is unset or does \
             not match the key it was sealed with. The upstream was not contacted.",
            provider.name
        ))));
    }

    let url = kind.primary_upstream_url(&provider.api_base);
    let mut headers = Vec::new();
    if kind == rolter_core::ProviderKind::Anthropic {
        headers.push((
            "anthropic-version".to_string(),
            rolter_core::ANTHROPIC_VERSION.to_string(),
        ));
    }
    let started = std::time::Instant::now();
    let outcome = send_cache_probe(
        &state.egress,
        kind,
        &url,
        headers,
        secret.as_deref(),
        &cache_probe_body(kind, model),
    )
    .await;
    let latency_ms = started.elapsed().as_millis() as u64;
    let failed = |status: Option<u16>, error: String| CacheProbeResult {
        recorded: false,
        verdict: None,
        probed_url: url.clone(),
        status,
        latency_ms,
        error: Some(error),
    };
    let resp = match outcome {
        Ok(resp) => resp,
        // never surface the raw reqwest error: it can carry the URL
        Err(e) => {
            return Ok(Json(failed(
                None,
                if e.is_timeout() {
                    format!("no response within {CACHE_PROBE_TIMEOUT_SECS}s")
                } else if e.is_connect() {
                    "could not connect — check the host, port and TLS".to_string()
                } else {
                    "the request could not be completed".to_string()
                },
            )))
        }
    };
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        return Ok(Json(failed(
            Some(status),
            format!("{status}: the upstream did not complete the request, so nothing was learned about its cache reporting"),
        )));
    }
    let body = resp.json::<serde_json::Value>().await.ok();
    let Some((reports, spelling)) = body.as_ref().and_then(cache_verdict) else {
        return Ok(Json(failed(
            Some(status),
            format!("{status}: the answer carried no usage object, so nothing was learned about its cache reporting"),
        )));
    };
    let verdict = ProviderCacheProbeRepo(pool(&state))
        .record(id, reports, spelling, model)
        .await?;
    Ok(Json(CacheProbeResult {
        recorded: true,
        verdict: Some(verdict),
        probed_url: url,
        status: Some(status),
        latency_ms,
        error: None,
    }))
}

/// How long a cache probe waits for the upstream: a full completion, so
/// longer than the connectivity test's model listing.
const CACHE_PROBE_TIMEOUT_SECS: u64 = 30;

/// POST the probe body through the connect-time egress client, authenticated
/// the way the proxy authenticates `kind`.
async fn send_cache_probe(
    egress: &std::sync::Arc<rolter_core::EgressPolicy>,
    kind: rolter_core::ProviderKind,
    url: &str,
    headers: impl IntoIterator<Item = (String, String)>,
    secret: Option<&str>,
    body: &serde_json::Value,
) -> Result<reqwest::Response, reqwest::Error> {
    let client = crate::egress_client::builder(egress)
        .timeout(std::time::Duration::from_secs(CACHE_PROBE_TIMEOUT_SECS))
        .build()?;
    let mut req = client.post(url).json(body);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    if let Some(secret) = secret {
        let (name, value) = kind.auth_header(secret);
        req = req.header(name, value.as_ref());
    }
    req.send().await
}

/// The stored cache-reporting verdicts of the providers the caller may see. A
/// provider with no entry has never been probed.
async fn list_provider_cache_probes(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<rolter_store::postgres::models::ProviderCacheProbe>>> {
    let rows = ProviderRepo(pool(&state)).list(org_id).await?;
    let visible = visible_in_scope(
        &state,
        &principal,
        cap!("provider", Read),
        org_id,
        rows,
        |row| row.project_id,
    )
    .await?;
    let ids: std::collections::HashSet<Uuid> = visible.iter().map(|row| row.id).collect();
    let probes = ProviderCacheProbeRepo(pool(&state)).list(org_id).await?;
    Ok(Json(
        probes
            .into_iter()
            .filter(|probe| ids.contains(&probe.provider_id))
            .collect(),
    ))
}

#[cfg(test)]
mod cache_probe_tests {
    use super::*;
    use rolter_core::ProviderKind;
    use serde_json::json;

    #[test]
    fn a_stated_zero_is_a_provider_that_reports() {
        let zero = json!({"usage":{"prompt_tokens":8,"prompt_tokens_details":{"cached_tokens":0}}});
        assert_eq!(
            cache_verdict(&zero),
            Some((true, Some("/prompt_tokens_details/cached_tokens")))
        );
        let anthropic = json!({"usage":{"input_tokens":8,"cache_read_input_tokens":0}});
        assert_eq!(
            cache_verdict(&anthropic),
            Some((true, Some("/cache_read_input_tokens")))
        );
    }

    #[test]
    fn an_absent_field_is_a_provider_that_does_not_report_and_no_usage_is_no_verdict() {
        let none = json!({"usage":{"prompt_tokens":8,"completion_tokens":1}});
        assert_eq!(cache_verdict(&none), Some((false, None)));
        assert_eq!(cache_verdict(&json!({"choices":[]})), None);
        assert_eq!(cache_verdict(&json!({"usage":null})), None);
    }

    #[test]
    fn only_kinds_that_complete_chat_with_a_plain_credential_are_probed() {
        assert!(cache_probe_supported(ProviderKind::Openai));
        assert!(cache_probe_supported(ProviderKind::Anthropic));
        assert!(cache_probe_supported(ProviderKind::OpenaiCompatible));
        for kind in [
            ProviderKind::Tei,
            ProviderKind::SystemOne,
            ProviderKind::Bedrock,
            ProviderKind::Vertex,
            ProviderKind::AzureOpenai,
            ProviderKind::GeminiNative,
        ] {
            assert!(!cache_probe_supported(kind), "{kind:?}");
        }
    }

    #[test]
    fn the_probe_asks_for_one_token_in_the_kinds_own_spelling() {
        let openai = cache_probe_body(ProviderKind::Openai, "m");
        assert_eq!(openai["max_completion_tokens"], 1);
        let other = cache_probe_body(ProviderKind::OpenaiCompatible, "m");
        assert_eq!(other["max_tokens"], 1);
        assert_eq!(other["model"], "m");
    }
}
