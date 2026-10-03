import { describe, expect, test } from "bun:test";

import i18n from "@/lib/i18n";
import { roleLabel } from "@/lib/roles";

describe("roleLabel", () => {
  test("translates a known role", () => {
    expect(roleLabel(i18n.t, "admin")).toBe(i18n.t("shell.roles.admin"));
    expect(roleLabel(i18n.t, "admin")).not.toBe("shell.roles.admin");
  });

  test("falls back to the raw value for a role it has no label for", () => {
    expect(roleLabel(i18n.t, "superadmin")).toBe("superadmin");
  });
});
