-- Global security policy is a singleton because it governs the shared gateway
-- ingress. Dashboard credentials are write-only AES-GCM material; snapshots
-- receive policy only and never these ciphertext columns.
create table if not exists security_settings (
    id                              boolean primary key default true check (id),
    virtual_key_required            boolean not null default false,
    allow_direct_provider_keys      boolean not null default false,
    allowed_origins                 text[] not null default '{}',
    allowed_headers                 text[] not null default '{}',
    required_headers                jsonb not null default '{}'::jsonb,
    auth_bypass_routes              text[] not null default '{}',
    dashboard_auth_enabled          boolean not null default false,
    dashboard_credential_ref        text,
    dashboard_credential_ciphertext bytea,
    dashboard_credential_nonce      bytea,
    updated_at                      timestamptz not null default now(),
    check ((dashboard_credential_ciphertext is null) = (dashboard_credential_nonce is null))
);

insert into security_settings (id) values (true) on conflict (id) do nothing;

drop trigger if exists security_settings_bump_config_version on security_settings;
create trigger security_settings_bump_config_version
after insert or update or delete on security_settings
for each statement execute function bump_config_version();
