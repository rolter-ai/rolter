-- scope audit_log entries to an org so the control-plane API can list them
-- per-org; nullable since some actions (superadmin bootstrap, system jobs)
-- have no org context
alter table audit_log add column if not exists org_id uuid references orgs (id) on delete cascade;
create index if not exists audit_log_org_id_at_idx on audit_log (org_id, at desc);
