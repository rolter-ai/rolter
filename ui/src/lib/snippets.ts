// "Copy as code" for the Playground (#936).
//
// The Playground proves a route works. The next thing an operator does is wire
// that route into an application, and the useful output of a successful
// Playground call is therefore *the code that reproduces it* — not the reply.
//
// Everything here is a pure function over the request the Playground just sent
// and the gateway address to send it to, so the snippets are unit-testable and
// cannot drift from each other.

import type { GatewayBase } from "@/lib/gateway";

/** One turn of the conversation, as the snippet writes it out. */
export interface SnippetMessage {
  role: "system" | "user" | "assistant";
  /** the turn's text; multimodal parts are flattened to their text */
  content: string;
}

/** The request the Playground issued, in OpenAI terms. */
export interface SnippetRequest {
  /** the rolter route name, which is what the caller passes as `model` */
  model: string;
  /**
   * every turn the request carried, in order: a chat column sends its whole
   * thread, so a snippet with only the last prompt would not reproduce it (#2058)
   */
  messages: SnippetMessage[];
  /** whether the caller asked for SSE */
  stream?: boolean;
}

export type SnippetLang = "curl" | "python" | "javascript";

export const SNIPPET_LANGS: SnippetLang[] = ["curl", "python", "javascript"];

/**
 * The key is referenced through an environment variable, never inlined.
 *
 * The Playground holds a real virtual key in localStorage, and inlining it
 * would mean every snippet an operator pastes into a ticket, a README or a
 * chat leaks a working credential. An env var is also what the provider SDK
 * docs do, so the snippet reads as idiomatic rather than defensive.
 */
export const KEY_ENV = "ROLTER_API_KEY";

/** JSON-encode for embedding inside a source literal. */
const j = (v: unknown) => JSON.stringify(v);

/**
 * Quote for a POSIX shell's single quotes. Nothing inside them is special
 * except the closing quote itself, which an assistant turn ("it's") brings in
 * as soon as the body carries more than the operator's own prompt.
 */
const shellQuote = (s: string) => `'${s.replace(/'/g, `'\\''`)}'`;

/** The turns, one line each, indented and written in `item`'s literal syntax. */
const turnLines = (req: SnippetRequest, indent: string, item: (m: SnippetMessage) => string) =>
  req.messages.map((m) => `${indent}${item(m)}`);

function curl(req: SnippetRequest, base: GatewayBase): string {
  const body = {
    model: req.model,
    messages: req.messages.map(({ role, content }) => ({ role, content })),
    ...(req.stream ? { stream: true } : {}),
  };
  // a single prompt stays the one-liner it has always been. a conversation is
  // laid out a turn to a line, the way the thread it came from reads
  const json =
    req.messages.length > 1
      ? `{
  "model": ${j(req.model)},
  "messages": [
${turnLines(req, "    ", (m) => `{"role": ${j(m.role)}, "content": ${j(m.content)}}`).join(",\n")}
  ]${req.stream ? `,\n  "stream": true` : ""}
}`
      : j(body);
  return `curl ${base.url}/v1/chat/completions \\
  -H "Authorization: Bearer $${KEY_ENV}" \\
  -H "Content-Type: application/json" \\
  -d ${shellQuote(json)}`;
}

function python(req: SnippetRequest, base: GatewayBase): string {
  // a JSON string is a valid Python string literal, escapes and all
  const messages = `messages=[
${turnLines(req, "        ", (m) => `{"role": ${j(m.role)}, "content": ${j(m.content)}},`).join("\n")}
    ],`;
  const call = req.stream
    ? `stream = client.chat.completions.create(
    model=${j(req.model)},
    ${messages}
    stream=True,
)
for chunk in stream:
    print(chunk.choices[0].delta.content or "", end="")`
    : `response = client.chat.completions.create(
    model=${j(req.model)},
    ${messages}
)
print(response.choices[0].message.content)`;

  return `# pip install openai
import os
from openai import OpenAI

client = OpenAI(
    base_url=${j(`${base.url}/v1`)},
    api_key=os.environ[${j(KEY_ENV)}],
)

${call}`;
}

function javascript(req: SnippetRequest, base: GatewayBase): string {
  const messages = `messages: [
${turnLines(req, "    ", (m) => `{ role: ${j(m.role)}, content: ${j(m.content)} },`).join("\n")}
  ],`;
  const call = req.stream
    ? `const stream = await client.chat.completions.create({
  model: ${j(req.model)},
  ${messages}
  stream: true,
});
for await (const chunk of stream) {
  process.stdout.write(chunk.choices[0]?.delta?.content ?? "");
}`
    : `const response = await client.chat.completions.create({
  model: ${j(req.model)},
  ${messages}
});
console.log(response.choices[0].message.content);`;

  return `// npm install openai
import OpenAI from "openai";

const client = new OpenAI({
  baseURL: ${j(`${base.url}/v1`)},
  apiKey: process.env.${KEY_ENV},
});

${call}`;
}

/**
 * Render one request as runnable client code, addressed to `base` — the
 * saved public base URL `useGatewayBase()` returns, so it is the same address
 * every other snippet in the dashboard hands out. There is no `/gw` form:
 * that proxy needs a dashboard session an external client does not have.
 */
export function renderSnippet(lang: SnippetLang, req: SnippetRequest, base: GatewayBase): string {
  switch (lang) {
    case "curl":
      return curl(req, base);
    case "python":
      return python(req, base);
    case "javascript":
      return javascript(req, base);
  }
}
