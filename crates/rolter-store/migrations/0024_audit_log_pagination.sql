-- Stable audit-log keyset pagination and the filter paths used by the control
-- plane. `id` breaks timestamp ties so concurrent inserts cannot reshuffle an
-- already-issued cursor window.
create index if not exists audit_log_org_at_id_idx
    on audit_log (org_id, at desc, id desc);
create index if not exists audit_log_org_actor_at_id_idx
    on audit_log (org_id, actor_user_id, at desc, id desc);
create index if not exists audit_log_org_action_at_id_idx
    on audit_log (org_id, action, at desc, id desc);
create index if not exists audit_log_org_target_type_at_id_idx
    on audit_log (org_id, target_type, at desc, id desc);
