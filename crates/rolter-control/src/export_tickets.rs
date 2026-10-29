//! Single-use download tickets for the analytics CSV exports (#3504).
//!
//! Every dashboard call authenticates with a bearer header, and a link the
//! browser's download manager follows cannot carry one. Where the File System
//! Access API is missing (Firefox, Safari, a dashboard served over plain http)
//! the dashboard used to read the export into the tab instead, which a
//! million-row request log can exhaust. A ticket stands in for the bearer on
//! one download, so the browser saves the file itself and the tab holds none of
//! it. The design is ADR-0041
//! (`docs/dev-docs/adr/2026-10-29-export-download-tickets.md`).
//!
//! * **Mint** (`POST /api/v1/analytics/exports/tickets`) takes the caller's
//!   bearer, one of the three export paths and the query string the button
//!   would send. The export's own extractors run first, so a caller who could
//!   not run the export cannot mint for it; the query is parsed as the export
//!   parses it; and for the request log the [`ExportGate`] is asked, without
//!   taking a place, whether it would let the export in now, so a full gate is
//!   the usual `429` straight away.
//! * **The secret** is 32 bytes from the OS-seeded CSPRNG, base64url, returned
//!   once and stored only as its SHA-256. The bearer it was minted with is never
//!   stored: the [`Minter`] keeps what can be checked again at redemption.
//! * **Download** (`GET /api/v1/analytics/exports/download/{ticket}`) consumes
//!   the ticket in one statement, so it is single use even across replicas, and
//!   even when the export then fails or is refused. Unknown, used and expired
//!   secrets get one uniform `410`. The minter is re-resolved (a revoked
//!   session, a deactivated account or a deleted or revoked API token is
//!   refused), and the export runs through [`analytics_export::run_redeemed`]:
//!   the bearer route's handler, with every check it runs for a bearer request.
//! * **Status** (`GET /api/v1/analytics/exports/tickets/{id}`) is read by the
//!   ticket's non-secret id, by the principal that minted it only. The page
//!   cannot read a response the download manager receives, so this is how it
//!   learns progress, a `429` or a truncated end. The replica serving the
//!   download writes it a handful of times per export: at the start, at most
//!   every [`PROGRESS_EVERY`] while rows flow, and at the end.
//! * **Never logged.** The secret is a path segment of a route whose span and
//!   metrics carry the route template, never the path; the API's `404` fallback
//!   echoes a path through [`redact_path`]; no error body here names it; and the
//!   download answers with `Cache-Control: no-store` and
//!   `Referrer-Policy: no-referrer`. A reverse proxy's access log may still see
//!   it, which the 60-second lifetime and single use are there to bound.
//!
//! The store is Postgres when the control plane has one, so any replica can
//! redeem a ticket another minted, and an in-memory map otherwise: the
//! store-less mode runs as one replica, and so does a build without postgres.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::analytics_access::AnalyticsAccess;
use crate::analytics_export::{
    self, caller_key, too_many_exports, Caller, Cut, ExportKind, ExportProgress, RequestVersion,
};
use crate::conn_close::ConnectionHandle;
use crate::ControlState;

/// How long a ticket may wait to be redeemed.
pub(crate) const TICKET_LIFETIME: Duration = Duration::from_secs(60);

/// The least time between two progress writes while an export streams.
const PROGRESS_EVERY: Duration = Duration::from_secs(2);

/// How long a ticket's row outlives its expiry, so its status can still be read
/// after a long export, before the next mint purges it.
const RETAIN_PAST_EXPIRY: Duration = Duration::from_secs(24 * 60 * 60);

/// The most tickets the in-memory store holds. One replica, and each ticket is
/// minted by an authenticated caller, so this only stops a loop from growing
/// the map without bound.
const MEMORY_CAPACITY: usize = 10_000;

/// The longest query string a ticket stores. The export URLs the dashboard
/// builds are a few hundred bytes.
const MAX_QUERY_LEN: usize = 4096;

/// The prefix of the download route. Everything after it is the secret.
pub(crate) const DOWNLOAD_PREFIX: &str = "/api/v1/analytics/exports/download/";

/// What a minted secret starts with, so one found in a log can be told for
/// what it is.
const SECRET_PREFIX: &str = "rolter_dl_";

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/analytics/exports/tickets", post(mint))
        .route("/api/v1/analytics/exports/tickets/{id}", get(status))
        .route("/api/v1/analytics/exports/download/{ticket}", get(download))
}

/// `path` with a download secret in it replaced, for anything that echoes a
/// request path: the API's `404` fallback answers `no such endpoint: <path>`,
/// and a mistyped download URL must not hand its secret back in the body.
pub(crate) fn redact_path(path: &str) -> std::borrow::Cow<'_, str> {
    match path.strip_prefix(DOWNLOAD_PREFIX) {
        Some(rest) if !rest.is_empty() => format!("{DOWNLOAD_PREFIX}…").into(),
        _ => path.into(),
    }
}

/// Who minted a ticket, as what can be checked again when it is redeemed. Never
/// the bearer itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Minter {
    /// open mode: there is no credential, and a ticket redeems only while
    /// there is still none
    Open,
    /// the admin token, by a digest of it, so a token rotated since no longer
    /// redeems
    Admin { digest: String },
    /// a session, by its row id
    #[cfg(feature = "postgres")]
    Session(Uuid),
    /// an API token, by its row id
    #[cfg(feature = "postgres")]
    ApiToken(Uuid),
}

impl Minter {
    /// The `minter_kind` column's value.
    #[cfg(feature = "postgres")]
    fn kind(&self) -> &'static str {
        match self {
            Minter::Open => "open",
            Minter::Admin { .. } => "admin",
            #[cfg(feature = "postgres")]
            Minter::Session(_) => "session",
            #[cfg(feature = "postgres")]
            Minter::ApiToken(_) => "api_token",
        }
    }
}

/// The digest of the admin token a ticket stores: a domain-separated SHA-256,
/// so it matches no other digest this control plane keeps of the token.
fn admin_digest(token: &str) -> String {
    rolter_auth::hash_key("rolter-export-ticket", token)
}

/// Whether `admin`, the admin token configured now, is the one a ticket's
/// `digest` was taken of.
fn same_admin(admin: &str, digest: &str) -> bool {
    subtle::ConstantTimeEq::ct_eq(admin_digest(admin).as_bytes(), digest.as_bytes()).into()
}

/// The SHA-256 a secret is stored and looked up as.
fn secret_hash(secret: &str) -> String {
    use sha2::Digest;
    rolter_auth::hex::encode(&sha2::Sha256::digest(secret.as_bytes()))
}

/// A fresh secret: 32 bytes of CSPRNG output, base64url, behind its prefix.
fn new_secret() -> String {
    use base64::Engine;
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!(
        "{SECRET_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
}

/// Why an export stopped before its last row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TruncatedReason {
    /// more rows than `ROLTER_EXPORT_MAX_ROWS`
    RowLimit,
    /// longer than `ROLTER_EXPORT_MAX_SECS`
    TimeLimit,
    /// a ClickHouse page failed part way
    ReadError,
    /// the browser stopped the download, or the connection dropped
    ClientClosed,
}

