-- scoped semantic guardrail policies (#459): one policy per scope picks the
-- guardrail providers that judge a request and the thresholds their scored
-- categories are held to. scope precedence (key > route > project > org >
-- deployment) is resolved by the gateway; the store only keeps one enabled
-- policy per scope so the resolution is never ambiguous
create table guardrail_policies (
    id              uuid primary key default gen_random_uuid(),
    name            text not null unique check (length(trim(name)) > 0),
    enabled         boolean not null default true,
    scope_kind      text not null default 'deployment'
                    check (scope_kind in ('deployment', 'org', 'project', 'route', 'key')),
    -- an org / project / virtual key id, or a route's public model name
    scope_id        text,
    -- category -> threshold in [0, 1]; validated by the control plane
    thresholds      jsonb not null default '{}'::jsonb check (jsonb_typeof(thresholds) = 'object'),
    on_violation    text not null default 'block' check (on_violation in ('block', 'annotate')),
    streaming       text not null default 'reject' check (streaming in ('reject', 'buffer')),
    description     text,
    created_at      timestamptz not null default now(),
    updated_at      timestamptz not null default now(),
    check (
        (scope_kind = 'deployment' and scope_id is null)
        or
        (scope_kind <> 'deployment' and scope_id is not null and length(trim(scope_id)) > 0)
    )
);

create unique index guardrail_policies_one_enabled_per_scope
    on guardrail_policies (scope_kind, coalesce(scope_id, ''))
    where enabled;

-- the providers a policy consults, in order. a provider a policy lists cannot
-- be deleted out from under it: the policy would silently stop checking
create table guardrail_policy_providers (
    policy_id       uuid not null references guardrail_policies (id) on delete cascade,
    provider_id     uuid not null references guardrail_providers (id) on delete restrict,
    position        integer not null check (position >= 0),
    primary key (policy_id, provider_id)
);

create index guardrail_policy_providers_provider on guardrail_policy_providers (provider_id);
