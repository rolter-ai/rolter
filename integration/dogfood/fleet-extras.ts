#!/usr/bin/env bun
// The rest of the fake fleet for operator dogfooding: the upstreams `fleet.ts`
// cannot stand in for because they do not speak the OpenAI dialect.
//
//   - an Anthropic-shaped provider (`/v1/models`, `/v1/messages`, streaming and
//     not, `x-api-key` auth), so the Anthropic adapter and the `/v1/messages`
//     surface have a real target instead of only the built-in `fake-llm`
//   - three MCP servers over streamable HTTP, one per auth shape rolter
//     supports (none, an API key in a header, a bearer token), with one of them
//     deliberately slow and flaky so the MCP health and logs screens have
//     something other than green to show
//
//   bun integration/dogfood/fleet-extras.ts
//
// Every key below is fake and loopback-only, the same as `keys.env`. They are
// fixed rather than random so the printed sheet stays valid across a restart.

const HOST = "127.0.0.1";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...headers },
  });

const TEXT =
  "A gateway earns its keep on the bad days: the provider that times out, the " +
  "key that was rotated, the tool server that answers every third call. This " +
  "reply is fake, but the pacing is not, so the latency views have a real shape.";

// ── Anthropic ────────────────────────────────────────────────────────────────

export const ANTHROPIC = {
  port: 18016,
  label: "anthropic-direct",
  apiKey: "sk-ant-dogfood-5e19c0a7d2",
  ttftMs: 220,
  tpotMs: 9,
  models: ["claude-sonnet-4-5", "claude-haiku-4-5", "claude-opus-4-1"],
};

/** Anthropic-shaped error body, which is what rolter's adapter parses. */
const antError = (type: string, message: string, status: number) =>
  json({ type: "error", error: { type, message } }, status);

