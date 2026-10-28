import { describe, expect, test } from "bun:test";

import {
  argumentsSkeleton,
  connectionView,
  failureLabel,
  parseToolArguments,
  prettyPayload,
  probeLabel,
  probeTone,
  truncatedPayload,
  type McpContactFields,
} from "@/lib/mcp-connection";

const none: McpContactFields = {
  last_contact_at: null,
  last_protocol_version: null,
  last_server_name: null,
  last_error_kind: null,
  last_error_at: null,
};
const OLD = "2026-09-12T08:00:00Z";
const NEW = "2026-09-13T08:00:00Z";

describe("connectionView", () => {
  test("neither field set is never checked", () => {
    expect(connectionView(none)).toMatchObject({ state: "unchecked", tone: "neutral", at: null });
  });
  test("a contact alone is reachable at that time", () => {
    expect(connectionView({ ...none, last_contact_at: OLD })).toEqual({
      state: "reachable",
      tone: "success",
      at: OLD,
      failure: null,
      earlier: null,
    });
  });
  test("a failure alone is failing, named by its kind", () => {
    expect(
      connectionView({ ...none, last_error_kind: "auth_rejected", last_error_at: OLD }),
    ).toMatchObject({ state: "failing", failure: "auth_rejected", at: OLD });
  });
  test("a failure newer than the contact is the current state", () => {
    expect(
      connectionView({
        ...none,
        last_contact_at: OLD,
        last_error_kind: "unreachable",
        last_error_at: NEW,
      }),
    ).toMatchObject({ state: "failing", failure: "unreachable", at: NEW, earlier: null });
  });
  test("a contact newer than the failure is reachable and keeps the failure as history", () => {
    expect(
      connectionView({
        ...none,
        last_contact_at: NEW,
        last_error_kind: "egress_denied",
        last_error_at: OLD,
      }),
    ).toMatchObject({
      state: "reachable",
      at: NEW,
      earlier: { failure: "egress_denied", at: OLD },
    });
  });
  test("a tie reads as reachable", () => {
    expect(
      connectionView({
        ...none,
        last_contact_at: OLD,
        last_error_kind: "unreachable",
        last_error_at: OLD,
      }).state,
    ).toBe("reachable");
  });
  test("a kind this build does not know is still a failure", () => {
    expect(
      connectionView({ ...none, last_error_kind: "brand_new" as never, last_error_at: OLD }),
    ).toMatchObject({ state: "failing", failure: "unknown" });
  });
  test("a kind with no time cannot be placed, so it is ignored", () => {
    expect(connectionView({ ...none, last_error_kind: "unreachable" }).state).toBe("unchecked");
  });
});

describe("probe vocabulary", () => {
  test("every status has its own label, and unknown codes fall back", () => {
    expect(probeLabel("tool_errored")).toBe("tool_errored");
    expect(probeLabel("needs_session")).toBe("needs_session");
    expect(probeLabel("brand_new")).toBe("unknown");
    expect(probeLabel(null)).toBe("unknown");
    expect(failureLabel("unknown")).toBe("unknown");
  });
  test("a tool that errored is not toned like a connection failure", () => {
    expect(probeTone("tool_errored")).not.toBe(probeTone("unreachable"));
    expect(probeTone("success")).toBe("success");
    expect(probeTone("brand_new")).toBe("neutral");
  });
});

describe("parseToolArguments", () => {
  test("blank is no arguments", () => {
    expect(parseToolArguments("  \n")).toEqual({ ok: true, value: {} });
  });
  test("an object passes", () => {
    expect(parseToolArguments('{"q":"x"}')).toEqual({ ok: true, value: { q: "x" } });
  });
  test("invalid JSON is refused with the parser's detail", () => {
    const parsed = parseToolArguments('{"q":');
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) {
      expect(parsed.reason).toBe("invalid_json");
      expect(parsed.detail).not.toBe("");
    }
  });
  test.each(["[1]", "3", "null", '"s"'])("%s parses but is not an object", (text) => {
    expect(parseToolArguments(text)).toMatchObject({ ok: false, reason: "not_object" });
  });
});

describe("argumentsSkeleton", () => {
  test("required properties get the empty value of their type", () => {
    const schema = {
      type: "object",
      required: ["query", "limit", "deep", "tags", "opts", "odd"],
      properties: {
        query: { type: "string" },
        limit: { type: "integer" },
        deep: { type: "boolean" },
        tags: { type: "array" },
        opts: { type: "object" },
        extra: { type: "string" },
      },
    };
    expect(JSON.parse(argumentsSkeleton(schema))).toEqual({
      query: "",
      limit: 0,
      deep: false,
      tags: [],
      opts: {},
      odd: "",
    });
  });
  test("anything else is an empty object", () => {
    expect(argumentsSkeleton(null)).toBe("{}");
    expect(argumentsSkeleton({ type: "object" })).toBe("{}");
    expect(argumentsSkeleton("nope")).toBe("{}");
  });
});

describe("payloads", () => {
  test("a truncated response is recognised by its shape", () => {
    expect(truncatedPayload({ truncated: true, size_bytes: 9000, preview: "{" })).toEqual({
      sizeBytes: 9000,
    });
    expect(truncatedPayload({ truncated: false, size_bytes: 1 })).toBeNull();
    expect(truncatedPayload(null)).toBeNull();
    expect(truncatedPayload({ result: {} })).toBeNull();
  });
  test("an unrecorded message has no text", () => {
    expect(prettyPayload(null)).toBeNull();
    expect(prettyPayload({ a: 1 })).toBe('{\n  "a": 1\n}');
  });
});
