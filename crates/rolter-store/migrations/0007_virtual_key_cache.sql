-- per-virtual-key response-cache override (ROL-235 part 2). nullable tri-state:
-- NULL inherits the route's cache decision, false forces the key's responses to
-- bypass the cache, true caches them even on a route that didn't opt in. the
-- global [cache] switch is still required either way.

alter table virtual_keys
    add column if not exists cache_enabled boolean;
