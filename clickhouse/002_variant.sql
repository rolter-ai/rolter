-- add variant attribution to request logs (ROL-186)
--
-- backfills the column on an existing deployment; fresh installs already get it
-- from 001_logs.sql. empty string is the classic single-pool path (no variant).

alter table request_logs
    add column if not exists variant String after target;
