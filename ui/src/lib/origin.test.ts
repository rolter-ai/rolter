import { describe, expect, it } from "bun:test";

import { movesOrigin } from "@/lib/origin";

describe("movesOrigin", () => {
  it("keeps another path on the same origin where it is", () => {
    expect(
      movesOrigin("https://otlp.example.com/v1/logs", "https://otlp.example.com/v2/logs"),
    ).toBe(false);
    expect(movesOrigin("https://otlp.example.com/v1/logs", "https://otlp.example.com:443/x")).toBe(
      false,
    );
  });

  it("moves on another scheme, host or port", () => {
    expect(movesOrigin("https://otlp.example.com/v1/logs", "http://otlp.example.com/v1/logs")).toBe(
      true,
    );
    expect(
      movesOrigin("https://otlp.example.com/v1/logs", "https://otlp.example.net/v1/logs"),
    ).toBe(true);
    expect(movesOrigin("https://otlp.example.com/v1/logs", "https://otlp.example.com:8443/")).toBe(
      true,
    );
  });

  it("ignores the whitespace a pasted endpoint carries", () => {
    expect(movesOrigin("https://otlp.example.com/a", "  https://otlp.example.com/b  ")).toBe(false);
  });

  it("moves nothing while either side does not parse", () => {
    expect(movesOrigin("https://otlp.example.com/a", "")).toBe(false);
    expect(movesOrigin("https://otlp.example.com/a", "otlp.example.com")).toBe(false);
    expect(movesOrigin("not a url", "https://otlp.example.com")).toBe(false);
  });
});
