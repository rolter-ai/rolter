import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { useLocation } from "react-router";
import { expect, userEvent, waitFor, within } from "storybook/test";

import McpLogs from "./McpLogs";
import {
  expectAnalyticsUnavailable,
  expectEmptyState,
  expectLoadError,
  expectListTable,
  expectSkeleton,
  Harness,
  json,
  openOptions,
  pickOption,
  pending,
  recording,
  routes,
  scoped,
  expectNoUxEvent,
  expectUxEvent,
  recordUxEvents,
  uxEvents,
} from "./story-harness";
import type { McpLogRow } from "@/lib/api";
import { AuthProvider } from "@/lib/auth";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { atMobile, expectNoHorizontalOverflow } from "@/lib/story-viewport";
import { UxScreenProvider } from "@/lib/ux-react";

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

const statusFiltered = recording(loaded);

export const FiltersCallsNoReplyWasAwaitedFor: Story = {
  render: () => (
    <Harness fetchStub={statusFiltered.stub}>
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const filter = await canvas.findByRole("combobox", { name: "Filter by status" });
    await pickOption(filter, "Not awaited");
    await waitFor(() => expect(lastLogsQuery(statusFiltered).get("status")).toBe("not_awaited"));
  },
};

// how a call ended reads in the dashboard's language; the transport is the
// protocol's own identifier and stays as stored
export const StatusesInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={loaded}>
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const { statuses } = ru.pages.mcpLogs;
    await waitFor(() => expect(canvas.getAllByText(statuses.success).length).toBeGreaterThan(0));
    await expect(canvas.getAllByText(statuses.timeout).length).toBeGreaterThan(0);
    await expect(canvas.queryByText("success")).toBeNull();
  },
};

/**
 * Figures go through `useFormat()`, the window is stated beside the pager, and the
 * drawer's ids can be copied and followed to the request log (#2165).
 */
export const FormatsFiguresAndOffersTheIds: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/mcp/logs/summary", () => ({ data: [{ ...SUMMARY, calls: 12345, failures: 1234 }] })],
        ["/mcp/logs/evt-1", () => ({ ...call(), arguments: null, result: null })],
        ["/mcp/logs", () => ({ data: [call()], next_cursor: null })],
      ])}
    >
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("12,345");
    await expect(canvas.getByText("1,234")).toBeInTheDocument();
    await expect(canvas.getByRole("combobox", { name: "Time window" })).toHaveValue(
      "Last 24 hours",
    );
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open call details for search_issues/ }),
    );
    const drawer = within(await canvas.findByRole("complementary", { name: "MCP call details" }));
    await expect(await drawer.findByText("req-1")).toBeInTheDocument();
    await expect(drawer.getByText("trace-1")).toBeInTheDocument();
    // the drawer fades in; poll visibility rather than read the first frame (#2287)
    await waitFor(() => {
      expect(
        drawer.getByRole("button", { name: new RegExp(en.pages.mcpLogs.copyRequest) }),
      ).toBeVisible();
      expect(
        drawer.getByRole("button", { name: new RegExp(en.pages.mcpLogs.copyTrace) }),
      ).toBeVisible();
    });
    await expect(drawer.getByRole("link", { name: en.pages.mcpLogs.openInLogs })).toHaveAttribute(
      "href",
      "/logs?request_id=req-1",
    );
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
    // the empty state says what lands here and points at where it comes from
    await expect(
      within(canvasElement).getByRole("link", { name: en.pages.mcpLogs.emptyAction }),
    ).toHaveAttribute("href", "/mcp-catalog");
  },
};

// "My calls" narrows to nothing: a filter answer whose button clears every
// filter, so it is labelled for filters rather than for a search (#2294)
export const NoMatch: Story = {
  render: () => (
    <SignedIn>
      <Harness
        fetchStub={scoped(async (input) => {
          const url = new URL(String(input), "http://localhost");
          if (url.pathname.endsWith("/mcp/logs/summary")) return json({ data: [SUMMARY] });
          const user = url.searchParams.get("user");
          return json({ data: user ? [] : [call()], next_cursor: null });
        })}
      >
        <McpLogs />
      </Harness>
    </SignedIn>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "My calls" }));
    await expectEmptyState(canvasElement, /No calls match these filters/);
    await userEvent.click(canvas.getByRole("button", { name: en.common.clearFilters }));
    await waitFor(() => expect(canvas.getAllByText("search_issues").length).toBeGreaterThan(0));
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
    // the summary failing is announced on its own, not left as four dashes (#2165)
    await expectLoadError(canvasElement, /failed to return MCP call summary/i);
    const canvas = within(canvasElement);
    await expect(canvas.getAllByText("—")).toHaveLength(4);
    await expect(canvas.getAllByRole("button", { name: /try again/i }).length).toBeGreaterThan(0);
  },
};

