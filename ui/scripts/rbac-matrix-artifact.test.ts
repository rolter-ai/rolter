import { describe, it, expect } from "bun:test";
import { readFileSync } from "node:fs";

import { ARTIFACT, SNAPSHOT, parseMatrix, readMatrix } from "./rbac-matrix-artifact";
import { CHAIN_TRIMS, chainAt, type ScopeChain } from "../src/lib/rbac-capabilities";

describe("the checked-in capability fixture", () => {
  // the merge gate (#1298, #1369): the stories render as a role by deriving
  // that role's capabilities from `src/lib/rbac-capabilities.json`, so a
  // resource, action or authority the control plane changed and this copy did
  // not would leave the gating stories asserting a deployment nobody runs. the
  // rust suite already pins the artifact to `CAPABILITIES`; this pins the copy
  // to the artifact
  it("is a byte-for-byte copy of crates/rolter-control/rbac-matrix.json", () => {
    const [artifact, snapshot] = [readFileSync(ARTIFACT, "utf8"), readFileSync(SNAPSHOT, "utf8")];
    expect(
      artifact === snapshot
        ? "up to date"
        : "src/lib/rbac-capabilities.json differs from crates/rolter-control/rbac-matrix.json; run `bun run gen:rbac`",
    ).toBe("up to date");
  });

  it("gives every action exactly one authority, as the fixtures read it", () => {
    expect(() => readMatrix(SNAPSHOT)).not.toThrow();
  });

  it("names each resource once", () => {
    const resources = readMatrix(ARTIFACT).resources.map((r) => r.resource);
    expect(resources.filter((r, i) => resources.indexOf(r) !== i)).toEqual([]);
  });
});

describe("the chain each row is decided at", () => {
  const WHOLE: ScopeChain = { orgId: "org", teamId: "team", projectId: "project" };
  const FIELD: Record<string, keyof ScopeChain> = {
    org: "orgId",
    team: "teamId",
    project: "projectId",
  };
  const artifact = readFileSync(ARTIFACT, "utf8");
  const withChainAt = (chain_at: unknown) =>
    JSON.stringify({ ...(JSON.parse(artifact) as object), chain_at });

  // the dashboard's port of `rbac/effective` decides each row at the part of
  // the chain `chain_at` names (#2376); were the two to differ, a gating story
  // would promise a team or project member what the guard refuses them
  it("is the rule src/lib/rbac-capabilities.ts ports", () => {
    const { chain_at } = readMatrix(ARTIFACT);
    const ported = Object.fromEntries(
      Object.keys(chain_at).map((scope) => {
        const at = chainAt(scope, WHOLE);
        return [scope, (["org", "team", "project"] as const).filter((f) => at[FIELD[f]!] === null)];
      }),
    );
    expect(ported).toEqual(chain_at);
  });

  it("names no scope the table does not use", () => {
    const { chain_at } = readMatrix(ARTIFACT);
    expect(Object.keys(CHAIN_TRIMS).filter((scope) => !(scope in chain_at))).toEqual([]);
  });

  it("refuses an artifact without the rule", () => {
    expect(() => parseMatrix(withChainAt(undefined), "fixture")).toThrow(/no `chain_at` table/);
  });

  it("refuses a scope the rule does not cover", () => {
    const { chain_at } = readMatrix(ARTIFACT);
    const { team: _team, ...rest } = chain_at;
    expect(() => parseMatrix(withChainAt(rest), "fixture")).toThrow(
      /no `chain_at` rule for scope team/,
    );
  });

  it("refuses a chain field it does not know", () => {
    const { chain_at } = readMatrix(ARTIFACT);
    expect(() => parseMatrix(withChainAt({ ...chain_at, org: ["tenant"] }), "fixture")).toThrow(
      /org:tenant/,
    );
  });
});
