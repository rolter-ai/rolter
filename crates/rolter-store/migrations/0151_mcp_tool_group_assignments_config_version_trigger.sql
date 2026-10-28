-- The gateway reads `mcp_tool_group_assignments` through the snapshot (#3295),
-- so an edit has to bump `config_version` or a running gateway keeps resolving
-- callers against the old assignments. Same statement trigger as 0113.

drop trigger if exists mcp_tool_group_assignments_bump_config_version
    on mcp_tool_group_assignments;
create trigger mcp_tool_group_assignments_bump_config_version
after insert or update or delete on mcp_tool_group_assignments
for each statement execute function bump_config_version();
