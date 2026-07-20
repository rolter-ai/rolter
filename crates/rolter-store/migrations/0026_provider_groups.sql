-- provider groups: address a fleet of same-kind providers as `group-slug/model`
-- (ADR-0017 addendum, ADR-0022). a group is org-scoped and its slug shares the
-- provider slug namespace (unique per org, same charset as providers.slug from
-- migration 0014). a request that resolves to a group fans out across its
-- members with the group's balancing strategy.

create table if not exists provider_groups (
    id          uuid primary key default gen_random_uuid(),
    org_id      uuid not null references orgs (id) on delete cascade,
    name        text not null,
    slug        text not null,
    strategy    text not null default 'round_robin'
                check (strategy in (
                    'round_robin', 'random', 'power_of_two', 'consistent_hash',
                    'cache_aware', 'weighted', 'pipeline', 'cheapest', 'fastest',
                    'precise_cache_aware', 'lmcache_aware'
                )),
    created_at  timestamptz not null default now(),
    constraint provider_groups_org_slug_unique unique (org_id, slug),
    constraint provider_groups_slug_charset check (slug ~ '^[a-z0-9][a-z0-9-]{0,62}$')
);

-- members of a group. `upstream_model` rewrites the forwarded model for this
-- member; null forwards the requested model as-is (passthrough). a provider may
-- belong to several groups (many-to-many). `position` orders the members
-- deterministically for stable fan-out.
create table if not exists provider_group_members (
    group_id        uuid not null references provider_groups (id) on delete cascade,
    provider_id     uuid not null references providers (id) on delete restrict,
    upstream_model  text,
    weight          integer not null default 1 check (weight > 0),
    position        integer not null default 0,
    primary key (group_id, provider_id)
);
