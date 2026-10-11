import { describe, expect, test } from "bun:test";

import { blankTelemetry, kvPayload, lmPayload, telemetryFieldOf } from "./provider-telemetry";

describe("telemetryFieldOf", () => {
  test("reads a dotted key", () => {
    expect(telemetryFieldOf("provider 'x' kv_events.endpoint must be a non-empty tcp:// URL")).toBe(
      "kvEndpoint",
    );
    expect(telemetryFieldOf("provider 'x' lmcache.endpoint must be an http(s) URL")).toBe(
      "lmEndpoint",
    );
  });
  test("flags the first key a range refusal lists", () => {
    expect(
      telemetryFieldOf(
        "provider 'x' kv_events max_blocks and stale_secs must be greater than zero",
      ),
    ).toBe("kvMaxBlocks");
    expect(
      telemetryFieldOf(
        "provider 'x' lmcache refresh_secs and stale_secs must be greater than zero",
      ),
    ).toBe("lmRefreshSecs");
  });
  test("ignores a message about something else", () => {
    expect(telemetryFieldOf("the api key was rejected upstream")).toBeNull();
  });
});

describe("payloads", () => {
  test("blank numbers are left to the defaults", () => {
    const d = { ...blankTelemetry(), kvEndpoint: " tcp://vllm:5557 ", lmEndpoint: "http://lm" };
    expect(kvPayload(d)).toEqual({ endpoint: "tcp://vllm:5557" });
    expect(lmPayload(d)).toEqual({ endpoint: "http://lm" });
  });
  test("typed numbers are sent as numbers, zero included", () => {
    const d = { ...blankTelemetry(), kvEndpoint: "tcp://a:1", kvMaxBlocks: "0", kvTopic: "t" };
    expect(kvPayload(d)).toEqual({ endpoint: "tcp://a:1", topic: "t", max_blocks: 0 });
  });
});
