-- local semantic guard providers persist their total scoring budget and the
-- decision for text past it (#3558); guardrail_providers_bump_config_version
-- from 0053 already propagates writes to the gateways
alter table guardrail_providers
    add column max_scored_bytes integer,
    add column on_truncated text not null default 'score'
        check (on_truncated in ('score', 'fail_closed'));

alter table guardrail_providers add constraint guardrail_providers_scored_bytes_check
    check (max_scored_bytes is null or
        (max_scored_bytes >= max_body_bytes and max_scored_bytes <= 16777216));
