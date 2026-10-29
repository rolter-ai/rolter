//! Single-use download tickets for the analytics CSV exports (#3504) and the
//! audit log's CSV and JSON exports (#3545).
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
//!   bearer, one of the export paths and the query string the button would
//!   send. The export's own checks run first, so a caller who could not run the
//!   export cannot mint for it: the request log's reach for the analytics
//!   exports, and the audit log's own capability for its two (`audit_log:read`
//!   on the org, `deployment_audit_log:read` for the whole deployment). The
//!   query is parsed as the export parses it; and for the request log the
//!   [`ExportGate`] is asked, without taking a place, whether it would let the
//!   export in now, so a full gate is the usual `429` straight away.
//! * **The secret** is 32 bytes from the OS-seeded CSPRNG, base64url, returned
//!   once and stored only as its SHA-256. The bearer it was minted with is never
//!   stored: the [`Minter`] keeps what can be checked again at redemption.
//! * **Download** (`GET /api/v1/analytics/exports/download/{ticket}`) consumes
//!   the ticket in one statement, so it is single use even across replicas, and
//!   even when the export then fails or is refused. Unknown, used and expired
//!   secrets get one uniform `410`. The minter is re-resolved (a revoked
//!   session, a deactivated account or a deleted or revoked API token is
//!   refused), and the export runs through [`analytics_export::run_redeemed`]:
//!   the bearer route's handler, with every check it runs for a bearer request,
//!   the audit log's capability check included.
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
use axum::extract::{FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
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

/// How long the in-memory store keeps a ticket once it can change no more (it
/// expired unused, or its download ended), so the page's last status reads
/// still find it.
const MEMORY_KEEP_AFTER_END: Duration = Duration::from_secs(5 * 60);

/// The most tickets the in-memory store holds at once. Finished tickets leave
/// within [`MEMORY_KEEP_AFTER_END`], so only a loop of mints reaches it.
const MEMORY_CAPACITY: usize = 10_000;

/// How long a streaming ticket may go without a progress write before its
/// status reads as interrupted, when exports have no time limit. With one, the
/// limit plus two progress intervals is used instead: a live export writes at
/// least that often, or the limit ends it.
const UNBOUNDED_STALE_AFTER: Duration = Duration::from_secs(60 * 60);

/// The longest query string a ticket stores. The export URLs the dashboard
/// builds are a few hundred bytes.
const MAX_QUERY_LEN: usize = 4096;

/// The prefix of the download route. Everything after it is the secret.
pub(crate) const DOWNLOAD_PREFIX: &str = "/api/v1/analytics/exports/download/";

/// What a minted secret starts with, so one found in a log can be told for
/// what it is.
const SECRET_PREFIX: &str = "rolter_dl_";

/// The largest mint body read: a path and a query of at most
/// [`MAX_QUERY_LEN`], with room for the JSON around them.
const MAX_MINT_BODY: usize = 16 * 1024;

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

/// The digest of the admin token a ticket stores, keyed with the ticket's own
/// secret. The store holds only the secret's hash, so a dump of it cannot be
/// used to test guesses at the admin token offline; at redemption the presented
/// secret recomputes the digest, which tells a token rotated since.
fn admin_digest(secret: &str, token: &str) -> String {
    rolter_auth::hash_key(secret, token)
}

/// Whether `admin`, the admin token configured now, is the one a ticket's
/// `digest` was taken of under `secret`.
fn same_admin(secret: &str, admin: &str, digest: &str) -> bool {
    subtle::ConstantTimeEq::ct_eq(admin_digest(secret, admin).as_bytes(), digest.as_bytes()).into()
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
    /// the replica serving it stopped reporting: it went away mid-export
    Interrupted,
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

    /// Whether `next` may replace this state. Writes only move forward: nothing
    /// replaces an end, and a progress write never lowers the rows, so a late
    /// or reordered progress write cannot undo the end it raced.
    fn may_become(&self, next: &TicketState) -> bool {
        match (self, next) {
            (TicketState::Pending, next) => *next != TicketState::Pending,
            (TicketState::Streaming { rows }, TicketState::Streaming { rows: next }) => {
                next >= rows
            }
            (TicketState::Streaming { .. }, next) => next.is_terminal(),
            _ => false,
        }
    }

    /// The rows a progress write carries, for the store's forward-only check.
    #[cfg(feature = "postgres")]
    fn rows(&self) -> u64 {
        match self {
            TicketState::Streaming { rows }
            | TicketState::Complete { rows }
            | TicketState::Truncated { rows, .. } => *rows,
            _ => 0,
        }
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
    /// when the ticket was minted: a token rotated after it no longer redeems
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    minted_at: DateTime<Utc>,
}

/// A ticket as its status read sees it.
#[derive(Debug)]
struct TicketRecord {
    owner: String,
    export: ExportKind,
    expires_at: DateTime<Utc>,
    state: TicketState,
    /// how long since its state was last written
    idle: Duration,
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
    /// Record where the ticket's export has got to, if the stored state may
    /// become it ([`TicketState::may_become`]); answers whether it was written.
    async fn report(&self, id: Uuid, state: &TicketState) -> anyhow::Result<bool>;
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
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    used: bool,
    state: TicketState,
}

impl MemoryTicket {
    /// Whether the store still needs this ticket at `now`: one that can change
    /// no more is kept only for the page's last few status reads.
    fn wanted(&self, now: DateTime<Utc>) -> bool {
        let keep = chrono_span(MEMORY_KEEP_AFTER_END);
        if !self.used {
            self.expires_at > now - keep
        } else if self.state.is_terminal() {
            self.updated_at > now - keep
        } else {
            self.updated_at > now - chrono_span(RETAIN_PAST_EXPIRY)
        }
    }
}

fn chrono_span(duration: Duration) -> chrono::Duration {
    chrono::Duration::from_std(duration).unwrap_or(chrono::Duration::MAX)
}

#[async_trait::async_trait]
impl TicketStore for MemoryTickets {
    async fn insert(&self, ticket: NewTicket, lifetime: Duration) -> anyhow::Result<DateTime<Utc>> {
        let now = Utc::now();
        let mut map = self.tickets.lock();
        let MemoryMap { by_id, by_hash } = &mut *map;
        by_id.retain(|_, held| held.wanted(now));
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
                created_at: now,
                expires_at,
                updated_at: now,
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
        held.updated_at = now;
        Ok(Some(Redeemed {
            id,
            minter: held.minter.clone(),
            export: held.export,
            query: held.query.clone(),
            minted_at: held.created_at,
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
            idle: (now - held.updated_at).to_std().unwrap_or_default(),
        }))
    }

    async fn report(&self, id: Uuid, state: &TicketState) -> anyhow::Result<bool> {
        let mut map = self.tickets.lock();
        let Some(held) = map.by_id.get_mut(&id) else {
            return Ok(false);
        };
        if !held.state.may_become(state) {
            return Ok(false);
        }
        held.state = state.clone();
        held.updated_at = Utc::now();
        Ok(true)
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
        .bind(ticket.export.stored())
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
            DateTime<Utc>,
        );
        let row: Option<Row> = sqlx::query_as(
            "update export_tickets set used_at = now(), updated_at = now() \
             where secret_hash = $1 and used_at is null and expires_at > now() \
             returning id, minter_kind, session_id, api_token_id, admin_digest, export, query, \
                       created_at",
        )
        .bind(hash)
        .fetch_optional(self.0)
        .await?;
        Ok(row.and_then(
            |(id, kind, session, token, digest, export, query, minted_at)| {
                Some(Redeemed {
                    id,
                    minter: Self::minter_from(&kind, session, token, digest)?,
                    export: ExportKind::parse(&export)?,
                    query,
                    minted_at,
                })
            },
        ))
    }

    async fn read(&self, id: Uuid) -> anyhow::Result<Option<TicketRecord>> {
        type Row = (String, String, DateTime<Utc>, serde_json::Value, bool, f64);
        let row: Option<Row> = sqlx::query_as(
            "select owner, export, expires_at, status, \
                        (used_at is null and expires_at <= now()) as lapsed, \
                        greatest(extract(epoch from now() - updated_at), 0)::double precision \
                 from export_tickets where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await?;
        Ok(
            row.and_then(|(owner, export, expires_at, status, lapsed, idle)| {
                let state = match serde_json::from_value(status).ok()? {
                    TicketState::Pending if lapsed => TicketState::Expired,
                    state => state,
                };
                Some(TicketRecord {
                    owner,
                    export: ExportKind::parse(&export)?,
                    expires_at,
                    state,
                    idle: Duration::try_from_secs_f64(idle).unwrap_or_default(),
                })
            }),
        )
    }

    async fn report(&self, id: Uuid, state: &TicketState) -> anyhow::Result<bool> {
        // the forward-only rule of `TicketState::may_become`, in the statement
        // so two replicas' writes cannot interleave around a read
        let written = sqlx::query(
            "update export_tickets set status = $2, updated_at = now() \
             where id = $1 and ( \
                 status->>'state' = 'pending' \
                 or (status->>'state' = 'streaming' \
                     and ($3 or coalesce((status->>'rows')::bigint, 0) <= $4)))",
        )
        .bind(id)
        .bind(serde_json::to_value(state)?)
        .bind(state.is_terminal())
        .bind(i64::try_from(state.rows()).unwrap_or(i64::MAX))
        .execute(self.0)
        .await?
        .rows_affected();
        Ok(written > 0)
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
    async fn report(&self, id: Uuid, state: &TicketState) -> anyhow::Result<bool> {
        self.0.report(id, state).await
    }
}

/// Record `state` for ticket `id`, logging rather than failing: the export the
/// status describes is already under way, and a lost write only leaves the page
/// polling an older state. Answers whether the store answered at all, refused
/// or not, so a caller knows whether a resend could still help.
async fn report(state: &ControlState, id: Uuid, ticket_state: &TicketState) -> bool {
    match store(state).report(id, ticket_state).await {
        Ok(_) => true,
        Err(error) => {
            tracing::warn!(%error, ticket = %id, "recording an export download's progress failed");
            false
        }
    }
}

/// [`report`] without waiting for it, for a write that must not hold up the
/// body it describes.
fn report_later(state: &ControlState, id: Uuid, ticket_state: TicketState) {
    let state = state.clone();
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(async move {
            report(&state, id, &ticket_state).await;
        });
    }
}

/// How long a streaming ticket may go unwritten before its status reads as
/// interrupted: the export time limit plus two progress intervals, since a live
/// export either writes that often or is ended by the limit.
fn stale_after(state: &ControlState) -> Duration {
    state
        .export_gate
        .max_duration()
        .map_or(UNBOUNDED_STALE_AFTER, |limit| limit + 2 * PROGRESS_EVERY)
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
    secret: &str,
) -> Result<Minter, Response> {
    use crate::rbac::Principal;
    let Some(admin) = state.admin_token.as_deref() else {
        return Ok(Minter::Open);
    };
    match principal {
        Principal::Token(held) => Ok(Minter::ApiToken(held.token.id)),
        Principal::Superadmin { account: None } => Ok(Minter::Admin {
            digest: admin_digest(secret, admin),
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
async fn minter_of(
    state: &ControlState,
    _: &HeaderMap,
    _: &Caller,
    secret: &str,
) -> Result<Minter, Response> {
    // without a database the admin token is the only credential there is, and
    // the extractor that ran before has checked it
    Ok(match state.admin_token.as_deref() {
        None => Minter::Open,
        Some(admin) => Minter::Admin {
            digest: admin_digest(secret, admin),
        },
    })
}

/// The principal a ticket's minter is now, or `None` when that credential no
/// longer holds: a revoked or expired session, a deactivated account, a revoked,
/// expired, deleted or since-rotated API token, a rotated admin token, or an
/// open-mode ticket on a control plane that has since been given a token.
/// `secret` is the presented ticket, which an admin token's digest is keyed by.
#[cfg(feature = "postgres")]
async fn principal_of(
    state: &ControlState,
    redeemed: &Redeemed,
    secret: &str,
) -> anyhow::Result<Option<Caller>> {
    use crate::rbac::Principal;
    use rolter_store::postgres::repo::{SessionRepo, UserRepo};
    match (&redeemed.minter, state.admin_token.as_deref()) {
        (Minter::Open, None) => Ok(Some(Principal::Superadmin { account: None })),
        (Minter::Admin { digest }, Some(admin)) => Ok(
            same_admin(secret, admin, digest).then_some(Principal::Superadmin { account: None })
        ),
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
            let held = crate::api_tokens::resolve_id(pool, *id)
                .await
                .map_err(|failure| anyhow::anyhow!("{failure:?}"))?;
            // a rotation keeps the row and replaces the secret, so a ticket
            // minted with the old one goes with it
            Ok(held
                .filter(|held| {
                    held.token
                        .rotated_at
                        .is_none_or(|rotated| rotated <= redeemed.minted_at)
                })
                .map(Principal::Token))
        }
        _ => Ok(None),
    }
}

#[cfg(not(feature = "postgres"))]
async fn principal_of(
    state: &ControlState,
    redeemed: &Redeemed,
    secret: &str,
) -> anyhow::Result<Option<Caller>> {
    Ok(match (&redeemed.minter, state.admin_token.as_deref()) {
        (Minter::Open, None) => Some(()),
        (Minter::Admin { digest }, Some(admin)) => same_admin(secret, admin, digest).then_some(()),
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

/// Whether the control plane has a database, which the audit log and the
/// account credentials live in.
fn has_store(state: &ControlState) -> bool {
    #[cfg(feature = "postgres")]
    return state.pool.is_some();
    #[cfg(not(feature = "postgres"))]
    {
        let _ = state;
        false
    }
}

/// Who is asking, by the credential checks every export runs first. Without
/// a database the request log's reach is what refuses a wrong bearer, and
/// `Principal` would reach for the pool that is not there, so it goes first.
#[allow(clippy::result_large_err)]
async fn caller_of(state: &ControlState, parts: &mut Parts) -> Result<Caller, Response> {
    if !has_store(state) {
        AnalyticsAccess::from_request_parts(parts, state).await?;
    }
    Caller::from_request_parts(parts, state)
        .await
        .map_err(IntoResponse::into_response)
}

/// The caller, if the export `kind` would run for it: the checks its bearer
/// route runs before it reads anything.
#[allow(clippy::result_large_err)]
// without postgres every kind is an analytics export, and `kind` picks nothing
#[cfg_attr(not(feature = "postgres"), allow(unused_variables))]
async fn may_export(
    state: &ControlState,
    parts: &mut Parts,
    kind: ExportKind,
) -> Result<Caller, Response> {
    // the audit log exists only with a database, so only there is a kind one
    #[cfg(feature = "postgres")]
    if kind.is_audit() {
        let principal = caller_of(state, parts).await?;
        crate::audit_export::authorize_export(state, &principal, kind)
            .await
            .map_err(IntoResponse::into_response)?;
        return Ok(principal);
    }
    AnalyticsAccess::from_request_parts(parts, state).await?;
    Caller::from_request_parts(parts, state)
        .await
        .map_err(IntoResponse::into_response)
}

/// The paths a ticket may name, for the refusal of any other.
fn known_paths(audit: bool) -> String {
    let mut paths: Vec<String> = ExportKind::ANALYTICS
        .iter()
        .map(|kind| kind.path().into_owned())
        .collect();
    if audit {
        paths.push("/api/v1/audit-log/export".to_string());
        paths.push("/api/v1/orgs/{org_id}/audit-log/export".to_string());
    }
    format!("path must be one of {}", paths.join(", "))
}

/// `POST /api/v1/analytics/exports/tickets`: mint a download ticket for one
/// export. The caller is checked as the export itself checks it, so a caller
/// the export would refuse is refused here.
async fn mint(State(state): State<ControlState>, request: axum::extract::Request) -> Response {
    let (mut parts, body) = request.into_parts();
    // which export decides which checks run, so the body is read first; a
    // body that names none is refused only once the bearer has been, as the
    // analytics exports always did
    let body = axum::body::to_bytes(body, MAX_MINT_BODY).await;
    // read by hand rather than through the CRUD API's `SafeJson`, which only a
    // postgres build has. the path is matched against the export paths and the
    // query is parsed as a URI below, so neither can carry a control character
    let body: Result<MintRequest, String> = match body {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|failure| format!("invalid request body: {failure}")),
        Err(_) => Err("the request body is too large".to_string()),
    };
    // the audit log is there only on a control plane with a database
    let audit = has_store(&state);
    let named = body
        .as_ref()
        .ok()
        .and_then(|body| ExportKind::from_path(&body.path, audit));
    let principal = match named {
        Some(kind) => may_export(&state, &mut parts, kind).await,
        None => caller_of(&state, &mut parts).await,
    };
    // without postgres the principal is `()`, which is all the export takes there
    #[allow(clippy::let_unit_value)]
    let principal = match principal {
        Ok(principal) => principal,
        Err(refused) => return refused,
    };
    let headers = parts.headers;
    let body = match body {
        Ok(body) => body,
        Err(message) => return error(StatusCode::BAD_REQUEST, &message),
    };
    let Some(export) = named else {
        return error(StatusCode::BAD_REQUEST, &known_paths(audit));
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
    let secret = new_secret();
    let minter = match minter_of(&state, &headers, &principal, &secret).await {
        Ok(minter) => minter,
        Err(refused) => return refused,
    };
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
    State(state): State<ControlState>,
    Path(id): Path<String>,
    request: axum::extract::Request,
) -> Response {
    let (mut parts, _) = request.into_parts();
    // the minter's own credential is enough: the ticket was only minted once
    // the export's checks had passed, and the status says nothing but how it
    // went. an audit log reader who cannot read the request log reads it too
    #[allow(clippy::let_unit_value)]
    let principal = match caller_of(&state, &mut parts).await {
        Ok(principal) => principal,
        Err(refused) => return refused,
    };
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
    // a replica that went away mid-export leaves its ticket streaming with
    // nobody to end it: past the time a live export writes within, it reads
    // as cut short
    let ticket_state = match record.state {
        TicketState::Streaming { rows } if record.idle > stale_after(&state) => {
            TicketState::Truncated {
                rows,
                reason: TruncatedReason::Interrupted,
            }
        }
        ticket_state => ticket_state,
    };
    let mut response = Json(TicketStatus {
        id,
        export: record.export.as_str(),
        expires_at: record.expires_at,
        done: ticket_state.is_terminal(),
        state: ticket_state,
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
///
/// axum answers `HEAD` with the `GET` handler, and a link checker or a proxy
/// that sends one must not spend the ticket or take a place in the export
/// gate, so anything but `GET` is a `405` before the ticket is looked at.
async fn download(
    State(state): State<ControlState>,
    method: Method,
    uri: Uri,
    version: RequestVersion,
    connection: Option<Extension<ConnectionHandle>>,
) -> Response {
    let mut response = if method == Method::GET {
        let secret = uri.path().strip_prefix(DOWNLOAD_PREFIX).unwrap_or_default();
        redeem(state, secret, version, connection).await
    } else {
        let mut refused = error(
            StatusCode::METHOD_NOT_ALLOWED,
            "a download link is followed with GET only",
        );
        refused
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET"));
        refused
    };
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
    let principal = match principal_of(&state, &redeemed, secret).await {
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
    // the reach over the request log is what the analytics exports read; the
    // audit log's handler checks its own capability instead
    let access = if redeemed.export.is_audit() {
        None
    } else {
        match reach_of(&state, &principal).await {
            Ok(access) => Some(access),
            Err(refused) => return refused_with_status(&state, id, refused).await,
        }
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
///
/// Progress writes are spawned, so a slow store never stalls the body. The end
/// is kept here once it is known, and written once more from `Drop` if its
/// write did not complete, which covers a body dropped while that write was
/// still awaited. The store only moves a ticket forward, so a late progress
/// write cannot overwrite the end.
struct Tracker {
    state: ControlState,
    id: Uuid,
    progress: Arc<ExportProgress>,
    reported_at: Instant,
    /// the end this download reached, once it has one
    end: Option<TicketState>,
    /// the store has answered the end's write, or a spawned write carries it
    end_sent: bool,
}

impl Tracker {
    /// A chunk went out: a progress write, if the last one is old enough.
    fn tick(&mut self) {
        if self.reported_at.elapsed() >= PROGRESS_EVERY {
            self.reported_at = Instant::now();
            let rows = self.progress.rows();
            report_later(&self.state, self.id, TicketState::Streaming { rows });
        }
    }

    /// The last chunk has been handed on: write the end and wait for it.
    async fn finish(&mut self, end: TicketState) {
        self.end = Some(end.clone());
        self.end_sent = report(&self.state, self.id, &end).await;
    }

    /// The body failed: write the end without holding up the failure.
    fn fail(&mut self, end: TicketState) {
        self.end = Some(end.clone());
        self.end_sent = true;
        report_later(&self.state, self.id, end);
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
        if self.end_sent {
            return;
        }
        // the body was dropped before its end (the browser cancelled, or the
        // connection went), or while the end's own write was under way
        let end = self
            .end
            .take()
            .unwrap_or_else(|| self.truncated(TruncatedReason::ClientClosed));
        report_later(&self.state, self.id, end);
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
        end: None,
        end_sent: false,
    };
    let body = stream::unfold(
        (body.into_data_stream(), tracker),
        |(mut inner, mut tracker)| async move {
            if tracker.end.is_some() {
                return None;
            }
            match inner.next().await {
                Some(Ok(chunk)) => {
                    tracker.tick();
                    Some((Ok(chunk), (inner, tracker)))
                }
                Some(Err(failure)) => {
                    let end = tracker.truncated(TruncatedReason::ReadError);
                    tracker.fail(end);
                    Some((Err(failure), (inner, tracker)))
                }
                None => {
                    let rows = tracker.progress.rows();
                    tracker.finish(TicketState::Complete { rows }).await;
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

    /// The status once it can change no more: the end of a download is
    /// written after its last chunk, and a failure's without waiting for it.
    async fn settled(app: &Router, bearer: &str, id: &str) -> serde_json::Value {
        let mut status = json!(null);
        for _ in 0..200 {
            status = status_of(app, bearer, id).await;
            if status["done"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        status
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
        let status = settled(&app, &token, &id).await;
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
            let status = settled(&app, &token, body["id"].as_str().unwrap()).await;
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
        let status = settled(&app, &token, &id).await;
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
        let status = settled(&app, &token, &id).await;
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

    #[tokio::test]
    async fn a_head_request_is_refused_without_spending_the_ticket() {
        let (app, token) = memory_app(10, unbounded(), TICKET_LIFETIME).await;
        let (id, url) = minted(&app, &token, "").await;
        let head = axum::http::Request::head(&url)
            .body(Body::empty())
            .expect("a valid request");
        let refused = send(&app, head).await;
        assert_eq!(refused.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(refused.headers()[header::ALLOW], "GET");
        assert_eq!(refused.headers()[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(status_of(&app, &token, &id).await["state"], "pending");
        // the browser's GET still gets the file
        let response = send(&app, get(&url, None)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(drain(response).await, (10, true));
    }

    fn new_ticket(minter: Minter) -> NewTicket {
        NewTicket {
            id: Uuid::new_v4(),
            secret_hash: secret_hash(&new_secret()),
            minter,
            owner: "admin".into(),
            export: ExportKind::Invocations,
            query: String::new(),
        }
    }

    #[test]
    fn a_state_only_moves_forward() {
        use TicketState::*;
        let complete = Complete { rows: 5 };
        assert!(Pending.may_become(&Streaming { rows: 0 }));
        assert!(Pending.may_become(&complete));
        assert!(Streaming { rows: 1 }.may_become(&Streaming { rows: 2 }));
        assert!(!Streaming { rows: 2 }.may_become(&Streaming { rows: 1 }));
        assert!(Streaming { rows: 2 }.may_become(&complete));
        assert!(!complete.may_become(&Streaming { rows: 9 }));
        assert!(!complete.may_become(&Expired));
        assert!(!Streaming { rows: 1 }.may_become(&Pending));
    }

    /// A progress write that lands after the end, as a spawned one can, does
    /// not undo it.
    #[tokio::test]
    async fn a_late_progress_write_does_not_overwrite_the_end() {
        let store = MemoryTickets::default();
        let ticket = new_ticket(Minter::Open);
        let id = ticket.id;
        store.insert(ticket, TICKET_LIFETIME).await.unwrap();
        assert!(store
            .report(id, &TicketState::Streaming { rows: 10 })
            .await
            .unwrap());
        let end = TicketState::Complete { rows: 20 };
        assert!(store.report(id, &end).await.unwrap());
        assert!(!store
            .report(id, &TicketState::Streaming { rows: 15 })
            .await
            .unwrap());
        assert_eq!(store.read(id).await.unwrap().unwrap().state, end);
    }

    #[tokio::test]
    async fn the_memory_store_lets_finished_tickets_go_promptly() {
        let store = MemoryTickets::default();
        let ended = new_ticket(Minter::Open);
        let ended_id = ended.id;
        let lapsed = new_ticket(Minter::Open);
        let lapsed_id = lapsed.id;
        let live = new_ticket(Minter::Open);
        let live_id = live.id;
        for ticket in [ended, lapsed, live] {
            store.insert(ticket, TICKET_LIFETIME).await.unwrap();
        }
        {
            let mut map = store.tickets.lock();
            let long_ago = Utc::now() - chrono_span(2 * MEMORY_KEEP_AFTER_END);
            let ended = map.by_id.get_mut(&ended_id).unwrap();
            ended.used = true;
            ended.state = TicketState::Complete { rows: 1 };
            ended.updated_at = long_ago;
            map.by_id.get_mut(&lapsed_id).unwrap().expires_at = long_ago;
        }
        store
            .insert(new_ticket(Minter::Open), TICKET_LIFETIME)
            .await
            .unwrap();
        let map = store.tickets.lock();
        assert!(!map.by_id.contains_key(&ended_id));
        assert!(!map.by_id.contains_key(&lapsed_id));
        assert!(map.by_id.contains_key(&live_id));
        assert_eq!(map.by_hash.len(), map.by_id.len());
    }

    /// A replica that went away mid-export never writes the end: past the time
    /// a live export writes within, the status reads as interrupted.
    #[tokio::test]
    async fn a_ticket_left_streaming_reads_as_interrupted() {
        let token = admin_token();
        let mut state = crate::tests::state_with_token(Some(&token));
        state.export_gate = Arc::new(ExportGate::new(ExportLimits::new(0, 1, 0, 0)));
        let app = app_with(state.clone());
        let (id, _) = minted(&app, &token, "").await;
        let id_uuid = id.parse::<Uuid>().unwrap();
        {
            let mut map = state.export_tickets.memory.tickets.lock();
            let held = map.by_id.get_mut(&id_uuid).unwrap();
            held.used = true;
            held.state = TicketState::Streaming { rows: 7 };
            held.updated_at = Utc::now() - chrono::Duration::seconds(30);
        }
        let status = status_of(&app, &token, &id).await;
        assert_eq!(status["state"], "truncated", "{status}");
        assert_eq!(status["reason"], "interrupted");
        assert_eq!(status["rows"], 7);
        assert_eq!(status["done"], true);
    }

    #[test]
    fn an_admin_ticket_stores_no_unkeyed_digest_of_the_token() {
        let admin = admin_token();
        let (one, two) = (new_secret(), new_secret());
        // keyed by the ticket: the same token leaves a different digest per
        // ticket, so a dump holds nothing to test guesses against
        assert_ne!(admin_digest(&one, &admin), admin_digest(&two, &admin));
        let digest = admin_digest(&one, &admin);
        assert!(same_admin(&one, &admin, &digest));
        assert!(!same_admin(&two, &admin, &digest));
        assert!(!same_admin(&one, &admin_token(), &digest));
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
            let status = settled(&app, &alice, &id).await;
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

        /// A ClickHouse that answers three log rows to every page and keeps the
        /// query parameters of each request, so a test can compare the row
        /// scope a bearer export and a ticket download bound.
        async fn recording_log() -> (String, Arc<parking_lot::Mutex<Vec<Vec<(String, String)>>>>) {
            let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
            let kept = Arc::clone(&seen);
            let app = axum::Router::new().fallback(
                move |axum::extract::Query(params): axum::extract::Query<Vec<(String, String)>>,
                      body: String| {
                    let kept = Arc::clone(&kept);
                    async move {
                        if body.contains("system.columns") {
                            return axum::Json(json!({"data": []}));
                        }
                        kept.lock().push(params);
                        let rows: Vec<serde_json::Value> = (1..=3)
                            .map(|seq| {
                                json!({"ts": "2026-10-01T00:00:00.000Z", "seq": seq.to_string(),
                                       "request_id": format!("req-{seq}"), "status": 200})
                            })
                            .collect();
                        axum::Json(json!({ "data": rows }))
                    }
                },
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            tokio::spawn(async move { axum::serve(listener, app).await });
            (url, seen)
        }

        /// The row-scope bindings a request carried: everything `AnalyticsAccess` binds.
        fn scope(params: &[(String, String)]) -> Vec<(String, String)> {
            let mut scope: Vec<_> = params
                .iter()
                .filter(|(k, _)| {
                    [
                        "param_member_",
                        "param_granted_",
                        "param_unrestricted",
                        "param_read_rank",
                    ]
                    .iter()
                    .any(|prefix| k.starts_with(prefix))
                })
                .cloned()
                .collect();
            scope.sort();
            scope
        }

        /// A viewer of one org, signed in, as `(user, org, bearer)`.
        async fn org_viewer(pool: &sqlx::PgPool) -> (Uuid, Uuid, String) {
            let org: Uuid =
                sqlx::query_scalar("insert into orgs (name, slug) values ($1, $1) returning id")
                    .bind(format!("tickets-{}", Uuid::new_v4().simple()))
                    .fetch_one(pool)
                    .await
                    .unwrap();
            let user: Uuid =
                sqlx::query_scalar("insert into users (email) values ($1) returning id")
                    .bind(format!("viewer-{}@example.test", Uuid::new_v4().simple()))
                    .fetch_one(pool)
                    .await
                    .unwrap();
            sqlx::query(
                "insert into memberships (user_id, org_id, role) values ($1, $2, 'viewer')",
            )
            .bind(user)
            .bind(org)
            .execute(pool)
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
            (user, org, bearer)
        }

        #[tokio::test]
        async fn a_scoped_minter_downloads_exactly_its_bearer_scope() {
            let Some(db) = scratch().await else { return };
            let (url, seen) = recording_log().await;
            let mut state = crate::test_state(db.pool().clone(), Some(admin_token()), None);
            state.clickhouse = Some(ClickHouseClient::new(&url));
            let app = app_with(state);
            let (_, org, viewer) = org_viewer(db.pool()).await;

            let bearer = text_of(send(&app, get(LOG, Some(&viewer))).await).await;
            let bearer_scope = scope(&seen.lock().last().unwrap().clone());
            // a viewer's scope: restricted, and naming their org
            assert!(bearer_scope.contains(&("param_unrestricted".into(), "0".into())));
            assert!(bearer_scope
                .iter()
                .any(|(k, v)| k == "param_member_org_ids" && v.contains(&org.to_string())));

            let (_, link) = minted(&app, &viewer, "").await;
            let ticket = text_of(send(&app, get(&link, None)).await).await;
            assert_eq!(ticket, bearer);
            assert_eq!(scope(&seen.lock().last().unwrap().clone()), bearer_scope);
        }

        #[tokio::test]
        async fn a_minter_removed_from_the_org_since_downloads_under_the_narrower_scope() {
            let Some(db) = scratch().await else { return };
            let (url, seen) = recording_log().await;
            let mut state = crate::test_state(db.pool().clone(), Some(admin_token()), None);
            state.clickhouse = Some(ClickHouseClient::new(&url));
            let app = app_with(state);
            let (user, org, viewer) = org_viewer(db.pool()).await;
            let (_, link) = minted(&app, &viewer, "").await;
            // removed from the org between the mint and the download
            sqlx::query("delete from memberships where user_id = $1")
                .bind(user)
                .execute(db.pool())
                .await
                .unwrap();
            let response = send(&app, get(&link, None)).await;
            let _ = text_of(response).await;
            let bound = scope(&seen.lock().last().unwrap().clone());
            assert!(bound.contains(&("param_unrestricted".into(), "0".into())));
            assert!(
                !bound.iter().any(|(_, v)| v.contains(&org.to_string())),
                "the export still reached the org it was removed from: {bound:?}"
            );
        }

        #[tokio::test]
        async fn a_ticket_from_an_api_token_rotated_since_is_refused() {
            let Some(db) = scratch().await else { return };
            let app = pg_app(&db, &admin_token(), TICKET_LIFETIME).await;
            let (token_id, bearer) = api_token(db.pool()).await;
            let (_, url) = minted(&app, &bearer, "").await;
            sqlx::query(
                "update api_tokens set rotated_at = now() + interval '1 second' where id = $1",
            )
            .bind(token_id)
            .execute(db.pool())
            .await
            .unwrap();
            assert_eq!(
                send(&app, get(&url, None)).await.status(),
                StatusCode::UNAUTHORIZED
            );
        }

        // ------------------------------------------- audit log exports (#3545)

        /// A live session for `user`, as its bearer.
        async fn session_for(pool: &sqlx::PgPool, user: Uuid) -> String {
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
            bearer
        }

        /// A signed-in member of a fresh org with `role`, as `(user, org, bearer)`.
        async fn org_member(pool: &sqlx::PgPool, role: &str) -> (Uuid, Uuid, String) {
            let org: Uuid =
                sqlx::query_scalar("insert into orgs (name, slug) values ($1, $1) returning id")
                    .bind(format!("audit-{}", Uuid::new_v4().simple()))
                    .fetch_one(pool)
                    .await
                    .unwrap();
            let user: Uuid =
                sqlx::query_scalar("insert into users (email) values ($1) returning id")
                    .bind(format!("{role}-{}@example.test", Uuid::new_v4().simple()))
                    .fetch_one(pool)
                    .await
                    .unwrap();
            sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, $3)")
                .bind(user)
                .bind(org)
                .bind(role)
                .execute(pool)
                .await
                .unwrap();
            (user, org, session_for(pool, user).await)
        }

        /// `count` audit rows on `org` (or org-less), under one action the
        /// exports below filter on, so their own `audit_log.export` rows stay
        /// out of the files they compare.
        async fn audit_rows(pool: &sqlx::PgPool, org: Option<Uuid>, count: i32) {
            sqlx::query(
                "insert into audit_log (org_id, action, target_type) \
                 select $1, 'route.update', 'route' from generate_series(1, $2)",
            )
            .bind(org)
            .bind(count)
            .execute(pool)
            .await
            .unwrap();
        }

        /// The whole control plane, so a bearer audit export can be compared.
        fn full_app(db: &TestSchema, admin: &str) -> Router {
            let mut state = crate::test_state(db.pool().clone(), Some(admin.to_string()), None);
            state.export_tickets = Arc::new(Tickets::with_lifetime(TICKET_LIFETIME));
            crate::build_app_with(state, true)
        }

        /// Mint for `path` and answer `(id, download url)`.
        async fn minted_for(
            app: &Router,
            bearer: &str,
            path: &str,
            query: &str,
        ) -> (String, String) {
            let response = mint_for(app, bearer, path, query).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}?{query}");
            let body = json_of(response).await;
            (
                body["id"].as_str().unwrap().to_string(),
                body["download_url"].as_str().unwrap().to_string(),
            )
        }

        #[tokio::test]
        async fn an_audit_log_ticket_downloads_the_bearer_file_once() {
            let Some(db) = scratch().await else { return };
            let app = full_app(&db, &admin_token());
            let (_, org, admin) = org_member(db.pool(), "admin").await;
            audit_rows(db.pool(), Some(org), 3).await;
            let path = format!("/api/v1/orgs/{org}/audit-log/export");
            for (format, extension) in [("csv", "csv"), ("json", "json")] {
                let query = format!("format={format}&action=route.update");
                let bearer = send(&app, get(&format!("{path}?{query}"), Some(&admin))).await;
                assert_eq!(bearer.status(), StatusCode::OK);
                let expected = text_of(bearer).await;

                let (id, url) = minted_for(&app, &admin, &path, &query).await;
                assert_eq!(status_of(&app, &admin, &id).await["state"], "pending");
                let response = send(&app, get(&url, None)).await;
                assert_eq!(response.status(), StatusCode::OK, "{format}");
                let disposition = response.headers()[header::CONTENT_DISPOSITION]
                    .to_str()
                    .unwrap()
                    .to_string();
                assert!(disposition.starts_with("attachment; filename=\"audit-log-audit-"));
                assert!(
                    disposition.ends_with(&format!(".{extension}\"")),
                    "{disposition}"
                );
                assert_eq!(text_of(response).await, expected, "{format}");
                let status = settled(&app, &admin, &id).await;
                assert_eq!(status["state"], "complete", "{status}");
                assert_eq!(status["rows"], 3);
                assert_eq!(status["export"], "org_audit_log");
                // stored with the org it reads, so the redemption reads that org
                let stored: String =
                    sqlx::query_scalar("select export from export_tickets where id = $1")
                        .bind(id.parse::<Uuid>().unwrap())
                        .fetch_one(db.pool())
                        .await
                        .unwrap();
                assert_eq!(stored, format!("org_audit_log:{org}"));
                // a second GET of the same link is refused
                assert_eq!(send(&app, get(&url, None)).await.status(), StatusCode::GONE);
            }
        }

        #[tokio::test]
        async fn the_deployment_audit_log_redeems_for_an_auditor_without_analytics() {
            let Some(db) = scratch().await else { return };
            let app = full_app(&db, &admin_token());
            audit_rows(db.pool(), None, 2).await;
            // a security auditor reads the deployment's audit log and not the
            // request log, so the ticket and its status must not ask for that
            let user: Uuid = sqlx::query_scalar(
                "insert into users (email, deployment_role) \
                 values ($1, 'security_auditor') returning id",
            )
            .bind(format!("auditor-{}@example.test", Uuid::new_v4().simple()))
            .fetch_one(db.pool())
            .await
            .unwrap();
            let auditor = session_for(db.pool(), user).await;
            let query = "format=json&action=route.update";
            let path = "/api/v1/audit-log/export";
            let bearer = send(&app, get(&format!("{path}?{query}"), Some(&auditor))).await;
            assert_eq!(bearer.status(), StatusCode::OK);
            let expected = text_of(bearer).await;
            let (id, url) = minted_for(&app, &auditor, path, query).await;
            let response = send(&app, get(&url, None)).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(text_of(response).await, expected);
            let status = settled(&app, &auditor, &id).await;
            assert_eq!(status["state"], "complete", "{status}");
            assert_eq!(status["export"], "audit_log");
            assert_eq!(send(&app, get(&url, None)).await.status(), StatusCode::GONE);
        }

        #[tokio::test]
        async fn an_audit_log_ticket_is_minted_only_where_the_export_would_run() {
            let Some(db) = scratch().await else { return };
            let app = full_app(&db, &admin_token());
            let (_, org, admin) = org_member(db.pool(), "admin").await;
            let (_, viewer_org, viewer) = org_member(db.pool(), "viewer").await;
            let (_, other, _) = org_member(db.pool(), "admin").await;
            let own = format!("/api/v1/orgs/{org}/audit-log/export");
            // a viewer cannot read the audit log, so cannot mint for it
            let theirs = format!("/api/v1/orgs/{viewer_org}/audit-log/export");
            for (bearer, path, status) in [
                (&viewer, theirs.as_str(), StatusCode::FORBIDDEN),
                // another org's log
                (
                    &admin,
                    &*format!("/api/v1/orgs/{other}/audit-log/export"),
                    StatusCode::FORBIDDEN,
                ),
                // the deployment's log is not an org admin's
                (&admin, "/api/v1/audit-log/export", StatusCode::FORBIDDEN),
            ] {
                let response = mint_for(&app, bearer, path, "format=csv").await;
                assert_eq!(response.status(), status, "{path}");
            }
            // a query the export would refuse is refused at mint
            for query in [
                "format=xml",
                "",
                "format=csv&start_at=2026-02-01T00:00:00Z&end_at=2026-01-01T00:00:00Z",
            ] {
                let response = mint_for(&app, &admin, &own, query).await;
                assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
            }
            // an org id spelled any way but one is no export path
            let unhyphenated = format!("/api/v1/orgs/{}/audit-log/export", org.simple());
            let response = mint_for(&app, &admin, &unhyphenated, "format=csv").await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            // nothing was stored for any of them
            let stored: i64 = sqlx::query_scalar("select count(*) from export_tickets")
                .fetch_one(db.pool())
                .await
                .unwrap();
            assert_eq!(stored, 0);
        }

        #[tokio::test]
        async fn an_audit_log_ticket_checks_the_capability_again_at_redemption() {
            let Some(db) = scratch().await else { return };
            let app = full_app(&db, &admin_token());
            let (user, org, admin) = org_member(db.pool(), "admin").await;
            let path = format!("/api/v1/orgs/{org}/audit-log/export");
            let (id, url) = minted_for(&app, &admin, &path, "format=csv").await;
            // demoted between the mint and the download
            sqlx::query("update memberships set role = 'viewer' where user_id = $1")
                .bind(user)
                .execute(db.pool())
                .await
                .unwrap();
            let response = send(&app, get(&url, None)).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let status = settled(&app, &admin, &id).await;
            assert_eq!(
                (status["state"].clone(), status["status"].clone()),
                (json!("refused"), json!(403))
            );
            // and the refused export recorded nothing as started
            let recorded: i64 = sqlx::query_scalar(
                "select count(*) from audit_log where action = 'audit_log.export' and org_id = $1",
            )
            .bind(org)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(recorded, 0);
        }

        #[tokio::test]
        async fn the_postgres_status_only_moves_forward() {
            let Some(db) = scratch().await else { return };
            let store = PgTickets(db.pool());
            let ticket = new_ticket(Minter::Open);
            let id = ticket.id;
            store.insert(ticket, TICKET_LIFETIME).await.unwrap();
            assert!(store
                .report(id, &TicketState::Streaming { rows: 10 })
                .await
                .unwrap());
            assert!(!store
                .report(id, &TicketState::Streaming { rows: 5 })
                .await
                .unwrap());
            let end = TicketState::Truncated {
                rows: 12,
                reason: TruncatedReason::RowLimit,
            };
            assert!(store.report(id, &end).await.unwrap());
            assert!(!store
                .report(id, &TicketState::Streaming { rows: 20 })
                .await
                .unwrap());
            assert!(!store
                .report(id, &TicketState::Complete { rows: 20 })
                .await
                .unwrap());
            assert_eq!(store.read(id).await.unwrap().unwrap().state, end);
        }
    }
}
