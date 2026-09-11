-- MCP OAuth client compliance with the current specification (#1347): RFC 9728
-- protected-resource-metadata discovery, RFC 8707 resource indicators and
-- RFC 9207 issuer validation.
--
-- Every column is nullable or carries a default, so a row configured by hand
-- before this migration keeps working exactly as it did: discovery is attempted
-- first and the hand-configured endpoints remain the fallback.

-- what the last successful discovery resolved, plus the issuer an operator may
-- pin by hand for an authorization server that publishes no metadata. the
-- discovered columns are a cache: the interactive authorize refreshes them, and
-- the background refresher and the token exchange read them, so neither has to
-- re-probe an upstream off a user's request.
alter table mcp_servers
    add column if not exists oauth_issuer                   text,
    add column if not exists oauth_discovery                text not null default 'auto',
    add column if not exists oauth_discovered_issuer        text,
    add column if not exists oauth_discovered_authorize_url text,
    add column if not exists oauth_discovered_token_url     text,
    add column if not exists oauth_discovered_iss_supported boolean not null default false,
    add column if not exists oauth_discovered_at            timestamptz;

do $$
begin
    alter table mcp_servers
        add constraint mcp_servers_oauth_discovery_kind
            check (oauth_discovery in ('auto', 'manual'));
exception
    when duplicate_object then null;
end
$$;

-- RFC 9207 §2.4 is decided at the callback, so what it decides against has to
-- travel with the request that started the flow: the issuer of the validated
-- authorization-server metadata and whether that metadata advertised the `iss`
-- parameter. `resource` and `token_url` are recorded for the same reason — the
-- token request must carry the very resource identifier the authorization
-- request carried, and must go to the endpoint whose issuer was validated,
-- rather than to whatever the server row says by the time the browser returns.
--
-- deliberately no bump_config_version() trigger here: the data plane never
-- reads mcp_oauth_login_states, and a trigger would bump the snapshot version
-- on every consent that is started, redeemed or swept.
alter table mcp_oauth_login_states
    add column if not exists expected_issuer text,
    add column if not exists iss_supported   boolean not null default false,
    add column if not exists resource        text,
    add column if not exists token_url       text;
