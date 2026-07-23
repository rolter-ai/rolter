-- broaden provider kind coverage for hosted adapter expansion
alter table providers drop constraint if exists providers_kind_check;
alter table providers add constraint providers_kind_check
    check (kind in (
        'openai', 'anthropic', 'openai_compatible', 'ollama', 'ollama_cloud',
        'llama_cpp', 'openrouter', 'tei', 'azure_openai', 'bedrock', 'vertex',
        'gemini', 'gemini_native', 'mistral', 'groq', 'xai', 'meta_llama_api',
        'cohere', 'perplexity', 'together', 'fireworks', 'databricks',
        'aleph_alpha', 'nebius', 'ovhcloud', 'scaleway', 'deepseek', 'qwen',
        'zhipu', 'kimi', 'ernie', 'doubao', 'hunyuan', 'yi', 'minimax',
        'baichuan', 'gigachat', 'yandex_gpt', 'cloud_ru', 'mts_ai', 'naver',
        'upstage', 'rinna', 'rakuten', 'sarvam', 'krutrim', 'falcon'
    ));
