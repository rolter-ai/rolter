-- a semantic guard provider can be a classifier the gateway runs in process
-- (#3329, ADR-0039): kind 'local' names a weights file on the gateways' file
-- system and its pinned sha256 instead of a url. the table already bumps
-- config_version through guardrail_providers_bump_config_version (0053)
alter table guardrail_providers
    add column kind text not null default 'webhook' check (kind in ('webhook', 'local')),
    add column model_path text,
    add column model_sha256 text;

-- the url check from 0052 refused every row without an http(s) url; it now
-- holds for webhooks only, and a local row carries no url, auth or retries
alter table guardrail_providers drop constraint guardrail_providers_url_check;

alter table guardrail_providers add constraint guardrail_providers_kind_fields check (
    (kind = 'webhook' and url ~ '^https?://' and model_path is null and model_sha256 is null)
    or
    (kind = 'local' and url = '' and auth_kind = 'none' and max_retries = 0
        and model_path is not null and length(trim(model_path)) > 0
        and model_sha256 ~ '^[0-9a-f]{64}$')
);
