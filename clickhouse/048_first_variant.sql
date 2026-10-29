-- The weighted variant a request was first dialled to, which the route's split
-- assigned it (#3505). `variant` names the variant that answered, which for a
-- request that failed over to another variant's target is not the one the
-- router picked, so by-variant could not show the share the split gave a
-- flaky variant. Empty on the classic pool, on a request that never dialled,
-- and for every row older than the column.
--
-- A gateway inserts with `input_format_skip_unknown_fields=1` and leaves the
-- field out of a row while it is empty, so one against a ClickHouse without
-- this file keeps logging and loses only the field. The control plane reads
-- the column only when the table has it (`assigned_requests` is `null`
-- otherwise), so applying this file is not a precondition of upgrading it.

alter table request_logs
    add column if not exists first_variant LowCardinality(String) default '';
