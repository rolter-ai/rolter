-- Record what `tools/list` returned from an MCP server (#1283).
--
-- `tools` stays the manifest tool groups validate against: discovery writes
-- into it, and the curated catalog's static list is only the fallback a server
-- carries until discovery has run. A tool a later discovery no longer returns
-- is kept in `tools` and named in `tools_stale`, never pruned, so a tool group
-- that references it keeps validating and the dashboard can flag it.
--
-- `tools_discovery_status` is what lets a card tell "not discovered yet"
-- (`pending`) from "declares no tools" (`discovered` with an empty `tools`) and
-- from "could not be asked" (`failed`). `tool_schemas` holds the descriptions
-- and input schemas discovery saw; it is read through its own endpoint rather
-- than carried on every server listing.
--
-- No trigger change: `mcp_servers` already bumps `config_version` on update
-- (0051), and the gateway reads `tools` for the declared-tool check.

alter table mcp_servers
    add column if not exists tool_schemas jsonb not null default '[]'::jsonb
        check (jsonb_typeof(tool_schemas) = 'array'),
    add column if not exists tools_stale text[] not null default '{}',
    add column if not exists tools_discovery_status text not null default 'pending'
        check (tools_discovery_status in ('pending', 'discovered', 'failed')),
    add column if not exists tools_discovery_error text,
    add column if not exists tools_discovered_at timestamptz,
    add column if not exists tools_discovery_attempted_at timestamptz;
