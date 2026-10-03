-- one-time codes that hand a browser's sso sign-in to the dashboard (#2297).
-- the callback is a top-level navigation, so it cannot return the session
-- token in a body the dashboard reads, and a token in a redirect query string
-- would land in history, logs and referrers. it redirects with one of these
-- codes instead, and the dashboard redeems it once over POST.
--
-- only the sha-256 of the code is stored, so a read of this table is not a
-- sign-in. the session is minted when the code is redeemed, which means no
-- bearer token ever rests here either. rows are deleted on redemption and
-- swept by expiry. the data plane never reads this table, so there is no
-- bump_config_version() trigger.
create table if not exists sso_exchange_codes (
    code_hash     text primary key,
    user_id       uuid not null references users (id) on delete cascade,
    provider_id   uuid not null references sso_providers (id) on delete cascade,
    granted_roles text[] not null default '{}',
    expires_at    timestamptz not null,
    created_at    timestamptz not null default now()
);

create index if not exists sso_exchange_codes_expires_idx on sso_exchange_codes (expires_at);