// a read past the request timeout is a busy store, not a dead one (#2713)
export const QueryTimeout: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async () =>
        json(
          { error: { message: "analytics query timed out", code: "analytics_query_timeout" } },
          504,
        ),
      )}
    >
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /MCP tool-call logs took too long/i);
    const canvas = within(canvasElement);
    await expect(canvas.queryByText(/failed to return/i)).toBeNull();
    await expect(canvas.getAllByRole("button", { name: /try again/i }).length).toBeGreaterThan(0);
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
    await expect(canvas.queryByText(en.pages.mcpLogs.calls)).toBeNull();
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

// What a member gets: the screen, narrowed to their scope by the server (#2396).
//
// `mcp_log` is a project-scoped read with a viewer floor, so the screen mounts
// for any role. The stub answers with a good payload on purpose: re-adding a
// superadmin-only wrapper would swap this for the refusal and fail the story.
export const SeenByAMember: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="member">
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("search_issues").length).toBeGreaterThan(0));
    await expect(canvas.queryByRole("alert")).toBeNull();
    await expectListTable(canvasElement, "MCP Logs");
  },
};

export const SeenByAViewer: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="viewer">
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByText("search_issues").length).toBeGreaterThan(0));
    await expect(canvas.queryByRole("alert")).toBeNull();
  },
};

// a member whose scope holds no calls: the empty copy says whose calls the
// screen lists, so an empty page is not read as a broken one
export const EmptyForAMember: Story = {
  render: () => (
    <Harness
      role="member"
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
    await expect(canvasElement).toHaveTextContent(/hold a role in/);
  },
};

// the server blanks arguments and result below the payload floor and says so;
// the drawer names the role rather than showing a call with nothing in it
export const PayloadWithheldForAViewer: Story = {
  render: () => (
    <Harness
      role="viewer"
      fetchStub={routes([
        ["/mcp/logs/summary", () => ({ data: [SUMMARY] })],
        [
          "/mcp/logs/evt-1",
          () => ({ ...call(), arguments: null, result: null, payload_withheld: 1 }),
        ],
        ["/mcp/logs", () => ({ data: [call()], next_cursor: null })],
      ])}
    >
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open call details for search_issues/ }),
    );
    const drawer = within(await canvas.findByRole("complementary", { name: "MCP call details" }));
    // the drawer fades in; poll visibility rather than read the first frame (#2287)
    await waitFor(async () =>
      expect(await drawer.findByText(/hidden for your role/i)).toBeVisible(),
    );
    await expect(drawer.getByRole("heading", { name: "Arguments and result" })).toBeVisible();
    await expect(drawer.queryByText("Arguments")).toBeNull();
  },
};

// the by-id read is scoped too: an event outside the caller's reach answers
// 404, which is a statement about the call and not a failure to retry
export const DetailNotFound: Story = {
  render: () => (
    <Harness
      role="member"
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/mcp/logs/summary")) return json({ data: [SUMMARY] });
        if (url.includes("/mcp/logs/evt-1")) return json({ error: { message: "not found" } }, 404);
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
    const drawer = within(await canvas.findByRole("complementary", { name: "MCP call details" }));
    await waitFor(async () =>
      expect(await drawer.findByText(en.pages.mcpLogs.notFoundTitle)).toBeVisible(),
    );
    await expect(drawer.queryByRole("alert")).toBeNull();
    await expect(drawer.queryByRole("button", { name: /try again/i })).toBeNull();
  },
};

const ME = { id: "u-me", email: "ada@acme.dev", is_superadmin: false };

