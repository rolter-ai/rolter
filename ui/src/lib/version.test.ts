import { describe, expect, test } from "bun:test";

import type { VersionStatus } from "@/lib/api";
import { experimentalNavKeysFrom, updateHintFrom } from "@/lib/version";

const status = (over: Partial<VersionStatus> = {}): VersionStatus => ({
  current: "0.1.0",
  latest: "0.2.0",
  release_url: "https://github.com/rolter-ai/rolter/releases/tag/v0.2.0",
  update_available: true,
  checked_at: "2026-09-05T00:00:00Z",
  enabled: true,
  ...over,
});

describe("updateHintFrom", () => {
  test("a newer release yields the hint with its page", () => {
    expect(updateHintFrom(status())).toEqual({
      latest: "0.2.0",
      url: "https://github.com/rolter-ai/rolter/releases/tag/v0.2.0",
    });
  });

  test("a missing url falls back to the releases page", () => {
    expect(updateHintFrom(status({ release_url: null }))?.url).toBe(
      "https://github.com/rolter-ai/rolter/releases/latest",
    );
  });

  test("checking, disabled, offline and current all show nothing", () => {
    expect(updateHintFrom(undefined)).toBeNull();
    expect(updateHintFrom(status({ enabled: false }))).toBeNull();
    expect(updateHintFrom(status({ update_available: false }))).toBeNull();
    expect(
      updateHintFrom(
        status({ latest: null, release_url: null, checked_at: null, update_available: false }),
      ),
    ).toBeNull();
    // a payload that claims an update without naming one is not a hint
    expect(updateHintFrom(status({ latest: null }))).toBeNull();
  });
});

describe("experimentalNavKeysFrom", () => {
  const mcp = {
    id: "mcp_settings",
    stability: "experimental" as const,
    note: "stored but not read by the proxy yet",
    nav_keys: ["mcp-settings"],
  };

  test("each listed subsystem marks every nav key it names", () => {
    const marked = experimentalNavKeysFrom([
      mcp,
      {
        id: "mcp_tool_groups",
        stability: "experimental",
        note: "membership is not an access boundary",
        nav_keys: ["tool-groups", "mcp-catalog"],
      },
    ]);
    expect([...marked.keys()].sort()).toEqual(["mcp-catalog", "mcp-settings", "tool-groups"]);
    expect(marked.get("mcp-settings")).toBe(mcp.note);
  });

  // a control plane that predates #1385, a failed read, or a session still
  // being checked: the rail renders, it just renders no markers
  test("a missing, empty or absent answer marks nothing", () => {
    expect(experimentalNavKeysFrom(undefined).size).toBe(0);
    expect(experimentalNavKeysFrom([]).size).toBe(0);
  });

  // the level travels with each entry precisely so a future one cannot be read
  // as this one just by being in the list
  test("an entry at another level is not treated as experimental", () => {
    const marked = experimentalNavKeysFrom([
      { ...mcp, stability: "deprecated" as unknown as "experimental" },
    ]);
    expect(marked.size).toBe(0);
  });

  // a subsystem with no screen of its own — a gateway surface, or a
  // cross-cutting concept like labels — is documented, not navigated
  test("a subsystem with no nav keys contributes nothing", () => {
    const marked = experimentalNavKeysFrom([
      { id: "realtime", stability: "experimental", note: "unmetered", nav_keys: [] },
    ]);
    expect(marked.size).toBe(0);
  });
});
