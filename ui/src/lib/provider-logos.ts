// which logo a provider kind wears in the pickers (#3059).
//
// the marks come from `simple-icons` (CC0-1.0), imported one by one so the
// bundle carries only the ones named here, never the 3000-icon set and never a
// request to a CDN or to the vendor's site: the dashboard runs air-gapped.
// A kind that has no mark in the set (`openai` itself was withdrawn from it,
// and most of the smaller vendors were never in it) is not listed, and the
// picker draws a monogram for it instead. Several kinds can share one mark:
// the three gemini dialects are all Google's.
//
// adding a provider kind: add its row here when the set has a mark for it, and
// leave it out when it does not; `provider-logos.test.ts` pins that every kind
// in `PROVIDER_KINDS` either has a mark or is named in `MONOGRAM_KINDS`, so the
// choice is made on purpose rather than by forgetting.

import {
  siAnthropic,
  siBaidu,
  siBytedance,
  siDatabricks,
  siDeepseek,
  siGooglecloud,
  siGooglegemini,
  siHuggingface,
  siKimi,
  siMeta,
  siMinimax,
  siMistralai,
  siNaver,
  siOllama,
  siOpenrouter,
  siOvh,
  siPerplexity,
  siQwen,
  siRakuten,
  siScaleway,
  siTencenthy,
  siYandexcloud,
  type SimpleIcon,
} from "simple-icons";

export const PROVIDER_LOGOS: Readonly<Record<string, SimpleIcon>> = {
  anthropic: siAnthropic,
  ollama: siOllama,
  ollama_cloud: siOllama,
  openrouter: siOpenrouter,
  // text-embeddings-inference is Hugging Face's server
  tei: siHuggingface,
  vertex: siGooglecloud,
  gemini: siGooglegemini,
  gemini_native: siGooglegemini,
  gemini_interactions: siGooglegemini,
  mistral: siMistralai,
  meta_llama_api: siMeta,
  perplexity: siPerplexity,
  databricks: siDatabricks,
  ovhcloud: siOvh,
  scaleway: siScaleway,
  deepseek: siDeepseek,
  qwen: siQwen,
  kimi: siKimi,
  ernie: siBaidu,
  doubao: siBytedance,
  hunyuan: siTencenthy,
  minimax: siMinimax,
  yandex_gpt: siYandexcloud,
  naver: siNaver,
  rakuten: siRakuten,
};

/** the logo's SVG path for a kind, or `undefined` where the picker draws a monogram */
export function providerLogoPath(kind: string): string | undefined {
  return PROVIDER_LOGOS[kind]?.path;
}
