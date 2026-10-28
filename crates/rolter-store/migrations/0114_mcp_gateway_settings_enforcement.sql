-- The MCP proxy now reads an organization's saved MCP settings (#1404), so
-- `mcp_gateway_settings` joins the tables the snapshot carries. 0055 left it out
-- of config propagation while nothing on the request path read it; with a
-- consumer, saving the settings has to wake snapshot polling or a running
-- gateway would keep applying the previous values.

drop trigger if exists mcp_gateway_settings_bump_config_version on mcp_gateway_settings;
create trigger mcp_gateway_settings_bump_config_version
after insert or update or delete on mcp_gateway_settings
for each statement execute function bump_config_version();
