// "Copy as code" for the Playground (#936).
//
// The Playground proves a route works. The next thing an operator does is wire
// that route into an application, and the useful output of a successful
// Playground call is therefore *the code that reproduces it* — not the reply.
//
// Everything here is a pure function over the request the Playground just sent,
// so the snippets are unit-testable and cannot drift from each other.

/** The request the Playground issued, in OpenAI terms. */
export interface SnippetRequest {
  /** the rolter route name, which is what the caller passes as `model` */
  model: string;
  /** user-visible prompt text; multimodal parts are flattened to their text */
  prompt: string;
  /** whether the caller asked for SSE */
  stream?: boolean;
}

export type SnippetLang = "curl" | "python" | "javascript";

export const SNIPPET_LANGS: SnippetLang[] = ["curl", "python", "javascript"];

/**
 * Base URL to put in a snippet.
 *
 * The dashboard reaches the gateway through its own `/gw` reverse proxy, and
 * that path is a working OpenAI-compatible surface — so the snippet runs as-is
 * from anywhere the dashboard is reachable. It is still the dashboard's port,
 * not the gateway's, which is why every snippet says so in a comment: in
 * production you point a client at the gateway directly.
 */
export function snippetBaseUrl(origin: string): string {
  return `${origin.replace(/\/$/, "")}/gw/v1`;
}

/**
 * The key is referenced through an environment variable, never inlined.
 *
 * The Playground holds a real virtual key in localStorage, and inlining it
 * would mean every snippet an operator pastes into a ticket, a README or a
 * chat leaks a working credential. An env var is also what the provider SDK
 * docs do, so the snippet reads as idiomatic rather than defensive.
 */
const KEY_ENV = "ROLTER_API_KEY";

/** JSON-encode for embedding inside a source literal. */
const j = (v: unknown) => JSON.stringify(v);

function curl(req: SnippetRequest, base: string): string {
  const body = {
    model: req.model,
    messages: [{ role: "user", content: req.prompt }],
    ...(req.stream ? { stream: true } : {}),
  };
  return `# base url is the dashboard's gateway proxy; in production point this at the gateway itself
curl ${base}/chat/completions \\
  -H "Authorization: Bearer $${KEY_ENV}" \\
  -H "Content-Type: application/json" \\
  -d '${JSON.stringify(body)}'`;
}

function python(req: SnippetRequest, base: string): string {
  const call = req.stream
    ? `stream = client.chat.completions.create(
    model=${j(req.model)},
    messages=[{"role": "user", "content": ${j(req.prompt)}}],
    stream=True,
)
for chunk in stream:
    print(chunk.choices[0].delta.content or "", end="")`
    : `response = client.chat.completions.create(
    model=${j(req.model)},
    messages=[{"role": "user", "content": ${j(req.prompt)}}],
)
print(response.choices[0].message.content)`;

  return `# pip install openai
import os
from openai import OpenAI

# base_url is the dashboard's gateway proxy; in production point this at the gateway itself
client = OpenAI(
    base_url=${j(base)},
    api_key=os.environ[${j(KEY_ENV)}],
)

${call}`;
}

function javascript(req: SnippetRequest, base: string): string {
  const call = req.stream
    ? `const stream = await client.chat.completions.create({
  model: ${j(req.model)},
  messages: [{ role: "user", content: ${j(req.prompt)} }],
  stream: true,
});
for await (const chunk of stream) {
  process.stdout.write(chunk.choices[0]?.delta?.content ?? "");
}`
    : `const response = await client.chat.completions.create({
  model: ${j(req.model)},
  messages: [{ role: "user", content: ${j(req.prompt)} }],
});
console.log(response.choices[0].message.content);`;

  return `// npm install openai
import OpenAI from "openai";

// baseURL is the dashboard's gateway proxy; in production point this at the gateway itself
const client = new OpenAI({
  baseURL: ${j(base)},
  apiKey: process.env.${KEY_ENV},
});

${call}`;
}

/** Render one request as runnable client code. */
export function renderSnippet(lang: SnippetLang, req: SnippetRequest, origin: string): string {
  const base = snippetBaseUrl(origin);
  switch (lang) {
    case "curl":
      return curl(req, base);
    case "python":
      return python(req, base);
    case "javascript":
      return javascript(req, base);
  }
}
