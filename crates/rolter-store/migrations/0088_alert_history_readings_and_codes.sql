-- the reading an alert transition was written for, and stable codes beside the
-- free-text delivery detail and rule error (#2301, #2334). older rows have
-- none of these, so every column is nullable. neither table is read by the
-- data plane, so no bump_config_version() trigger is needed
alter table alert_notification_history
    add column if not exists detail_code   text,
    add column if not exists detail_params jsonb,
    add column if not exists value         double precision,
    add column if not exists threshold     double precision;

alter table alert_rules
    add column if not exists last_error_code   text,
    add column if not exists last_error_params jsonb;

-- keyset paging over the whole history (#2333): the newest-first order is
-- (sent_at, id), so an unfiltered page needs this rather than the per-rule index
create index if not exists alert_notification_history_sent_idx
    on alert_notification_history (sent_at desc, id desc);
