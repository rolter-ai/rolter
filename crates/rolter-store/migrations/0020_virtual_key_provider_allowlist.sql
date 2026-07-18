-- Restrict a virtual key to explicitly approved upstream providers. An empty
-- array retains the existing permissive behavior for backward compatibility.
alter table virtual_keys
    add column if not exists providers text[] not null default '{}';
