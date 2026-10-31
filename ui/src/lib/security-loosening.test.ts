import { describe, expect, it } from "bun:test";

import { loosenings, type SecurityPolicy } from "@/lib/security-loosening";

const CLOSED: SecurityPolicy = {
  authBypassRoutes: ["/v1/models"],
  requiredHeaders: ["x-tenant", "x-mesh-id"],
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
        ...CLOSED,
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

describe("a save that removes a required header", () => {
  it("names each header that is gone, after the routes added", () => {
    expect(
      loosenings(CLOSED, {
        authBypassRoutes: ["/v1/models", "/v1/ping"],
        requiredHeaders: [],
      }),
    ).toEqual([
      { kind: "bypassRoute", route: "/v1/ping" },
      { kind: "requiredHeader", name: "x-tenant" },
      { kind: "requiredHeader", name: "x-mesh-id" },
    ]);
  });

  it("names only the one that went", () => {
    expect(loosenings(CLOSED, { ...CLOSED, requiredHeaders: ["x-mesh-id"] })).toEqual([
      { kind: "requiredHeader", name: "x-tenant" },
    ]);
  });

  it("compares names without regard to case, as the gateway does", () => {
    expect(loosenings(CLOSED, { ...CLOSED, requiredHeaders: ["X-Tenant", "X-MESH-ID"] })).toEqual(
      [],
    );
  });

  it("counts a header listed twice once", () => {
    const twice: SecurityPolicy = { ...CLOSED, requiredHeaders: ["x-tenant", "X-Tenant"] };
    expect(loosenings(twice, { ...twice, requiredHeaders: [] })).toEqual([
      { kind: "requiredHeader", name: "x-tenant" },
    ]);
  });
});

describe("a save that does not loosen", () => {
  it("is silent when a header is required or its value changes", () => {
    expect(
      loosenings(CLOSED, { ...CLOSED, requiredHeaders: ["x-tenant", "x-mesh-id", "x-waf"] }),
    ).toEqual([]);
  });

  it("is silent when a required header is renamed in case only", () => {
    expect(loosenings(CLOSED, { ...CLOSED, requiredHeaders: ["X-Tenant", "x-mesh-id"] })).toEqual(
      [],
    );
  });

  it("is silent when nothing changed", () => {
    expect(loosenings(CLOSED, CLOSED)).toEqual([]);
  });

  it("is silent when an empty list stays empty", () => {
    const open: SecurityPolicy = { authBypassRoutes: [], requiredHeaders: [] };
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
