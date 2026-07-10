-- bump config_version atomically with any write to the tables that feed the
-- gateway snapshot. the bump commits (or rolls back) together with the
-- mutation itself, closing the window where a write landed but the version
-- bump was lost (crash between the two statements) that existed when the
-- control plane bumped in a separate statement.

create or replace function bump_config_version() returns trigger
language plpgsql as $$
begin
    update config_version set version = version + 1, updated_at = now() where id = 1;
    return null;
end;
$$;

drop trigger if exists providers_bump_config_version on providers;
create trigger providers_bump_config_version
    after insert or update or delete on providers
    for each statement execute function bump_config_version();

drop trigger if exists routes_bump_config_version on routes;
create trigger routes_bump_config_version
    after insert or update or delete on routes
    for each statement execute function bump_config_version();

drop trigger if exists route_targets_bump_config_version on route_targets;
create trigger route_targets_bump_config_version
    after insert or update or delete on route_targets
    for each statement execute function bump_config_version();

drop trigger if exists virtual_keys_bump_config_version on virtual_keys;
create trigger virtual_keys_bump_config_version
    after insert or update or delete on virtual_keys
    for each statement execute function bump_config_version();
