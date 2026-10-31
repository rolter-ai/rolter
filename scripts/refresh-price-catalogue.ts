#!/usr/bin/env bun
// The vendored model price catalogue (#966): LiteLLM's
// `model_prices_and_context_window.json` (MIT), cut down to the fields the
// importer reads, so the repository carries a few hundred kilobytes rather than
// the 3 MB original and a deployment never fetches it at runtime.
//
// This is the only place the catalogue is downloaded, and only when a person
// runs the recipe on purpose (`just refresh-prices`). The importer in
// `crates/rolter-control/src/price_catalogue.rs` reads the result through
// `include_str!`, so an air-gapped control plane has every price it can import.
//
//   bun scripts/refresh-price-catalogue.ts                # fetch upstream
//   bun scripts/refresh-price-catalogue.ts --from FILE    # cut down a local copy

import { readFileSync, writeFileSync } from "node:fs";

const SOURCE_URL =
  "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const LICENSE_URL = "https://raw.githubusercontent.com/BerriAI/litellm/main/LICENSE";
const OUT = "crates/rolter-control/catalogue/litellm-prices.json";
const LICENSE_OUT = "crates/rolter-control/catalogue/LICENSE-litellm";

// the request kinds rolter prices by the token; image, audio, video, rerank and
// the rest are billed by units the importer does not read
const MODES = new Set(["chat", "completion", "responses", "embedding"]);
const KEEP = [
  "input_cost_per_token",
  "output_cost_per_token",
  "cache_read_input_token_cost",
  "cache_creation_input_token_cost",
] as const;

const fromIndex = process.argv.indexOf("--from");
const raw =
  fromIndex > 0
    ? readFileSync(process.argv[fromIndex + 1], "utf8")
    : await (await fetch(SOURCE_URL)).text();
const all = JSON.parse(raw) as Record<string, Record<string, unknown>>;

const models: Record<string, Record<string, number>> = {};
for (const id of Object.keys(all).sort()) {
  const entry = all[id];
  if (id === "sample_spec" || typeof entry !== "object" || entry === null) continue;
  if (!MODES.has(String(entry.mode))) continue;
  if (typeof entry.input_cost_per_token !== "number") continue;
  const kept: Record<string, number> = {};
  for (const key of KEEP) {
    const value = entry[key];
    if (typeof value === "number") kept[key] = value;
  }
  models[id] = kept;
}

const meta = {
  source: "litellm",
  url: SOURCE_URL,
  licence: "MIT (see LICENSE-litellm)",
  currency: "USD",
  fetched_at: new Date().toISOString().slice(0, 10),
  note: "prices are per token in USD; LiteLLM's file carries no currency field, so USD is a convention the importer states explicitly",
};
writeFileSync(OUT, JSON.stringify({ _meta: meta, models }) + "\n");
if (fromIndex < 0) {
  writeFileSync(LICENSE_OUT, await (await fetch(LICENSE_URL)).text());
}
console.log(`wrote ${Object.keys(models).length} models to ${OUT}`);
