import { describe, it, expect } from "bun:test";
import { readFileSync } from "node:fs";

import { ARTIFACT, SNAPSHOT, readMatrix } from "./rbac-matrix-artifact";

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
