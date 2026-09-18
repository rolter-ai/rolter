import type { Meta, StoryObj } from "@storybook/react";
import { expect, waitFor, within } from "storybook/test";

import McpLogs from "./McpLogs";
import {
  expectEmptyState,
  expectForbidden,
  expectLoadError,
  expectSkeleton,
  Harness,
  json,
  pending,
  routes,
  scoped,
} from "./story-harness";
import type { McpLogRow } from "@/lib/api";

const call = (over: Partial<McpLogRow> = {}): McpLogRow => ({
  ts: "2026-08-06T10:00:00Z",
  event_id: "evt-1",
  server: "github",
  tool: "search_issues",
  transport: "streamable_http",
  status: "success",
  latency_ms: 320,
  org_id: "org-1",
  team_id: "team-1",
  project_id: "project-1",
  virtual_key_id: "vk-1",
  user_id: "u-1",
  request_id: "req-1",
  trace_id: "trace-1",
  error: null,
  ...over,
});

const SUMMARY = { calls: 128, failures: 3, avg_latency_ms: 410, p95_latency_ms: 980 };

// `/logs/summary` is listed before `/logs`, which is a prefix of it
const loaded = routes([
  ["/mcp/logs/summary", () => ({ data: [SUMMARY] })],
  [
    "/mcp/logs",
    () => ({
      data: [call(), call({ event_id: "evt-2", status: "timeout", error: "deadline exceeded" })],
      next_cursor: null,
    }),
  ],
]);

const meta = {
  title: "Screens/McpLogs",
  component: McpLogs,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof McpLogs>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("search_issues").length).toBeGreaterThan(0));
    // latency goes through useFormat and the analytics.ms unit, not a bare template (#1379)
    expect(canvas.getByText("410 ms")).toBeInTheDocument();
    expect(canvas.getByText("980 ms")).toBeInTheDocument();
    expect(canvas.getAllByText("320 ms").length).toBeGreaterThan(0);
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

// no filter is set, so the copy says the deployment has proxied no tool call —
// not that a filter excluded them
export const Empty: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/mcp/logs/summary", () => ({ data: [] })],
        ["/mcp/logs", () => ({ data: [], next_cursor: null })],
      ])}
    >
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No MCP tool calls yet/);
  },
};

/**
 * The summary envelope comes back empty while the log list itself loads fine.
 *
 * `fetchMcpSummary` resolved that to `undefined`, and react-query v5 rejects a
 * query function that resolves to `undefined` — so the one query this screen
 * waits on failed, and a deployment that had simply proxied no tool call in the
 * window read as an unreachable control plane (#1611, the same shape as #1608).
 *
 * The `Empty` story above cannot catch it: there the *rows* are empty too, and
 * the empty state it asserts renders either way.
 */
export const EmptySummaryEnvelope: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/mcp/logs/summary", () => ({ data: [] })],
        ["/mcp/logs", () => ({ data: [call(), call({ event_id: "evt-2" })], next_cursor: null })],
      ])}
    >
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the rows the list did return are on screen, not an error panel over them
    await waitFor(() => expect(canvas.getAllByText("search_issues").length).toBeGreaterThan(0));
    await expect(canvas.queryByRole("alert")).toBeNull();
    // a window with no calls in it is a count of zero, not an unknown one: the
    // dash here meant the query had failed, which is exactly the bug
    await waitFor(async () => expect(await canvas.findAllByText("0")).toHaveLength(2));
    await expect(canvasElement.textContent ?? "").not.toMatch(/NaN|undefined/);
  },
};

export const Error_: Story = {
  name: "Error",
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "boom" } }, 500))}>
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return MCP tool-call logs/i);
  },
};

// MCP logs are a deployment-scope read, so a non-superadmin gets 403 — and the
// screen now names who can widen the role instead of printing a grey sentence
export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "forbidden" } }, 403))}>
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /You do not have access to MCP tool-call logs/);
  },
};

// the same deployment shape as Logs, plus a control plane too old to serve
// /api/v1/mcp/logs at all — a 404 the fetcher reads the same way (#1236)
export const NoAnalyticsStore: Story = {
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "no clickhouse_url" } }, 503))}>
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /Analytics are not configured/i);
    await expect(canvas.getByText(/CLICKHOUSE_URL/)).toBeVisible();
    await expect(canvas.queryByRole("button", { name: /try again/i })).toBeNull();
  },
};

// What a non-superadmin gets, which is the screen refused before it asks
// (#1606).
//
// The MCP call log is is a deployment-scoped resource, so `superadminOnly` never mounts the
// screen for an org role however high. The stub answers the screen's own
// request with a perfectly good payload on purpose: if the wrapper is dropped
// the screen renders that payload and this story fails, which the `Forbidden`
// story cannot do — it stubs the 403 itself, so it passes either way.
export const RefusedToAnAdmin: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="admin">
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

export const RefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="viewer">
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};