impl From<Cut> for TruncatedReason {
    fn from(cut: Cut) -> Self {
        match cut {
            Cut::RowLimit => TruncatedReason::RowLimit,
            Cut::TimeLimit => TruncatedReason::TimeLimit,
        }
    }
}

/// Where a ticket's export has got to, as the status read reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum TicketState {
    /// minted, not yet redeemed
    Pending,
    /// redeemed, and the file is being written
    Streaming { rows: u64 },
    /// the last row was written
    Complete { rows: u64 },
    /// the body ended in an error after `rows` rows
    Truncated { rows: u64, reason: TruncatedReason },
    /// the download was answered with an error instead of a file: the `429` of
    /// a full export gate (with its `Retry-After`), a credential no longer
    /// valid, a query the export refused, or analytics being unavailable
    Refused {
        status: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after: Option<u64>,
    },
    /// never redeemed, and now past its lifetime
    Expired,
}

impl TicketState {
    fn is_terminal(&self) -> bool {
        !matches!(self, TicketState::Pending | TicketState::Streaming { .. })
    }
}

/// What a mint stores.
struct NewTicket {
    id: Uuid,
    secret_hash: String,
    minter: Minter,
    owner: String,
    export: ExportKind,
    query: String,
}

/// What a redemption hands back.
#[derive(Debug)]
struct Redeemed {
    id: Uuid,
    minter: Minter,
    export: ExportKind,
    query: String,
}

/// A ticket as its status read sees it.
#[derive(Debug)]
struct TicketRecord {
    owner: String,
    export: ExportKind,
    expires_at: DateTime<Utc>,
    state: TicketState,
}

/// Where tickets live: Postgres with a store, memory without.
#[async_trait::async_trait]
trait TicketStore: Send + Sync {
    /// Store `ticket`, expiring `lifetime` from now, and answer when that is.
    /// Purges rows long past their expiry on the way.
    async fn insert(&self, ticket: NewTicket, lifetime: Duration) -> anyhow::Result<DateTime<Utc>>;
    /// Consume the unused, unexpired ticket whose secret hashes to `hash`, in
    /// one step: of two racing redemptions, one gets it.
    async fn redeem(&self, hash: &str) -> anyhow::Result<Option<Redeemed>>;
    /// The ticket with the non-secret `id`, expiry applied.
    async fn read(&self, id: Uuid) -> anyhow::Result<Option<TicketRecord>>;
    /// Record where the ticket's export has got to.
    async fn report(&self, id: Uuid, state: &TicketState) -> anyhow::Result<()>;
}

/// The in-memory store, for the store-less mode and builds without postgres.
#[derive(Default)]
struct MemoryTickets {
    tickets: parking_lot::Mutex<MemoryMap>,
}

#[derive(Default)]
struct MemoryMap {
    by_id: HashMap<Uuid, MemoryTicket>,
    by_hash: HashMap<String, Uuid>,
}

struct MemoryTicket {
    secret_hash: String,
    minter: Minter,
    owner: String,
    export: ExportKind,
    query: String,
    expires_at: DateTime<Utc>,
    used: bool,
    state: TicketState,
}

fn chrono_span(duration: Duration) -> chrono::Duration {
    chrono::Duration::from_std(duration).unwrap_or(chrono::Duration::MAX)
}

#[async_trait::async_trait]
impl TicketStore for MemoryTickets {
    async fn insert(&self, ticket: NewTicket, lifetime: Duration) -> anyhow::Result<DateTime<Utc>> {
        let now = Utc::now();
        let mut map = self.tickets.lock();
        let horizon = now - chrono_span(RETAIN_PAST_EXPIRY);
        let MemoryMap { by_id, by_hash } = &mut *map;
        by_id.retain(|_, held| held.expires_at > horizon);
        by_hash.retain(|_, id| by_id.contains_key(id));
        if by_id.len() >= MEMORY_CAPACITY {
            anyhow::bail!("the in-memory ticket store is full");
        }
        let expires_at = now + chrono_span(lifetime);
        by_hash.insert(ticket.secret_hash.clone(), ticket.id);
        by_id.insert(
            ticket.id,
            MemoryTicket {
                secret_hash: ticket.secret_hash,
                minter: ticket.minter,
                owner: ticket.owner,
                export: ticket.export,
                query: ticket.query,
                expires_at,
                used: false,
                state: TicketState::Pending,
            },
        );
        Ok(expires_at)
    }

    async fn redeem(&self, hash: &str) -> anyhow::Result<Option<Redeemed>> {
        let now = Utc::now();
        let mut map = self.tickets.lock();
        let Some(id) = map.by_hash.get(hash).copied() else {
            return Ok(None);
        };
        let Some(held) = map.by_id.get_mut(&id) else {
            return Ok(None);
        };
        if held.used || held.expires_at <= now || held.secret_hash != hash {
            return Ok(None);
        }
        held.used = true;
        Ok(Some(Redeemed {
            id,
            minter: held.minter.clone(),
            export: held.export,
            query: held.query.clone(),
        }))
    }

    async fn read(&self, id: Uuid) -> anyhow::Result<Option<TicketRecord>> {
        let now = Utc::now();
        let map = self.tickets.lock();
        Ok(map.by_id.get(&id).map(|held| TicketRecord {
            owner: held.owner.clone(),
            export: held.export,
            expires_at: held.expires_at,
            state: match &held.state {
                TicketState::Pending if !held.used && held.expires_at <= now => {
                    TicketState::Expired
                }
                state => state.clone(),
            },
        }))
    }

    async fn report(&self, id: Uuid, state: &TicketState) -> anyhow::Result<()> {
        if let Some(held) = self.tickets.lock().by_id.get_mut(&id) {
            held.state = state.clone();
        }
        Ok(())
    }
}

/// The Postgres store (`0229_export_tickets.sql`), shared by every replica.
#[cfg(feature = "postgres")]
struct PgTickets<'a>(&'a sqlx::PgPool);

#[cfg(feature = "postgres")]
impl PgTickets<'_> {
    fn minter_from(
        kind: &str,
        session: Option<Uuid>,
        token: Option<Uuid>,
        digest: Option<String>,
    ) -> Option<Minter> {
        match kind {
            "open" => Some(Minter::Open),
            "admin" => digest.map(|digest| Minter::Admin { digest }),
            "session" => session.map(Minter::Session),
            "api_token" => token.map(Minter::ApiToken),
            _ => None,
        }
    }
}

