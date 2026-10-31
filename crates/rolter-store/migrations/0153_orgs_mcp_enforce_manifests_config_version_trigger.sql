-- The gateway reads `orgs.mcp_enforce_manifests` through the snapshot (#3334).
-- Nothing in the API changes it after the organization is created, but a
-- change made any other way (an operator's SQL, a later tool) has to reach a
-- running gateway, so it bumps `config_version` like every other column the
-- snapshot carries. Scoped to that column so renaming an organization stays
-- quiet.

drop trigger if exists orgs_mcp_enforce_manifests_bump_config_version on orgs;
create trigger orgs_mcp_enforce_manifests_bump_config_version
after update of mcp_enforce_manifests on orgs
for each statement execute function bump_config_version();
