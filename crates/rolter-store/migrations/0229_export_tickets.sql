-- #3504: single-use download tickets for the analytics CSV exports.
--
-- Every dashboard call authenticates with a bearer header, and a link the
-- browser's download manager follows cannot carry one. A ticket stands in for
-- the header on exactly one download: the dashboard mints it with its bearer,
-- names one of the three analytics exports and the query it would send, and
-- then follows `/api/v1/analytics/exports/download/<secret>`. See
-- docs/dev-docs/adr/2026-10-29-export-download-tickets.md.
--
-- * `secret_hash` is the SHA-256 of the secret, which is returned once and
--   never stored. The bearer the ticket was minted with is not stored either:
--   the minter is kept as what can be checked again at redemption, a session
--   or API token row id, or a digest of the admin token.
-- * A ticket lives 60 seconds and is consumed by one statement
--   (`used_at = now() ... where used_at is null and expires_at > now()`), so
--   two replicas racing on one secret cannot both redeem it.
-- * `status` is the progress the replica serving the download writes, read
--   through the non-secret `id` by the minting principal (`owner`) only.
--
-- Read only by the control plane, so it carries no `bump_config_version()`
-- trigger. Rows are purged opportunistically a day after they expire; the
-- foreign keys only make a deleted session or token take its tickets with it.

create table if not exists export_tickets (
    id uuid primary key,
    secret_hash text not null unique,
    -- `open`, `admin`, `session` or `api_token`
    minter_kind text not null
        check (minter_kind in ('open', 'admin', 'session', 'api_token')),
    session_id uuid references sessions (id) on delete cascade,
    api_token_id uuid references api_tokens (id) on delete cascade,
    -- digest of the admin token a `admin` ticket was minted with
    admin_digest text,
    -- who may read the status: `user:<id>`, `token:<id>` or `admin`
    owner text not null,
    -- `attribution`, `timeseries` or `invocations`
    export text not null,
    query text not null,
    status jsonb not null default '{"state": "pending"}',
    created_at timestamptz not null default now(),
    expires_at timestamptz not null,
    used_at timestamptz,
    updated_at timestamptz not null default now(),
    constraint export_tickets_minter_ids check (
        (minter_kind = 'session') = (session_id is not null)
        and (minter_kind = 'api_token') = (api_token_id is not null)
        and (minter_kind = 'admin') = (admin_digest is not null)
    )
);

create index if not exists export_tickets_expires_at_idx on export_tickets (expires_at);
