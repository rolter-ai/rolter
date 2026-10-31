-- Whether the upstream stated its prompt-cache reads on a request, zero
-- included (#3410). `cache_read_tokens = 0` alone cannot tell a provider that
-- never hits its cache from one that does not report it, so the dashboard's
-- read share is computed over the prompt tokens of the requests that set
-- this. The gateway leaves the field out of a row while it is 0, and writes
-- with `input_format_skip_unknown_fields=1`, so a gateway against a
-- ClickHouse without this column keeps writing. Rows older than the column
-- read 0: not measured.

alter table request_logs
    add column if not exists cache_usage_reported UInt8 default 0;