function serveAnthropic() {
  const a = ANTHROPIC;
  Bun.serve({
    hostname: HOST,
    port: a.port,
    idleTimeout: 120,
    async fetch(req) {
      const url = new URL(req.url);
      if (url.pathname === "/health") return json({ status: "ok", label: a.label });

      // auth first, before the catalogue, so "Test connection" with a wrong key
      // fails the way the real API does
      if (req.headers.get("x-api-key") !== a.apiKey) {
        return antError("authentication_error", "invalid x-api-key", 401);
      }

      if (url.pathname === "/v1/models") {
        return json({
          data: a.models.map((id) => ({
            type: "model",
            id,
            display_name: id,
            created_at: "2025-09-29T00:00:00Z",
          })),
          has_more: false,
          first_id: a.models[0],
          last_id: a.models[a.models.length - 1],
        });
      }

      if (url.pathname !== "/v1/messages" || req.method !== "POST") {
        return antError("not_found_error", `Unknown route ${url.pathname}`, 404);
      }

      const body = await req.json().catch(() => ({}));
      const model: string = body.model ?? a.models[0];
      const words = TEXT.split(" ");
      const inputTokens = JSON.stringify(body.messages ?? []).length >> 2;
      const id = `msg_${Math.random().toString(36).slice(2, 14)}`;

      if (!body.stream) {
        await sleep(a.ttftMs + a.tpotMs * words.length);
        return json({
          id,
          type: "message",
          role: "assistant",
          model,
          content: [{ type: "text", text: `[${a.label}] ${TEXT}` }],
          stop_reason: "end_turn",
          stop_sequence: null,
          usage: { input_tokens: inputTokens, output_tokens: words.length },
        });
      }

      const stream = new ReadableStream({
        async start(controller) {
          const enc = new TextEncoder();
          const send = (event: string, data: unknown) =>
            controller.enqueue(enc.encode(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`));

          await sleep(a.ttftMs);
          send("message_start", {
            type: "message_start",
            message: {
              id,
              type: "message",
              role: "assistant",
              model,
              content: [],
              stop_reason: null,
              stop_sequence: null,
              usage: { input_tokens: inputTokens, output_tokens: 1 },
            },
          });
          send("content_block_start", {
            type: "content_block_start",
            index: 0,
            content_block: { type: "text", text: "" },
          });
          send("content_block_delta", {
            type: "content_block_delta",
            index: 0,
            delta: { type: "text_delta", text: `[${a.label}] ` },
          });
          for (const w of words) {
            await sleep(a.tpotMs);
            send("content_block_delta", {
              type: "content_block_delta",
              index: 0,
              delta: { type: "text_delta", text: `${w} ` },
            });
          }
          send("content_block_stop", { type: "content_block_stop", index: 0 });
          send("message_delta", {
            type: "message_delta",
            delta: { stop_reason: "end_turn", stop_sequence: null },
            usage: { output_tokens: words.length },
          });
          send("message_stop", { type: "message_stop" });
          controller.close();
        },
      });
      return new Response(stream, {
        headers: { "content-type": "text/event-stream", "cache-control": "no-cache" },
      });
    },
  });
}

// ── MCP ──────────────────────────────────────────────────────────────────────

interface Tool {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
  run: (args: Record<string, unknown>) => unknown;
}

interface McpServer {
  port: number;
  label: string;
  /** how the server authenticates, mirroring rolter's `auth_kind` */
  auth: { kind: "none" } | { kind: "header"; header: string; value: string } | { kind: "bearer"; token: string };
  /** added before every response, milliseconds */
  latencyMs: number;
  /** fraction of tool calls that fail with a 500 */
  errorRate?: number;
  tools: Tool[];
}

const obj = (props: Record<string, unknown>, required: string[] = []) => ({
  type: "object",
  properties: props,
  required,
});

const FILES: Record<string, string> = {
  "README.md": "# payments-service\n\nHandles card capture and refunds.",
  "src/refund.ts": "export function refund(id: string) { /* … */ }",
  "runbooks/oncall.md": "1. check the dashboard\n2. page the owner",
};

let ticketSeq = 4120;
const TICKETS = [
  { id: "OPS-4117", title: "Gateway p99 regression after deploy", status: "open" },
  { id: "OPS-4118", title: "Rotate the staging provider key", status: "in_progress" },
  { id: "OPS-4119", title: "Budget alert fired for team ml-research", status: "done" },
];

export const MCP_SERVERS: McpServer[] = [
  {
    port: 18101,
    label: "mcp-files",
    auth: { kind: "none" },
    latencyMs: 30,
    tools: [
      {
        name: "list_files",
        description: "List files in the repository",
        inputSchema: obj({ prefix: { type: "string" } }),
        run: ({ prefix }) =>
          Object.keys(FILES).filter((p) => !prefix || p.startsWith(String(prefix))),
      },
      {
        name: "read_file",
        description: "Read one file by path",
        inputSchema: obj({ path: { type: "string" } }, ["path"]),
        run: ({ path }) => {
          const text = FILES[String(path)];
          if (text === undefined) throw new Error(`no such file: ${path}`);
          return text;
        },
      },
    ],
  },
  {
    port: 18102,
    label: "mcp-tickets",
    auth: { kind: "header", header: "x-api-key", value: "mcp-tickets-dogfood-4c2e81" },
    latencyMs: 120,
    tools: [
      {
        name: "search_tickets",
        description: "Search tickets by text",
        inputSchema: obj({ query: { type: "string" } }, ["query"]),
        run: ({ query }) => {
          const q = String(query).toLowerCase();
          return TICKETS.filter((t) => t.title.toLowerCase().includes(q));
        },
      },
      {
        name: "create_ticket",
        description: "Create a ticket",
        inputSchema: obj({ title: { type: "string" } }, ["title"]),
        run: ({ title }) => {
          const t = { id: `OPS-${ticketSeq++}`, title: String(title), status: "open" };
          TICKETS.push(t);
          return t;
        },
      },
    ],
  },
  {
    port: 18103,
    label: "mcp-weather (slow, flaky)",
    auth: { kind: "bearer", token: "mcp-weather-dogfood-a91f37" },
    latencyMs: 650,
    errorRate: 0.2,
    tools: [
      {
        name: "get_forecast",
        description: "Three-day forecast for a city",
        inputSchema: obj({ city: { type: "string" } }, ["city"]),
        run: ({ city }) => ({
          city,
          days: ["sun 21°", "cloud 18°", "rain 15°"],
        }),
      },
    ],
  },
];

const rpcResult = (id: unknown, result: unknown, headers: Record<string, string> = {}) =>
  json({ jsonrpc: "2.0", id, result }, 200, headers);
const rpcError = (id: unknown, code: number, message: string) =>
  json({ jsonrpc: "2.0", id, error: { code, message } });

function authorized(s: McpServer, req: Request): boolean {
  switch (s.auth.kind) {
    case "none":
      return true;
    case "header":
      return req.headers.get(s.auth.header) === s.auth.value;
    case "bearer":
      return req.headers.get("authorization") === `Bearer ${s.auth.token}`;
  }
}

function serveMcp(s: McpServer) {
  Bun.serve({
    hostname: HOST,
    port: s.port,
    idleTimeout: 120,
    async fetch(req) {
      const url = new URL(req.url);
      if (url.pathname === "/health") return json({ status: "ok", label: s.label });
      if (!authorized(s, req)) {
        return json({ error: "unauthorized" }, 401, { "www-authenticate": "Bearer" });
      }
      // streamable HTTP: JSON-RPC over POST. a GET would open the optional
      // server-to-client stream, which these servers do not offer
      if (req.method === "GET") return new Response(null, { status: 405 });
      if (req.method === "DELETE") return new Response(null, { status: 204 });
      if (req.method !== "POST") return new Response(null, { status: 405 });

      const msg = await req.json().catch(() => null);
      if (!msg || typeof msg !== "object") return rpcError(null, -32700, "parse error");
      await sleep(s.latencyMs);

      // a notification has no id and gets no body
      if (msg.id === undefined) return new Response(null, { status: 202 });

      switch (msg.method) {
        case "initialize":
          return rpcResult(
            msg.id,
            {
              protocolVersion: msg.params?.protocolVersion ?? "2025-06-18",
              capabilities: { tools: { listChanged: false } },
              serverInfo: { name: s.label, version: "0.0.0-dogfood" },
            },
            { "mcp-session-id": crypto.randomUUID() },
          );
        case "ping":
          return rpcResult(msg.id, {});
        case "tools/list":
          return rpcResult(msg.id, {
            tools: s.tools.map(({ name, description, inputSchema }) => ({
              name,
              description,
              inputSchema,
            })),
          });
        case "tools/call": {
          if (s.errorRate && Math.random() < s.errorRate) {
            return json({ error: "upstream weather feed unavailable" }, 500);
          }
          const tool = s.tools.find((t) => t.name === msg.params?.name);
          if (!tool) return rpcError(msg.id, -32602, `unknown tool ${msg.params?.name}`);
          try {
            const out = tool.run(msg.params?.arguments ?? {});
            const text = typeof out === "string" ? out : JSON.stringify(out, null, 2);
            return rpcResult(msg.id, { content: [{ type: "text", text }], isError: false });
          } catch (e) {
            return rpcResult(msg.id, {
              content: [{ type: "text", text: (e as Error).message }],
              isError: true,
            });
          }
        }
        case "resources/list":
          return rpcResult(msg.id, { resources: [] });
        case "prompts/list":
          return rpcResult(msg.id, { prompts: [] });
        default:
          return rpcError(msg.id, -32601, `method not found: ${msg.method}`);
      }
    },
  });
}

if (import.meta.main) {
  serveAnthropic();
  for (const s of MCP_SERVERS) serveMcp(s);
  console.log(`fake extras up on ${HOST}`);
  console.log(`  :${ANTHROPIC.port}  ${ANTHROPIC.label.padEnd(26)} x-api-key  ${ANTHROPIC.models.join(", ")}`);
  for (const s of MCP_SERVERS) {
    console.log(`  :${s.port}  ${s.label.padEnd(26)} ${s.auth.kind.padEnd(9)}  ${s.tools.map((t) => t.name).join(", ")}`);
  }
}
