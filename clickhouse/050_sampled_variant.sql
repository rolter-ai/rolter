-- The weighted variant the route's split sampled as a request's primary
-- (#3547), before the gateway skipped any variant whose targets were parked
-- on a cooldown, unhealthy or behind an open breaker. `first_variant` (048)
-- names the variant first dialled, which while a variant's breaker is open is
-- not the one sampled, so by-variant could not tell a split that is wrong
-- from a gateway routing around a variant. Empty on the classic pool and for
-- every row older than the column.
--
-- A gateway inserts with `input_format_skip_unknown_fields=1` and leaves the
-- field out of a row while it is empty, so one against a ClickHouse without
-- this file keeps logging and loses only the field. The control plane reads
-- the column only when the table has it (`sampled_requests` is `null`
-- otherwise), so applying this file is not a precondition of upgrading it.

alter table request_logs
    add column if not exists sampled_variant LowCardinality(String) default '';
