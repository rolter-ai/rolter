import { describe, expect, test } from "bun:test";

import type { AuditLogEntry, AuditLogPage } from "./api";
import {
  auditCsv,
  auditExportName,
  auditExportRow,
  auditJson,
  collectAuditLog,
} from "./audit-export";

const entry = (over: Partial<AuditLogEntry> = {}): AuditLogEntry => ({
  id: "e-1",
  org_id: "org-1",
  actor_user_id: "11111111-1111-1111-1111-111111111111",
  action: "provider.create",
  target_type: "provider",
  target_id: "22222222-2222-2222-2222-222222222222",
  detail: { name: 'a "quoted", name' },
  at: "2026-08-06T10:00:00Z",
  ...over,
});

describe("audit export rows", () => {
  test("carry the whole ids, the e-mail and a UTC instant", () => {
    const row = auditExportRow(entry(), () => "ada@example.com");
    expect(row.actor_email).toBe("ada@example.com");
    expect(row.actor_user_id).toBe("11111111-1111-1111-1111-111111111111");
    expect(row.target_id).toBe("22222222-2222-2222-2222-222222222222");
    expect(row.at).toBe("2026-08-06T10:00:00Z");
  });

  test("csv quotes commas and quotes, and defangs a formula", () => {
    const csv = auditCsv([auditExportRow(entry({ target_id: "=1+1" }), () => undefined)]);
    const [header, line] = csv.split("\r\n");
    expect(header).toBe(
      "at,actor_email,actor_user_id,action,target_type,target_id,org_id,entry_id,detail",
    );
    expect(line).toContain(`'=1+1`);
    expect(line).toContain(`"{""name"":""a \\""quoted\\"", name""}"`);
  });

  test("json keeps detail as a value", () => {
    const parsed = JSON.parse(auditJson([auditExportRow(entry(), () => undefined)]));
    expect(parsed[0].detail).toEqual({ name: 'a "quoted", name' });
  });
});

describe("audit export name", () => {
  test("names the org and the window", () => {
    expect(
      auditExportName({
        org: "Acme Corp",
        from: "2026-07-01T00:00:00.000Z",
        to: "2026-10-01T09:00:00Z",
        format: "csv",
      }),
    ).toBe("audit-log-acme-corp-2026-07-01_to_2026-10-01.csv");
    expect(auditExportName({ org: "o", to: "2026-10-01T00:00:00Z", format: "json" })).toBe(
      "audit-log-o-all_to_2026-10-01.json",
    );
  });
});

describe("collectAuditLog", () => {
  const page = (ids: string[], next: string | null): AuditLogPage => ({
    items: ids.map((id) => entry({ id })),
    next_cursor: next,
    previous_cursor: null,
    has_next: next !== null,
    has_previous: false,
  });

  test("walks the cursor with the filters on every page", async () => {
    const seen: unknown[] = [];
    const out = await collectAuditLog(
      async (q) => {
        seen.push(q);
        return q.cursor ? page(["c"], null) : page(["a", "b"], "n1");
      },
      { action: "route.delete" },
    );
    expect(out.entries.map((e) => e.id)).toEqual(["a", "b", "c"]);
    expect(out.truncated).toBe(false);
    expect(seen).toEqual([
      { action: "route.delete", limit: 500, cursor: undefined },
      { action: "route.delete", limit: 500, cursor: "n1" },
    ]);
  });

  test("stops at the row limit and says so", async () => {
    const out = await collectAuditLog(async () => page(["a", "b"], "more"), {}, 3);
    expect(out.entries).toHaveLength(3);
    expect(out.truncated).toBe(true);
  });
});