// signed in the way a login leaves the browser, minus the token, so the
// provider knows the account without asking /auth/me for it
function SignedIn({ children }: { children: React.ReactNode }) {
  React.useState(() => {
    localStorage.setItem("rolter.session.email", ME.email);
    localStorage.setItem("rolter.session.user", JSON.stringify(ME));
    localStorage.removeItem("rolter.session.token");
  });
  React.useEffect(
    () => () => {
      localStorage.removeItem("rolter.session.email");
      localStorage.removeItem("rolter.session.user");
    },
    [],
  );
  return <AuthProvider>{children}</AuthProvider>;
}

// the control plane narrows on the `user` filter before the page is cut
const mineOnly = () =>
  recording(
    scoped(async (input) => {
      const url = new URL(String(input), "http://localhost");
      if (url.pathname.endsWith("/mcp/logs/summary")) return json({ data: [SUMMARY] });
      const user = url.searchParams.get("user");
      const all = [call({ user_id: ME.id }), call({ ...TIMED_OUT, user_id: "u-other" })];
      return json({ data: all.filter((r) => !user || r.user_id === user), next_cursor: null });
    }),
  );

const lastLogsQuery = (recorder: ReturnType<typeof recording>) => {
  const reads = recorder.calls.filter(
    (c) => c.url.includes("/mcp/logs") && !c.url.includes("/summary"),
  );
  return new URL(reads[reads.length - 1]?.url ?? "", "http://localhost").searchParams;
};

/**
 * #2515: "My calls" sets the `user` filter to the signed-in account's id, for
 * every role that reaches the screen (#2396), and a second click lifts it.
 */
const asMember = mineOnly();
const asViewer = mineOnly();

async function expectMyCalls(canvasElement: HTMLElement, recorder: ReturnType<typeof mineOnly>) {
  const canvas = within(canvasElement);
  await waitFor(() => expect(canvas.getAllByText("create_issue").length).toBeGreaterThan(0));
  const mine = await canvas.findByRole("button", { name: "My calls" });
  await expect(mine).toHaveAttribute("aria-pressed", "false");

  await userEvent.click(mine);
  await waitFor(() => expect(canvas.queryByText("create_issue")).toBeNull());
  await expect(canvas.getAllByText("search_issues").length).toBeGreaterThan(0);
  await expect(lastLogsQuery(recorder).get("user")).toBe(ME.id);
  await expect(mine).toHaveAttribute("aria-pressed", "true");

  await userEvent.click(mine);
  await waitFor(() => expect(canvas.getAllByText("create_issue").length).toBeGreaterThan(0));
  await expect(lastLogsQuery(recorder).has("user")).toBe(false);
}

export const MyCallsForAMember: Story = {
  render: () => (
    <SignedIn>
      <Harness fetchStub={asMember.stub} role="member">
        <McpLogs />
      </Harness>
    </SignedIn>
  ),
  play: ({ canvasElement }) => expectMyCalls(canvasElement, asMember),
};

export const MyCallsForAViewer: Story = {
  render: () => (
    <SignedIn>
      <Harness fetchStub={asViewer.stub} role="viewer">
        <McpLogs />
      </Harness>
    </SignedIn>
  ),
  play: ({ canvasElement }) => expectMyCalls(canvasElement, asViewer),
};

/**
 * Each failed read is one `error_state` under its own region (#2444): the call
 * list's alert records `mcp-logs`, the region its empty state names, and the
 * summary, which has no alert of its own, `mcp-log-summary`.
 */
export const EachFailedReadIsOneErrorState: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <UxScreenProvider screen="mcp-logs">
      <Harness fetchStub={scoped(async () => json({ error: { message: "boom" } }, 500))}>
        <McpLogs />
      </Harness>
    </UxScreenProvider>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return MCP tool-call logs/i);
    await expectUxEvent("error_state", "mcp-logs");
    await expectUxEvent("error_state", "mcp-log-summary");
    await expect(
      uxEvents()
        .filter((e) => e.action === "error_state")
        .map((e) => e.target)
        .sort(),
    ).toEqual(["mcp-log-summary", "mcp-logs"]);
  },
};

/** No analytics store is a supported deployment, stated calmly: it is not an error state. */
export const NoAnalyticsStoreIsNotAnErrorState: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <UxScreenProvider screen="mcp-logs">
      <Harness
        fetchStub={scoped(async () => json({ error: { message: "no clickhouse_url" } }, 503))}
      >
        <McpLogs />
      </Harness>
    </UxScreenProvider>
  ),
  play: async ({ canvasElement }) => {
    await expectAnalyticsUnavailable(
      canvasElement,
      en.pages.mcpLogs.noAnalytics.title,
      "no clickhouse_url",
    );
    await expectUxEvent("time_to_interactive");
    expectNoUxEvent("error_state");
  },
};

