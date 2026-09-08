import { describe, it, expect } from "bun:test";
import { fileURLToPath, URL } from "node:url";

import { drift, parseCapabilities, readCapabilities, type CapabilityRow } from "./rbac-matrix-source";
import snapshot from "../src/lib/rbac-capabilities.json";

const RBAC_MATRIX = fileURLToPath(
  new URL("../../crates/rolter-control/src/rbac_matrix.rs", import.meta.url),
);

const TABLE = `
const CAPABILITIES: &[Capability] = &[
    // an org is created out of band
    Capability {
        resource: "org",
        scope: "org",
        read: VIEWER,
        create: SUPER,
        update: NA,
        delete: ADMIN,
    },
    Capability {
        resource: "model_price",
        scope: "deployment",
        read: ANYONE,
        // a price is upserted, never created
        create: NA,
        update: SUPER,
        delete: SUPER,
    },
];

impl Capability {}
`;

describe("the capability table parser", () => {
  it("reads a resource, its scope and the authority of each action", () => {
    expect(parseCapabilities(TABLE)).toEqual([
      { resource: "org", scope: "org", read: "viewer", create: "superadmin", update: null, delete: "admin" },
      {
        resource: "model_price",
        scope: "deployment",
        read: "authenticated",
        create: null,
        update: "superadmin",
        delete: "superadmin",
      },
    ]);
  });

  it("refuses a table it only partly understood", () => {
    // the real table annotates individual fields, and an entry whose shape the
    // pattern misses would otherwise vanish from the fixture without a word —
    // which is the failure mode this whole file exists to prevent
    const mangled = TABLE.replace("read: VIEWER,", "read: VIEWER, extra: 1,");
    expect(() => parseCapabilities(mangled)).toThrow(/parsed 1 of 2 capabilities/);
  });

  it("refuses an authority it has never heard of", () => {
    expect(() => parseCapabilities(TABLE.replace("create: SUPER,", "create: WIZARD,"))).toThrow(
      /unknown authority `WIZARD` on `org.create`/,
    );
  });

  it("names the pair that drifted", () => {
    const [a, b] = [parseCapabilities(TABLE), parseCapabilities(TABLE)];
    b[1]!.update = "admin";
    expect(drift(a, b)).toEqual([
      "model_price:update takes superadmin in crates/rolter-control/src/rbac_matrix.rs, admin in the fixture",
    ]);
  });

  it("names a resource that only one side has", () => {
    const table = parseCapabilities(TABLE);
    expect(drift(table, table.slice(0, 1))).toEqual([
      "model_price: in crates/rolter-control/src/rbac_matrix.rs, missing from the fixture",
    ]);
    expect(drift(table.slice(0, 1), table)).toEqual([
      "model_price: in the fixture, missing from crates/rolter-control/src/rbac_matrix.rs",
    ]);
  });
});

describe("the checked-in capability fixture", () => {
  // the merge gate (#1298): the stories render as a role by deriving that
  // role's capabilities from `src/lib/rbac-capabilities.json`, so a resource,
  // action or authority added to the control plane and not regenerated here
  // would leave the gating stories asserting a deployment nobody runs
  it("is what crates/rolter-control/src/rbac_matrix.rs publishes", () => {
    const differences = drift(readCapabilities(RBAC_MATRIX), snapshot.capabilities as CapabilityRow[]);
    expect(differences.join("\n") || "up to date").toBe("up to date");
  });

  it("keeps the table's own order, which is the order the matrix is published in", () => {
    expect((snapshot.capabilities as CapabilityRow[]).map((c) => c.resource)).toEqual(
      readCapabilities(RBAC_MATRIX).map((c) => c.resource),
    );
  });
});
