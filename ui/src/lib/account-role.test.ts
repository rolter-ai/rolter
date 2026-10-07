import { describe, expect, test } from "bun:test";

import { accountRole, accountRoleLine, accountRoleName } from "@/lib/account-role";
import type { MeMembership } from "@/lib/api";
import i18n from "@/lib/i18n";

const grant = (over: Partial<MeMembership>): MeMembership => ({
  id: "m",
  user_id: "u",
  role: "viewer",
  created_at: "2026-01-01T00:00:00Z",
  source: "manual",
  ...over,
});

const SCOPE = { orgId: "org-1", teamId: "team-1", projectId: "project-1" };
const USER = { is_superadmin: false };

describe("accountRole", () => {
  test("a superadmin is one thing everywhere, grants or none", () => {
    expect(accountRole({ is_superadmin: true }, [], SCOPE)).toEqual({ kind: "superadmin" });
    expect(
      accountRole({ is_superadmin: true }, [grant({ org_id: "org-1", role: "viewer" })], SCOPE),
    ).toEqual({ kind: "superadmin" });
  });

  test("names the level an org, team and project grant was made at", () => {
    expect(accountRole(USER, [grant({ org_id: "org-1", role: "admin" })], SCOPE)).toEqual({
      kind: "grant",
      role: "admin",
      level: "org",
    });
    expect(
      accountRole(USER, [grant({ org_id: "org-1", team_id: "team-1", role: "member" })], SCOPE),
    ).toEqual({ kind: "grant", role: "member", level: "team" });
    expect(accountRole(USER, [grant({ project_id: "project-1", role: "viewer" })], SCOPE)).toEqual({
      kind: "grant",
      role: "viewer",
      level: "project",
    });
  });

  test("the strongest grant that reaches the scope wins", () => {
    const grants = [
      grant({ org_id: "org-1", role: "viewer" }),
      grant({ project_id: "project-1", role: "admin" }),
    ];
    expect(accountRole(USER, grants, SCOPE)).toEqual({
      kind: "grant",
      role: "admin",
      level: "project",
    });
  });

  test("equally strong grants name the broader one", () => {
    const grants = [
      grant({ project_id: "project-1", role: "member" }),
      grant({ org_id: "org-1", role: "member" }),
    ];
    expect(accountRole(USER, grants, SCOPE)).toEqual({
      kind: "grant",
      role: "member",
      level: "org",
    });
  });

  test("a grant elsewhere does not explain this scope", () => {
    const grants = [
      grant({ project_id: "project-9", role: "admin" }),
      grant({ team_id: "team-9", role: "member" }),
    ];
    expect(accountRole(USER, grants, SCOPE)).toEqual({ kind: "role", role: "admin" });
  });

  test("a role this build has no name for ranks below every one it has", () => {
    const grants = [
      grant({ org_id: "org-1", role: "auditor" }),
      grant({ org_id: "org-1", role: "viewer" }),
    ];
    expect(accountRole(USER, grants, SCOPE)).toEqual({
      kind: "grant",
      role: "viewer",
      level: "org",
    });
  });

  test("with no account, or no grants, nothing is known", () => {
    expect(accountRole(null, [], SCOPE)).toEqual({ kind: "unknown" });
    expect(accountRole(USER, [], SCOPE)).toEqual({ kind: "unknown" });
  });
});

describe("the role as words", () => {
  const names = { org: "acme", team: "platform", project: "default" };

  test("the line names the level and what it was granted on", () => {
    const line = (role: string, level: "org" | "team" | "project") =>
      accountRoleLine(i18n.t, { kind: "grant", role, level }, names);
    expect(line("admin", "org")).toBe("Admin · org acme");
    expect(line("member", "team")).toBe("Member · team platform");
    expect(line("viewer", "project")).toBe("Viewer · project default");
  });

  test("a superadmin is the whole deployment", () => {
    expect(accountRoleLine(i18n.t, { kind: "superadmin" }, names)).toBe(
      "Superadmin · whole deployment",
    );
    expect(accountRoleName(i18n.t, { kind: "superadmin" })).toBe("Superadmin");
  });

  test("with no level to name the line is the role alone", () => {
    expect(accountRoleLine(i18n.t, { kind: "role", role: "member" }, names)).toBe("Member");
    expect(accountRoleName(i18n.t, { kind: "grant", role: "admin", level: "org" })).toBe("Admin");
  });

  test("an account nothing is known about keeps the fallback label", () => {
    expect(accountRoleName(i18n.t, { kind: "unknown" })).toBe(i18n.t("shell.role"));
  });
});
