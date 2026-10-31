import { afterEach, describe, expect, it } from "bun:test";

import { GatewayProbeError, fetchGatewayReadiness, readinessFrom } from "@/lib/gateway";
import {
  GATEWAY_HEALTH_POLL_MS,
  GATEWAY_HEALTH_STALE_MS,
  fetchGatewayCheck,
  fleetHealth,
  gatewayHealthFrom,
  type GatewayHealthQuery,
} from "@/lib/gateway-health";

const originalFetch = globalThis.fetch;
const originalStorage = globalThis.localStorage;

afterEach(() => {
  globalThis.fetch = originalFetch;
  globalThis.localStorage = originalStorage;
});

describe("readinessFrom", () => {
  it("reads the gateway's own answers", () => {
    expect(readinessFrom(200, "ok")).toBe("ready");
    expect(readinessFrom(200, "ok\n")).toBe("ready");
    expect(readinessFrom(503, "draining")).toBe("draining");
  });

  it("reads the answers of a gateway that has no config yet (#3348, #3350)", () => {
    expect(readinessFrom(503, "awaiting_snapshot")).toBe("awaitingSnapshot");
    expect(readinessFrom(503, "snapshot_format_unsupported")).toBe("snapshotFormatUnsupported");
    // a body that only looks like a key of the table is still unknown
    expect(readinessFrom(503, "toString")).toBeNull();
  });

  it("reads the /gw proxy's own 502 as an unreachable gateway", () => {
    const body = JSON.stringify({ error: { message: "gateway unreachable: connection refused" } });
    expect(readinessFrom(502, body)).toBe("unreachable");
  });

  it("does not take an SPA fallback's 200 for a ready gateway", () => {
    expect(readinessFrom(200, "<!doctype html><html></html>")).toBeNull();
    expect(readinessFrom(200, "[]")).toBeNull();
  });

  it("does not blame the gateway for a 502 from in front of the control plane", () => {
    expect(readinessFrom(502, "<html><body>502 Bad Gateway</body></html>")).toBeNull();
    expect(readinessFrom(502, JSON.stringify({ message: "upstream error" }))).toBeNull();
  });

  it("leaves every other answer unknown", () => {
    expect(readinessFrom(503, "service unavailable")).toBeNull();
    expect(readinessFrom(401, "")).toBeNull();
    expect(readinessFrom(404, "not found")).toBeNull();
    expect(readinessFrom(504, "")).toBeNull();
  });
});

describe("fetchGatewayReadiness", () => {
  it("asks /gw/readyz with the dashboard session, and nothing else", async () => {
    globalThis.localStorage = {
      getItem: (k: string) => (k === "rolter.session.token" ? "sess-1" : null),
    } as unknown as Storage;
    let sent: { url: string; init?: RequestInit } | undefined;
    globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
      sent = { url: String(input), init };
      return new Response("ok", { status: 200 });
    }) as unknown as typeof fetch;

    expect(await fetchGatewayReadiness()).toBe("ready");
    expect(sent?.url).toBe("/gw/readyz");
    expect(sent?.init?.headers).toEqual({ Authorization: "Bearer sess-1" });
    expect(sent?.init?.cache).toBe("no-store");
  });

  it("sends no Authorization in open mode", async () => {
    globalThis.localStorage = { getItem: () => null } as unknown as Storage;
    let headers: HeadersInit | undefined;
    globalThis.fetch = (async (_input: RequestInfo | URL, init?: RequestInit) => {
      headers = init?.headers;
      return new Response("ok", { status: 200 });
    }) as unknown as typeof fetch;
    await fetchGatewayReadiness();
    expect(headers).toEqual({});
  });

  it("throws on an answer that is not the gateway's", async () => {
    globalThis.fetch = (async () =>
      new Response("<html></html>", { status: 502 })) as unknown as typeof fetch;
    const error = await fetchGatewayReadiness().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(GatewayProbeError);
    expect((error as GatewayProbeError).status).toBe(502);
  });

  it("throws with status 0 when there was no response at all", async () => {
    globalThis.fetch = (async () => {
      throw new TypeError("Failed to fetch");
    }) as unknown as typeof fetch;
    const error = await fetchGatewayReadiness().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(GatewayProbeError);
    expect((error as GatewayProbeError).status).toBe(0);
  });

  it("gives up when the caller does", async () => {
    globalThis.fetch = ((_input: RequestInfo | URL, init?: RequestInit) =>
      new Promise<Response>((_resolve, reject) => {
        init?.signal?.addEventListener("abort", () => reject(new DOMException("", "AbortError")));
      })) as typeof fetch;
    const controller = new AbortController();
    const probe = fetchGatewayReadiness(controller.signal).catch((e: unknown) => e);
    controller.abort();
    expect(await probe).toBeInstanceOf(GatewayProbeError);
  });
});