// ---------------------------------------------------------------------------
// the server and tool filters live in the address (#951)

// stands in for the address bar: the filters are read from it and written to it
function Where() {
  return <output data-testid="where">{useLocation().search}</output>;
}

const REGISTRY = [
  { slug: "github", tools: ["search_issues", "create_issue"] },
  { slug: "sentry", tools: ["list_issues"] },
];
// answers by the filters it was asked for, so a story proves they were sent
const byFilters = scoped(async (input) => {
  const url = new URL(String(input), "http://localhost");
  if (url.pathname.endsWith("/mcp/logs/summary")) return json({ data: [SUMMARY] });
  if (url.pathname.endsWith("/mcp-servers")) return json(REGISTRY);
  const server = url.searchParams.get("server");
  const tool = url.searchParams.get("tool");
  const rows = [call(), call(TIMED_OUT)].filter(
    (row) => (!server || row.server === server) && (!tool || row.tool === tool),
  );
  return json({ data: rows, next_cursor: null });
});

// the catalog's "View logs" link lands here: the filters are applied before anyone touches a control
const linked = recording(byFilters);
export const FiltersComeFromTheAddress: Story = {
  render: () => (
    <Harness fetchStub={linked.stub} route="/mcp-logs?server=github&tool=create_issue">
      <McpLogs />
      <Where />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("combobox", { name: "Filter by server" })).toHaveValue(
      "github",
    );
    await expect(canvas.getByRole("combobox", { name: "Filter by tool" })).toHaveValue(
      "create_issue",
    );
    await waitFor(() =>
      expect(
        linked.calls.some(
          (c) =>
            c.url.includes("/mcp/logs?") &&
            c.url.includes("server=github") &&
            c.url.includes("tool=create_issue"),
        ),
      ).toBe(true),
    );
    // only the filtered call is listed
    await waitFor(() => expect(canvas.getAllByText("create_issue").length).toBeGreaterThan(0));
    await expect(canvas.queryByText("search_issues")).not.toBeInTheDocument();
  },
};

export const PickingAServerWritesTheAddress: Story = {
  render: () => (
    <Harness fetchStub={byFilters} route="/mcp-logs?status=ignored">
      <McpLogs />
      <Where />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const server = await canvas.findByRole("combobox", { name: "Filter by server" });
    await userEvent.click(server);
    await userEvent.click(await within(document.body).findByRole("option", { name: "sentry" }));
    await waitFor(() =>
      expect(canvas.getByTestId("where")).toHaveTextContent("status=ignored&server=sentry"),
    );
    // the tool suggestions follow the server: sentry has one, github's are not offered
    const listbox = await openOptions(canvas.getByRole("combobox", { name: "Filter by tool" }));
    await expect(within(listbox).getByRole("option", { name: "list_issues" })).toBeVisible();
    await expect(within(listbox).queryByRole("option", { name: "create_issue" })).toBeNull();
    await userEvent.click(within(listbox).getByRole("option", { name: "list_issues" }));
    await waitFor(() => expect(canvas.getByTestId("where")).toHaveTextContent("tool=list_issues"));
  },
};

export const ClearingTheFiltersLeavesTheAddressBare: Story = {
  render: () => (
    <Harness fetchStub={byFilters} route="/mcp-logs?server=ghost&tool=nothing">
      <McpLogs />
      <Where />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // a filter from the address narrows to nothing: a filter answer, not "no calls yet"
    await expectEmptyState(canvasElement, /No calls match these filters/);
    await expect(canvas.getByText(/matched the filters you selected/)).toBeVisible();
    await userEvent.click(canvas.getByRole("button", { name: en.common.clearFilters }));
    await waitFor(() => expect(canvas.getByTestId("where")).toBeEmptyDOMElement());
    await waitFor(() => expect(canvas.getAllByText("search_issues").length).toBeGreaterThan(0));
  },
};

// the tool filter counts as a filter on its own
export const AToolAloneIsAFilter: Story = {
  render: () => (
    <Harness fetchStub={byFilters} route="/mcp-logs?tool=nothing">
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No calls match these filters/);
  },
};

