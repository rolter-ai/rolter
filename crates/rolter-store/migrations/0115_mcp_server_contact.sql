-- What the control plane last learned by talking to an MCP server (#951): when
-- it last answered, what it negotiated, and the time and reason of the last
-- failure. The last failure is kept after a later success on purpose, since
-- "when did this last break and why" is the question a status badge cannot
-- answer from the current state alone.
alter table mcp_servers
    add column if not exists last_contact_at timestamptz,
    add column if not exists last_protocol_version text,
    add column if not exists last_server_name text,
    add column if not exists last_error_kind text
        check (last_error_kind is null or last_error_kind in (
            'unreachable', 'handshake_failed', 'auth_rejected', 'egress_denied'
        )),
    add column if not exists last_error_at timestamptz;

-- Wake snapshot polling only when a column the gateway reads changes.
--
-- 0051 bumped `config_version` on every update of `mcp_servers`. That was right
-- while every column was configuration, but discovery (0112) and contact
-- bookkeeping (above) now write this table from the control plane's own
-- background work, and a status probe clicked in the dashboard is not a policy
-- change: waking every gateway to refetch an identical snapshot for each one is
-- cost with nothing to show for it. The list below is exactly what
-- `load_mcp_servers` selects, plus `enabled` and `org_id`, which decide which
-- rows are served and to whom; the OAuth session policy that joins this table
-- reads `required_scopes`, which is in it. `after update of` fires when a
-- column is *assigned*, not only when it changes, so an edit that rewrites the
-- manifest still bumps once, as before.
drop trigger if exists mcp_servers_bump_config_version on mcp_servers;
create trigger mcp_servers_bump_config_version
after insert or delete on mcp_servers
for each statement execute function bump_config_version();

drop trigger if exists mcp_servers_policy_bump_config_version on mcp_servers;
create trigger mcp_servers_policy_bump_config_version
after update of org_id, slug, url, transport, enabled, tools, required_scopes,
    auth_kind, auth_header_name, credential_ciphertext, credential_nonce,
    connect_timeout_ms, request_timeout_ms, max_retries
on mcp_servers
for each statement execute function bump_config_version();
