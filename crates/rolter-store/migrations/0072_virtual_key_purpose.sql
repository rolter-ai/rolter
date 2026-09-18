-- why a virtual key exists, when it was not minted by hand (#1640).
--
-- The playground mints a key for the signed-in operator so nobody has to paste
-- a long-lived one into the browser. Such a key looks exactly like a hand-made
-- one in the Keys list, which is the problem: it expires in minutes and is
-- replaced on demand, so a reader needs to be told that rather than left to
-- infer it from a short `expires_at`.
--
-- NULL is the default and means "minted by a person, for a reason of their
-- own". A check constraint rather than an enum type: the set is small and a
-- new value should be one migration, not a type alteration that locks the
-- table against every writer.
alter table virtual_keys
    add column if not exists purpose text
        check (purpose is null or purpose in ('playground'));

-- the Keys screen lists a user's own keys newest-first and marks the
-- playground ones; the partial index keeps that filter off a sequential scan
-- without paying for the NULLs, which are most rows
create index if not exists virtual_keys_purpose_idx
    on virtual_keys (purpose)
    where purpose is not null;

-- no config_version trigger is added here: `virtual_keys` already has one from
-- 0003, and it fires on any insert, update or delete of the table rather than
-- on a column list, so it covers this column from the moment it exists
