import { describe, expect, test } from "bun:test";

import type { VersionStatus } from "@/lib/api";
import { updateHintFrom } from "@/lib/version";

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
