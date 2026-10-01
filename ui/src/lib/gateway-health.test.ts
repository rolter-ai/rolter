import { afterEach, describe, expect, it } from "bun:test";

import { GatewayProbeError, fetchGatewayReadiness, readinessFrom } from "@/lib/gateway";
import {
  GATEWAY_HEALTH_POLL_MS,
  GATEWAY_HEALTH_STALE_MS,
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
    });
    expect(gatewayHealthFrom(query({ isError: true, errorUpdatedAt: T0 }))).toEqual({
      health: "unknown",
      live: false,
      answeredAt: null,
    });
  });

  it("maps each gateway answer onto a state, live", () => {
    const at = { dataUpdatedAt: T0 };
    expect(gatewayHealthFrom(query({ ...at, data: "ready" }))).toEqual({
      health: "healthy",
      live: true,
      answeredAt: T0,
    });
    expect(gatewayHealthFrom(query({ ...at, data: "draining" })).health).toBe("degraded");
    expect(gatewayHealthFrom(query({ ...at, data: "unreachable" })).health).toBe("down");
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
    expect(view).toEqual({ health: "healthy", live: false, answeredAt: T0 });
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
    expect(view).toEqual({ health: "unknown", live: false, answeredAt: T0 });
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
    expect(view).toEqual({ health: "down", live: true, answeredAt: T0 + 60_000 });
  });
});
