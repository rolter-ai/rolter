-- weighted variants of a database-defined route (#391): the stored form of
-- `[[routes.variants]]` in rolter.toml. a route with variants samples one by
-- weight per request and falls back through the rest in `position` order;
-- each variant carries its own ordered targets and param defaults.
--
-- normalised rather than a jsonb column on `routes`: a variant target names a
-- provider, and it needs the same `on delete restrict` foreign key a route
-- target has, so a provider cannot be deleted (or narrowed to another project)
-- from under a variant that still sends traffic to it. per-variant attribution
-- needs no join, since request logs carry the variant's name.

create table if not exists route_variants (
    id          uuid primary key default gen_random_uuid(),
    route_id    uuid not null references routes (id) on delete cascade,
    name        text not null check (btrim(name) <> ''),
    -- relative traffic share. the api refuses 0 and writes 1 or more; the
    -- check allows 0 so a rolter.toml variant with weight 0 still imports, and
    -- the gateway samples such a 0 as 1 until #3341 gives it a meaning
    weight      integer not null default 1 check (weight >= 0),
    -- the declared order: the fallback chain after the sampled primary
    position    integer not null check (position >= 0),
    params      jsonb not null default '{}'::jsonb check (jsonb_typeof(params) = 'object'),
    created_at  timestamptz not null default now(),
    unique (route_id, name),
    unique (route_id, position)
);

create table if not exists route_variant_targets (
    id              uuid primary key default gen_random_uuid(),
    variant_id      uuid not null references route_variants (id) on delete cascade,
    provider_id     uuid not null references providers (id) on delete restrict,
    -- the model name sent upstream; null forwards the route's public name
    upstream_model  text,
    weight          integer not null default 1 check (weight >= 0),
    -- the provider routing order inside the variant
    position        integer not null check (position >= 0),
    created_at      timestamptz not null default now(),
    unique (variant_id, position)
);

create index if not exists idx_route_variant_targets_provider
    on route_variant_targets (provider_id);
