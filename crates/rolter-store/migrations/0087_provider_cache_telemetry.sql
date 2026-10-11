-- #2236: per-provider cache telemetry for the cache-aware strategies.
--
-- `precise_cache_aware` reads a provider's vLLM kv-event stream and
-- `lmcache_aware` polls its LMCache controller, but only a provider written in
-- rolter.toml could name either source: the store loaded every database
-- provider with neither, so a provider added in the dashboard never got the
-- telemetry. Both settings are stored as the same json object the file config
-- carries (`kv_events`: endpoint, topic, max_blocks, stale_secs; `lmcache`:
-- endpoint, refresh_secs, stale_secs). null means the provider has no such
-- source, which is every existing row.
--
-- the table already carries the bump_config_version() statement triggers
-- (0003, 0082), so a write to either column reaches /internal/snapshot with no
-- trigger of its own. the check only keeps a value of the wrong json type out;
-- the control api validates the fields, and the loader degrades a row it
-- cannot read rather than failing the snapshot.
alter table providers
    add column kv_events jsonb,
    add column lmcache jsonb,
    add constraint providers_kv_events_object
        check (kv_events is null or jsonb_typeof(kv_events) = 'object'),
    add constraint providers_lmcache_object
        check (lmcache is null or jsonb_typeof(lmcache) = 'object');
