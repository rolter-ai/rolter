import { describe, expect, it } from "bun:test";

import type { ClusterNodeRow } from "@/lib/api";
import { gatewayPickup } from "@/lib/gateway-pickup";

const node = (over: Partial<ClusterNodeRow>): ClusterNodeRow => ({
  id: "gw-1",
  role: "gateway",
  build_version: "0.9.0",
  config_version: 12,
  desired_state: "active",
  state_changed_at: "2026-09-01T00:00:00Z",
  first_seen_at: "2026-09-01T00:00:00Z",
  last_seen_at: "2026-09-01T00:00:00Z",
  live: true,
  converged: true,
  ...over,
});

describe("what the gateways report after a save", () => {
  it("waits while the inventory has not answered", () => {
    expect(gatewayPickup(undefined, false)).toEqual({ state: "checking" });
  });

  it("says nothing is known when the read failed", () => {
    expect(gatewayPickup(undefined, true)).toEqual({ state: "unavailable" });
    expect(gatewayPickup([node({})], true)).toEqual({ state: "unavailable" });
  });

  it("says nothing is known when the answer is not an inventory", () => {
    expect(gatewayPickup({ auth_bypass_routes: [] } as never, false)).toEqual({
      state: "unavailable",
    });
  });

  it("reports every live gateway converged", () => {
    expect(gatewayPickup([node({ id: "a" }), node({ id: "b" })], false)).toEqual({
      state: "converged",
      total: 2,
    });
  });

  it("counts the live gateways that have not picked it up", () => {
    expect(
      gatewayPickup(
        [
          node({ id: "a" }),
          node({ id: "b", converged: false }),
          node({ id: "c", converged: false }),
        ],
        false,
      ),
    ).toEqual({ state: "lagging", converged: 1, total: 3 });
  });

  it("does not wait on a gateway that stopped polling", () => {
    expect(
      gatewayPickup(
        [node({ id: "a" }), node({ id: "gone", live: false, converged: false })],
        false,
      ),
    ).toEqual({ state: "converged", total: 1 });
  });

  it("does not count the control plane as a gateway", () => {
    expect(
      gatewayPickup(
        [node({ id: "a" }), node({ id: "cp", role: "control", converged: false })],
        false,
      ),
    ).toEqual({ state: "converged", total: 1 });
  });

  it("says so when no live gateway has reported", () => {
    expect(gatewayPickup([], false)).toEqual({ state: "none" });
    expect(gatewayPickup([node({ live: false }), node({ role: "control" })], false)).toEqual({
      state: "none",
    });
  });
});
