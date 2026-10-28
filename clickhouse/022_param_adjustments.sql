-- Which request parameters dialect translation did not forward as sent (#1586).
--
-- One entry per parameter, as `<disposition>:<param>` where the disposition is
-- `dropped` (the upstream dialect has no equivalent), `clamped` (sent limited
-- to the range the upstream accepts) or `translated` (sent as an approximation
-- that does not carry its whole meaning), and the param is its name in the
-- caller's dialect: `dropped:top_k`, `translated:tool_choice`. Names only: a
-- parameter's value never reaches a log row. The same entries are returned to
-- the caller in the `x-rolter-param-adjustments` response header. It is empty
-- for every row older than the column and for a request whose parameters all
-- went through, which is every request on a route that does not translate.
--
-- A gateway inserts with `input_format_skip_unknown_fields=1` and leaves the
-- field out of a row with no adjustments, so one upgraded before this is
-- applied keeps logging and loses only the adjustments. Applying this file is
-- not a precondition of upgrading the gateway; apply it to keep them.

alter table request_logs
    add column if not exists param_adjustments Array(String) default [];
