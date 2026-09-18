import { describe, expect, it } from "bun:test";

import type { RbacEffective, RbacMatrix } from "@/lib/api";
import { decide, decideSuperadmin, requirementFor, type CapabilityValue } from "@/lib/can";
import { visibleNav, type NavDef } from "@/lib/nav";

const effective = (over: Partial<RbacEffective> = {}): RbacEffective => ({
  superadmin: false,
  role: "member",
  allowed: ["provider:read", "virtual_key:read", "virtual_key:create"],
  custom_roles: [],
  model_policy: null,
  ...over,
});

const value = (over: Partial<CapabilityValue> = {}): CapabilityValue => ({
  effective: effective(),
  matrix: null,
  resolved: true,
  ...over,
});

describe("decide", () => {
  it("answers from the pairs the control plane listed", () => {
    expect(decide(value(), "virtual_key", "create")).toBe(true);
    expect(decide(value(), "provider", "create")).toBe(false);
  });

  // the two uncertain cases both fall open. a control that starts disabled and
  // enables itself a request later reads as broken, and disabling the whole
  // dashboard because one query failed would be a bigger outage than the one
  // it was guessing about
  it("says nothing while the answer is still in flight", () => {
    expect(decide(value({ resolved: false }), "provider", "create")).toBeUndefined();
  });

  it("says nothing when there is no provider above", () => {
    expect(decide(null, "provider", "create")).toBeUndefined();
  });

  it("says nothing when the question could not be answered", () => {
    // an old control plane 404s the endpoint, or it was unreachable: resolved,
    // with nothing to resolve to. the 403 stays the backstop
    expect(decide(value({ effective: null }), "provider", "create")).toBeUndefined();
  });

  it("lets a superadmin do everything, listed or not", () => {
    const superadmin = value({ effective: effective({ superadmin: true, allowed: [] }) });
    expect(decide(superadmin, "feature_flags", "update")).toBe(true);
    expect(decide(superadmin, "anything_at_all", "delete")).toBe(true);
  });

  it("refuses a pair for a caller with no membership at the scope", () => {
    const stranger = value({ effective: effective({ role: null, allowed: [] }) });
    expect(decide(stranger, "virtual_key", "read")).toBe(false);
  });

  // a resource that is a prefix of another must not answer for it
  it("matches the whole pair, not a prefix of it", () => {
    const v = value({ effective: effective({ allowed: ["virtual_key:read"] }) });
    expect(decide(v, "virtual", "read")).toBe(false);
    expect(decide(v, "virtual_key", "read")).toBe(true);
  });
});

describe("decideSuperadmin", () => {
  it("is unknown until the answer is in", () => {
    expect(decideSuperadmin(null)).toBeUndefined();
    expect(decideSuperadmin(value({ resolved: false }))).toBeUndefined();
    expect(decideSuperadmin(value({ effective: null }))).toBeUndefined();
  });

  it("reports what the control plane said", () => {
    expect(decideSuperadmin(value())).toBe(false);
    expect(decideSuperadmin(value({ effective: effective({ superadmin: true }) }))).toBe(true);
  });
});

const matrix: RbacMatrix = {
  roles: [{ role: "admin", rank: 2 }],
  resources: [
    {
      resource: "provider",
      scope: "org",
      actions: [
        {
          action: "create",
          minimum_role: "admin",
          superadmin_only: false,
          authenticated_only: false,
        },
      ],
    },
    {
      resource: "feature_flags",
      scope: "deployment",
      actions: [
        {
          action: "update",
          minimum_role: null,
          superadmin_only: true,
          authenticated_only: false,
        },
      ],
    },
  ],
  custom_roles: [],
};

describe("requirementFor", () => {
  it("names the minimum role a scoped action takes", () => {
    expect(requirementFor(matrix, "provider", "create")).toBe("admin");
  });

  it("distinguishes superadmin from a role nobody can be granted", () => {
    expect(requirementFor(matrix, "feature_flags", "update")).toBe("superadmin");
  });

  // an action the resource does not have is absent from the matrix rather than
  // present with a null authority, so "not found" must not read as "no role"
  it("has no answer for a pair the matrix does not carry", () => {
    expect(requirementFor(matrix, "provider", "delete")).toBeNull();
    expect(requirementFor(null, "provider", "create")).toBeNull();
  });
});

describe("visibleNav", () => {
  const nav: NavDef[] = [
    { key: "playground", icon: null },
    {
      key: "settings",
      icon: null,
      children: [
        { key: "feature-flags", icon: null, resource: "feature_flags" },
        { key: "security", icon: null, resource: "security_settings" },
      ],
    },
    {
      key: "governance",
      icon: null,
      children: [
        { key: "virtual-keys", icon: null, resource: "virtual_key" },
        { key: "gov-users", icon: null, resource: "user" },
      ],
    },
  ];

  const keys = (defs: NavDef[]): string[] =>
    defs.flatMap((d) => (d.children ? [d.key, ...keys(d.children)] : [d.key]));

  it("drops a group whose every leaf is unreadable", () => {
    const deployment = new Set(["feature_flags", "security_settings"]);
    const visible = visibleNav((resource) => !deployment.has(resource), nav);
    expect(keys(visible)).toEqual(["playground", "governance", "virtual-keys", "gov-users"]);
  });

  it("keeps a group that still has one readable leaf", () => {
    const visible = visibleNav((resource) => resource !== "user", nav);
    expect(keys(visible)).toContain("governance");
    expect(keys(visible)).not.toContain("gov-users");
  });

  it("shows everything while the answer is unknown", () => {
    expect(keys(visibleNav(() => undefined, nav))).toEqual(keys(nav));
  });

  it("never hides a leaf that names no resource", () => {
    expect(keys(visibleNav(() => false, nav))).toEqual(["playground"]);
  });
});
