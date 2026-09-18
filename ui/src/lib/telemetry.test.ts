import { describe, it, expect, beforeEach } from "bun:test";
import { initTelemetry, isOpenMode, resetTelemetryForTests } from "./telemetry";

describe("browser telemetry", () => {
  beforeEach(() => {
    resetTelemetryForTests();
  });

  it("stays off when no runtime config was injected", async () => {
    // the default for every deployment: the control plane injected nothing, so
    // the dashboard must load no SDK and export nothing
    expect(await initTelemetry(undefined)).toBe(false);
  });

  it("stays off when the config carries no endpoint", async () => {
    // a config block exists (the control plane always writes one) but
    // observability is not configured on this deployment
    expect(await initTelemetry({})).toBe(false);
    expect(await initTelemetry({ otelServiceName: "rolter-ui" })).toBe(false);
  });

  it("stays off outside a browser even with an endpoint", async () => {
    // the web instrumentations touch window/document when constructed. this
    // runs without a DOM, so the guard is what keeps it from throwing — the
    // same guard that protects any future server-side render
    expect(await initTelemetry({ otelEndpoint: "http://localhost:4318/v1/traces" })).toBe(false);
  });
});

describe("open mode", () => {
  it("reports gated whenever the control plane did not say otherwise", () => {
    // the control plane only injects openMode when it is open, so every other
    // shape has to read as gated — a false positive banner on a properly
    // secured deployment would teach operators to ignore it
    expect(isOpenMode(undefined)).toBe(false);
    expect(isOpenMode({})).toBe(false);
    expect(isOpenMode({ version: "1.4.2" })).toBe(false);
    expect(isOpenMode({ openMode: false })).toBe(false);
  });

  it("reports open only on an explicit true", () => {
    expect(isOpenMode({ openMode: true })).toBe(true);
  });
});