#[cfg(feature = "postgres")]
#[async_trait::async_trait]
impl TicketStore for PgTickets<'_> {
    async fn insert(&self, ticket: NewTicket, lifetime: Duration) -> anyhow::Result<DateTime<Utc>> {
        // opportunistic: the expiry index makes this a short range scan, and a
        // failure here is no reason to refuse the mint
        if let Err(error) = sqlx::query(
            "delete from export_tickets \
             where expires_at < now() - $1::double precision * interval '1 second'",
        )
        .bind(RETAIN_PAST_EXPIRY.as_secs_f64())
        .execute(self.0)
        .await
        {
            tracing::warn!(%error, "purging expired export download tickets failed");
        }
        let (session, token, digest) = match &ticket.minter {
            Minter::Open => (None, None, None),
            Minter::Admin { digest } => (None, None, Some(digest.clone())),
            Minter::Session(id) => (Some(*id), None, None),
            Minter::ApiToken(id) => (None, Some(*id), None),
        };
        let (expires_at,): (DateTime<Utc>,) = sqlx::query_as(
            "insert into export_tickets \
                 (id, secret_hash, minter_kind, session_id, api_token_id, admin_digest, \
                  owner, export, query, expires_at) \
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, \
                     now() + $10::double precision * interval '1 second') \
             returning expires_at",
        )
        .bind(ticket.id)
        .bind(&ticket.secret_hash)
        .bind(ticket.minter.kind())
        .bind(session)
        .bind(token)
        .bind(digest)
        .bind(&ticket.owner)
        .bind(ticket.export.as_str())
        .bind(&ticket.query)
        .bind(lifetime.as_secs_f64())
        .fetch_one(self.0)
        .await?;
        Ok(expires_at)
    }

    async fn redeem(&self, hash: &str) -> anyhow::Result<Option<Redeemed>> {
        type Row = (
            Uuid,
            String,
            Option<Uuid>,
            Option<Uuid>,
            Option<String>,
            String,
            String,
        );
        let row: Option<Row> = sqlx::query_as(
            "update export_tickets set used_at = now(), updated_at = now() \
             where secret_hash = $1 and used_at is null and expires_at > now() \
             returning id, minter_kind, session_id, api_token_id, admin_digest, export, query",
        )
        .bind(hash)
        .fetch_optional(self.0)
        .await?;
        Ok(
            row.and_then(|(id, kind, session, token, digest, export, query)| {
                Some(Redeemed {
                    id,
                    minter: Self::minter_from(&kind, session, token, digest)?,
                    export: ExportKind::parse(&export)?,
                    query,
                })
            }),
        )
    }

    async fn read(&self, id: Uuid) -> anyhow::Result<Option<TicketRecord>> {
        let row: Option<(String, String, DateTime<Utc>, serde_json::Value, bool)> = sqlx::query_as(
            "select owner, export, expires_at, status, \
                        (used_at is null and expires_at <= now()) as lapsed \
                 from export_tickets where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await?;
        Ok(row.and_then(|(owner, export, expires_at, status, lapsed)| {
            let state = match serde_json::from_value(status).ok()? {
                TicketState::Pending if lapsed => TicketState::Expired,
                state => state,
            };
            Some(TicketRecord {
                owner,
                export: ExportKind::parse(&export)?,
                expires_at,
                state,
            })
        }))
    }

    async fn report(&self, id: Uuid, state: &TicketState) -> anyhow::Result<()> {
        sqlx::query("update export_tickets set status = $2, updated_at = now() where id = $1")
            .bind(id)
            .bind(serde_json::to_value(state)?)
            .execute(self.0)
            .await?;
        Ok(())
    }
}

/// The ticket machinery a control plane holds: the in-memory store, used when
/// there is no database, and the lifetime, which tests shorten.
pub(crate) struct Tickets {
    memory: MemoryTickets,
    lifetime: Duration,
}

impl Default for Tickets {
    fn default() -> Self {
        Self::with_lifetime(TICKET_LIFETIME)
    }
}

impl Tickets {
    /// Tickets that live `lifetime` rather than [`TICKET_LIFETIME`].
    pub(crate) fn with_lifetime(lifetime: Duration) -> Self {
        Self {
            memory: MemoryTickets::default(),
            lifetime,
        }
    }
}

/// The store for this control plane: its database when it has one.
fn store(state: &ControlState) -> Box<dyn TicketStore + '_> {
    #[cfg(feature = "postgres")]
    if let Some(pool) = state.pool.as_ref() {
        return Box::new(PgTickets(pool));
    }
    Box::new(MemoryRef(&state.export_tickets.memory))
}

/// [`MemoryTickets`] borrowed from the state, so the box does not own it.
struct MemoryRef<'a>(&'a MemoryTickets);

#[async_trait::async_trait]
impl TicketStore for MemoryRef<'_> {
    async fn insert(&self, ticket: NewTicket, lifetime: Duration) -> anyhow::Result<DateTime<Utc>> {
        self.0.insert(ticket, lifetime).await
    }
    async fn redeem(&self, hash: &str) -> anyhow::Result<Option<Redeemed>> {
        self.0.redeem(hash).await
    }
    async fn read(&self, id: Uuid) -> anyhow::Result<Option<TicketRecord>> {
        self.0.read(id).await
    }
    async fn report(&self, id: Uuid, state: &TicketState) -> anyhow::Result<()> {
        self.0.report(id, state).await
    }
}

/// Record `state` for ticket `id`, logging rather than failing: the export the
/// status describes is already under way, and a lost write only leaves the page
/// polling an older state.
async fn report(state: &ControlState, id: Uuid, ticket_state: &TicketState) {
    if let Err(error) = store(state).report(id, ticket_state).await {
        tracing::warn!(%error, ticket = %id, "recording an export download's progress failed");
    }
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": {"message": message}}))).into_response()
}

/// The one refusal for a secret that is unknown, used or expired: which of the
/// three it was is no business of whoever holds it.
fn invalid_ticket() -> Response {
    error(
        StatusCode::GONE,
        "this download link is not valid: it was already used, it expired, or it never existed; \
         start the export again",
    )
}

fn store_failed(error: &anyhow::Error) -> Response {
    tracing::warn!(error = %error, "the export download ticket store failed");
    internal_error()
}

fn internal_error() -> Response {
    error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "an internal server error occurred",
    )
}

/// What the mint takes: one of the export paths and the query string it would
/// be sent with, without the `?`.
#[derive(Debug, Deserialize)]
struct MintRequest {
    path: String,
    #[serde(default)]
    query: String,
}

/// What the mint answers.
#[derive(Debug, Serialize)]
struct Minted {
    /// the non-secret id the status is read by
    id: Uuid,
    /// the same-origin URL that downloads the export once, with the secret in
    /// it. Followed by the browser, never fetched by the page
    download_url: String,
    /// where the minting principal reads the export's progress
    status_url: String,
    expires_at: DateTime<Utc>,
}

