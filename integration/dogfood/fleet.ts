#!/usr/bin/env bun
// A fake upstream fleet for operator dogfooding (#924).
//
// The built-in `fake-llm` model answers "does the gateway work at all". It does
// not answer "what is it like to run this", because one route with one target
// exercises none of the screens an operator actually lives in: provider groups,
// routing strategies, per-target health, key scoping.
//
// So this stands up a fleet that looks like a real one — three shapes of
// upstream, named the way each of them names things:
//
//   - one OpenAI-shaped endpoint serving OpenAI's model names
//   - one OpenRouter-shaped endpoint serving `vendor/model` names
//   - a spread of single-model vLLM instances, the way a self-hosted fleet
//     actually looks: one process, one model, one port
//
// Every endpoint speaks enough of the OpenAI dialect for rolter to treat it as
// a real provider: `/v1/models`, `/v1/chat/completions` (streaming and not),
// and `/v1/embeddings` where the model is an embedding model.
//
//   bun integration/dogfood/fleet.ts
//
// Latency is per-instance and deliberately uneven — a fleet where every target
// answers in the same time makes every balancing strategy look identical, which
// would make the routing screens impossible to evaluate.

const HOST = "127.0.0.1";

interface Instance {
  /** the port it listens on */
  port: number;
  /** how the operator will label it */
  label: string;
  /** models this endpoint serves */
  models: string[];
  /** required bearer token, or null for an open endpoint */
  apiKey: string | null;
  /** rough time-to-first-token, milliseconds */
  ttftMs: number;
  /** rough per-token time, milliseconds */
  tpotMs: number;
  /** fraction of requests that fail with a 503, to give health screens something to show */
  errorRate?: number;
  /** serves /v1/embeddings instead of chat */
  embedding?: boolean;
}

// keys are fixed rather than random so the printed sheet stays valid across a
// restart of this script — an operator mid-session should not have to re-copy
// every key because the fleet bounced
export const FLEET: Instance[] = [
  {
    port: 18001,
    label: "openai-compatible edge",
    apiKey: "sk-dogfood-openai-4f9c2a7b1e",
    ttftMs: 220,
    tpotMs: 9,
    models: ["gpt-4o", "gpt-4o-mini", "gpt-4.1", "gpt-4.1-mini", "o3-mini"],
  },
  {
    port: 18002,
    label: "openrouter-compatible edge",
    apiKey: "sk-or-v1-dogfood-83bd41e6c0",
    ttftMs: 340,
    tpotMs: 14,
    models: [
      "anthropic/claude-sonnet-4",
      "meta-llama/llama-3.3-70b-instruct",
      "google/gemini-2.0-flash-001",
      "deepseek/deepseek-r1",
      "qwen/qwen-2.5-72b-instruct",
    ],
  },

  // the self-hosted half. one model per process, like a real vllm deployment.
  // roughly half are behind a key, which is what a mixed fleet looks like when
  // some instances sit on a trusted subnet and some do not
  { port: 18003, label: "vllm-a100-01", apiKey: "vllm-local-7c1d9e", ttftMs: 120, tpotMs: 6, models: ["meta-llama/Llama-3.1-8B-Instruct"] },
  { port: 18004, label: "vllm-a100-02", apiKey: null, ttftMs: 135, tpotMs: 6, models: ["meta-llama/Llama-3.1-8B-Instruct"] },
  { port: 18005, label: "vllm-a100-03", apiKey: null, ttftMs: 480, tpotMs: 21, models: ["meta-llama/Llama-3.1-8B-Instruct"] },
  { port: 18006, label: "vllm-h100-01", apiKey: "vllm-local-2a8f43", ttftMs: 90, tpotMs: 4, models: ["Qwen/Qwen2.5-32B-Instruct"] },
  { port: 18007, label: "vllm-h100-02", apiKey: null, ttftMs: 95, tpotMs: 4, models: ["Qwen/Qwen2.5-32B-Instruct"] },
  { port: 18008, label: "vllm-h100-03", apiKey: "vllm-local-b60c15", ttftMs: 150, tpotMs: 5, models: ["mistralai/Mistral-7B-Instruct-v0.3"] },
  { port: 18009, label: "vllm-l40s-01", apiKey: null, ttftMs: 260, tpotMs: 11, models: ["mistralai/Mistral-7B-Instruct-v0.3"] },
  { port: 18010, label: "vllm-l40s-02", apiKey: "vllm-local-d4e701", ttftMs: 310, tpotMs: 13, models: ["google/gemma-2-27b-it"] },
  { port: 18011, label: "vllm-l40s-03", apiKey: null, ttftMs: 295, tpotMs: 12, models: ["google/gemma-2-27b-it"] },
  // the flaky one. a fleet with no failures makes the health and breaker
  // screens unreadable, because they never leave the green state
  { port: 18012, label: "vllm-spot-01 (flaky)", apiKey: null, ttftMs: 200, tpotMs: 8, errorRate: 0.25, models: ["deepseek-ai/DeepSeek-R1-Distill-Qwen-32B"] },
  { port: 18013, label: "vllm-spot-02 (slow)", apiKey: "vllm-local-9f22ac", ttftMs: 1400, tpotMs: 38, models: ["deepseek-ai/DeepSeek-R1-Distill-Qwen-32B"] },
  { port: 18014, label: "tei-embed-01", apiKey: null, ttftMs: 40, tpotMs: 0, embedding: true, models: ["BAAI/bge-large-en-v1.5"] },
  { port: 18015, label: "tei-embed-02", apiKey: "vllm-local-3ba8d7", ttftMs: 55, tpotMs: 0, embedding: true, models: ["intfloat/e5-mistral-7b-instruct"] },
];

