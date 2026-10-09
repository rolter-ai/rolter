-- #2891: a separate rate for input tokens written to the provider's 1 hour
-- prompt cache.
--
-- anthropic bills a 5 minute cache write at 1.25 times the input rate and a 1
-- hour one at 2 times, and reports the two apart in `usage.cache_creation`.
-- `cache_write_per_mtok` (0083) is one rate, so a deployment that uses both
-- caches priced every write at it and could not be exact. null falls back to
-- `cache_write_per_mtok` (and from there to the input rate), so every existing
-- row charges exactly what it did before this migration.
--
-- no new trigger: `model_prices_bump_config_version` (0004) already fires on
-- every insert, update and delete of the table, which is how a write to this
-- column reaches /internal/snapshot and the gateway's next reload.
--
-- held to the same rule as the four rates 0084 checks: at least zero and not
-- NaN, because a negative rate would turn a request into a credit against a
-- budget and `numeric` sorts NaN above every number, so `>= 0` alone lets it
-- through. the API refuses both first; this keeps a row written some other way
-- from reaching the snapshot. the column is new, so every row is null (a check
-- passes on null) and the constraint can be validated as it is added, without
-- 0084's `not valid` step.

alter table model_prices
    add column if not exists cache_write_1h_per_mtok numeric(12, 6),
    add constraint model_prices_cache_write_1h_per_mtok_rate
        check (cache_write_1h_per_mtok >= 0 and cache_write_1h_per_mtok <> 'NaN'::numeric);
