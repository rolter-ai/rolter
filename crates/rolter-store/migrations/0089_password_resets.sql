-- Administrator-initiated password resets (#2240).
--
-- A reset is a one-time link, not a password an administrator picks: the
-- member chooses their own, so no administrator ever knows a credential that
-- is not theirs. Only the peppered digest of the token is stored, like
-- invitations, sessions and virtual keys, so a database dump alone yields no
-- usable link. The control plane reads this table; the data plane does not, so
-- no bump_config_version() trigger is needed.

create table if not exists password_resets (
    id          uuid primary key default gen_random_uuid(),
    user_id     uuid not null references users (id) on delete cascade,
    -- peppered sha-256 of the token handed to the administrator; the token
    -- itself is returned once and never stored
    token_hash  text not null unique,
    -- the administrator who issued it; kept null when that account is deleted
    created_by  uuid references users (id) on delete set null,
    expires_at  timestamptz not null,
    used_at     timestamptz,
    revoked_at  timestamptz,
    created_at  timestamptz not null default now()
);

-- one live reset per account: issuing another replaces the first, so an old
-- link stops working the moment a new one exists
create unique index if not exists password_resets_live_user_idx
    on password_resets (user_id)
    where used_at is null and revoked_at is null;
