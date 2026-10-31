-- A per-process start sequence for ordering requests inside one millisecond
-- (#1392).
--
-- `ts` is a `DateTime64(3)`, so requests that begin inside the same
-- millisecond share a stamp and `ts` alone cannot order them. The column
-- stays as it is and so does the sorting key: `ts` is part of
-- `order by (ts, project_id, model)`, which ClickHouse forbids altering in
-- place, and a table rebuild is a data migration for every deployment with
-- history. This adds a second column instead, so nothing is rewritten.
--
-- Each gateway process stamps `seq` from a monotonic clock when the request
-- starts (nanoseconds since the process began, counted from 1), so within one
-- gateway instance a larger `seq` is a later start. It is meaningful only per
-- instance: two gateways' sequences are unrelated, and a restart begins again
-- near 1. A reader that orders by `(ts, seq)` therefore orders one instance's
-- rows exactly and a mixed fleet's rows deterministically, no more. Rows older
-- than the column, and rows from a gateway that predates it, read `0`.
--
-- A gateway inserts with `input_format_skip_unknown_fields=1`, so one upgraded
-- before this is applied keeps logging and loses only the sequence. The control
-- plane lists requests with `seq` read as `0` while the table lacks the column,
-- so applying this file is not a precondition of upgrading it.

alter table request_logs
    add column if not exists seq UInt64 default 0;
