import { describe, expect, it } from "bun:test";

import type { MembershipRow } from "@/lib/api";
import { afterRevoke, grantScope, higherRole, membershipScope, sameScope } from "@/lib/role-grants";

const ORG = "org-1";

function grant(id: string, role: string, scope: Partial<MembershipRow>): MembershipRow {
  return {
    id,
    user_id: "user-1",
    org_id: null,
    team_id: null,
    project_id: null,
    role,
    created_at: "2026-01-01T00:00:00Z",
    ...scope,
  };
}

const teams: Record<string, string> = { "project-1": "team-1", "project-2": "team-2" };
const teamOf = (project: string) => teams[project];

describe("grantScope", () => {
  it("reads the most specific id, as the control plane stores it", () => {
    expect(grantScope(grant("a", "admin", { org_id: ORG }))).toEqual({ type: "org", id: ORG });
    expect(grantScope(grant("b", "member", { org_id: ORG, team_id: "team-1" }))).toEqual({
      type: "team",
      id: "team-1",
    });
    expect(grantScope(grant("c", "viewer", { team_id: "team-1", project_id: "p" }))).toEqual({
      type: "project",
      id: "p",
    });
  });

  it("compares two grants by that scope alone", () => {
    const team = grant("a", "member", { team_id: "team-1" });
    expect(sameScope(team, grant("b", "viewer", { team_id: "team-1" }))).toBe(true);
    expect(sameScope(team, grant("c", "viewer", { team_id: "team-2" }))).toBe(false);
    expect(sameScope(team, grant("d", "viewer", { org_id: ORG }))).toBe(false);
  });
});

describe("membershipScope", () => {
  it("names the project, else the team, else the org", () => {
    expect(membershipScope({}, ORG)).toEqual({ scope_type: "org", scope_id: ORG });
    expect(membershipScope({ team_id: "team-1" }, ORG)).toEqual({
      scope_type: "team",
      scope_id: "team-1",
    });
    expect(membershipScope({ project_id: "project-1" }, ORG)).toEqual({
      scope_type: "project",
      scope_id: "project-1",
    });
  });
});

describe("higherRole", () => {
  it("is the one resolve_role picks at a single scope", () => {
    expect(higherRole("viewer", "admin")).toBe("admin");
    expect(higherRole("admin", "member")).toBe("admin");
    expect(higherRole("member", "viewer")).toBe("member");
  });

  it("never prefers a role this build does not know", () => {
    expect(higherRole("viewer", "owner")).toBe("viewer");
  });
});

describe("afterRevoke", () => {
  it("says none when nothing else reaches the scope", () => {
    const target = grant("t", "member", { team_id: "team-1" });
    const elsewhere = grant("o", "admin", { team_id: "team-2" });
    expect(afterRevoke(target, [target, elsewhere], ORG, teamOf)).toEqual({ kind: "none" });
  });

  it("falls back to the org grant above a team grant", () => {
    const target = grant("t", "admin", { team_id: "team-1" });
    const org = grant("o", "viewer", { org_id: ORG });
    expect(afterRevoke(target, [target, org], ORG, teamOf)).toEqual({
      kind: "fallback",
      grant: org,
    });
  });

  it("prefers the team grant over the org grant for a project", () => {
    const target = grant("t", "viewer", { project_id: "project-1" });
    const team = grant("tm", "member", { team_id: "team-1" });
    const org = grant("o", "admin", { org_id: ORG });
    expect(afterRevoke(target, [org, target, team], ORG, teamOf)).toEqual({
      kind: "fallback",
      grant: team,
    });
  });

  it("reports a wider role when the revoked grant was the narrower, lower one", () => {
    // the most specific grant wins, so a project viewer who is an org admin is
    // an admin on the project again once the narrower grant goes
    const target = grant("t", "viewer", { project_id: "project-1" });
    const org = grant("o", "admin", { org_id: ORG });
    expect(afterRevoke(target, [target, org], ORG, teamOf)).toEqual({
      kind: "fallback",
      grant: org,
    });
  });

  it("picks the higher of two grants at the same scope", () => {
    const target = grant("t", "admin", { org_id: ORG });
    const viewer = grant("v", "viewer", { org_id: ORG });
    const member = grant("m", "member", { org_id: ORG });
    expect(afterRevoke(target, [target, viewer, member], ORG, teamOf)).toEqual({
      kind: "fallback",
      grant: member,
    });
  });

  it("ignores a team grant that does not own the project", () => {
    const target = grant("t", "member", { project_id: "project-2" });
    const team = grant("tm", "admin", { team_id: "team-1" });
    expect(afterRevoke(target, [target, team], ORG, teamOf)).toEqual({ kind: "none" });
  });

  it("does not let a team grant reach the org itself", () => {
    const target = grant("t", "admin", { org_id: ORG });
    const team = grant("tm", "admin", { team_id: "team-1" });
    expect(afterRevoke(target, [target, team], ORG, teamOf)).toEqual({ kind: "none" });
  });

  it("skips a grant whose role this build does not know", () => {
    const target = grant("t", "member", { team_id: "team-1" });
    const unknown = grant("u", "owner", { org_id: ORG });
    expect(afterRevoke(target, [target, unknown], ORG, teamOf)).toEqual({ kind: "none" });
  });

  it("will not guess when a project's team is unknown", () => {
    const target = grant("t", "member", { project_id: "project-9" });
    const team = grant("tm", "admin", { team_id: "team-1" });
    expect(afterRevoke(target, [target, team], ORG, teamOf)).toEqual({ kind: "unknown" });
  });
});
