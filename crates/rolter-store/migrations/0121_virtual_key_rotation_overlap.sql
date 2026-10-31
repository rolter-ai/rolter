-- rotating a shared virtual key without an outage (#1837).
--
-- An admin rotation issues a new secret on the same `virtual_keys` row, so every
-- setting, attribution and history row carries over. The secret it replaces can
-- keep authenticating for an overlap window, so a fleet that rolls the new key
-- out gradually is not broken half-way through the rollout.
--
-- Only the peppered digest of the previous secret is stored, exactly like
-- `key_hash`: nothing here can be turned back into a key. `previous_key_prefix`
-- is the first characters of the old key, kept so the Keys screen can say which
-- secret is still live. All three columns are NULL when no overlap is running,
-- and a second rotation replaces them (one previous secret at most).
alter table virtual_keys
    add column if not exists previous_key_hash text,
    add column if not exists previous_key_prefix text,
    add column if not exists previous_expires_at timestamptz;

-- the pair is all or nothing: a digest with no end would authenticate forever,
-- and an end with no digest would be a date the gateway can do nothing with
alter table virtual_keys
    drop constraint if exists virtual_keys_previous_secret_complete;
alter table virtual_keys
    add constraint virtual_keys_previous_secret_complete
    check ((previous_key_hash is null) = (previous_expires_at is null));

-- a presented key is matched against both digests, so neither may collide with
-- another row's; partial because most rows have no previous secret
create unique index if not exists virtual_keys_previous_key_hash_idx
    on virtual_keys (previous_key_hash)
    where previous_key_hash is not null;

-- no config_version trigger is added here: `virtual_keys` already has one from
-- 0003, and it fires on any insert, update or delete of the table rather than on
-- a column list, so the gateway sees a rotation and an early revoke at the next
-- snapshot
