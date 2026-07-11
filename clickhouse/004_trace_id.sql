-- add inbound trace-id to request logs (ROL-60)
--
-- backfills the column on an existing deployment; fresh installs already get it
-- from 001_logs.sql. empty string means the caller propagated no W3C traceparent
-- or B3 trace header, so the request started its own trace.

alter table request_logs
    add column if not exists trace_id String after request_id;
