-- per-model inference param defaults + override policy for db-defined routes
-- (ROL-181, the store half of ROL-136). mirrors the config-side
-- `[routes.params]` / `[routes.param_policy]`.
--
-- params      : jsonb object of default inference params injected into the
--               request body (e.g. {"temperature": 0, "max_tokens": 1024}).
-- param_policy : jsonb {mode: "allow"|"deny", allow: [..], deny: [..]} governing
--               whether callers may override each default. empty object = the
--               permissive default (mode "allow", no exceptions).

alter table routes
    add column if not exists params jsonb not null default '{}'::jsonb;
alter table routes
    add column if not exists param_policy jsonb not null default '{}'::jsonb;

-- the strategy check constraint predates the 'weighted' (0005) and 'pipeline'
-- strategies; the 0005 widen dropped 'pipeline'. re-assert the full set so
-- db-defined routes can use every strategy the gateway supports.
alter table routes drop constraint if exists routes_strategy_check;
alter table routes
    add constraint routes_strategy_check
    check (strategy in (
        'round_robin', 'random', 'power_of_two',
        'consistent_hash', 'cache_aware', 'weighted', 'pipeline'
    ));