const T0 = 1_800_000_000_000;

function query(partial: Partial<GatewayHealthQuery>): GatewayHealthQuery {
  return { data: undefined, dataUpdatedAt: 0, errorUpdatedAt: 0, isError: false, ...partial };
}

describe("gatewayHealthFrom", () => {
  it("is checking until the first answer, and unknown if that answer failed", () => {
    expect(gatewayHealthFrom(query({}))).toEqual({
      health: "checking",
      live: false,
      answeredAt: null,
      fleet: null,
      refusal: null,
    });
    expect(gatewayHealthFrom(query({ isError: true, errorUpdatedAt: T0 }))).toEqual({
      health: "unknown",
      live: false,
      answeredAt: null,
      fleet: null,
      refusal: null,
    });
  });

  it("maps each gateway answer onto a state, live", () => {
    const at = { dataUpdatedAt: T0 };
    expect(gatewayHealthFrom(query({ ...at, data: "ready" }))).toEqual({
      health: "healthy",
      live: true,
      answeredAt: T0,
      fleet: null,
      refusal: null,
    });
    expect(gatewayHealthFrom(query({ ...at, data: "draining" })).health).toBe("degraded");
    expect(gatewayHealthFrom(query({ ...at, data: "unreachable" })).health).toBe("down");
  });

  it("reads a gateway with no config as down, and says why", () => {
    const at = { dataUpdatedAt: T0 };
    for (const data of ["awaitingSnapshot", "snapshotFormatUnsupported"] as const) {
      const view = gatewayHealthFrom(query({ ...at, data }));
      expect(view.health).toBe("down");
      expect(view.refusal).toBe(data);
    }
    expect(gatewayHealthFrom(query({ ...at, data: "unreachable" })).refusal).toBeNull();
  });

  it("holds a known answer through one failed check, without calling it live", () => {
    const view = gatewayHealthFrom(
      query({
        data: "ready",
        dataUpdatedAt: T0,
        isError: true,
        errorUpdatedAt: T0 + GATEWAY_HEALTH_POLL_MS,
      }),
    );
    expect(view).toEqual({
      health: "healthy",
      live: false,
      answeredAt: T0,
      fleet: null,
      refusal: null,
    });
  });

  it("admits it does not know once the answer is older than the grace window", () => {
    const view = gatewayHealthFrom(
      query({
        data: "ready",
        dataUpdatedAt: T0,
        isError: true,
        errorUpdatedAt: T0 + GATEWAY_HEALTH_STALE_MS,
      }),
    );
    expect(view).toEqual({
      health: "unknown",
      live: false,
      answeredAt: T0,
      fleet: null,
      refusal: null,
    });
  });

  it("gives up on the second failed poll, not the third", () => {
    // a slow answer (5s) followed by two fast failures, 30s apart
    const answered = T0 + 5_000;
    const second = T0 + 2 * GATEWAY_HEALTH_POLL_MS + 100;
    expect(
      gatewayHealthFrom(
        query({ data: "ready", dataUpdatedAt: answered, isError: true, errorUpdatedAt: second }),
      ).health,
    ).toBe("unknown");
  });

  it("is live again once a check succeeds after a failure", () => {
    const view = gatewayHealthFrom(
      query({
        data: "unreachable",
        dataUpdatedAt: T0 + 60_000,
        isError: false,
        errorUpdatedAt: T0 + 30_000,
      }),
    );
    expect(view).toEqual({
      health: "down",
      live: true,
      answeredAt: T0 + 60_000,
      fleet: null,
      refusal: null,
    });
  });
});