/// The principal behind this request as what a ticket stores. The extractors
/// that ran before have already accepted the bearer.
#[cfg(feature = "postgres")]
#[allow(clippy::result_large_err)]
async fn minter_of(
    state: &ControlState,
    headers: &HeaderMap,
    principal: &Caller,
) -> Result<Minter, Response> {
    use crate::rbac::Principal;
    let Some(admin) = state.admin_token.as_deref() else {
        return Ok(Minter::Open);
    };
    match principal {
        Principal::Token(held) => Ok(Minter::ApiToken(held.token.id)),
        Principal::Superadmin { account: None } => Ok(Minter::Admin {
            digest: admin_digest(admin),
        }),
        // a signed-in account: the session it signed in on is what can be
        // revoked, so that is what the ticket names
        Principal::Superadmin { account: Some(_) } | Principal::User(_) => {
            let pool = state.pool.as_ref().ok_or_else(internal_error)?;
            let bearer = crate::auth::bearer_token(headers).unwrap_or_default();
            let hash = rolter_auth::hash_key(&crate::auth::session_pepper(), bearer);
            match rolter_store::postgres::repo::SessionRepo(pool)
                .find_active_by_hash(&hash)
                .await
            {
                Ok(Some(session)) => Ok(Minter::Session(session.id)),
                Ok(None) => Err(error(
                    StatusCode::UNAUTHORIZED,
                    "missing or invalid credentials",
                )),
                Err(failure) => Err(store_failed(&anyhow::Error::from(failure))),
            }
        }
    }
}

#[cfg(not(feature = "postgres"))]
#[allow(clippy::result_large_err)]
async fn minter_of(state: &ControlState, _: &HeaderMap, _: &Caller) -> Result<Minter, Response> {
    // without a database the admin token is the only credential there is, and
    // the extractor that ran before has checked it
    Ok(match state.admin_token.as_deref() {
        None => Minter::Open,
        Some(admin) => Minter::Admin {
            digest: admin_digest(admin),
        },
    })
}

/// The principal a ticket's minter is now, or `None` when that credential no
/// longer holds: a revoked or expired session, a deactivated account, a revoked
/// or deleted API token, a rotated admin token, or an open-mode ticket on a
/// control plane that has since been given a token.
#[cfg(feature = "postgres")]
async fn principal_of(state: &ControlState, minter: &Minter) -> anyhow::Result<Option<Caller>> {
    use crate::rbac::Principal;
    use rolter_store::postgres::repo::{SessionRepo, UserRepo};
    match (minter, state.admin_token.as_deref()) {
        (Minter::Open, None) => Ok(Some(Principal::Superadmin { account: None })),
        (Minter::Admin { digest }, Some(admin)) => {
            Ok(same_admin(admin, digest).then_some(Principal::Superadmin { account: None }))
        }
        (Minter::Session(id), Some(_)) => {
            let Some(pool) = state.pool.as_ref() else {
                return Ok(None);
            };
            let Some(session) = SessionRepo(pool).find_active_by_id(*id).await? else {
                return Ok(None);
            };
            let user = UserRepo(pool).get(session.user_id).await?;
            if user.deactivated_at.is_some() {
                return Ok(None);
            }
            Ok(Some(Principal::for_user(user)))
        }
        (Minter::ApiToken(id), Some(_)) => {
            let Some(pool) = state.pool.as_ref() else {
                return Ok(None);
            };
            Ok(crate::api_tokens::resolve_id(pool, *id)
                .await
                .map_err(|failure| anyhow::anyhow!("{failure:?}"))?
                .map(Principal::Token))
        }
        _ => Ok(None),
    }
}

#[cfg(not(feature = "postgres"))]
async fn principal_of(state: &ControlState, minter: &Minter) -> anyhow::Result<Option<Caller>> {
    Ok(match (minter, state.admin_token.as_deref()) {
        (Minter::Open, None) => Some(()),
        (Minter::Admin { digest }, Some(admin)) => same_admin(admin, digest).then_some(()),
        _ => None,
    })
}

/// The reach of a re-resolved principal over the request log, by the same
/// reduction a bearer request gets.
#[cfg(feature = "postgres")]
#[allow(clippy::result_large_err)]
async fn reach_of(state: &ControlState, principal: &Caller) -> Result<AnalyticsAccess, Response> {
    crate::analytics_access::analytics_reach(state, principal).await
}

#[cfg(not(feature = "postgres"))]
#[allow(clippy::result_large_err)]
async fn reach_of(_: &ControlState, _: &Caller) -> Result<AnalyticsAccess, Response> {
    // the admin token and open mode see every row
    Ok(AnalyticsAccess::unrestricted())
}

/// `POST /api/v1/analytics/exports/tickets`: mint a download ticket for one
/// export. `principal` and `access` are the extractors the export itself takes,
/// so a caller the export would refuse is refused here.
async fn mint(
    // the access first: without a database it is what refuses a wrong bearer,
    // and `Principal` would reach for the pool that is not there
    _access: AnalyticsAccess,
    principal: Caller,
    State(state): State<ControlState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // read by hand rather than through the CRUD API's `SafeJson`, which only a
    // postgres build has. the path is matched against three constants and the
    // query is parsed as a URI below, so neither can carry a control character
    let body: MintRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(failure) => {
            return error(
                StatusCode::BAD_REQUEST,
                &format!("invalid request body: {failure}"),
            )
        }
    };
    let Some(export) = ExportKind::from_path(&body.path) else {
        return error(
            StatusCode::BAD_REQUEST,
            "path must be one of /api/v1/analytics/by-attribution/export, \
             /api/v1/analytics/timeseries/export or /api/v1/analytics/invocations/export",
        );
    };
    let query = body.query.strip_prefix('?').unwrap_or(&body.query);
    if query.len() > MAX_QUERY_LEN {
        return error(StatusCode::BAD_REQUEST, "the export query is too long");
    }
    if let Err(refused) = analytics_export::check_query(&state, export, query).await {
        return refused;
    }
    // the gate only bounds the request log; a full one is refused now rather
    // than through the status once the browser has started a download
    if export.is_gated() {
        if let Err(busy) = state.export_gate.would_admit(&caller_key(&principal)) {
            return too_many_exports(busy);
        }
    }
    let minter = match minter_of(&state, &headers, &principal).await {
        Ok(minter) => minter,
        Err(refused) => return refused,
    };
    let secret = new_secret();
    let id = Uuid::new_v4();
    let ticket = NewTicket {
        id,
        secret_hash: secret_hash(&secret),
        minter,
        owner: caller_key(&principal),
        export,
        query: query.to_string(),
    };
    let expires_at = match store(&state)
        .insert(ticket, state.export_tickets.lifetime)
        .await
    {
        Ok(expires_at) => expires_at,
        Err(failure) => return store_failed(&failure),
    };
    let mut response = Json(Minted {
        id,
        download_url: format!("{DOWNLOAD_PREFIX}{secret}"),
        status_url: format!("/api/v1/analytics/exports/tickets/{id}"),
        expires_at,
    })
    .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// What the status read answers.
#[derive(Debug, Serialize)]
struct TicketStatus {
    id: Uuid,
    export: &'static str,
    expires_at: DateTime<Utc>,
    /// the state is terminal: the page may stop asking
    done: bool,
    #[serde(flatten)]
    state: TicketState,
}

