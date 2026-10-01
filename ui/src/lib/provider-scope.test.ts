import { describe, expect, it } from "bun:test";

import { providersUsableFrom, rowGateScope, usableFrom } from "./provider-scope";

interface Row {
  id: string;
  project_id?: string | null;
}
const org: Row = { id: "org-wide", project_id: null };
const legacy: Row = { id: "legacy" };
const a: Row = { id: "in-a", project_id: "a" };
const b: Row = { id: "in-b", project_id: "b" };
const all = [org, legacy, a, b];

describe("providersUsableFrom", () => {
  it("offers a project its own providers and the org-wide ones", () => {
    expect(providersUsableFrom(all, "a").map((p) => p.id)).toEqual(["org-wide", "legacy", "in-a"]);
  });

  it("offers an org-wide owner org-wide providers only", () => {
    expect(providersUsableFrom(all, null).map((p) => p.id)).toEqual(["org-wide", "legacy"]);
    expect(providersUsableFrom(all, undefined).map((p) => p.id)).toEqual(["org-wide", "legacy"]);
  });
});

describe("usableFrom", () => {
  it("agrees with the list filter", () => {
    expect(usableFrom(a, "a")).toBe(true);
    expect(usableFrom(a, "b")).toBe(false);
    expect(usableFrom(a, null)).toBe(false);
    expect(usableFrom(org, null)).toBe(true);
    // a provider the list does not hold is not judged
    expect(usableFrom(undefined, "a")).toBe(true);
  });
});

describe("rowGateScope", () => {
  const byTeam = [{ team: { id: "team-1" }, projects: [{ id: "project-1" }] }];

  it("an org-wide row is gated at the org", () => {
    expect(rowGateScope({ project_id: null }, byTeam)).toEqual({ projectId: null });
    expect(rowGateScope({}, byTeam)).toEqual({ projectId: null });
  });

  it("a project row is gated at its own team and project", () => {
    expect(rowGateScope({ project_id: "project-1" }, byTeam)).toEqual({
      projectId: "project-1",
      teamId: "team-1",
    });
  });

  it("a project the caller cannot list keeps the page's answer", () => {
    expect(rowGateScope({ project_id: "gone" }, byTeam)).toBeUndefined();
  });
});
