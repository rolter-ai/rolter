-- Opt-in control-plane destinations for observability exports. Connectors do
-- not feed the gateway snapshot: delivery is owned by the control plane and
-- deployments remain fully air-gapped until an operator explicitly enables one.
create table if not exists observability_connectors (
    id                      uuid primary key default gen_random_uuid(),
    name                    text not null unique,
    kind                    text not null check (kind = 'otlp_http'),
    endpoint                text not null,
    enabled                 boolean not null default false,
    sampling_rate           double precision not null default 1.0
                                check (sampling_rate >= 0 and sampling_rate <= 1),
    auth_secret_ref         text,
    auth_secret_ciphertext  bytea,
    auth_secret_nonce       bytea,
    health_status           text not null default 'unknown'
                                check (health_status in ('unknown', 'healthy', 'unhealthy')),
    health_checked_at       timestamptz,
    health_error            text,
    created_at              timestamptz not null default now(),
    updated_at              timestamptz not null default now(),
    check ((auth_secret_ciphertext is null) = (auth_secret_nonce is null))
);

create index if not exists observability_connectors_enabled_idx
    on observability_connectors (enabled) where enabled;
