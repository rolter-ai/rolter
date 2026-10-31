-- the circuit-breaker state each gateway node reports for each (model, target)
-- it has a non-closed or failing breaker for (#2254).
--
-- Every gateway replica keeps its own breakers in memory, so the control plane
-- can only show the fleet by being told. This is a scoreboard, not history: one
-- row per (node, model, target index), replaced as a whole by each node's
-- report, so a target whose breaker has closed again simply drops out. An
-- absent row therefore means "closed with no failures" for a node that is still
-- reporting, which is exactly what the gateway's own registry means by it.
--
-- Deliberately NO bump_config_version() trigger. The data plane never reads
-- this table — it writes it — and a version bump on every report would make
-- each sample look like a config change and wake every gateway in the fleet.
create table if not exists breaker_state (
    node_id              text not null check (length(trim(node_id)) between 1 and 128),
    model                text not null check (length(trim(model)) between 1 and 512),
    -- position in the route's target list on that node, which can lag the
    -- stored route until the node converges on its config version
    target_index         integer not null check (target_index >= 0),
    -- which upstream the target forwards to, so a reader can name it
    provider             text not null default '',
    upstream_model       text not null default '',
    state                text not null check (state in ('closed', 'open', 'half_open')),
    consecutive_failures integer not null default 0,
    -- seconds until an open breaker admits its half-open probe; null otherwise
    retry_after_secs     integer,
    reported_at          timestamptz not null default now(),
    primary key (node_id, model, target_index)
);

create index if not exists breaker_state_reported_idx
    on breaker_state (reported_at desc);
