-- The virtual key on a captured payload row, so one subject's bodies can be
-- erased without joining back to request_logs (#1085).
--
-- `request_payloads` carries `org_id` and `project_id` but not the key, so a
-- deletion by virtual key (or by the user who owns the keys) had to find the
-- bodies through their log rows. A ClickHouse mutation's subquery is evaluated
-- when the mutation runs, not when it is submitted, which races the deletion of
-- those same log rows. With the key on the payload row the predicate is the
-- row's own.
--
-- Defaults to the empty string, which is what every row written before this
-- migration holds. The erasure endpoint matches those through their log row's
-- `log_id` when it is submitted.

alter table request_payloads
    add column if not exists virtual_key_id String default '';
