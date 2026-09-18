-- #1643: the data plane indexes `config.provider_groups` for `group-slug/model`
-- addressing, so both group tables feed the gateway snapshot — but neither had
-- a `bump_config_version()` trigger. A group created through the dashboard was
-- carried by `/internal/snapshot` and still 404'd at the gateway, because the
-- watcher polls the version and saw no change. The bump has to ride inside the
-- write transaction, the way every other data-plane-visible table does.

drop trigger if exists provider_groups_bump_config_version on provider_groups;
create trigger provider_groups_bump_config_version
    after insert or update or delete on provider_groups
    for each statement execute function bump_config_version();

drop trigger if exists provider_group_members_bump_config_version on provider_group_members;
create trigger provider_group_members_bump_config_version
    after insert or update or delete on provider_group_members
    for each statement execute function bump_config_version();
