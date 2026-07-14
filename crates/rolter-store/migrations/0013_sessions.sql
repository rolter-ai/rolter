-- local-account sessions (ROL-32). opaque bearer tokens, hashed the same way
-- as virtual keys (`rolter_auth::hash_key`), so the plaintext token never
-- touches the database. server-side storage gives real revocation on logout,
-- which a pure stateless jwt would need a blocklist to match -- and this
-- deployment already runs postgres for everything else auth-adjacent
-- (users, memberships, virtual keys), so a table is the path of least
-- surprise rather than introducing redis as a hard dependency for login.
create table if not exists sessions (
    id            uuid primary key default gen_random_uuid(),
    user_id       uuid not null references users (id) on delete cascade,
    token_hash    text not null unique,
    created_at    timestamptz not null default now(),
    expires_at    timestamptz not null,
    last_seen_at  timestamptz not null default now()
);

create index if not exists idx_sessions_user on sessions (user_id);
create index if not exists idx_sessions_expires on sessions (expires_at);
