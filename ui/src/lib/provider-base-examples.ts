/**
 * The base URL `rolter.example.toml` gives each provider kind, which the
 * provider sheet shows as the base field's placeholder (#3666): a hosted kind
 * gets its operator's real address and a self-hosted one a typical server, in
 * place of one `https://api.example.com/v1` for every kind.
 *
 * Copied rather than read at run time, since the dashboard ships without the
 * file; `ui/scripts/provider-base-examples.test.ts` holds this table to the
 * file's `kind` / `api_base` pairs, the first one for each kind. A kind the file
 * gives no base keeps the placeholder that follows its `/v1` rule.
 */
export const BASE_EXAMPLES: Readonly<Record<string, string>> = {
  openai: "https://api.openai.com",
  anthropic: "https://api.anthropic.com",
  openrouter: "https://openrouter.ai/api/v1",
  openai_compatible: "http://vllm-team:8000",
  azure_openai: "https://RESOURCE.openai.azure.com/openai/v1",
  bedrock: "https://bedrock-runtime.us-east-1.amazonaws.com/openai/v1",
  vertex:
    "https://aiplatform.googleapis.com/v1/projects/PROJECT/locations/global/endpoints/openapi",
  gemini: "https://generativelanguage.googleapis.com/v1beta/openai",
  gemini_native: "https://generativelanguage.googleapis.com/v1beta",
  gemini_interactions: "https://generativelanguage.googleapis.com/v1beta",
  system_one: "https://api.typesafe.ai",
  infinity: "http://infinity:7997",
  mistral: "https://api.mistral.ai/v1",
  groq: "https://api.groq.com/openai/v1",
  xai: "https://api.x.ai/v1",
  gigachat: "https://api.giga.chat/v1",
  ollama_cloud: "https://ollama.com",
  ollama: "http://localhost:11434",
};