const LOREM =
  "Routing a request is mostly bookkeeping: pick a target, forward the bytes, " +
  "and account for what came back. The interesting part is what happens when a " +
  "target stops answering, because that is when the operator finds out whether " +
  "the gateway was ever really balancing or just round-robining with extra steps.";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Deterministic pseudo-embedding, so repeated calls are comparable. */
function embed(text: string, dims = 1024): number[] {
  const out = new Array<number>(dims);
  let h = 2166136261;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  for (let i = 0; i < dims; i++) {
    h = Math.imul(h ^ (h >>> 15), 2246822519);
    out[i] = ((h >>> 0) / 4294967295) * 2 - 1;
  }
  const norm = Math.hypot(...out.slice(0, 64)) || 1;
  return out.map((v) => v / norm);
}

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

/** OpenAI-shaped error, which is what rolter's adapters expect to parse. */
const oaiError = (message: string, type: string, status: number) =>
  json({ error: { message, type, param: null, code: null } }, status);

function serve(inst: Instance) {
  Bun.serve({
    hostname: HOST,
    port: inst.port,
    idleTimeout: 120,
    async fetch(req) {
      const url = new URL(req.url);

      if (url.pathname === "/health" || url.pathname === "/v1/health") {
        return json({ status: "ok", label: inst.label });
      }

      // auth is checked before /v1/models, the way the real APIs do. that
      // matters for "Test connection": the probe is a model-list call, so an
      // endpoint that served its catalogue unauthenticated would report a
      // wrong key as a healthy provider
      if (inst.apiKey) {
        const auth = req.headers.get("authorization") ?? "";
        if (auth !== `Bearer ${inst.apiKey}`) {
          return oaiError(
            "Incorrect API key provided.",
            "invalid_request_error",
            401,
          );
        }
      }

      if (url.pathname === "/v1/models") {
        return json({
          object: "list",
          data: inst.models.map((id) => ({
            id,
            object: "model",
            created: 1735689600,
            owned_by: inst.label,
          })),
        });
      }

      if (inst.errorRate && Math.random() < inst.errorRate) {
        await sleep(inst.ttftMs);
        return oaiError(
          "The engine is currently overloaded. Please try again.",
          "server_error",
          503,
        );
      }

      const body = req.method === "POST" ? await req.json().catch(() => ({})) : {};
      const model: string = body.model ?? inst.models[0];

      if (url.pathname === "/v1/embeddings") {
        await sleep(inst.ttftMs);
        const input: string[] = Array.isArray(body.input)
          ? body.input
          : [body.input ?? ""];
        return json({
          object: "list",
          model,
          data: input.map((text, i) => ({
            object: "embedding",
            index: i,
            embedding: embed(String(text)),
          })),
          usage: { prompt_tokens: input.length * 8, total_tokens: input.length * 8 },
        });
      }

      if (url.pathname !== "/v1/chat/completions") {
        return oaiError(`Unknown route ${url.pathname}`, "invalid_request_error", 404);
      }

      const words = LOREM.split(" ");
      const promptTokens = JSON.stringify(body.messages ?? []).length >> 2;
      const id = `chatcmpl-${Math.random().toString(36).slice(2, 12)}`;
      const created = Math.floor(Date.now() / 1000);

      if (!body.stream) {
        await sleep(inst.ttftMs + inst.tpotMs * words.length);
        return json({
          id,
          object: "chat.completion",
          created,
          model,
          choices: [
            {
              index: 0,
              message: { role: "assistant", content: `[${inst.label}] ${LOREM}` },
              finish_reason: "stop",
            },
          ],
          usage: {
            prompt_tokens: promptTokens,
            completion_tokens: words.length,
            total_tokens: promptTokens + words.length,
          },
        });
      }

      // SSE. token-at-a-time with the instance's real pacing, so the dashboard's
      // streaming views and rolter's ITL metrics see a plausible shape
      const stream = new ReadableStream({
        async start(controller) {
          const enc = new TextEncoder();
          const send = (o: unknown) =>
            controller.enqueue(enc.encode(`data: ${JSON.stringify(o)}\n\n`));

          await sleep(inst.ttftMs);
          send({
            id,
            object: "chat.completion.chunk",
            created,
            model,
            choices: [{ index: 0, delta: { role: "assistant", content: `[${inst.label}] ` }, finish_reason: null }],
          });

          for (const w of words) {
            await sleep(inst.tpotMs);
            send({
              id,
              object: "chat.completion.chunk",
              created,
              model,
              choices: [{ index: 0, delta: { content: `${w} ` }, finish_reason: null }],
            });
          }

          send({
            id,
            object: "chat.completion.chunk",
            created,
            model,
            choices: [{ index: 0, delta: {}, finish_reason: "stop" }],
            usage: {
              prompt_tokens: promptTokens,
              completion_tokens: words.length,
              total_tokens: promptTokens + words.length,
            },
          });
          controller.enqueue(enc.encode("data: [DONE]\n\n"));
          controller.close();
        },
      });

      return new Response(stream, {
        headers: {
          "content-type": "text/event-stream",
          "cache-control": "no-cache",
          connection: "keep-alive",
        },
      });
    },
  });
}

for (const inst of FLEET) serve(inst);

console.log(`fake fleet up — ${FLEET.length} endpoints on ${HOST}`);
for (const i of FLEET) {
  console.log(
    `  :${i.port}  ${i.label.padEnd(26)} ${i.apiKey ? "key" : "open"}  ${i.models.join(", ")}`,
  );
}
