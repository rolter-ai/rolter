-- the record of a subject-scoped deletion of request logs and captured payloads
-- (#1085). one row per request to erase, kept after the data is gone: the
-- erasure record cannot be erased by the erasure, so this table is never touched
-- by the deletion it describes, and `audit_log` carries the same event.
--
-- clickhouse applies `alter table … delete` as an asynchronous mutation, so a
-- row starts `pending` and is only marked `completed` once the control plane
-- has seen every mutation it submitted report done. the id doubles as the
-- marker the mutations are found by in `system.mutations`.
--
-- the gateway never reads this table, so it needs no `bump_config_version()`
-- trigger. `scope_id` is text, not a foreign key: it names a key, user or org
-- that may itself be deleted later, and the record has to outlive that.
create table if not exists log_erasures (
    id            uuid primary key default gen_random_uuid(),
    scope_kind    text not null check (scope_kind in ('virtual_key', 'user', 'org', 'request')),
    -- the key, user or org id; for `request` the caller's request id
    scope_id      text not null,
    -- tenancy and exact time of a `request` scope, since a request id is
    -- whatever the caller sent and is not unique
    org_id        uuid,
    request_ts    timestamptz,
    -- false erases captured bodies only and keeps the log metadata
    include_logs  boolean not null default true,
    reason        text not null,
    requested_by  uuid references users (id) on delete set null,
    status        text not null default 'pending'
                  check (status in ('pending', 'completed', 'failed', 'unverified')),
    -- how many clickhouse statements were submitted
    statements    integer not null default 0,
    error         text,
    created_at    timestamptz not null default now(),
    completed_at  timestamptz
);

create index if not exists log_erasures_created_idx on log_erasures (created_at desc);
