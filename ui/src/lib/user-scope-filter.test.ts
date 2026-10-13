import { describe, expect, it } from "bun:test";

import type { MembershipRow, OrgProjectRow, TeamRow } from "@/lib/api";
import {
  filterValue,
  parseFilterValue,
  readUserScopeFilter,
  scopeExists,
  usersInScope,
  writeUserScopeFilter,
} from "./user-scope-filter";

const grant = (user_id: string, at: Partial<MembershipRow>) =>
  ({ id: user_id, user_id, role: "member", created_at: "", ...at }) as MembershipRow;
const project = (id: string, team_id: string) => ({ id, team_id, name: id }) as OrgProjectRow;

const GRANTS = [
  grant("ada", { team_id: "t1" }),
  grant("bob", { project_id: "p1" }),
  grant("cy", { project_id: "p2" }),
  grant("eve", { org_id: "o1" }),
];
const PROJECTS = [project("p1", "t1"), project("p2", "t2")];

describe("user scope filter", () => {
  it("reads the team or project a link names", () => {
    expect(readUserScopeFilter(new URLSearchParams("team=t1"))).toEqual({ kind: "team", id: "t1" });
    expect(readUserScopeFilter(new URLSearchParams("project=p1&team=t1"))).toEqual({
      kind: "project",
      id: "p1",
    });
    expect(readUserScopeFilter(new URLSearchParams("team="))).toBeNull();
  });

  it("writes one parameter and keeps the rest", () => {
    const next = writeUserScopeFilter(new URLSearchParams("team=t1&x=1"), {
      kind: "project",
      id: "p1",
    });
    expect(next.toString()).toBe("x=1&project=p1");
    expect(writeUserScopeFilter(next, null).toString()).toBe("x=1");
  });

  it("round-trips the combobox value", () => {
    const filter = { kind: "team", id: "a:b" } as const;
    expect(parseFilterValue(filterValue(filter))).toEqual(filter);
    expect(parseFilterValue("")).toBeNull();
  });

  it("counts a team's project people and not org-wide grants", () => {
    const set = usersInScope({ kind: "team", id: "t1" }, GRANTS, PROJECTS);
    expect([...set].sort()).toEqual(["ada", "bob"]);
  });

  it("narrows a project to its own grants", () => {
    expect([...usersInScope({ kind: "project", id: "p2" }, GRANTS, PROJECTS)]).toEqual(["cy"]);
  });

  it("knows an id the org does not have", () => {
    const teams = [{ id: "t1" }] as TeamRow[];
    expect(scopeExists({ kind: "team", id: "t1" }, teams, PROJECTS)).toBe(true);
    expect(scopeExists({ kind: "team", id: "nope" }, teams, PROJECTS)).toBe(false);
    expect(scopeExists({ kind: "project", id: "p2" }, teams, PROJECTS)).toBe(true);
  });
});
