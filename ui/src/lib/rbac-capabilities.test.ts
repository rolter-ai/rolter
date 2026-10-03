import { describe, it, expect } from "bun:test";

import {
  ACTIONS,
  CAPABILITIES,
  allowedFor,
  chainAt,
  effectiveFor,
  matrixFixture,
  resolveRole,
  type Membership,
  type ScopeChain,
} from "./rbac-capabilities";

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

// the `chain()` and `membership()` fixtures of the Rust tests, and the cases
// they state about a caller whose role sits below the org (#2376)
const CHAIN: ScopeChain = { orgId: "o", teamId: "t", projectId: "p" };
const ORG_ONLY: ScopeChain = { orgId: "o", teamId: null, projectId: null };
const at = (role: Membership["role"], level: "org" | "team" | "project"): Membership => ({
  role,
  orgId: "o",
  teamId: level === "org" ? null : "t",
  projectId: level === "project" ? "p" : null,
});
const allowedAt = (memberships: Membership[], chain = CHAIN) => allowedFor({ memberships, chain });

describe("each capability decided at its own scope", () => {
  it("does not promise a team admin org-scoped capabilities", () => {
    const allowed = allowedAt([at("admin", "team")]);
    // org-scoped: the guard checks the org alone and a team membership does
    // not reach it
    expect(has(allowed, "team:create")).toBe(false);
    expect(has(allowed, "custom_role:read")).toBe(false);
    expect(has(allowed, "plugin:create")).toBe(false);
    // project-scoped: the whole chain, which the team membership does reach
    expect(has(allowed, "route:create")).toBe(true);
    expect(has(allowed, "provider:create")).toBe(true);
    // and the wire `role` is still the one resolved at the whole chain
    expect(effectiveFor({ memberships: [at("admin", "team")], chain: CHAIN }).role).toBe("admin");
  });

  it("decides a team-scoped row at org + team, so a project admin does not create projects", () => {
    expect(chainAt("team", CHAIN)).toEqual({ orgId: "o", teamId: "t", projectId: null });
    expect(has(allowedAt([at("admin", "project")]), "project:create")).toBe(false);
    expect(has(allowedAt([at("admin", "team")]), "project:create")).toBe(true);
  });

  it("does not let a project member read org-scoped resources", () => {
    const allowed = allowedAt([at("member", "project")]);
    expect(has(allowed, "custom_role:read")).toBe(false);
    expect(has(allowed, "team:read")).toBe(false);
  });

  it("lets a project viewer read budgets and rate limits and write neither", () => {
    const viewer = [at("viewer", "project")];
    for (const res of ["budget", "rate_limit"]) {
      expect(has(allowedAt(viewer), `${res}:read`)).toBe(true);
      for (const action of ["create", "update", "delete"]) {
        expect(has(allowedAt(viewer), `${res}:${action}`)).toBe(false);
      }
    }
    // asked at the org alone, or by a caller with no role, nothing is read
    expect(has(allowedAt(viewer, ORG_ONLY), "budget:read")).toBe(false);
    expect(has(allowedAt([]), "rate_limit:read")).toBe(false);
    // an org viewer still reads at any chain
    for (const chain of [CHAIN, ORG_ONLY]) {
      expect(has(allowedAt([at("viewer", "org")], chain), "budget:read")).toBe(true);
    }
  });

  it("promises a project admin provider writes on their own project only", () => {
    const admin = [at("admin", "project")];
    for (const res of ["provider", "provider_group"]) {
      for (const action of ACTIONS) {
        expect(has(allowedAt(admin), `${res}:${action}`)).toBe(true);
      }
    }
    expect(has(allowedAt(admin), "team:create")).toBe(false);
    expect(has(allowedAt([at("viewer", "project")]), "provider:create")).toBe(false);
    // asked at the org alone, a project membership does not reach it
    expect(has(allowedAt(admin, ORG_ONLY), "provider:create")).toBe(false);
    // an org admin still passes, with or without a project in the query
    for (const chain of [CHAIN, ORG_ONLY]) {
      expect(has(allowedAt([at("admin", "org")], chain), "provider:create")).toBe(true);
    }
  });

  it("resolves the most specific membership, ties to the higher role", () => {
    // a project viewer in an org they administer is a viewer on the project
    expect(resolveRole([at("admin", "org"), at("viewer", "project")], CHAIN)).toBe("viewer");
    expect(resolveRole([at("admin", "org"), at("viewer", "project")], ORG_ONLY)).toBe("admin");
    expect(resolveRole([at("viewer", "team"), at("member", "team")], CHAIN)).toBe("member");
    // a membership on another project reaches nothing here
    expect(resolveRole([{ role: "admin", projectId: "other" }], CHAIN)).toBe(null);
  });

  // the mirror of `allowed_for_agrees_with_authorize_on_every_row`: for every
  // row, the answer is the role resolved at the chain that row's guard asks at
  it("agrees with the guard on every row, for every membership level and role", () => {
    const levels: Membership[][] = (["viewer", "member", "admin"] as const).flatMap((role) => [
      [at(role, "org")],
      [at(role, "team")],
      [at(role, "project")],
      [{ role, teamId: "t" }],
      [{ role, projectId: "p" }],
    ]);
    const rank = { viewer: 0, member: 1, admin: 2 } as const;
    for (const ms of levels) {
      const allowed = allowedAt(ms);
      for (const c of CAPABILITIES) {
        const guard: ScopeChain =
          c.scope === "org"
            ? ORG_ONLY
            : c.scope === "team"
              ? { orgId: "o", teamId: "t", projectId: null }
              : CHAIN;
        const role = resolveRole(ms, guard);
        for (const action of ACTIONS) {
          const authority = c[action];
          if (authority === null || authority === "superadmin" || authority === "authenticated")
            continue;
          const decided = role !== null && rank[role] >= rank[authority];
          expect([`${c.resource}:${action}`, has(allowed, `${c.resource}:${action}`)]).toEqual([
            `${c.resource}:${action}`,
            decided,
          ]);
        }
      }
    }
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
