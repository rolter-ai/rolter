-- Static credentials and per-server transport overrides for MCP servers (#952).
--
-- The registry could express exactly one way to authenticate: the OAuth 2.1
-- authorization-code flow from #707. Most hosted MCP servers today present a
-- long-lived bearer token or an API key in a header instead, and those could
-- not be registered at all.
--
-- Note what is deliberately absent. stdio is not re-added: #783 removed it
-- because a hosted control plane cannot dial a local subprocess, and the MCP
-- specification independently tells stdio servers to take credentials from the
-- environment rather than through an auth flow, so there is nothing here to
-- store for one. mTLS is out too — a client certificate is an identity, not a
-- string, and has no home in this schema yet.

alter table mcp_servers
    -- 'oauth' is the flow #707 already implements and is unchanged by this
    -- migration; the other three are new
    add column if not exists auth_kind text not null default 'none'
        check (auth_kind in ('none', 'bearer', 'header', 'oauth')),
    -- the header an api key is presented in, for auth_kind = 'header' only
    add column if not exists auth_header_name text,
    add column if not exists credential_ciphertext bytea,
    add column if not exists credential_nonce bytea,
    -- per-server overrides of mcp_gateway_settings; null inherits the org default
    add column if not exists connect_timeout_ms integer
        check (connect_timeout_ms between 100 and 60000),
    add column if not exists request_timeout_ms integer
        check (request_timeout_ms between 1000 and 300000),
    add column if not exists max_retries integer
        check (max_retries between 0 and 5);

-- a server registered with an oauth client was already using oauth; leaving it
-- at the 'none' default would silently relabel a configured server as
-- unauthenticated
update mcp_servers
   set auth_kind = 'oauth'
 where client_id is not null
   and auth_kind = 'none';

do $$
begin
    alter table mcp_servers
        add constraint mcp_servers_credential_pair check (
            (credential_ciphertext is null) = (credential_nonce is null)
        );
exception
    when duplicate_object then null;
end
$$;

-- which columns each auth kind is allowed to carry. without this a row could
-- claim 'none' while holding a sealed credential, and the read API's
-- has_credential projection would then contradict what the row says it does
do $$
begin
    alter table mcp_servers
        add constraint mcp_servers_auth_kind_shape check (
            case auth_kind
                when 'bearer' then
                    credential_ciphertext is not null and auth_header_name is null
                when 'header' then
                    credential_ciphertext is not null and auth_header_name is not null
                else
                    credential_ciphertext is null and auth_header_name is null
            end
        );
exception
    when duplicate_object then null;
end
$$;

-- an RFC 9110 field name, and never one that would let header mode overwrite a
-- hop-by-hop header or forge the Authorization the bearer path owns
do $$
begin
    alter table mcp_servers
        add constraint mcp_servers_auth_header_name_shape check (
            auth_header_name is null
            or (auth_header_name ~ '^[A-Za-z0-9!#$%&''*+.^_`|~-]{1,64}$'
                and lower(auth_header_name) not in (
                    'authorization', 'host', 'content-length', 'content-type',
                    'connection', 'transfer-encoding', 'upgrade', 'te', 'trailer',
                    'proxy-authorization'
                ))
        );
exception
    when duplicate_object then null;
end
$$;
