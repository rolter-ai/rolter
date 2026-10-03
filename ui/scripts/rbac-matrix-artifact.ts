import { readFileSync } from "node:fs";
import { join } from "node:path";

import type { RbacResourceView, RbacRoleView } from "../src/lib/api";

/**
 * Where the capability matrix lives on each side (#1369).
 *
 * `ARTIFACT` is written by a `rolter-control` unit test from `CAPABILITIES`
 * and checked against it on every `cargo test`, so it is the control plane's
 * own rendering of `GET /api/v1/rbac/matrix` minus the per-tenant custom roles.
 * `SNAPSHOT` is the dashboard's copy of it, which the story fixtures import —
 * `ui/` builds on its own, so it cannot reach into `crates/` at runtime.
 */
const UI = join(import.meta.dir, "..");
export const ARTIFACT = join(UI, "..", "crates", "rolter-control", "rbac-matrix.json");
export const SNAPSHOT = join(UI, "src", "lib", "rbac-capabilities.json");

/** A part of the chain, as `ScopeChain` spells it. */
export type ChainField = "org" | "team" | "project";

const CHAIN_FIELDS: readonly string[] = ["org", "team", "project"];

/**
 * The artifact's shape: the published matrix without `custom_roles`, plus
 * `chain_at` — for every scope the table uses, the chain fields `chain_at`
 * clears for a row of that scope, written by calling the function rather than
 * by reading its source (#2376).
 */
export interface MatrixArtifact {
  roles: RbacRoleView[];
  resources: RbacResourceView[];
  chain_at: Record<string, ChainField[]>;
}

/** Read a matrix file, refusing one that is not the shape the fixtures derive from. */
export function readMatrix(path: string): MatrixArtifact {
  return parseMatrix(readFileSync(path, "utf8"), path);
}

/** Parse a matrix read from `path`, refusing one the fixtures would have to guess at. */
export function parseMatrix(text: string, path: string): MatrixArtifact {
  const parsed = JSON.parse(text) as Partial<MatrixArtifact>;
  if (!Array.isArray(parsed.roles) || !Array.isArray(parsed.resources)) {
    throw new Error(`${path} carries no \`roles\` and \`resources\` arrays`);
  }
  // `chainAt` in `src/lib/rbac-capabilities.ts` is pinned to this table, so a
  // scope missing from it would pass the pin while the port guessed its rule
  const chainAt = parsed.chain_at;
  if (chainAt === null || typeof chainAt !== "object" || Array.isArray(chainAt)) {
    throw new Error(`${path} carries no \`chain_at\` table`);
  }
  const unknown = Object.entries(chainAt).flatMap(([scope, fields]) =>
    Array.isArray(fields)
      ? fields.filter((f) => !CHAIN_FIELDS.includes(f)).map((f) => `${scope}:${f}`)
      : [`${scope}:${String(fields)}`],
  );
  if (unknown.length > 0) {
    throw new Error(`${path} clears chain fields it does not name: ${unknown.join(", ")}`);
  }
  const unscoped = [...new Set(parsed.resources.map((r) => r.scope))].filter(
    (scope) => !(scope in chainAt),
  );
  if (unscoped.length > 0) {
    throw new Error(`${path} gives no \`chain_at\` rule for scope ${unscoped.join(", ")}`);
  }
  // `src/lib/rbac-capabilities.ts` reads each action as exactly one authority,
  // the way `resource_view` writes it; refuse a view it would have to guess at
  const ambiguous = parsed.resources.flatMap(({ resource, actions }) =>
    actions
      .filter(
        (a) =>
          [a.minimum_role !== null, a.superadmin_only, a.authenticated_only].filter(Boolean)
            .length !== 1,
      )
      .map((a) => `${resource}:${a.action}`),
  );
  if (ambiguous.length > 0) {
    throw new Error(`${path} names no single authority for ${ambiguous.join(", ")}`);
  }
  return parsed as MatrixArtifact;
}
