import { describe, it, expect } from "bun:test";

import { ACTIONS, CAPABILITIES, allowedFor, effectiveFor, matrixFixture } from "./rbac-capabilities";

// The mirror of the `rbac_matrix` unit tests in
// `crates/rolter-control/src/rbac_matrix.rs`: the fixture is only worth pinning
// to the table if it also *derives* the two payloads the way the control plane
// does, so these assert the same statements about the same rows (#1298).

const has = (allowed: string[], pair: string) => allowed.includes(pair);

describe("effective capabilities", () => {
  it("lets a viewer read everything scoped and write nothing", () => {
    const allowed = allowedFor("viewer");
    expect(has(allowed, "provider:read")).toBe(true);
    expect(has(allowed, "provider:create")).toBe(false);
    expect(has(allowed, "route:update")).toBe(false);
    expect(has(allowed, "my_virtual_key:create")).toBe(false);
  });

  it("lets a member act on their own behalf and nothing more", () => {
    const allowed = allowedFor("member");
    expect(has(allowed, "my_virtual_key:create")).toBe(true);
    expect(has(allowed, "virtual_key:create")).toBe(false);
  });

  it("lets an admin write scoped resources but not deployment policy", () => {
    const allowed = allowedFor("admin");
    expect(has(allowed, "provider:update")).toBe(true);
    expect(has(allowed, "audit_log:read")).toBe(true);
    expect(has(allowed, "feature_flags:update")).toBe(false);
    expect(has(allowed, "runtime_policy:read")).toBe(false);
  });

  it("keeps the deployment-wide catalogs read-only for everyone below a superadmin", () => {
    // the drift #1258 hit: an admin who could create a model or a price would
    // have made two gates pass against a control plane that defines neither
    const allowed = allowedFor("admin");
    expect(has(allowed, "model:read")).toBe(true);
    expect(has(allowed, "model_price:read")).toBe(true);
    expect(has(allowed, "model_price:update")).toBe(false);
    expect(has(allowed, "model:delete")).toBe(false);
  });

  it("gives a caller with no membership the global catalogs alone", () => {
    expect(allowedFor(null)).toEqual(
      CAPABILITIES.flatMap((c) =>
        ACTIONS.filter((a) => c[a] === "authenticated").map((a) => `${c.resource}:${a}`),
      ),
    );
  });

  it("gives a superadmin every supported action, and no unsupported one", () => {
    const { allowed, superadmin, role } = effectiveFor(null, true);
    // a superadmin's role is null on the wire: `Principal::Superadmin` holds no
    // membership for `get_effective` to resolve one from
    expect([superadmin, role]).toEqual([true, null]);
    expect(allowed.length).toBe(
      CAPABILITIES.reduce((n, c) => n + ACTIONS.filter((a) => c[a] !== null).length, 0),
    );
    // orgs have no update route and an audit log is append-only
    expect(has(allowed, "org:update")).toBe(false);
    expect(has(allowed, "audit_log:create")).toBe(false);
  });
});

describe("the published matrix", () => {
  it("omits an action the resource does not have rather than denying it", () => {
    const org = matrixFixture().resources.find((r) => r.resource === "org");
    expect(org?.actions.map((a) => a.action)).toEqual(["read", "create", "delete"]);
  });

  it("names the minimum role for a scoped action and no role for the rest", () => {
    const { resources } = matrixFixture();
    const action = (resource: string, name: string) =>
      resources.find((r) => r.resource === resource)?.actions.find((a) => a.action === name);
    expect(action("provider", "create")).toEqual({
      action: "create",
      minimum_role: "admin",
      superadmin_only: false,
      authenticated_only: false,
    });
    expect(action("feature_flags", "update")).toEqual({
      action: "update",
      minimum_role: null,
      superadmin_only: true,
      authenticated_only: false,
    });
    expect(action("model", "read")).toEqual({
      action: "read",
      minimum_role: null,
      superadmin_only: false,
      authenticated_only: true,
    });
  });

  it("carries every resource the table defines, in its order", () => {
    expect(matrixFixture().resources.map((r) => r.resource)).toEqual(
      CAPABILITIES.map((c) => c.resource),
    );
  });
});
