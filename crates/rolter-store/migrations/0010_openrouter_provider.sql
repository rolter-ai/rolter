-- add the first-class openrouter provider kind
alter table providers drop constraint providers_kind_check;
alter table providers add constraint providers_kind_check
    check (kind in ('openai', 'anthropic', 'openai_compatible', 'ollama', 'ollama_cloud', 'llama_cpp', 'openrouter'));
