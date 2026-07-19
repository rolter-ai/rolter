-- Opt-in, control-plane-owned alerting. Rules only evaluate when enabled and
-- a ClickHouse analytics endpoint is configured.
create table if not exists alert_channels (
    id                  uuid primary key default gen_random_uuid(),
    name                text not null unique,
    kind                text not null check (kind = 'webhook'),
    endpoint            text not null,
    enabled             boolean not null default false,
    secret_ciphertext   bytea,
    secret_nonce        bytea,
    created_at          timestamptz not null default now(),
    updated_at          timestamptz not null default now(),
    check ((secret_ciphertext is null) = (secret_nonce is null))
);

create table if not exists alert_rules (
    id                  uuid primary key default gen_random_uuid(),
    name                text not null unique,
    signal              text not null check (signal in ('error_rate', 'p95_latency_ms', 'spend_velocity', 'request_volume', 'provider_health_flaps')),
    threshold           double precision not null check (threshold >= 0),
    window_secs         integer not null check (window_secs between 60 and 86400),
    channel_id          uuid references alert_channels (id) on delete set null,
    enabled             boolean not null default false,
    state               text not null default 'unknown' check (state in ('unknown', 'ok', 'firing', 'error')),
    last_value          double precision,
    last_evaluated_at   timestamptz,
    last_error          text,
    created_at          timestamptz not null default now(),
    updated_at          timestamptz not null default now()
);

create table if not exists alert_notification_history (
    id                  uuid primary key default gen_random_uuid(),
    rule_id             uuid not null references alert_rules (id) on delete cascade,
    channel_id          uuid references alert_channels (id) on delete set null,
    state               text not null check (state in ('firing', 'resolved')),
    delivery_status     text not null check (delivery_status in ('delivered', 'failed', 'skipped')),
    detail              text,
    sent_at             timestamptz not null default now()
);

create index if not exists alert_notification_history_rule_sent_idx
    on alert_notification_history (rule_id, sent_at desc);