export const ServerFilterInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={byFilters} route="/mcp-logs?server=github">
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByRole("combobox", { name: ru.pages.mcpLogs.serverFilterAria }),
    ).toHaveValue("github");
    await expect(
      canvas.getByRole("combobox", { name: ru.pages.mcpLogs.toolFilterAria }),
    ).toBeVisible();
  },
};

/** the whole days a bound reaches back from now, so a story does not depend on the clock to the second */
const daysBack = (url: string) => {
  const since = new URL(url, "http://x").searchParams.get("since");
  return since ? Math.round((Date.now() - Date.parse(since)) / 86_400_000) : null;
};

const windowed = recording(loaded);

/**
 * #3300: the window is a name the address carries. A link to `?window=7d`
 * reads both the tiles and the list over seven days, and the picker says so.
 */
export const TheWindowComesFromTheAddress: Story = {
  render: () => (
    <Harness fetchStub={windowed.stub} route="/mcp-logs?window=7d">
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("combobox", { name: "Time window" })).toHaveValue(
      "Last 7 days",
    );
    await waitFor(() => {
      const summary = windowed.calls.find((c) => c.url.includes("/mcp/logs/summary"));
      const list = windowed.calls.find((c) => c.url.includes("/mcp/logs?"));
      expect(summary && daysBack(summary.url)).toBe(7);
      expect(list && daysBack(list.url)).toBe(7);
    });
  },
};

const ranged = recording(loaded);

/**
 * A custom range of days comes from the address and reads the tiles and the
 * list over those days (#3407): the start of the first day up to the start of
 * the day after the last, and the picker shows the two days.
 */
export const ACustomRangeComesFromTheAddress: Story = {
  render: () => (
    <Harness
      fetchStub={ranged.stub}
      route="/mcp-logs?window=custom&since=2026-10-01&until=2026-10-07"
    >
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("combobox", { name: "Time window" })).toHaveValue(
      "Custom range…",
    );
    await expect(canvas.getByLabelText("From")).toHaveValue("2026-10-01");
    await expect(canvas.getByLabelText("To")).toHaveValue("2026-10-07");
    await waitFor(() => {
      for (const path of ["/mcp/logs/summary", "/mcp/logs?"]) {
        const call = ranged.calls.find((c) => c.url.includes(path));
        const q = new URL(call?.url ?? "", "http://x").searchParams;
        expect(q.get("since")).toBe(new Date(2026, 9, 1).toISOString());
        expect(q.get("until")).toBe(new Date(2026, 9, 8).toISOString());
      }
    });
  },
};

const picking = recording(loaded);

/** #3300: picking another window writes the address and reads it again, from the first page. */
export const PickingAWindowWritesTheAddress: Story = {
  render: () => (
    <Harness fetchStub={picking.stub} route="/mcp-logs?server=github">
      <McpLogs />
      <Where />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await pickOption(await canvas.findByRole("combobox", { name: "Time window" }), "Last 30 days");
    await waitFor(() =>
      expect(canvas.getByTestId("where")).toHaveTextContent("server=github&window=30d"),
    );
    await waitFor(() =>
      expect(
        picking.calls.some((c) => c.url.includes("/mcp/logs?") && daysBack(c.url) === 30),
      ).toBe(true),
    );
    // back on the default, the parameter leaves the address
    await pickOption(canvas.getByRole("combobox", { name: "Time window" }), "Last 24 hours");
    await waitFor(() => expect(canvas.getByTestId("where")).toHaveTextContent(/^\?server=github$/));
  },
};

// a notification awaits no reply, and a legacy `sse` server answers a call
// `202` and sends the reply on its stream (#3529)
const NOTIFICATION: Partial<McpLogRow> = {
  event_id: "evt-note",
  tool: "notify_build",
  status: "success",
  notification: 1,
};
const ACCEPTED: Partial<McpLogRow> = {
  event_id: "evt-accepted",
  tool: "list_repos",
  transport: "sse",
  status: "success",
  outcome: "accepted",
};

const notAwaited = routes([
  [
    "/mcp/logs/summary",
    () => ({
      data: [{ ...SUMMARY, not_awaited: 5, successes: 120, failures: 3, failure_rate: 0.0244 }],
    }),
  ],
  [
    "/mcp/logs/evt-accepted",
    () => ({ ...call(ACCEPTED), notification: 0, arguments: null, result: null }),
  ],
  ["/mcp/logs", () => ({ data: [call(), call(NOTIFICATION), call(ACCEPTED)], next_cursor: null })],
]);

