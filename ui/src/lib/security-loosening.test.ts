import { describe, expect, it } from "bun:test";

import { loosenings, type SecurityPolicy } from "@/lib/security-loosening";

const CLOSED: SecurityPolicy = {
  authBypassRoutes: ["/v1/models"],
};

describe("a save that loosens", () => {
  it("names each bypass route that is new", () => {
    expect(
      loosenings(CLOSED, {
        ...CLOSED,
        authBypassRoutes: ["/v1/models", "/v1/ping", "/v1/embeddings"],
      }),
    ).toEqual([
      { kind: "bypassRoute", route: "/v1/ping" },
      { kind: "bypassRoute", route: "/v1/embeddings" },
    ]);
  });

  it("lists exactly what opened, in the order typed, and nothing else", () => {
    expect(
      loosenings(CLOSED, {
        authBypassRoutes: ["/v1/embeddings", "/v1/models", "/v1/ping"],
      }),
    ).toEqual([
      { kind: "bypassRoute", route: "/v1/embeddings" },
      { kind: "bypassRoute", route: "/v1/ping" },
    ]);
  });

  it("counts a route typed twice once", () => {
    expect(
      loosenings(CLOSED, { ...CLOSED, authBypassRoutes: ["/v1/models", "/v1/ping", "/v1/ping"] }),
    ).toEqual([{ kind: "bypassRoute", route: "/v1/ping" }]);
  });
});

describe("a save that does not loosen", () => {
  it("is silent when nothing changed", () => {
    expect(loosenings(CLOSED, CLOSED)).toEqual([]);
  });

  it("is silent when an empty list stays empty", () => {
    const open: SecurityPolicy = { authBypassRoutes: [] };
    expect(loosenings(open, open)).toEqual([]);
  });

  it("is silent when a bypass route is taken away", () => {
    expect(loosenings(CLOSED, { ...CLOSED, authBypassRoutes: [] })).toEqual([]);
  });

  it("is silent when a route that was already exempt stays so, in any order", () => {
    const two: SecurityPolicy = { ...CLOSED, authBypassRoutes: ["/v1/models", "/v1/ping"] };
    expect(loosenings(two, { ...two, authBypassRoutes: ["/v1/ping", "/v1/models"] })).toEqual([]);
  });

  it("is silent when a route is swapped for one already exempt elsewhere in the list", () => {
    const two: SecurityPolicy = { ...CLOSED, authBypassRoutes: ["/v1/models", "/v1/ping"] };
    expect(loosenings(two, { ...two, authBypassRoutes: ["/v1/models"] })).toEqual([]);
  });

  it("compares routes exactly, as the gateway matches them", () => {
    expect(loosenings(CLOSED, { ...CLOSED, authBypassRoutes: ["/v1/models/"] })).toEqual([
      { kind: "bypassRoute", route: "/v1/models/" },
    ]);
  });
});