/// `GET /api/v1/analytics/exports/tickets/{id}`: how the ticket's export is
/// going, for the principal that minted it. Anyone else, and an id that is not
/// a ticket, gets the same `404`.
async fn status(
    _access: AnalyticsAccess,
    principal: Caller,
    State(state): State<ControlState>,
    Path(id): Path<String>,
) -> Response {
    let not_found = || error(StatusCode::NOT_FOUND, "no such export download");
    let Ok(id) = id.parse::<Uuid>() else {
        return not_found();
    };
    let record = match store(&state).read(id).await {
        Ok(Some(record)) => record,
        Ok(None) => return not_found(),
        Err(failure) => return store_failed(&failure),
    };
    let owner = caller_key(&principal);
    if !bool::from(subtle::ConstantTimeEq::ct_eq(
        record.owner.as_bytes(),
        owner.as_bytes(),
    )) {
        return not_found();
    }
    let mut response = Json(TicketStatus {
        id,
        export: record.export.as_str(),
        expires_at: record.expires_at,
        done: record.state.is_terminal(),
        state: record.state,
    })
    .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// `GET /api/v1/analytics/exports/download/{ticket}`: redeem a ticket and send
/// its export. The secret is read off the URI rather than through a `Path`
/// extractor, whose rejection could quote it.
async fn download(
    State(state): State<ControlState>,
    uri: Uri,
    version: RequestVersion,
    connection: Option<Extension<ConnectionHandle>>,
) -> Response {
    let secret = uri.path().strip_prefix(DOWNLOAD_PREFIX).unwrap_or_default();
    let mut response = redeem(state, secret, version, connection).await;
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

async fn redeem(
    state: ControlState,
    secret: &str,
    version: RequestVersion,
    connection: Option<Extension<ConnectionHandle>>,
) -> Response {
    if !secret.starts_with(SECRET_PREFIX) {
        return invalid_ticket();
    }
    let redeemed = match store(&state).redeem(&secret_hash(secret)).await {
        Ok(Some(redeemed)) => redeemed,
        Ok(None) => return invalid_ticket(),
        Err(failure) => return store_failed(&failure),
    };
    let id = redeemed.id;
    // the ticket is spent from here on, whatever happens next. without
    // postgres the principal is `()`, which is all the export takes there
    #[allow(clippy::let_unit_value)]
    let principal = match principal_of(&state, &redeemed.minter).await {
        Ok(Some(principal)) => principal,
        Ok(None) => {
            let refused = error(
                StatusCode::UNAUTHORIZED,
                "the sign-in this download was started with is no longer valid; sign in again",
            );
            report(
                &state,
                id,
                &TicketState::Refused {
                    status: refused.status().as_u16(),
                    retry_after: None,
                },
            )
            .await;
            return refused;
        }
        Err(failure) => {
            report(
                &state,
                id,
                &TicketState::Refused {
                    status: 500,
                    retry_after: None,
                },
            )
            .await;
            return store_failed(&failure);
        }
    };
    let access = match reach_of(&state, &principal).await {
        Ok(access) => access,
        Err(refused) => return refused_with_status(&state, id, refused).await,
    };
    report(&state, id, &TicketState::Streaming { rows: 0 }).await;
    let progress = Arc::new(ExportProgress::default());
    let response = analytics_export::run_redeemed(
        state.clone(),
        redeemed.export,
        &redeemed.query,
        principal,
        access,
        version,
        connection,
        Arc::clone(&progress),
    )
    .await;
    if !response.status().is_success() {
        return refused_with_status(&state, id, response).await;
    }
    track(state, id, progress, response)
}

/// Record a download answered with an error as `refused`, with its status and
/// any `Retry-After`, and pass the answer on.
async fn refused_with_status(state: &ControlState, id: Uuid, response: Response) -> Response {
    let retry_after = response
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok());
    report(
        state,
        id,
        &TicketState::Refused {
            status: response.status().as_u16(),
            retry_after,
        },
    )
    .await;
    response
}

/// The end of a ticket's export, written when the body ends, fails or is
/// dropped unfinished.
struct Tracker {
    state: ControlState,
    id: Uuid,
    progress: Arc<ExportProgress>,
    reported_at: Instant,
    ended: bool,
}

impl Tracker {
    /// A chunk went out: a progress write, if the last one is old enough.
    async fn tick(&mut self) {
        if self.reported_at.elapsed() >= PROGRESS_EVERY {
            self.reported_at = Instant::now();
            let rows = self.progress.rows();
            report(&self.state, self.id, &TicketState::Streaming { rows }).await;
        }
    }

    async fn end(&mut self, end: TicketState) {
        self.ended = true;
        report(&self.state, self.id, &end).await;
    }

    fn truncated(&self, otherwise: TruncatedReason) -> TicketState {
        TicketState::Truncated {
            rows: self.progress.rows(),
            reason: self.progress.cut().map_or(otherwise, TruncatedReason::from),
        }
    }
}

impl Drop for Tracker {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        // the body was dropped before its end: the browser cancelled, or the
        // connection went. the write is spawned, since a drop cannot wait
        let end = self.truncated(TruncatedReason::ClientClosed);
        let (state, id) = (self.state.clone(), self.id);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move { report(&state, id, &end).await });
        }
    }
}

