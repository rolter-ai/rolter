import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { CodeBlock } from "./code-block";

const JSON_PAYLOAD = `{
  "model": "gpt-4o",
  "messages": [
    { "role": "user", "content": "why is p99 latency up?" }
  ],
  "stream": true,
  "temperature": 0.2,
  "metadata": null
}`;

const YAML_PAYLOAD = `receivers:
  otlp:
    protocols:
      grpc:
        endpoint: 0.0.0.0:4317
exporters:
  # one pipeline per enabled connector
  otlphttp/datadog:
    endpoint: https://api.datadoghq.eu/otlp
    compression: gzip`;

const TOML_PAYLOAD = `[server]
port = 4000
host = "0.0.0.0"

# a route is a name a client passes as model
[[routes]]
name = "fast"
strategy = "least-latency"
providers = ["openai-prod", "vllm-cluster"]`;

const BASH_PAYLOAD = [
  "# base url is the dashboard's gateway proxy",
  "curl http://localhost:3000/gw/v1/chat/completions \\",
  '  -H "Authorization: Bearer $ROLTER_API_KEY" \\',
  '  -H "Content-Type: application/json" \\',
  `  -d '{"model":"fast","messages":[{"role":"user","content":"hi"}]}'`,
].join("\n");

const CSV_PAYLOAD = `ts,route,provider,latency_ms,tokens,cost_usd
2026-09-01T10:00:00Z,fast,openai-prod,412,1280,0.0104
2026-09-01T10:00:04Z,fast,vllm-cluster,208,1310,0.0000
2026-09-01T10:00:09Z,deep,anthropic-prod,1904,4820,0.0722`;

const LOG_PAYLOAD = `2026-09-01T10:00:00Z INFO  gateway listening on 0.0.0.0:4000
2026-09-01T10:00:04Z DEBUG route fast -> vllm-cluster (least-latency, 208ms)
2026-09-01T10:00:09Z WARN  openai-prod returned 429, retrying in 1200ms
2026-09-01T10:00:11Z ERROR upstream timeout after 30000ms: 10.0.4.19:8000`;

const TEXT_PAYLOAD = `payload logging is off for this route.
turn it on under Logs settings to capture request and response bodies.`;

/**
 * One line per language. This set exists to judge the palette, not to prove a
 * grammar — the per-language stories above carry the assertions — so it stays
 * small enough to render and check quickly.
 */
const SAMPLES = {
  json: '{ "route": "fast", "n": 2, "ok": true, "meta": null }',
  yaml: "route: fast # least-latency\nreplicas: 2",
  toml: "[server]\nport = 4000 # the data plane",
  bash: 'curl -H "Authorization: Bearer $ROLTER_API_KEY" http://localhost:4000/v1/models',
  csv: "route,latency_ms,cost_usd\nfast,208,0.0104",
  log: "2026-09-01T10:00:11Z ERROR upstream timeout after 30000ms",
  text: "payload logging is off for this route.",
};

const meta = {
  title: "Data/CodeBlock",
  component: CodeBlock,
  parameters: { layout: "padded" },
  args: { value: JSON_PAYLOAD, language: "json", label: "Request body" },
} satisfies Meta<typeof CodeBlock>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * The tokeniser arrives in a chunk of its own, so every assertion about colour
 * waits for it rather than racing it.
 */
const tokenised = (canvas: HTMLElement) =>
  waitFor(() => {
    const token = canvas.querySelector(".rl-code .token");
    if (!token) throw new Error("highlighter chunk has not landed yet");
    return token as HTMLElement;
  });

/** the classes Prism gave the tokens, so a story can assert the *kind* of one */
const kinds = (canvas: HTMLElement) =>
  [...canvas.querySelectorAll(".rl-code .token")].flatMap((el) => [...el.classList]);

export const Json: Story = {
  play: async ({ canvasElement }) => {
    await tokenised(canvasElement);
    // the four things that make a payload readable at a glance: which keys
    // there are, which values are strings, which are numbers, and what is null
    const found = kinds(canvasElement);
    await expect(found).toContain("property");
    await expect(found).toContain("string");
    await expect(found).toContain("number");
    await expect(found).toContain("null");
    // and the text is still exactly the payload
    await expect(canvasElement.querySelector("code")).toHaveTextContent(/"model": "gpt-4o"/);
  },
};

export const Yaml: Story = {
  args: { value: YAML_PAYLOAD, language: "yaml", label: "Collector config" },
  play: async ({ canvasElement }) => {
    await tokenised(canvasElement);
    await expect(kinds(canvasElement)).toContain("key");
    await expect(kinds(canvasElement)).toContain("comment");
  },
};

export const Toml: Story = {
  args: { value: TOML_PAYLOAD, language: "toml", label: "rolter.toml" },
  play: async ({ canvasElement }) => {
    await tokenised(canvasElement);
    await expect(kinds(canvasElement)).toContain("string");
    await expect(kinds(canvasElement)).toContain("comment");
  },
};

export const Bash: Story = {
  args: { value: BASH_PAYLOAD, language: "bash", label: "Code snippet" },
  play: async ({ canvasElement }) => {
    await tokenised(canvasElement);
    // the credential is an env var reference, and it reads as one
    await expect(kinds(canvasElement)).toContain("variable");
  },
};

export const Csv: Story = {
  args: { value: CSV_PAYLOAD, language: "csv", label: "Spend export" },
  play: async ({ canvasElement }) => {
    await tokenised(canvasElement);
    // the separators are what a person scans a csv by
    await expect(kinds(canvasElement)).toContain("punctuation");
  },
};

