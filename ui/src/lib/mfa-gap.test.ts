import { describe, expect, test } from "bun:test";

import type { OrgMemberMfa } from "@/lib/api";
import { mfaGap } from "@/lib/mfa-gap";

const row = (id: string, over: Partial<OrgMemberMfa> = {}): OrgMemberMfa => ({
  user_id: id,
  email: `${id}@example.com`,
  is_superadmin: false,
  enabled: false,
  recovery_codes_remaining: 0,
  ...over,
});

const ROSTER = [
  row("ada", { is_superadmin: true, enabled: true, recovery_codes_remaining: 8 }),
  row("root", { is_superadmin: true }),
  row("bob"),
  row("eve", { enabled: true, recovery_codes_remaining: 10 }),
];

describe("mfaGap", () => {
  test("required_all counts every member without an armed factor", () => {
    expect(mfaGap(ROSTER, "required_all")).toEqual({ missing: 2, total: 4 });
  });

  test("required_superadmin counts only superadmin rows", () => {
    expect(mfaGap(ROSTER, "required_superadmin")).toEqual({ missing: 1, total: 2 });
  });

  test("a policy that binds nobody has no gap", () => {
    expect(mfaGap(ROSTER, "off")).toBeNull();
    expect(mfaGap(ROSTER, "optional")).toBeNull();
  });

  test("an empty roster is zero of zero, not a failure", () => {
    expect(mfaGap([], "required_all")).toEqual({ missing: 0, total: 0 });
  });
});