/// `response` with a body that reports its progress and its end to the ticket.
fn track(
    state: ControlState,
    id: Uuid,
    progress: Arc<ExportProgress>,
    response: Response,
) -> Response {
    let (parts, body) = response.into_parts();
    let tracker = Tracker {
        state,
        id,
        progress,
        reported_at: Instant::now(),
        ended: false,
    };
    let body = stream::unfold(
        (body.into_data_stream(), tracker),
        |(mut inner, mut tracker)| async move {
            if tracker.ended {
                return None;
            }
            match inner.next().await {
                Some(Ok(chunk)) => {
                    tracker.tick().await;
                    Some((Ok(chunk), (inner, tracker)))
                }
                Some(Err(failure)) => {
                    let end = tracker.truncated(TruncatedReason::ReadError);
                    tracker.end(end).await;
                    Some((Err(failure), (inner, tracker)))
                }
                None => {
                    let rows = tracker.progress.rows();
                    tracker.end(TicketState::Complete { rows }).await;
                    None
                }
            }
        },
    );
    Response::from_parts(parts, Body::from_stream(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analytics::ClickHouseClient;
    use crate::analytics_export::{drain, fake_log, ExportGate, ExportLimits};
    use tower::ServiceExt;

    /// A bearer that is not shaped like any real credential, built at run time.
    fn admin_token() -> String {
        format!("tickets-admin-{}", Uuid::new_v4().simple())
    }

    fn app_with(state: ControlState) -> Router {
        crate::analytics_export::router().with_state(state)
    }

    /// A store-less control plane with an admin token, a fake ClickHouse
    /// holding `rows` log rows, the given bounds and ticket lifetime.
    async fn memory_app(rows: u64, limits: ExportLimits, lifetime: Duration) -> (Router, String) {
        let token = admin_token();
        let mut state = crate::tests::state_with_token(Some(&token));
        state.clickhouse = Some(ClickHouseClient::new(&fake_log(rows, Duration::ZERO).await));
        state.export_gate = Arc::new(ExportGate::new(limits));
        state.export_tickets = Arc::new(Tickets::with_lifetime(lifetime));
        (app_with(state), token)
    }

    async fn send(app: &Router, request: axum::http::Request<Body>) -> Response {
        app.clone()
            .oneshot(request)
            .await
            .expect("the router answers")
    }

    fn get(uri: &str, bearer: Option<&str>) -> axum::http::Request<Body> {
        let mut request = axum::http::Request::get(uri);
        if let Some(bearer) = bearer {
            request = request.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
        }
        request.body(Body::empty()).expect("a valid request")
    }

    async fn mint_for(app: &Router, bearer: &str, path: &str, query: &str) -> Response {
        let request = axum::http::Request::post("/api/v1/analytics/exports/tickets")
            .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"path": path, "query": query}).to_string(),
            ))
            .expect("a valid request");
        send(app, request).await
    }

    async fn json_of(response: Response) -> serde_json::Value {
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("a readable body");
        serde_json::from_slice(&body).expect("a json body")
    }

    async fn text_of(response: Response) -> String {
        let body = axum::body::to_bytes(response.into_body(), 1 << 24)
            .await
            .expect("a readable body");
        String::from_utf8_lossy(&body).into_owned()
    }

    /// Mint for the request log and answer `(id, download url)`.
    async fn minted(app: &Router, bearer: &str, query: &str) -> (String, String) {
        let response = mint_for(app, bearer, LOG, query).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_of(response).await;
        (
            body["id"].as_str().unwrap().to_string(),
            body["download_url"].as_str().unwrap().to_string(),
        )
    }

    async fn status_of(app: &Router, bearer: &str, id: &str) -> serde_json::Value {
        let response = send(
            app,
            get(
                &format!("/api/v1/analytics/exports/tickets/{id}"),
                Some(bearer),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        json_of(response).await
    }

    const LOG: &str = "/api/v1/analytics/invocations/export";

    fn unbounded() -> ExportLimits {
        ExportLimits::new(0, 0, 0, 0)
    }

    #[tokio::test]
    async fn a_download_path_is_redacted_wherever_it_is_echoed() {
        let secret = new_secret();
        let path = format!("{DOWNLOAD_PREFIX}{secret}/extra");
        let shown = redact_path(&path);
        assert!(!shown.contains(&secret), "{shown}");
        assert!(shown.starts_with(DOWNLOAD_PREFIX));
        // anything else is echoed as it was
        assert_eq!(redact_path("/api/v1/nope"), "/api/v1/nope");
        assert_eq!(redact_path(DOWNLOAD_PREFIX), DOWNLOAD_PREFIX);
        // and the api 404 the dashboard sees does not carry it
        let response = crate::api_not_found(&path);
        let body = text_of(response).await;
        assert!(!body.contains(&secret), "{body}");
    }

    #[test]
    fn a_secret_is_32_random_bytes_and_only_its_hash_is_kept() {
        let one = new_secret();
        let two = new_secret();
        assert_ne!(one, two);
        let encoded = one.strip_prefix(SECRET_PREFIX).unwrap();
        use base64::Engine;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .unwrap();
        assert_eq!(bytes.len(), 32);
        assert_eq!(secret_hash(&one).len(), 64);
        assert!(!secret_hash(&one).contains(encoded));
    }

    #[test]
    fn a_state_reads_as_the_status_documents_it() {
        let shape = |state: TicketState| serde_json::to_value(state).unwrap();
        assert_eq!(shape(TicketState::Pending), json!({"state": "pending"}));
        assert_eq!(
            shape(TicketState::Truncated {
                rows: 3,
                reason: TruncatedReason::RowLimit
            }),
            json!({"state": "truncated", "rows": 3, "reason": "row_limit"})
        );
        assert_eq!(
            shape(TicketState::Refused {
                status: 429,
                retry_after: Some(15)
            }),
            json!({"state": "refused", "status": 429, "retry_after": 15})
        );
        assert_eq!(
            shape(TicketState::Refused {
                status: 401,
                retry_after: None
            }),
            json!({"state": "refused", "status": 401})
        );
        for state in [
            TicketState::Pending,
            TicketState::Streaming { rows: 1 },
            TicketState::Complete { rows: 1 },
            TicketState::Expired,
        ] {
            let back: TicketState = serde_json::from_value(shape(state.clone())).unwrap();
            assert_eq!(back, state);
        }
    }

    #[tokio::test]
    async fn a_ticket_downloads_the_file_a_bearer_request_gets_once() {
        let (app, token) = memory_app(2500, unbounded(), TICKET_LIFETIME).await;
        let query = "status=all&since=2026-10-01T00:00:00Z";
        let bearer = send(&app, get(&format!("{LOG}?{query}"), Some(&token))).await;
        assert_eq!(bearer.status(), StatusCode::OK);
        let expected = text_of(bearer).await;

        let (id, url) = minted(&app, &token, query).await;
        assert_eq!(status_of(&app, &token, &id).await["state"], "pending");
        let response = send(&app, get(&url, None)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let headers = response.headers().clone();
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        assert_eq!(headers[header::REFERRER_POLICY], "no-referrer");
        assert!(headers[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("attachment; filename=\"request-log_2026-10-01"));
        assert_eq!(text_of(response).await, expected);
        let status = status_of(&app, &token, &id).await;
        assert_eq!(status["state"], "complete", "{status}");
        assert_eq!(status["rows"], 2500);
        assert_eq!(status["done"], true);
        assert_eq!(status["export"], "invocations");

        // the same link again is refused, and the refusal does not quote it
        let again = send(&app, get(&url, None)).await;
        assert_eq!(again.status(), StatusCode::GONE);
        assert_eq!(again.headers()[header::REFERRER_POLICY], "no-referrer");
        let secret = url.strip_prefix(DOWNLOAD_PREFIX).unwrap();
        assert!(!text_of(again).await.contains(secret));
    }

    #[tokio::test]
    async fn the_rollup_exports_redeem_too() {
        let (app, token) = memory_app(0, unbounded(), TICKET_LIFETIME).await;
        for (path, query) in [
            ("/api/v1/analytics/timeseries/export", "bucket=day"),
            (
                "/api/v1/analytics/by-attribution/export",
                "dimension=customer&include_unattributed=true",
            ),
        ] {
            let response = mint_for(&app, &token, path, query).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            let body = json_of(response).await;
            let download = send(&app, get(body["download_url"].as_str().unwrap(), None)).await;
            assert_eq!(download.status(), StatusCode::OK, "{path}");
            let _ = text_of(download).await;
            let status = status_of(&app, &token, body["id"].as_str().unwrap()).await;
            assert_eq!(status["state"], "complete", "{path}: {status}");
        }
    }

    #[tokio::test]
    async fn an_unknown_or_expired_ticket_gets_the_one_refusal() {
        let (app, token) = memory_app(10, unbounded(), Duration::ZERO).await;
        let (id, url) = minted(&app, &token, "").await;
        let expired = send(&app, get(&url, None)).await;
        assert_eq!(expired.status(), StatusCode::GONE);
        let expired = text_of(expired).await;
        assert_eq!(status_of(&app, &token, &id).await["state"], "expired");
        // a secret never minted reads exactly like an expired one
        let unknown = send(
            &app,
            get(&format!("{DOWNLOAD_PREFIX}{}", new_secret()), None),
        )
        .await;
        assert_eq!(unknown.status(), StatusCode::GONE);
        assert_eq!(text_of(unknown).await, expired);
        let garbage = send(&app, get(&format!("{DOWNLOAD_PREFIX}nothing"), None)).await;
        assert_eq!(garbage.status(), StatusCode::GONE);
    }

    #[tokio::test]
    async fn a_ticket_is_only_minted_for_an_export_with_a_query_it_takes() {
        let (app, token) = memory_app(10, unbounded(), TICKET_LIFETIME).await;
        for path in [
            "/api/v1/analytics/invocations",
            "/api/v1/audit-log/export",
            "/api/v1/analytics/invocations/export/",
            "/api/v1/analytics/exports/download/x",
        ] {
            let response = mint_for(&app, &token, path, "").await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        }
        // a bound the export would refuse is refused at mint
        let response = mint_for(&app, &token, LOG, "since=yesterday").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        // and nobody without the export's own credential mints at all
        let response = mint_for(&app, "not-the-admin-token", LOG, "").await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let status = send(
            &app,
            get(
                &format!("/api/v1/analytics/exports/tickets/{}", Uuid::new_v4()),
                None,
            ),
        )
        .await;
        assert_eq!(status.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_full_gate_is_refused_at_mint_and_reported_through_the_status() {
        let (app, token) = memory_app(10, ExportLimits::new(0, 0, 4, 1), TICKET_LIFETIME).await;
        // minted while the gate has room
        let (id, url) = minted(&app, &token, "").await;
        // then the caller's one place is taken by a bearer export, held open
        let held = send(&app, get(LOG, Some(&token))).await;
        assert_eq!(held.status(), StatusCode::OK);
        // a new mint is refused at once
        let refused = mint_for(&app, &token, LOG, "").await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(refused.headers()[header::RETRY_AFTER], "15");
        // a rollup is not gated, so it still mints
        let rollup = mint_for(&app, &token, "/api/v1/analytics/timeseries/export", "").await;
        assert_eq!(rollup.status(), StatusCode::OK);
        // and the ticket minted before is refused when the browser follows it,
        // which only the status can tell the page
        let download = send(&app, get(&url, None)).await;
        assert_eq!(download.status(), StatusCode::TOO_MANY_REQUESTS);
        let status = status_of(&app, &token, &id).await;
        assert_eq!(
            (status["state"].clone(), status["status"].clone()),
            (json!("refused"), json!(429))
        );
        assert_eq!(status["retry_after"], 15);
        assert_eq!(status["done"], true);
        drop(held);
    }

    #[tokio::test]
    async fn a_truncated_export_ends_as_truncated_with_its_reason() {
        let (app, token) =
            memory_app(2500, ExportLimits::new(1500, 0, 0, 0), TICKET_LIFETIME).await;
        let (id, url) = minted(&app, &token, "").await;
        let response = send(&app, get(&url, None)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(drain(response).await, (1500, false));
        let status = status_of(&app, &token, &id).await;
        assert_eq!(status["state"], "truncated", "{status}");
        assert_eq!(status["rows"], 1500);
        assert_eq!(status["reason"], "row_limit");
    }

    #[tokio::test]
    async fn a_download_dropped_part_way_ends_as_client_closed() {
        let (app, token) = memory_app(5000, unbounded(), TICKET_LIFETIME).await;
        let (id, url) = minted(&app, &token, "").await;
        let response = send(&app, get(&url, None)).await;
        assert_eq!(response.status(), StatusCode::OK);
        // the first page arrives, then the browser goes away
        let mut body = response.into_body().into_data_stream();
        assert!(body.next().await.is_some());
        drop(body);
        let mut status = json!(null);
        for _ in 0..100 {
            status = status_of(&app, &token, &id).await;
            if status["done"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(status["state"], "truncated", "{status}");
        assert_eq!(status["reason"], "client_closed");
    }

    #[tokio::test]
    async fn a_rotated_admin_token_no_longer_redeems() {
        let token = admin_token();
        let mut state = crate::tests::state_with_token(Some(&token));
        state.clickhouse = Some(ClickHouseClient::new(&fake_log(10, Duration::ZERO).await));
        let tickets = Arc::clone(&state.export_tickets);
        let app = app_with(state.clone());
        let (_, url) = minted(&app, &token, "").await;
        // the same in-memory store under a control plane given another token
        let mut rotated = crate::tests::state_with_token(Some(&admin_token()));
        rotated.clickhouse = state.clickhouse.clone();
        rotated.export_tickets = tickets;
        let response = send(&app_with(rotated), get(&url, None)).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let secret = url.strip_prefix(DOWNLOAD_PREFIX).unwrap();
        assert!(!text_of(response).await.contains(secret));
    }

    /// The same flows against the Postgres store, where principals are people
    /// and tokens.
    #[cfg(feature = "postgres")]
    mod postgres {
        use super::*;
        use rolter_store::postgres::test_schema::TestSchema;

        async fn scratch() -> Option<TestSchema> {
            let url = rolter_store::postgres::test_database::url()
                .await
                .or_else(|| {
                    eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
                    None
                })?;
            Some(TestSchema::migrated(&url).await)
        }

        async fn pg_app(db: &TestSchema, admin: &str, lifetime: Duration) -> Router {
            let mut state = crate::test_state(db.pool().clone(), Some(admin.to_string()), None);
            state.clickhouse = Some(ClickHouseClient::new(&fake_log(1200, Duration::ZERO).await));
            state.export_tickets = Arc::new(Tickets::with_lifetime(lifetime));
            app_with(state)
        }

        /// A superadmin account with a live session, as `(user id, bearer)`.
        async fn signed_in(pool: &sqlx::PgPool) -> (Uuid, String) {
            let user: Uuid = sqlx::query_scalar(
                "insert into users (email, is_superadmin) values ($1, true) returning id",
            )
            .bind(format!("tickets-{}@example.test", Uuid::new_v4().simple()))
            .fetch_one(pool)
            .await
            .unwrap();
            let bearer = format!("rolter_sess_{}", Uuid::new_v4().simple());
            sqlx::query(
                "insert into sessions (user_id, token_hash, expires_at) \
                 values ($1, $2, now() + interval '1 hour')",
            )
            .bind(user)
            .bind(rolter_auth::hash_key(
                &crate::auth::session_pepper(),
                &bearer,
            ))
            .execute(pool)
            .await
            .unwrap();
            (user, bearer)
        }

        /// A viewer API token on a fresh org, as `(token id, bearer)`.
        async fn api_token(pool: &sqlx::PgPool) -> (Uuid, String) {
            let org: Uuid =
                sqlx::query_scalar("insert into orgs (name, slug) values ($1, $1) returning id")
                    .bind(format!("tickets-{}", Uuid::new_v4().simple()))
                    .fetch_one(pool)
                    .await
                    .unwrap();
            let bearer = format!(
                "{}{}",
                crate::api_tokens::TOKEN_PREFIX,
                Uuid::new_v4().simple()
            );
            let id: Uuid = sqlx::query_scalar(
                "insert into api_tokens (org_id, name, role, token_hash, token_prefix) \
                 values ($1, 'export', 'viewer', $2, 'rolter_api_x') returning id",
            )
            .bind(org)
            .bind(rolter_auth::hash_key(
                &crate::auth::session_pepper(),
                &bearer,
            ))
            .fetch_one(pool)
            .await
            .unwrap();
            (id, bearer)
        }

        #[tokio::test]
        async fn a_session_ticket_downloads_once_and_only_its_minter_reads_it() {
            let Some(db) = scratch().await else { return };
            let app = pg_app(&db, &admin_token(), TICKET_LIFETIME).await;
            let (_, alice) = signed_in(db.pool()).await;
            let (_, bob) = signed_in(db.pool()).await;
            let bearer = text_of(send(&app, get(LOG, Some(&alice))).await).await;
            let (id, url) = minted(&app, &alice, "").await;
            // stored as the session's id and the secret's hash, never either bearer
            let (kind, hash, session): (String, String, Option<Uuid>) = sqlx::query_as(
                "select minter_kind, secret_hash, session_id from export_tickets where id = $1",
            )
            .bind(id.parse::<Uuid>().unwrap())
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(kind, "session");
            assert!(session.is_some());
            let secret = url.strip_prefix(DOWNLOAD_PREFIX).unwrap();
            assert_eq!(hash, secret_hash(secret));

            // bob signed in too, but the ticket is not his to watch
            let theirs = send(
                &app,
                get(
                    &format!("/api/v1/analytics/exports/tickets/{id}"),
                    Some(&bob),
                ),
            )
            .await;
            assert_eq!(theirs.status(), StatusCode::NOT_FOUND);

            let response = send(&app, get(&url, None)).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(text_of(response).await, bearer);
            let status = status_of(&app, &alice, &id).await;
            assert_eq!(status["state"], "complete", "{status}");
            assert_eq!(status["rows"], 1200);
            assert_eq!(send(&app, get(&url, None)).await.status(), StatusCode::GONE);
        }

        #[tokio::test]
        async fn a_ticket_from_a_session_since_revoked_is_refused() {
            let Some(db) = scratch().await else { return };
            let app = pg_app(&db, &admin_token(), TICKET_LIFETIME).await;
            // a session that lapsed after the mint
            let (_, lapsed) = signed_in(db.pool()).await;
            let (id, url) = minted(&app, &lapsed, "").await;
            sqlx::query(
                "update sessions set expires_at = now() - interval '1 second' \
                 where token_hash = $1",
            )
            .bind(rolter_auth::hash_key(
                &crate::auth::session_pepper(),
                &lapsed,
            ))
            .execute(db.pool())
            .await
            .unwrap();
            let response = send(&app, get(&url, None)).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            let secret = url.strip_prefix(DOWNLOAD_PREFIX).unwrap();
            assert!(!text_of(response).await.contains(secret));
            let (status,): (serde_json::Value,) =
                sqlx::query_as("select status from export_tickets where id = $1")
                    .bind(id.parse::<Uuid>().unwrap())
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert_eq!(status, json!({"state": "refused", "status": 401}));

            // an account deactivated since
            let (user, active) = signed_in(db.pool()).await;
            let (_, url) = minted(&app, &active, "").await;
            sqlx::query("update users set deactivated_at = now() where id = $1")
                .bind(user)
                .execute(db.pool())
                .await
                .unwrap();
            assert_eq!(
                send(&app, get(&url, None)).await.status(),
                StatusCode::UNAUTHORIZED
            );

            // a session signed out since takes its tickets with it
            let (_, signed_out) = signed_in(db.pool()).await;
            let (_, url) = minted(&app, &signed_out, "").await;
            sqlx::query("delete from sessions where token_hash = $1")
                .bind(rolter_auth::hash_key(
                    &crate::auth::session_pepper(),
                    &signed_out,
                ))
                .execute(db.pool())
                .await
                .unwrap();
            assert_eq!(send(&app, get(&url, None)).await.status(), StatusCode::GONE);
        }

        #[tokio::test]
        async fn a_ticket_from_an_api_token_since_revoked_is_refused() {
            let Some(db) = scratch().await else { return };
            let app = pg_app(&db, &admin_token(), TICKET_LIFETIME).await;
            let (token_id, bearer) = api_token(db.pool()).await;
            let (id, url) = minted(&app, &bearer, "").await;
            // the token reads its own status
            assert_eq!(status_of(&app, &bearer, &id).await["state"], "pending");
            sqlx::query("update api_tokens set revoked_at = now() where id = $1")
                .bind(token_id)
                .execute(db.pool())
                .await
                .unwrap();
            assert_eq!(
                send(&app, get(&url, None)).await.status(),
                StatusCode::UNAUTHORIZED
            );
        }

        #[tokio::test]
        async fn an_expired_ticket_is_refused_and_one_redemption_wins() {
            let Some(db) = scratch().await else { return };
            let admin = admin_token();
            let app = pg_app(&db, &admin, Duration::ZERO).await;
            let (id, url) = minted(&app, &admin, "").await;
            assert_eq!(send(&app, get(&url, None)).await.status(), StatusCode::GONE);
            assert_eq!(status_of(&app, &admin, &id).await["state"], "expired");

            // two replicas racing on one secret: the statement lets one through
            let app = pg_app(&db, &admin, TICKET_LIFETIME).await;
            let (_, url) = minted(&app, &admin, "").await;
            let (one, two) = tokio::join!(send(&app, get(&url, None)), send(&app, get(&url, None)));
            let mut statuses = [one.status(), two.status()];
            statuses.sort();
            assert_eq!(statuses, [StatusCode::OK, StatusCode::GONE]);
        }

        #[tokio::test]
        async fn an_expired_row_is_purged_by_a_later_mint() {
            let Some(db) = scratch().await else { return };
            let admin = admin_token();
            let app = pg_app(&db, &admin, TICKET_LIFETIME).await;
            let (old, _) = minted(&app, &admin, "").await;
            sqlx::query(
                "update export_tickets set expires_at = now() - interval '2 days' where id = $1",
            )
            .bind(old.parse::<Uuid>().unwrap())
            .execute(db.pool())
            .await
            .unwrap();
            let _ = minted(&app, &admin, "").await;
            let left: i64 = sqlx::query_scalar("select count(*) from export_tickets where id = $1")
                .bind(old.parse::<Uuid>().unwrap())
                .fetch_one(db.pool())
                .await
                .unwrap();
            assert_eq!(left, 0);
        }
    }
}
