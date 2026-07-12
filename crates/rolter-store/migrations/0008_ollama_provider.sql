-- add the first-class self-hosted ollama provider kind
alter table providers drop constraint if exists providers_kind_check;
alter table providers add constraint providers_kind_check
    check (kind in ('openai', 'anthropic', 'openai_compatible', 'ollama'));
