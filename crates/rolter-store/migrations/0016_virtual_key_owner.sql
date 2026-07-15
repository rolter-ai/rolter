-- ownership for virtual keys (ROL-224): stamp the local account that minted a
-- key so the self-service panel can show/rotate "my own" keys. nullable because
-- admin-created and bootstrap-config keys have no owning end-user; `on delete
-- set null` keeps a key alive (still project-scoped) when its creator is removed.
alter table virtual_keys
    add column if not exists created_by uuid references users (id) on delete set null;

create index if not exists virtual_keys_created_by_idx
    on virtual_keys (created_by);
