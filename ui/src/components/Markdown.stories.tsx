import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, waitFor, within } from "storybook/test";

import { Markdown } from "./Markdown";

/**
 * A reply of the shape the Playground actually receives: a sentence, a fenced
 * block, a list and a table.
 */
const REPLY = [
  "## Why p99 is up",
  "",
  "Two providers are in the route and one of them is **rate limiting**:",
  "",
  "```json",
  '{ "provider": "openai-prod", "status": 429, "retry_after_ms": 1200 }',
  "```",
  "",
  "| provider | p99 | share |",
  "| --- | --- | --- |",
  "| openai-prod | 1904ms | 62% |",
  "| vllm-cluster | 208ms | 38% |",
  "",
  "1. Shift the weight to `vllm-cluster`",
  "2. Re-check after five minutes",
  "",
  "> Read the [load balancing docs](https://example.test/docs) for the full story.",
].join("\n");

const meta = {
  title: "Components/Markdown",
  component: Markdown,
  parameters: { layout: "padded" },
  args: { source: REPLY },
} satisfies Meta<typeof Markdown>;

export default meta;
type Story = StoryObj<typeof meta>;

/** the parser is a chunk of its own, so the tree is awaited, never raced */
const parsed = (canvas: HTMLElement) =>
  waitFor(() => {
    const el = canvas.querySelector("p, ul, ol, table, .rl-code");
    if (!el) throw new Error("markdown chunk has not landed yet");
    return el;
  });

/** The whole point: the operator sees the reply, not the backticks. */
export const Rendered: Story = {
  play: async ({ canvasElement }) => {
    await parsed(canvasElement);
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("heading", { name: /Why p99 is up/ })).toBeVisible();
    await expect(canvas.getByRole("table")).toBeVisible();
    await expect(canvas.getByRole("list")).toBeVisible();
    await expect(canvasElement.querySelector("strong")).toHaveTextContent("rate limiting");
    // and none of the source delimiters survived into the text
    await expect(canvasElement.textContent).not.toContain("```");
  },
};

/** A fenced block goes through the same CodeBlock as every other payload, so
 *  it is highlighted and copyable rather than an indented paragraph (#949). */
export const FencedCodeIsHighlighted: Story = {
  play: async ({ canvasElement }) => {
    await waitFor(() => {
      const token = canvasElement.querySelector(".rl-code .token");
      if (!token) throw new Error("not highlighted yet");
    });
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("region", { name: /code block/i })).toBeVisible();
    // its own copy button, per block
    await expect(canvas.getAllByRole("button").length).toBeGreaterThan(0);
  },
};

/**
 * Model output is untrusted input. Markup in a reply is text an operator reads,
 * never markup a browser runs — there is no HTML in the path at all (#955).
 */
export const HtmlIsInert: Story = {
  args: {
    source: [
      "Here is the fix:",
      "",
      "<script>window.__rolter_pwned = true</script>",
      "",
      'and an <img src=x onerror="window.__rolter_pwned = true"> inline.',
    ].join("\n"),
  },
  play: async ({ canvasElement }) => {
    await parsed(canvasElement);
    // nothing ran
    await expect(
      (window as unknown as Record<string, unknown>).__rolter_pwned,
    ).toBeUndefined();
    // nothing was even created
    await expect(canvasElement.querySelector("script")).toBeNull();
    await expect(canvasElement.querySelector("img")).toBeNull();
    // the operator still sees exactly what the model emitted
    await expect(canvasElement.textContent).toContain("<script>window.__rolter_pwned = true</script>");
  },
};

/**
 * A `javascript:` link keeps its label and loses its href; a real link gets
 * `rel="noreferrer"` so the target learns nothing about the deployment.
 */
export const LinksAreConstrained: Story = {
  args: {
    source: "[safe](https://example.test/docs) and [unsafe](javascript:alert(1))",
  },
  play: async ({ canvasElement }) => {
    await parsed(canvasElement);
    const canvas = within(canvasElement);
    const link = canvas.getByRole("link", { name: "safe" });
    await expect(link).toHaveAttribute("href", "https://example.test/docs");
    await expect(link).toHaveAttribute("rel", "noreferrer");
    // the unsafe one is text, not a link
    await expect(canvas.queryByRole("link", { name: "unsafe" })).toBeNull();
    await expect(canvasElement.textContent).toContain("unsafe");
  },
};

/**
 * A remote image would breach the air-gapped guarantee and tell a third party
 * the reply had been read, so it is never emitted as an `<img>`.
 */
export const ImagesAreNotFetched: Story = {
  args: { source: "![a chart of p99](https://evil.test/pixel.png)" },
  play: async ({ canvasElement }) => {
    await parsed(canvasElement);
    await expect(canvasElement.querySelector("img")).toBeNull();
    await expect(canvasElement.textContent).toContain("a chart of p99");
  },
};

/**
 * Mid-stream the fence has not closed yet. The renderer treats it as the code
 * block it is about to become rather than dropping the column.
 */
export const StreamingPartialFence: Story = {
  args: {
    source: 'Here is the payload:\n\n```json\n{\n  "provider": "openai-prod",\n  "status": 4',
  },
  play: async ({ canvasElement }) => {
    await parsed(canvasElement);
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("region", { name: /code block/i })).toBeVisible();
    await expect(canvasElement.textContent).toContain('"provider": "openai-prod"');
  },
};

/**
 * Chunks arrive one after another and the tree is rebuilt each time. What must
 * not happen is an empty frame between them: the plain source is rendered until
 * the parse lands, so there is always something to read.
 */
export const StreamsWithoutFlicker: Story = {
  render: () => {
    const [n, setN] = React.useState(8);
    React.useEffect(() => {
      if (n >= REPLY.length) return;
      const id = setTimeout(() => setN((v) => Math.min(v + 37, REPLY.length)), 16);
      return () => clearTimeout(id);
    }, [n]);
    return <Markdown source={REPLY.slice(0, n)} />;
  },
  play: async ({ canvasElement }) => {
    // there is never nothing on screen
    await expect(canvasElement.textContent?.length).toBeGreaterThan(0);
    await waitFor(
      () => expect(within(canvasElement).getByRole("table")).toBeVisible(),
      { timeout: 5000 },
    );
  },
};

/**
 * Models emit broken markdown constantly — a table with a missing cell, a list
 * that never closes, a stray pipe. None of it is a reason to show nothing.
 */
export const MalformedMarkdown: Story = {
  args: {
    source: [
      "| a | b |",
      "| --- |",
      "| 1 | 2 | 3 |",
      "",
      "* item with **unclosed bold",
      "  - nested",
      "",
      "```",
      "an unterminated fence with a ~~~ inside",
    ].join("\n"),
  },
  play: async ({ canvasElement }) => {
    await parsed(canvasElement);
    const canvas = within(canvasElement);
    // the broken table is not a table; it renders as the text it is, rather
    // than as nothing
    await expect(canvas.queryByRole("table")).toBeNull();
    await expect(canvasElement.textContent).toContain("| a | b |");
    await expect(canvasElement.textContent).toContain("item with");
    // and the fence that never closed still shows its contents
    await expect(canvas.getByRole("region", { name: /code block/i })).toHaveTextContent(
      "an unterminated fence",
    );
  },
};

/** An empty reply renders nothing rather than an empty box. */
export const Empty: Story = {
  args: { source: "" },
  play: async ({ canvasElement }) => {
    await expect(canvasElement.textContent?.trim()).toBe("");
  },
};
