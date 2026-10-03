import { describe, it, expect } from "bun:test";
import { fileURLToPath, URL } from "node:url";

import {
  drift,
  parseCapabilities,
  parseChainTrims,
  readCapabilities,
  readChainTrims,
  type CapabilityRow,
} from "./rbac-matrix-source";
import snapshot from "../src/lib/rbac-capabilities.json";
import { CHAIN_TRIMS } from "../src/lib/rbac-capabilities";

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
      {
        resource: "org",
        scope: "org",
        read: "viewer",
        create: "superadmin",
        update: null,
        delete: "admin",
      },
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
    const differences = drift(
      readCapabilities(RBAC_MATRIX),
      snapshot.capabilities as CapabilityRow[],
    );
    expect(differences.join("\n") || "up to date").toBe("up to date");
  });

  it("keeps the table's own order, which is the order the matrix is published in", () => {
    expect((snapshot.capabilities as CapabilityRow[]).map((c) => c.resource)).toEqual(
      readCapabilities(RBAC_MATRIX).map((c) => c.resource),
    );
  });
});

describe("the chain each row is decided at", () => {
  const CHAIN_AT = `
fn chain_at(scope: &str, chain: ScopeChain) -> ScopeChain {
    match scope {
        "org" => ScopeChain {
            team: None,
            project: None,
            ..chain
        },
        "team" => ScopeChain {
            project: None,
            ..chain
        },
        _ => chain,
    }
}
`;

  it("reads the fields each scope clears", () => {
    expect(parseChainTrims(CHAIN_AT)).toEqual({ org: ["team", "project"], team: ["project"] });
  });

  it("refuses an arm it only partly understood", () => {
    const extra = CHAIN_AT.replace(
      "_ => chain,",
      '"tenant" => narrow(chain),\n        _ => chain,',
    );
    expect(() => parseChainTrims(extra)).toThrow(/parsed 2 of 4 `chain_at` arms/);
  });

  it("refuses a fallback that no longer hands back the whole chain", () => {
    expect(() =>
      parseChainTrims(CHAIN_AT.replace("_ => chain,", "_ => ScopeChain::default(),")),
    ).toThrow(/fallback arm/);
  });

  // the dashboard's port of `rbac/effective` decides each row at the part of
  // the chain `chain_at` names (#2376); were the two to differ, a gating story
  // would promise a team or project member what the guard refuses them
  it("is the rule src/lib/rbac-capabilities.ts ports", () => {
    const field: Record<string, string> = { orgId: "org", teamId: "team", projectId: "project" };
    const ported = Object.fromEntries(
      Object.entries(CHAIN_TRIMS).map(([scope, fields]) => [scope, fields.map((f) => field[f])]),
    );
    expect(ported).toEqual(readChainTrims(RBAC_MATRIX));
  });
});
