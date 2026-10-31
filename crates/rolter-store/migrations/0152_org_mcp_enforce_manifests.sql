-- A newly created organization enforces its MCP tool manifests (#3334).
--
-- Until now an organization that never saved its MCP settings allowed tools a
-- server's manifest leaves out (`allow_unlisted_tools` true in the proxy), and
-- 0137 set every saved row the same way, so that an upgrade refused no call.
-- A new organization has no tools that could break, so it starts strict; an
-- admin opts out by saving MCP settings with Allow unlisted tools on.
--
-- The marker lives on `orgs` rather than in `mcp_gateway_settings` on purpose:
-- creating that row at org creation would freeze its timeouts and retries at
-- the values of the day, and an organization with no row follows later changes
-- to the deployment defaults. A saved row still decides for itself.
--
-- Rows that exist when the column is added take the default of the moment,
-- false, so every existing organization keeps today's behaviour; the default is
-- then switched to true for the organizations created from here on.

alter table orgs add column if not exists mcp_enforce_manifests boolean not null default false;
alter table orgs alter column mcp_enforce_manifests set default true;
