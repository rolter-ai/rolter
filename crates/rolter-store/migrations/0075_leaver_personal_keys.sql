-- #1841: a personal virtual key (one with `created_by`) is served only while
-- its creator is active and still holds a role that reaches the key's project.
-- `load_virtual_keys` reads that from `users` and `access_profile_roles`, and
-- neither table had a `bump_config_version()` trigger: deactivating a leaver
-- changed what `/internal/snapshot` would build, but no gateway polled for it
-- until some unrelated write moved the version.
--
-- `update of` keeps the trigger on the two columns the snapshot reads. Every
-- account edit through `UserRepo::update` names `is_superadmin` in its `set`
-- list and so still bumps; account edits are rare, the fleet waking for one is
-- harmless, and a trigger that missed a superadmin flip would not be.

drop trigger if exists users_bump_config_version on users;
create trigger users_bump_config_version
    after update of deactivated_at, is_superadmin on users
    for each statement execute function bump_config_version();

drop trigger if exists access_profile_roles_bump_config_version on access_profile_roles;
create trigger access_profile_roles_bump_config_version
    after insert or update or delete on access_profile_roles
    for each statement execute function bump_config_version();

-- deleting an account must not revive its keys. `created_by` is `on delete set
-- null` (0016), which turns a leaver's personal keys into ownerless ones: the
-- snapshot then serves them as shared keys, without the access-profile policy
-- that followed their owner. Disabling them first keeps the rows (an admin can
-- still see, and deliberately re-enable, them) while they stop authenticating.
-- Before-row, because by the time an after trigger runs the foreign key has
-- already cleared `created_by` and the keys can no longer be told apart.
create or replace function disable_personal_keys_of_deleted_user() returns trigger
language plpgsql as $$
begin
    update virtual_keys set disabled = true
     where created_by = old.id and not disabled;
    return old;
end;
$$;

drop trigger if exists users_disable_personal_keys on users;
create trigger users_disable_personal_keys
    before delete on users
    for each row execute function disable_personal_keys_of_deleted_user();