describe("fleetHealth", () => {
  it("is healthy when every live gateway is current and none is stale", () => {
    expect(fleetHealth({ live: 3, stale: 0, lagging: 0 })).toBe("healthy");
  });

  it("is degraded while a live gateway lags or one has gone stale", () => {
    expect(fleetHealth({ live: 3, stale: 0, lagging: 1 })).toBe("degraded");
    expect(fleetHealth({ live: 2, stale: 1, lagging: 0 })).toBe("degraded");
  });

  it("is down only when no gateway is live", () => {
    expect(fleetHealth({ live: 0, stale: 2, lagging: 0 })).toBe("down");
  });
});

describe("gatewayHealthFrom on a fleet answer", () => {
  it("carries the counts, and holds them through a failed check like a probe", () => {
    const fleet = { live: 3, stale: 0, lagging: 1 };
    expect(gatewayHealthFrom(query({ data: fleet, dataUpdatedAt: T0 }))).toEqual({
      health: "degraded",
      live: true,
      answeredAt: T0,
      fleet,
      refusal: null,
    });
    const held = gatewayHealthFrom(
      query({
        data: fleet,
        dataUpdatedAt: T0,
        isError: true,
        errorUpdatedAt: T0 + GATEWAY_HEALTH_POLL_MS,
      }),
    );
    expect(held.live).toBe(false);
    expect(held.fleet).toEqual(fleet);
    const lost = gatewayHealthFrom(
      query({
        data: fleet,
        dataUpdatedAt: T0,
        isError: true,
        errorUpdatedAt: T0 + GATEWAY_HEALTH_STALE_MS,
      }),
    );
    expect(lost).toEqual({
      health: "unknown",
      live: false,
      answeredAt: T0,
      fleet: null,
      refusal: null,
    });
  });
});

describe("fetchGatewayCheck", () => {
  const summary = (body: unknown, status = 200) =>
    new Response(JSON.stringify(body), {
      status,
      headers: { "Content-Type": "application/json" },
    });
  /** answer the summary with `first`, and `/gw/readyz` with `ok`; remember what was asked */
  function stub(first: () => Response) {
    const asked: string[] = [];
    globalThis.fetch = (async (input: RequestInfo | URL) => {
      const path = new URL(String(input), "http://localhost").pathname;
      asked.push(path);
      return path === "/api/v1/cluster/summary" ? first() : new Response("ok", { status: 200 });
    }) as unknown as typeof fetch;
    return asked;
  }
  const counted = {
    inventory: true,
    gateways: { live: 3, stale: 1, lagging: 2 },
    config_version: 412,
    checked_at: "2026-10-27T09:14:02.511Z",
  };

  it("answers with the fleet's counts and never probes when the summary counted gateways", async () => {
    const asked = stub(() => summary(counted));
    expect(await fetchGatewayCheck()).toEqual({ live: 3, stale: 1, lagging: 2 });
    expect(asked).toEqual(["/api/v1/cluster/summary"]);
  });

  it("probes /readyz when the control plane has no inventory", async () => {
    const asked = stub(() =>
      summary({ ...counted, inventory: false, gateways: { live: 0, stale: 0, lagging: 0 } }),
    );
    expect(await fetchGatewayCheck()).toBe("ready");
    expect(asked).toEqual(["/api/v1/cluster/summary", "/gw/readyz"]);
  });

  it("probes when the inventory holds no gateway at all", async () => {
    // a gateway that sends no node id never enters the inventory
    stub(() => summary({ ...counted, gateways: { live: 0, stale: 0, lagging: 0 } }));
    expect(await fetchGatewayCheck()).toBe("ready");
  });

  it("probes when an older control plane answers the summary with a 404", async () => {
    const asked = stub(() => summary({ error: { message: "not found" } }, 404));
    expect(await fetchGatewayCheck()).toBe("ready");
    expect(asked).toEqual(["/api/v1/cluster/summary", "/gw/readyz"]);
  });

  it("probes when the summary request itself fails", async () => {
    const asked = stub(() => {
      throw new TypeError("Failed to fetch");
    });
    expect(await fetchGatewayCheck()).toBe("ready");
    expect(asked).toContain("/gw/readyz");
  });
});
