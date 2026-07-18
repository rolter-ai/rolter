alter table routes drop constraint if exists routes_strategy_check;
alter table routes
    add constraint routes_strategy_check
    check (strategy in (
        'round_robin', 'random', 'power_of_two', 'consistent_hash',
        'cache_aware', 'weighted', 'pipeline', 'cheapest', 'fastest',
        'precise_cache_aware', 'lmcache_aware'
    ));
