-- #2876: a rate for input tokens written to the provider's prompt cache.
--
-- anthropic bills a cache write at a premium over the input rate, and the
-- request log has recorded the count as `cache_write_tokens` for a while, but
-- a price row could only express input, output and cache-hit input, so every
-- written token was charged as ordinary input and write-heavy traffic was
-- under-stated. null keeps today's meaning, the input rate, so every existing
-- row charges exactly what it did before this migration.
--
-- no new trigger: `model_prices_bump_config_version` (0004) already fires on
-- every insert, update and delete of the table, which is how a write to this
-- column reaches /internal/snapshot and the gateway's next reload.
--
-- non-negative, because a negative rate would turn a request into a credit
-- against a budget. the API refuses one first; this keeps a row written some
-- other way from reaching the snapshot.

alter table model_prices
    add column if not exists cache_write_per_mtok numeric(12, 6)
        check (cache_write_per_mtok is null or cache_write_per_mtok >= 0);
