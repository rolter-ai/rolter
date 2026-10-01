import { describe, expect, it } from "bun:test";

import { capGateScope } from "./limit-scope";

const byTeam = [
  { team: { id: "team-1" }, projects: [{ id: "project-1" }] },
  { team: { id: "team-2" }, projects: [{ id: "project-2" }] },
];
const ctx = { byTeam, keyProjectId: "project-2" };

describe("capGateScope", () => {
  it("asks an org cap at the org alone", () => {
    expect(capGateScope({ scope_type: "org", scope_id: "org-1" }, ctx)).toEqual({
      projectId: null,
    });
  });

  it("asks a team cap at org + team", () => {
    expect(capGateScope({ scope_type: "team", scope_id: "team-1" }, ctx)).toEqual({
      projectId: null,
      teamId: "team-1",
    });
  });

  it("asks a project cap at org + team + project", () => {
    expect(capGateScope({ scope_type: "project", scope_id: "project-1" }, ctx)).toEqual({
      projectId: "project-1",
      teamId: "team-1",
    });
  });

  it("asks a virtual-key cap at the project the keys are listed under", () => {
    expect(capGateScope({ scope_type: "virtual_key", scope_id: "vk-1" }, ctx)).toEqual({
      projectId: "project-2",
      teamId: "team-2",
    });
  });

  it("keeps the page answer when the key's project is unknown", () => {
    expect(
      capGateScope({ scope_type: "virtual_key", scope_id: "vk-1" }, { byTeam }),
    ).toBeUndefined();
  });

  it("keeps the page answer for a project it cannot place", () => {
    expect(capGateScope({ scope_type: "project", scope_id: "gone" }, ctx)).toBeUndefined();
  });

  it("keeps the page answer for business units and customers", () => {
    expect(capGateScope({ scope_type: "business_unit", scope_id: "bu" }, ctx)).toBeUndefined();
    expect(capGateScope({ scope_type: "customer", scope_id: "c" }, ctx)).toBeUndefined();
  });
});
