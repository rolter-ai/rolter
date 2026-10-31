import { describe, expect, it } from "bun:test";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

const UI = join(import.meta.dir, "..");
const CORE = readFileSync(join(UI, "..", "crates", "rolter-core", "src", "mcp_log.rs"), "utf8");
const SCREEN = readFileSync(join(UI, "src", "pages", "McpLogs.tsx"), "utf8");
const LOCALES = join(UI, "src", "lib", "i18n", "locales");

function literals(source: string): string[] {
  return [...source.matchAll(/"([a-z_]+)"/g)].map((match) => match[1]!);
}

const outcomes = literals(/pub const MCP_OUTCOMES:.*?= &\[([\s\S]*?)\];/.exec(CORE)?.[1] ?? "");
const statuses = literals(/pub const MCP_STATUSES:.*?= &\[([\s\S]*?)\];/.exec(CORE)?.[1] ?? "");
const notes = literals(/type NotAwaited = ([\s\S]*?);/.exec(SCREEN)?.[1] ?? "");

type Copy = {
  statuses: Record<string, string>;
  notAwaited: Record<string, string>;
  notAwaitedDetail: Record<string, string>;
};

// the UI's hand-written outcome vocabulary must follow the shared wire names
// rather than quietly showing a newly added outcome as an ordinary success
// (#3526, #3528, #3529)
describe("MCP log vocabulary", () => {
  it("recognizes every shared outcome and the notification flag", () => {
    expect(outcomes.length).toBeGreaterThan(0);
    expect(statuses.length).toBeGreaterThan(0);
    expect([...notes].sort()).toEqual(["notification", ...outcomes].sort());
  });

  for (const locale of readdirSync(LOCALES).filter((file) => file.endsWith(".json"))) {
    it(`${locale} words every status, outcome and not-awaited explanation`, () => {
      const copy = JSON.parse(readFileSync(join(LOCALES, locale), "utf8")).pages.mcpLogs as Copy;
      expect(Object.keys(copy.statuses).sort()).toEqual(
        [...statuses, ...outcomes, "not_awaited"].sort(),
      );
      expect(Object.keys(copy.notAwaited).sort()).toEqual(["notification", ...outcomes].sort());
      expect(Object.keys(copy.notAwaitedDetail).sort()).toEqual(
        ["notification", "notificationFailed", ...outcomes].sort(),
      );
    });
  }
});