/**
 * Levels carry the same hues as the status badges in the table above them, so a
 * failing line in a payload and a failing row in the list are one colour.
 */
export const LogLines: Story = {
  args: { value: LOG_PAYLOAD, language: "log", label: "Gateway log" },
  play: async ({ canvasElement }) => {
    await tokenised(canvasElement);
    const found = kinds(canvasElement);
    await expect(found).toContain("level");
    await expect(found).toContain("error");
  },
};

/**
 * Plain text never loads a grammar at all: there is nothing to tokenise and no
 * reason to pay for the chunk.
 */
export const PlainText: Story = {
  args: { value: TEXT_PAYLOAD, language: "text", label: "Response body" },
  play: async ({ canvasElement }) => {
    await expect(canvasElement.querySelector("code")).toHaveTextContent(/payload logging is off/);
    await expect(canvasElement.querySelectorAll(".token")).toHaveLength(0);
  },
};

/**
 * The gutter is a CSS counter, never a DOM node — so a selection dragged across
 * the block copies the code and not the numbers.
 */
export const WithLineNumbers: Story = {
  args: { lineNumbers: true, label: "providers" },
  play: async ({ canvasElement }) => {
    await tokenised(canvasElement);
    const lines = canvasElement.querySelectorAll(".rl-code-line");
    await expect(lines).toHaveLength(JSON_PAYLOAD.split("\n").length);
    // the numbers are not part of the text, which is what makes copy exact
    await expect(canvasElement.querySelector("code")?.textContent).toBe(JSON_PAYLOAD);
  },
};

/**
 * Wrapping is opt-in: a drawer is narrow and a response body is long, so there
 * the wrap wins; a snippet scrolls sideways instead (#948).
 */
export const Wrapped: Story = {
  args: { wrap: true, maxHeight: 160, label: "Response body" },
  play: async ({ canvasElement }) => {
    const region = within(canvasElement).getByRole("region", { name: /Response body/i });
    await expect(region).toHaveStyle({ whiteSpace: "pre-wrap" });
  },
};

/**
 * A clipboard the story owns — the real one is unavailable in a headless
 * browser, so what was copied cannot be observed without standing one in.
 */
function WithClipboard({ sink, children }: { sink: string[]; children: React.ReactNode }) {
  const original = React.useRef<PropertyDescriptor | undefined>(undefined);
  React.useState(() => {
    original.current = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: async (value: string) => void sink.push(value) },
      configurable: true,
    });
    return null;
  });
  React.useEffect(
    () => () => {
      if (original.current) Object.defineProperty(navigator, "clipboard", original.current);
      else Reflect.deleteProperty(navigator, "clipboard");
    },
    [],
  );
  return <>{children}</>;
}

const copied: string[] = [];

/** What lands on the clipboard is the source, not the rendered spans. */
export const Copies: Story = {
  render: (args) => (
    <WithClipboard sink={copied}>
      <CodeBlock {...args} />
    </WithClipboard>
  ),
  play: async ({ canvasElement }) => {
    copied.length = 0;
    await tokenised(canvasElement);
    await userEvent.click(within(canvasElement).getByRole("button"));
    await waitFor(() => expect(copied).toEqual([JSON_PAYLOAD]));
  },
};

/**
 * The scroll container takes focus, so the part of a payload below the fold is
 * reachable without a mouse (#1181).
 */
export const KeyboardReachable: Story = {
  args: { maxHeight: 80 },
  play: async ({ canvasElement }) => {
    const region = within(canvasElement).getByRole("region", { name: /Request body/i });
    region.focus();
    await expect(region).toHaveFocus();
  },
};

/**
 * Past the cap the payload renders in full and unhighlighted. Prism tokenises
 * on the main thread; colour is the right thing to give up, content is not.
 */
export const TooLargeToHighlight: Story = {
  // built in `render` rather than in `args`: Storybook serialises args for the
  // controls panel and the interaction log, and a payload this size makes that
  // serialisation by far the slowest thing in the story
  render: () => (
    <CodeBlock
      value={`{"blob":"${"x".repeat(120_000)}"}`}
      language="json"
      label="Response body"
      maxHeight={160}
    />
  ),
  play: async ({ canvasElement }) => {
    await expect(canvasElement.querySelectorAll(".token")).toHaveLength(0);
    await expect(canvasElement.querySelector("code")?.textContent?.length).toBeGreaterThan(120_000);
  },
};

/** Every language side by side, which is how a palette gets judged. */
export const AllLanguages: Story = {
  render: () => (
    <div className="flex flex-col gap-4">
      <CodeBlock value={SAMPLES.json} language="json" label="json" copy={false} />
      <CodeBlock value={SAMPLES.yaml} language="yaml" label="yaml" copy={false} />
      <CodeBlock value={SAMPLES.toml} language="toml" label="toml" copy={false} />
      <CodeBlock value={SAMPLES.bash} language="bash" label="bash" copy={false} />
      <CodeBlock value={SAMPLES.csv} language="csv" label="csv" copy={false} />
      <CodeBlock value={SAMPLES.log} language="log" label="log" copy={false} />
      <CodeBlock value={SAMPLES.text} language="text" label="text" copy={false} />
    </div>
  ),
  play: async ({ canvasElement }) => {
    await tokenised(canvasElement);
    await expect(within(canvasElement).getAllByRole("region")).toHaveLength(7);
  },
};
