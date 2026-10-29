-- #3458: the breaker and adaptive-routing scoreboards key a route by its org
-- as well as its name.
--
-- Since #1857 two orgs may each have a route named `gpt-4o` (ADR-0040). Both
-- tables were keyed on the bare model name per node, so one org's adaptive row
-- overwrote the other's, and the breaker view could not tell whose route a row
-- belonged to. A gateway now reports the owning org beside the route name;
-- `org_id` is that org, empty for a route from the gateway's own config file
-- and for a report from a gateway that predates this column.
--
-- Deliberately NO bump_config_version() trigger, as in 0050 and 0108: the
-- data plane writes these tables and never reads them.
alter table breaker_state
    add column if not exists org_id text not null default ''
        check (length(org_id) <= 128);
alter table breaker_state drop constraint if exists breaker_state_pkey;
alter table breaker_state add primary key (node_id, org_id, model, target_index);

alter table adaptive_routing_telemetry
    add column if not exists org_id text not null default ''
        check (length(org_id) <= 128);
alter table adaptive_routing_telemetry drop constraint if exists adaptive_routing_telemetry_pkey;
alter table adaptive_routing_telemetry add primary key (node_id, org_id, model);
