-- How much of a request's cache write went to the provider's 1 hour cache
-- (#2891).
--
-- Anthropic bills a write to its 1 hour prompt cache at a different rate from
-- one to its 5 minute cache, and reports the two apart in
-- `usage.cache_creation`. `cache_write_tokens` is the total of both and stays
-- the total; this column is the 1 hour share of it, so the 5 minute share is
-- the difference. It is 0 for a provider that reports no split, which is every
-- provider but Anthropic, and for every row older than the column.
--
-- A gateway inserts with `input_format_skip_unknown_fields=1` and leaves the
-- field out of a row whose share is 0, so one upgraded before this is applied
-- keeps logging, and loses the share only on the requests that wrote to the
-- 1 hour cache. The control plane returns the column on the invocation list when
-- the table has it and reads 0 when it does not (#2903), so applying this file
-- is not a precondition of upgrading it; apply it to keep the share.

alter table request_logs
    add column if not exists cache_write_1h_tokens UInt32 default 0;
