import { describe, expect, it } from "bun:test";

import type { MembershipRow } from "@/lib/api";
import { teamPeople } from "./team-people";

const grant = (id: string, user_id: string, role: string, at: Partial<MembershipRow>) =>
  ({ id, user_id, role, created_at: "", ...at }) as MembershipRow;

const ROWS = [
  grant("1", "ada", "admin", { team_id: "t1" }),
  grant("2", "bob", "member", { team_id: "t1" }),
  grant("3", "bob", "viewer", { project_id: "p1" }),
  grant("4", "cy", "member", { project_id: "p1" }),
  grant("5", "dee", "admin", { team_id: "t2" }),
  grant("6", "eve", "admin", { org_id: "o1" }),
  grant("7", "fay", "member", { project_id: "p9" }),
];

describe("teamPeople", () => {
  it("counts each account once across the team and its projects", () => {
    expect(teamPeople(ROWS, "t1", ["p1"]).people).toEqual(["ada", "bob", "cy"]);
  });

  it("counts a team whose people all work at project scope", () => {
    expect(teamPeople(ROWS, "t3", ["p1"]).people).toEqual(["bob", "cy"]);
  });

  it("leaves out org-wide grants and other teams", () => {
    const { people } = teamPeople(ROWS, "t1", ["p1"]);
    expect(people).not.toContain("eve");
    expect(people).not.toContain("dee");
    expect(people).not.toContain("fay");
  });

  it("lists every admin of the team itself, not of its projects", () => {
    const rows = [...ROWS, grant("8", "gil", "admin", { team_id: "t1" })];
    expect(teamPeople(rows, "t1", ["p1"]).admins).toEqual(["ada", "gil"]);
    expect(
      teamPeople([grant("9", "hal", "admin", { project_id: "p1" })], "t1", ["p1"]).admins,
    ).toEqual([]);
  });
});
