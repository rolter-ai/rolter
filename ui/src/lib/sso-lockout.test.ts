import { describe, expect, it } from "bun:test";

import type { SsoProviderRow } from "@/lib/api";
import { distinctPeople, locksOutMembers, secretGap } from "@/lib/sso-lockout";

function provider(id: string, over: Partial<SsoProviderRow> = {}): SsoProviderRow {
  return {
    id,
    org_id: "org-1",
    name: id,
    slug: id,
    issuer: "https://idp.example",
    client_id: "client",
    has_client_secret: true,
    scopes: ["openid"],
    group_claim: "groups",
    default_role: "member",
    enabled: true,
    created_at: "2026-01-01T00:00:00Z",
    redirect_uri: `https://rolter.example/auth/sso/${id}/callback`,
    login_url: `https://rolter.example/auth/sso/${id}/start`,
    ...over,
  };
}

const PASSWORDS_OFF = { allow_password_login: false };
const PASSWORDS_ON = { allow_password_login: true };

describe("distinctPeople", () => {
  it("counts a person holding two grants once", () => {
    const rows = [{ user_id: "ada" }, { user_id: "ada" }, { user_id: "grace" }];
    expect(distinctPeople(rows)).toBe(2);
  });

  it("is zero with no grants", () => {
    expect(distinctPeople([])).toBe(0);
  });
});

describe("locksOutMembers", () => {
  const okta = provider("okta");
  const entra = provider("entra");

  it("is true for the only enabled provider while passwords are off", () => {
    expect(locksOutMembers([okta], okta, PASSWORDS_OFF)).toBe(true);
  });

  it("is true when the others are already out of service", () => {
    const parked = provider("entra", { enabled: false });
    expect(locksOutMembers([okta, parked], okta, PASSWORDS_OFF)).toBe(true);
  });

  it("is false while another provider stays enabled", () => {
    expect(locksOutMembers([okta, entra], okta, PASSWORDS_OFF)).toBe(false);
  });

  it("is false while passwords are on", () => {
    expect(locksOutMembers([okta], okta, PASSWORDS_ON)).toBe(false);
  });

  it("is false for a provider that is already out of service", () => {
    const parked = provider("entra", { enabled: false });
    expect(locksOutMembers([okta, parked], parked, PASSWORDS_OFF)).toBe(false);
  });
});

describe("secretGap", () => {
  it("lists the enabled providers without a stored secret", () => {
    const bare = provider("bare", { has_client_secret: false });
    const gap = secretGap([provider("okta"), bare]);
    expect(gap.missing.map((p) => p.id)).toEqual(["bare"]);
    expect(gap.all).toBe(false);
  });

  it("says when no enabled provider has one", () => {
    const gap = secretGap([provider("a", { has_client_secret: false })]);
    expect(gap.all).toBe(true);
  });

  it("ignores a disabled provider without a secret", () => {
    const parked = provider("parked", { enabled: false, has_client_secret: false });
    const gap = secretGap([provider("okta"), parked]);
    expect(gap.missing).toEqual([]);
    expect(gap.all).toBe(false);
  });

  it("is not a gap when there is no enabled provider at all", () => {
    expect(secretGap([])).toEqual({ missing: [], all: false });
  });
});
