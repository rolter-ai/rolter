-- The MCP proxy now enforces tool groups as an access boundary (#1403), so the
-- gateway reads `mcp_tool_groups`. 0055 deliberately left the table out of
-- config propagation because nothing on the request path read it; with a
-- consumer it needs the same statement trigger as the other tables the
-- snapshot carries, or editing a group would not reach a running gateway until
-- something else bumped the version.

drop trigger if exists mcp_tool_groups_bump_config_version on mcp_tool_groups;
create trigger mcp_tool_groups_bump_config_version
after insert or update or delete on mcp_tool_groups
for each statement execute function bump_config_version();
