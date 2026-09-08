import type { RbacAction, RbacActionView, RbacEffective, RbacMatrix, Role } from "@/lib/api";
import snapshot from "@/lib/rbac-capabilities.json";

// The control plane's capability table, and the two RBAC payloads derived from
// it the way the control plane derives them (#1298).
//
// `src/lib/rbac-capabilities.json` is a generated copy of `CAPABILITIES` in
// `crates/rolter-control/src/rbac_matrix.rs` — `bun run gen:rbac` writes it and
// `scripts/rbac-matrix-source.test.ts` fails the build when the two disagree.
// The gating stories stub both endpoints from here rather than from a
// hand-written table, so a resource, action or authority added to the control
// plane cannot go missing from the roles the stories render as.
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

/** The table, in the order the control plane publishes it. */
export const CAPABILITIES = snapshot.capabilities as CapabilityRow[];

/** every action, in the order the matrix presents them (`Action::ALL`) */
export const ACTIONS: RbacAction[] = ["read", "create", "update", "delete"];

/** total order over roles: viewer `0` < member `1` < admin `2` (`role_rank`) */
const RANK: Record<Role, number> = { viewer: 0, member: 1, admin: 2 };

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

/** The port of `resource_view`: one published row per resource. */
function actionViews(capability: CapabilityRow): RbacActionView[] {
  return ACTIONS.filter((action) => capability[action] !== null).map((action) => {
    const authority = capability[action] as Authority;
    return {
      action,
      minimum_role:
        authority === "superadmin" || authority === "authenticated" ? null : (authority as Role),
      superadmin_only: authority === "superadmin",
      authenticated_only: authority === "authenticated",
    };
  });
}

/** `GET /api/v1/rbac/matrix`, as this deployment's table publishes it. */
export function matrixFixture(): RbacMatrix {
  return {
    roles: [
      { role: "viewer", rank: RANK.viewer },
      { role: "member", rank: RANK.member },
      { role: "admin", rank: RANK.admin },
    ],
    resources: CAPABILITIES.map((capability) => ({
      resource: capability.resource,
      scope: capability.scope,
      actions: actionViews(capability),
    })),
    custom_roles: [],
  };
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
