import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import McpLogs from "./McpLogs";
import {
  expectAnalyticsUnavailable,
  expectEmptyState,
  expectForbidden,
  expectLoadError,
  expectListTable,
  expectSkeleton,
  Harness,
  json,
  pending,
  routes,
  scoped,
} from "./story-harness";
import type { McpLogRow } from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { atMobile, expectNoHorizontalOverflow } from "@/lib/story-viewport";

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

// a second call to a different tool, so each row's button has a name of its own
const TIMED_OUT: Partial<McpLogRow> = {
  event_id: "evt-2",
  tool: "create_issue",
  status: "timeout",
  error: "deadline exceeded",
};

// `/logs/summary` and the by-id `/logs/evt-2` are listed before `/logs`, which
// is a prefix of both
const loaded = routes([
  ["/mcp/logs/summary", () => ({ data: [SUMMARY] })],
  [
    "/mcp/logs/evt-2",
    () => ({ ...call(TIMED_OUT), arguments: '{"title":"flaky test"}', result: null }),
  ],
  ["/mcp/logs", () => ({ data: [call(), call(TIMED_OUT)], next_cursor: null })],
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
    await expectListTable(canvasElement, "MCP Logs");
  },
};

/**
 * An event opens from the keyboard, not only from a mouse click on its row
 * (#2022).
 *
 * The click sits on the row, and a `role="row"` takes no focus, so with no
 * control inside it Tab walked past every event and the redacted payloads were
 * mouse-only (WCAG 2.1.1). The row's button is reached by Tab, is named after
 * the event down to its time, and hands focus to the drawer and back. The story
 * opens the second row, so "focus went back to the control that opened it" has
 * a wrong answer available: the first row's button.
 */
export const OpensAnEventFromTheKeyboard: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const row = (await canvas.findByText("create_issue")).closest<HTMLElement>('[role="row"]')!;
    const time = within(row).getAllByRole("cell")[0].textContent;
    const open = within(row).getByRole("button", {
      name: `Open call details for create_issue on github at ${time}`,
    });

    // Tab alone gets there, the way a keyboard user arrives
    for (let i = 0; i < 30 && document.activeElement !== open; i++) await userEvent.tab();
    await expect(open).toHaveFocus();

    await userEvent.keyboard("{Enter}");
    const drawer = await canvas.findByRole("complementary", { name: "MCP call details" });
    await waitFor(() => expect(drawer).toHaveFocus());
    // the drawer fades in, so a single visibility read can land on its first
    // frame at opacity 0; poll it like any other state change (#2287)
    await waitFor(() => {
      expect(within(drawer).getByText("github → create_issue")).toBeVisible();
      expect(within(drawer).getByText("deadline exceeded")).toBeVisible();
    });

    // Escape closes it, and focus lands on this row's button again
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(canvas.queryByRole("complementary")).toBeNull());
    await waitFor(() => expect(open).toHaveFocus());

    // Space opens it as well, and the drawer's own close button hands focus
    // back the same way
    await userEvent.keyboard(" ");
    const again = await canvas.findByRole("complementary", { name: "MCP call details" });
    await waitFor(() => expect(again).toHaveFocus());
    await userEvent.tab();
    await expect(
      within(again).getByRole("button", { name: "Close MCP log details" }),
    ).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(canvas.queryByRole("complementary")).toBeNull());
    await waitFor(() => expect(open).toHaveFocus());
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

// the same deployment shape as Logs: a control plane with no clickhouse_url
// answers 503. It is a status, not the alert a 500 gets, and the stats and
// filters around it are not drawn, since every figure in them would be a zero
// the store never counted (#1236, #2016)
export const NoAnalyticsStore: Story = {
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "no clickhouse_url" } }, 503))}>
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const panel = await expectAnalyticsUnavailable(
      canvasElement,
      en.pages.mcpLogs.noAnalytics.title,
      "no clickhouse_url",
    );
    // what the screen will show once the store is there is named
    await expect(panel).toHaveTextContent(/every tool call they proxy/);
    await expect(canvas.queryByText(en.pages.mcpLogs.calls24h)).toBeNull();
    await expect(canvas.queryByRole("combobox")).toBeNull();
  },
};

// a control plane too old to serve /api/v1/mcp/logs at all answers 404, which
// the fetcher reads as the same answer (#1236)
export const NoAnalyticsRoute: Story = {
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "not found" } }, 404))}>
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectAnalyticsUnavailable(
      canvasElement,
      en.pages.mcpLogs.noAnalytics.title,
      "not found",
    );
  },
};

/**
 * The same panel at 375px and in Russian, where the title is the longest line
 * the screen says: it wraps inside the screen rather than pushing the page
 * sideways.
 */
export const NoAnalyticsStoreAtMobileInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "no clickhouse_url" } }, 503))}>
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectAnalyticsUnavailable(
      canvasElement,
      ru.pages.mcpLogs.noAnalytics.title,
      "no clickhouse_url",
    );
    await expectNoHorizontalOverflow();
  },
};

// the list loaded, so the store was there a moment ago: one row's detail read
// answering 503 is the same answer the list would have given, and it is said
// inside the drawer in the same calm voice rather than as a red alert (#2016)
export const TheStoreGoingAwayUnderADetail: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/mcp/logs/summary")) return json({ data: [SUMMARY] });
        if (url.includes("/mcp/logs/evt-1")) {
          return json({ error: { message: "no clickhouse_url" } }, 503);
        }
        return json({ data: [call()], next_cursor: null });
      })}
    >
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open call details for search_issues/ }),
    );
    const drawer = await canvas.findByRole("complementary", { name: "MCP call details" });
    const panel = await expectAnalyticsUnavailable(
      drawer,
      en.pages.mcpLogs.noAnalytics.title,
      "no clickhouse_url",
    );
    await expect(drawer).toContainElement(panel);
    // the list is still there: only the one read was refused
    await expect(canvas.getAllByText("search_issues").length).toBeGreaterThan(0);
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