/**
 * A call no reply was awaited for says so in its row and its detail, and the
 * summary counts it apart from the failure rate rather than as a success
 * (#3526, #3529).
 */
export const CallsNoReplyWasAwaitedForAreMarkedAndCountedApart: Story = {
  render: () => (
    <Harness fetchStub={notAwaited}>
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const copy = en.pages.mcpLogs;
    await canvas.findByText(copy.notAwaited.notification);
    await expect(canvas.getByText(copy.notAwaited.accepted)).toBeInTheDocument();
    // the accepted call reads as accepted, not as the success stored beside it
    await expect(canvas.getByText(copy.statuses.accepted)).toBeInTheDocument();
    await expect(canvas.getByText("5 not awaited, left out of the rates")).toBeInTheDocument();
    await expect(canvas.getByText("2.4% of awaited calls")).toBeInTheDocument();
    await expectListTable(canvasElement, "MCP Logs");
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open call details for list_repos/ }),
    );
    const drawer = within(await canvas.findByRole("complementary", { name: "MCP call details" }));
    await expect(await drawer.findByText(copy.notAwaitedDetail.accepted)).toBeInTheDocument();
  },
};

/** The same marks in the dashboard's language. */
export const CallsNoReplyWasAwaitedForInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={notAwaited}>
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const copy = ru.pages.mcpLogs;
    await canvas.findByText(copy.notAwaited.notification);
    await expect(canvas.getByText(copy.notAwaited.accepted)).toBeInTheDocument();
    await expect(canvas.getByText(copy.statuses.accepted)).toBeInTheDocument();
  },
};

/**
 * A body forwarded unread to an ungoverned server is one row with no tool: it
 * says its calls are not itemised, in the row and in the detail (#3528).
 */
export const ABodyForwardedUnreadIsOneRowWithNoTool: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/mcp/logs/summary", () => ({ data: [{ ...SUMMARY, not_awaited: 1 }] })],
        [
          "/mcp/logs/evt-unread",
          () => ({
            ...call({ event_id: "evt-unread", tool: "", outcome: "unread" }),
            arguments: null,
            result: null,
          }),
        ],
        [
          "/mcp/logs",
          () => ({
            data: [call({ event_id: "evt-unread", tool: "", outcome: "unread" })],
            next_cursor: null,
          }),
        ],
      ])}
    >
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const copy = en.pages.mcpLogs;
    await canvas.findByText(copy.notAwaited.unread);
    await expect(canvas.getByText(copy.statuses.unread)).toBeInTheDocument();
    await expect(canvas.getByText(copy.noTool)).toBeInTheDocument();
    await expect(canvas.getByText("1 not awaited, left out of the rates")).toBeInTheDocument();
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open call details for not itemised/ }),
    );
    const drawer = within(await canvas.findByRole("complementary", { name: "MCP call details" }));
    await expect(await drawer.findByText(copy.notAwaitedDetail.unread)).toBeInTheDocument();
  },
};

/**
 * A notification the server refused was not delivered: its detail says the
 * summary counts it as a failure, not that it is left out.
 */
export const AFailedNotificationCountsAsAFailure: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/mcp/logs/summary", () => ({ data: [SUMMARY] })],
        [
          "/mcp/logs/evt-note",
          () => ({
            ...call({ ...NOTIFICATION, status: "transport_error" }),
            arguments: null,
            result: null,
          }),
        ],
        [
          "/mcp/logs",
          () => ({
            data: [call({ ...NOTIFICATION, status: "transport_error" })],
            next_cursor: null,
          }),
        ],
      ])}
    >
      <McpLogs />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const copy = en.pages.mcpLogs;
    await canvas.findByText(copy.notAwaited.notification);
    await userEvent.click(
      await canvas.findByRole("button", { name: /Open call details for notify_build/ }),
    );
    const drawer = within(await canvas.findByRole("complementary", { name: "MCP call details" }));
    await expect(
      await drawer.findByText(copy.notAwaitedDetail.notificationFailed),
    ).toBeInTheDocument();
    await expect(drawer.queryByText(copy.notAwaitedDetail.notification)).toBeNull();
  },
};
