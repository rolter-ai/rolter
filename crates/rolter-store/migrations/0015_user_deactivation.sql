-- soft-deactivation for local accounts (ROL-223): a deactivated user keeps its
-- row, memberships and audit trail but can no longer authenticate. login checks
-- this column and existing sessions are deleted at deactivation time, so access
-- is revoked immediately without destroying history.
alter table users add column if not exists deactivated_at timestamptz;
