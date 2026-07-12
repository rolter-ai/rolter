-- runtime provider credentials: one active key per provider (rotation
-- replaces in place), and key writes must bump config_version so gateways
-- refetch the snapshot like any other config change

create unique index if not exists provider_keys_provider_id_key
    on provider_keys (provider_id);

drop trigger if exists provider_keys_bump_config_version on provider_keys;
create trigger provider_keys_bump_config_version
    after insert or update or delete on provider_keys
    for each statement execute function bump_config_version();
