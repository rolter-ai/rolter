import { expect, test, type Page } from "@playwright/test";

import { t } from "../i18n";

// MCP Logs distinguishes "this deployment can't serve these logs" from "the
// request failed" (#569/#663). Both look like a red herring to an operator if
// the UI conflates them: the first is a deployment fact, the second is a fault.
//
// The states are driven by stubbing the API rather than by reshaping the stack —
// a live control plane either has the endpoint or doesn't, so a real backend
// can't produce a 404 and a 500 in the same run.
//
// every headline is read from the catalog rather than copied into the spec, so
// rewording a state moves the assertion with it (#1504)

const LOGS = "**/api/v1/mcp/logs?**";
const SUMMARY = "**/api/v1/mcp/logs/summary**";
const DETAIL = "**/api/v1/mcp/logs/*";

const ROW = {
  event_id: "evt-1",
  ts: "2026-07-25T10:11:12Z",
  server: "files",
  tool: "read_file",
  status: "ok",
  transport: "stdio",
  latency_ms: 12,
};

const SUMMARY_OK = {
  data: [{ calls: 0, failures: 0, avg_latency_ms: null, p95_latency_ms: null }],
};

const RESOURCE = t("errors.resources.mcpLogs");
const UNAVAILABLE = t("errors.load.noAnalytics.title");
const FAILED = t("errors.load.server.title", { resource: RESOURCE });
const EMPTY = t("pages.mcpLogs.emptyTitle");

/**
 * MCP Logs is superadmin-only (#1183), and the seeded tenant user is an org
 * admin, so the screen would render its refusal before any of the states under
 * test. Answer the capability question as a superadmin: what the screen does
 * with the logs endpoint is the subject here, not who may open it.
 */
test.beforeEach(async ({ page }) => {
  // answered outright rather than by patching the live reply: a pass-through
  // can still be in flight when a short test tears the page down
  await page.route("**/api/v1/rbac/effective**", (route) =>
    route.fulfill({
      json: { superadmin: true, role: "admin", allowed: [], custom_roles: [], model_policy: null },
    }),
  );
});

/**
 * Stub the list + summary endpoints with one status.
 *
 * Failures carry the control API's own `{"error": {"message": ...}}` shape — a
 * bare string would quietly exercise the unparsable-body fallback instead of
 * the message the UI is meant to surface, which is most of what is under test.
 */
async function stubList(page: Page, status: number, body: unknown = {}) {
  const json = (payload: unknown) => ({
    status,
    contentType: "application/json",
    body: JSON.stringify(payload),
  });
  // a successful summary has its own shape; on failure both return the error
  await page.route(SUMMARY, (route) => route.fulfill(json(status < 400 ? SUMMARY_OK : body)));
  await page.route(LOGS, (route) => route.fulfill(json(body)));
}

test("an absent endpoint reads as unavailable, not as an error", async ({ page }) => {
  // a control plane too old to serve the route 404s it
  await stubList(page, 404, { error: { message: "not found" } });
  await page.goto("/mcp-logs");

  const alert = page.getByRole("alert");
  await expect(alert.getByText(UNAVAILABLE)).toBeVisible();
  // and names both causes, so the operator knows where to look
  await expect(
    alert.getByText(t("errors.load.noAnalytics.body", { resource: RESOURCE })),
  ).toBeVisible();
  await expect(alert).toContainText("CLICKHOUSE_URL");
  await expect(page.getByText(FAILED)).toHaveCount(0);
});

test("a control plane without clickhouse reads as unavailable too", async ({ page }) => {
  await stubList(page, 503, { error: { message: "clickhouse not configured" } });
  await page.goto("/mcp-logs");

  await expect(page.getByText(UNAVAILABLE)).toBeVisible();
});

test("a real failure reads as an error, leading with plain language", async ({ page }) => {
  await stubList(page, 500, { error: { message: "clickhouse read timed out" } });
  await page.goto("/mcp-logs");

  await expect(page.getByText(FAILED)).toBeVisible();
  // the raw message stays available to diagnose with, just not as the headline
  await expect(page.getByText(/clickhouse read timed out/)).toBeVisible();
  await expect(page.getByText(UNAVAILABLE)).toHaveCount(0);
});

test("a working endpoint with no calls reads as empty, not as broken", async ({ page }) => {
  await stubList(page, 200, { data: [], next_cursor: null });
  await page.goto("/mcp-logs");

  await expect(page.getByText(EMPTY)).toBeVisible();
  await expect(page.getByText(UNAVAILABLE)).toHaveCount(0);
  await expect(page.getByText(FAILED)).toHaveCount(0);
});

test("a missing single event does not condemn the whole page", async ({ page }) => {
  // the distinction #663 turns on: 404 on the *list* means the deployment can't
  // serve these logs, but 404 on one event means that event is gone. the second
  // must not blank the screen the operator is working on
  // registration order matters: Playwright matches the most recently added
  // route first, and `logs/*` also matches `logs/summary`. detail goes on
  // first so the two specific handlers below take precedence over it
  await page.route(DETAIL, (route) =>
    route.fulfill({
      status: 404,
      contentType: "application/json",
      body: JSON.stringify({ error: { message: "event not found" } }),
    }),
  );
  await page.route(SUMMARY, (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        data: [{ calls: 1, failures: 0, avg_latency_ms: 12, p95_latency_ms: 12 }],
      }),
    }),
  );
  await page.route(LOGS, (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ data: [ROW], next_cursor: null }),
    }),
  );

  await page.goto("/mcp-logs");
  await expect(page.getByText("read_file")).toBeVisible();

  await page.getByText("read_file").click();
  // the drawer reports the failure; the table behind it is still there
  await expect(page.getByText(/event not found/)).toBeVisible();
  await expect(page.getByText(UNAVAILABLE)).toHaveCount(0);
  await expect(page.getByText("read_file")).toBeVisible();
});
