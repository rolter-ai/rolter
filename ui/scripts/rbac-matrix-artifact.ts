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

/** The artifact's shape: the published matrix without `custom_roles`. */
export interface MatrixArtifact {
  roles: RbacRoleView[];
  resources: RbacResourceView[];
}

/** Read a matrix file, refusing one that is not the shape the fixtures derive from. */
export function readMatrix(path: string): MatrixArtifact {
  const parsed = JSON.parse(readFileSync(path, "utf8")) as Partial<MatrixArtifact>;
  if (!Array.isArray(parsed.roles) || !Array.isArray(parsed.resources)) {
    throw new Error(`${path} carries no \`roles\` and \`resources\` arrays`);
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
