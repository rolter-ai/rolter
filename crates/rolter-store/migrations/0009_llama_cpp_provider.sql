-- add the first-class self-hosted llama.cpp provider kind
alter table providers drop constraint providers_kind_check;
alter table providers add constraint providers_kind_check
    check (kind in ('openai', 'anthropic', 'openai_compatible', 'ollama', 'llama_cpp'));
