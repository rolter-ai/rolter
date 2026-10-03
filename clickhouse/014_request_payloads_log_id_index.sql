-- A data-skipping index for the `log_id` payload lookup (#2495).
--
-- `request_payloads` is ordered by `(request_id, ts)`, so the detail viewer's
-- lookup by the gateway-minted `log_id` (012_request_log_key.sql) cannot use the
-- primary key and reads every granule in its time window. A bloom filter lets
-- ClickHouse skip the granules that cannot hold the id. `log_id` is a UUID, so
-- the default 0.025 false-positive rate is plenty.
--
-- Only parts written after this migration carry the index; older parts are
-- still scanned until they merge or expire. The table's 7 day ttl bounds that,
-- which is why this does not `materialize index`, a mutation that rewrites
-- every part for rows about to age out anyway.

alter table request_payloads
    add index if not exists idx_request_payloads_log_id log_id type bloom_filter(0.025) granularity 4;
