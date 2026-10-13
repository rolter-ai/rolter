-- #1967: an alert evaluation now commits a `pending` history row and sends the
-- webhook afterwards, outside the transaction that holds the rule's row lock,
-- then settles the row to delivered / failed / skipped. `pending` also marks a
-- delivery in flight on another replica, so two replicas do not send one
-- transition twice.
--
-- The table is read by the control plane only, so no bump_config_version()
-- trigger is needed.
alter table alert_notification_history
    drop constraint if exists alert_notification_history_delivery_status_check;

alter table alert_notification_history
    add constraint alert_notification_history_delivery_status_check
    check (delivery_status in ('pending', 'delivered', 'failed', 'skipped'));
