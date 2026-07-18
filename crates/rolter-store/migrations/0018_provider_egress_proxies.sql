alter table providers
    add column if not exists egress_proxies jsonb not null default '[]'::jsonb;

alter table providers
    add constraint providers_egress_proxies_array
    check (jsonb_typeof(egress_proxies) = 'array');

