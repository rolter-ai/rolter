import type {
  RbacAction,
  RbacActionView,
  RbacEffective,
  RbacMatrix,
  RbacResourceView,
  RbacRoleView,
  Role,
} from "@/lib/api";
import snapshot from "@/lib/rbac-capabilities.json";

// The control plane's capability matrix, and the two RBAC payloads the gating
// stories stub from it (#1298, #1369).
//
// `src/lib/rbac-capabilities.json` is a copy of
// `crates/rolter-control/rbac-matrix.json`, which a rolter-control unit test
// writes from `CAPABILITIES` and verifies on every `cargo test`: it *is*
// `GET /api/v1/rbac/matrix` minus the per-tenant custom roles. `bun run
// gen:rbac` refreshes the copy and `scripts/rbac-matrix-artifact.test.ts`
// fails the build while the two differ. So the matrix payload needs no port of
// the control plane's rendering, and the effective payload is derived from it
// the way `allowed_for` derives it.
//
// Test-and-story fixture only: nothing the dashboard ships imports it, and the
// real screens read the answers off the wire like any deployment does.

/** The authority an action takes, spelled as the control plane's `Authority` spells it. */
export type Authority = "viewer" | "member" | "admin" | "superadmin" | "authenticated";

/** One row of the table: an authority per action, `null` where there is no such action. */
export interface CapabilityRow {
  resource: string;
  scope: string;
  read: Authority | null;
  create: Authority | null;
  update: Authority | null;
  delete: Authority | null;
}

const ROLES = snapshot.roles as RbacRoleView[];
const RESOURCES = snapshot.resources as RbacResourceView[];

/** every action, in the order the matrix presents them (`Action::ALL`) */
export const ACTIONS: RbacAction[] = ["read", "create", "update", "delete"];

/** The one authority a published action view stands for. */
function authorityOf(view: RbacActionView): Authority {
  if (view.superadmin_only) return "superadmin";
  if (view.authenticated_only) return "authenticated";
  // `resource_view` sets exactly one of the three, and `readMatrix` in
  // `scripts/rbac-matrix-artifact.ts` refuses a copy that does not; the
  // fallback is the least generous reading, never an open door
  return view.minimum_role ?? "superadmin";
}

/** The table, one row per resource, in the order the control plane publishes it. */
export const CAPABILITIES: CapabilityRow[] = RESOURCES.map(({ resource, scope, actions }) => {
  const row: CapabilityRow = {
    resource,
    scope,
    read: null,
    create: null,
    update: null,
    delete: null,
  };
  for (const view of actions) row[view.action] = authorityOf(view);
  return row;
});

/** total order over roles, as the matrix ranks them (`role_rank`) */
const RANK = Object.fromEntries(ROLES.map(({ role, rank }) => [role, rank])) as Record<
  Role,
  number
>;

/**
 * Whether `role` reaches `authority`, as `allowed_for` decides it.
 *
 * A superadmin holds every supported action, an `authenticated` catalog is
 * open to every signed-in caller, and a scoped action takes a role that ranks
 * at or above the one the table names. Access-profile grants are not modelled:
 * a story that needs one stubs the endpoint itself.
 */
function permits(authority: Authority, role: Role | null, superadmin: boolean): boolean {
  if (authority === "superadmin") return superadmin;
  if (authority === "authenticated") return true;
  return superadmin || (role !== null && RANK[role] >= RANK[authority]);
}

/**
 * The `resource:action` pairs a caller holding `role` may perform.
 *
 * The port of `allowed_for` — default-deny, table order, and an action the
 * table does not define for a resource is absent for everyone.
 */
export function allowedFor(role: Role | null, superadmin = false): string[] {
  const allowed: string[] = [];
  for (const capability of CAPABILITIES) {
    for (const action of ACTIONS) {
      const authority = capability[action];
      if (authority && permits(authority, role, superadmin)) {
        allowed.push(`${capability.resource}:${action}`);
      }
    }
  }
  return allowed;
}

/**
 * `GET /api/v1/rbac/matrix`, as this deployment's table publishes it.
 *
 * A fresh copy per call, so a story that edits its payload cannot leak the
 * edit into the next one.
 */
export function matrixFixture(): RbacMatrix {
  return structuredClone({ roles: ROLES, resources: RESOURCES, custom_roles: [] });
}

/**
 * `GET /api/v1/rbac/effective`, as the control plane would answer it.
 *
 * A superadmin's `role` is `null` on the wire — `get_effective` resolves it
 * from memberships and `Principal::Superadmin` holds none — and their
 * `allowed` list is the whole table rather than empty, because `allowed_for`
 * enumerates it for them too.
 */
export function effectiveFor(role: Role | null, superadmin = false): RbacEffective {
  return {
    superadmin,
    role: superadmin ? null : role,
    allowed: allowedFor(superadmin ? null : role, superadmin),
    custom_roles: [],
    model_policy: null,
  };
}
