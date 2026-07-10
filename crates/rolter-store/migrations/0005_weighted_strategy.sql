-- allow the 'weighted' balancing strategy (smooth weighted round-robin honouring
-- route_targets.weight). the original routes.strategy check constraint (migration
-- 0001) predates it, so widen the allowed set.

alter table routes drop constraint if exists routes_strategy_check;
alter table routes
    add constraint routes_strategy_check
    check (strategy in ('round_robin', 'random', 'power_of_two', 'consistent_hash', 'cache_aware', 'weighted'));
