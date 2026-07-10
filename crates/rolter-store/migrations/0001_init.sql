-- rolter initial schema (postgres)
--
-- source of truth for tenancy, rbac, providers, routes, virtual keys, pricing
-- and budgets. gen_random_uuid() is built in on postgres 13+.
-- this file is intentionally a single bootstrap migration; a migration tool
-- (sqlx/refinery) is wired in during the persistence phase (see TODO.md).

-- --- tenancy: org -> team -> project ---

create table if not exists orgs (
    id          uuid primary key default gen_random_uuid(),
    name        text not null,
    slug        text not null unique,
    created_at  timestamptz not null default now()
);

create table if not exists teams (
    id          uuid primary key default gen_random_uuid(),
    org_id      uuid not null references orgs (id) on delete cascade,
    name        text not null,
    created_at  timestamptz not null default now(),
    unique (org_id, name)
);

create table if not exists projects (
    id          uuid primary key default gen_random_uuid(),
    team_id     uuid not null references teams (id) on delete cascade,
    name        text not null,
    created_at  timestamptz not null default now(),
    unique (team_id, name)
);

-- --- identity + rbac ---

create table if not exists users (
    id              uuid primary key default gen_random_uuid(),
    email           text not null unique,
    password_hash   text,                  -- argon2id; null for sso-only users
    is_superadmin   boolean not null default false,
    created_at      timestamptz not null default now()
);

-- a membership grants a role at a scope; scope is the most specific non-null id
create table if not exists memberships (
    id          uuid primary key default gen_random_uuid(),
    user_id     uuid not null references users (id) on delete cascade,
    org_id      uuid references orgs (id) on delete cascade,
    team_id     uuid references teams (id) on delete cascade,
    project_id  uuid references projects (id) on delete cascade,
    role        text not null check (role in ('admin', 'member', 'viewer')),
    created_at  timestamptz not null default now()
);

-- --- providers + encrypted upstream keys ---

create table if not exists providers (
    id          uuid primary key default gen_random_uuid(),
    org_id      uuid not null references orgs (id) on delete cascade,
    name        text not null,
    kind        text not null check (kind in ('openai', 'anthropic', 'openai_compatible')),
    api_base    text not null,
    egress_proxy text,
    created_at  timestamptz not null default now(),
    unique (org_id, name)
);

-- envelope-encrypted upstream credentials (aes-256-gcm; kek from env/kms)
create table if not exists provider_keys (
    id          uuid primary key default gen_random_uuid(),
    provider_id uuid not null references providers (id) on delete cascade,
    ciphertext  bytea not null,
    nonce       bytea not null,
    created_at  timestamptz not null default now()
);

-- --- routes + targets ---

create table if not exists routes (
    id          uuid primary key default gen_random_uuid(),
    project_id  uuid not null references projects (id) on delete cascade,
    model       text not null,
    strategy    text not null default 'round_robin'
                check (strategy in ('round_robin', 'random', 'power_of_two', 'consistent_hash', 'cache_aware')),
    enabled     boolean not null default true,
    created_at  timestamptz not null default now(),
    unique (project_id, model)
);

create table if not exists route_targets (
    id              uuid primary key default gen_random_uuid(),
    route_id        uuid not null references routes (id) on delete cascade,
    provider_id     uuid not null references providers (id) on delete restrict,
    upstream_model  text,
    weight          integer not null default 1 check (weight > 0),
    created_at      timestamptz not null default now()
);

-- --- virtual keys ---

create table if not exists virtual_keys (
    id          uuid primary key default gen_random_uuid(),
    project_id  uuid not null references projects (id) on delete cascade,
    key_hash    text not null unique,      -- hash of the presented key
    key_prefix  text not null,             -- short non-secret prefix for display
    name        text,
    models      text[] not null default '{}',  -- empty = all allowed
    disabled    boolean not null default false,
    expires_at  timestamptz,
    created_at  timestamptz not null default now()
);

-- --- budgets + rate limits (attachable at any scope) ---

create table if not exists budgets (
    id          uuid primary key default gen_random_uuid(),
    scope_type  text not null check (scope_type in ('org', 'team', 'project', 'virtual_key')),
    scope_id    uuid not null,
    limit_usd   numeric(12, 4) not null,
    period      text not null default '30d',
    created_at  timestamptz not null default now()
);

create table if not exists rate_limits (
    id          uuid primary key default gen_random_uuid(),
    scope_type  text not null check (scope_type in ('org', 'team', 'project', 'virtual_key')),
    scope_id    uuid not null,
    rpm         integer,
    tpm         integer,
    created_at  timestamptz not null default now()
);

-- --- pricing catalog (usd per million tokens) ---

create table if not exists model_prices (
    id                      uuid primary key default gen_random_uuid(),
    model                   text not null,
    input_per_mtok          numeric(12, 6) not null default 0,
    output_per_mtok         numeric(12, 6) not null default 0,
    cached_input_per_mtok   numeric(12, 6),
    currency                text not null default 'USD',
    created_at              timestamptz not null default now(),
    unique (model)
);

-- --- audit log + config versioning (for reload-free propagation) ---

create table if not exists audit_log (
    id          uuid primary key default gen_random_uuid(),
    actor_user_id uuid references users (id) on delete set null,
    action      text not null,
    target_type text,
    target_id   uuid,
    detail      jsonb,
    at          timestamptz not null default now()
);

create table if not exists config_version (
    id          integer primary key default 1,
    version     bigint not null default 1,
    updated_at  timestamptz not null default now(),
    constraint config_version_singleton check (id = 1)
);

insert into config_version (id, version) values (1, 1)
on conflict (id) do nothing;

create index if not exists idx_memberships_user on memberships (user_id);
create index if not exists idx_routes_project on routes (project_id);
create index if not exists idx_route_targets_route on route_targets (route_id);
create index if not exists idx_virtual_keys_project on virtual_keys (project_id);
