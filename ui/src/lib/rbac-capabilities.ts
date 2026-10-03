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
 * One membership, by the scope ids it carries (`Membership`).
 *
 * Interpreted by its most-specific id, as the control plane interprets it: a
 * project membership reaches the chains naming its project, a team membership
 * those naming its team (and so every project below it), an org membership
 * those naming its org.
 */
export interface Membership {
  role: Role;
  orgId?: string | null;
  teamId?: string | null;
  projectId?: string | null;
}

/** The org/team/project chain `rbac/effective` is asked at (`ScopeChain`). */
export interface ScopeChain {
  orgId: string | null;
  teamId: string | null;
  projectId: string | null;
}

/** Who is asking, and at which chain. */
export interface Caller {
  memberships: Membership[];
  chain: ScopeChain;
}

/**
 * A caller, or a bare role standing for one org membership asked at a whole
 * chain — the shape of `as_role` in the Rust tests, which an org membership
 * makes chain-independent: it reaches the org, team and project parts alike.
 */
export type Holder = Role | null | Caller;

const WHOLE_CHAIN = { orgId: "org", teamId: "team", projectId: "project" } as const;

function callerOf(holder: Holder): Caller {
  if (holder !== null && typeof holder === "object") return holder;
  return {
    memberships: holder ? [{ role: holder, orgId: WHOLE_CHAIN.orgId }] : [],
    chain: { ...WHOLE_CHAIN },
  };
}

/**
 * The chain fields `chain_at` clears for a row of each scope: an org-scoped
 * guard asks at the org alone, a team-scoped one at org + team, and anything
 * else at the whole chain. `scripts/rbac-matrix-artifact.test.ts` pins this to
 * the `chain_at` table the rolter-control test suite writes into
 * `crates/rolter-control/rbac-matrix.json` by calling `chain_at` itself.
 */
export const CHAIN_TRIMS: Record<string, (keyof ScopeChain)[]> = {
  org: ["teamId", "projectId"],
  team: ["projectId"],
};

/** The port of `chain_at`: the part of `chain` a `scope` row is decided at. */
export function chainAt(scope: string, chain: ScopeChain): ScopeChain {
  const at = { ...chain };
  for (const field of CHAIN_TRIMS[scope] ?? []) at[field] = null;
  return at;
}

/**
 * The port of `scope_specificity`: `3` project, `2` team, `1` org, or `null`
 * when the membership's most-specific id is not the chain's.
 */
function specificity(m: Membership, chain: ScopeChain): number | null {
  if (m.projectId) return m.projectId === chain.projectId ? 3 : null;
  if (m.teamId) return m.teamId === chain.teamId ? 2 : null;
  if (m.orgId) return m.orgId === chain.orgId ? 1 : null;
  return null;
}

/**
 * The port of `resolve_role`: the most specific membership reaching `chain`
 * wins, a tie goes to the higher role, and none reaching it is `null`. A
 * project viewer in an org they administer is therefore a viewer there.
 */
export function resolveRole(memberships: Membership[], chain: ScopeChain): Role | null {
  let best: { specificity: number; role: Role } | null = null;
  for (const m of memberships) {
    const s = specificity(m, chain);
    if (s === null) continue;
    if (
      !best ||
      s > best.specificity ||
      (s === best.specificity && RANK[m.role] > RANK[best.role])
    ) {
      best = { specificity: s, role: m.role };
    }
  }
  return best?.role ?? null;
}

/**
 * Whether the caller reaches `authority` on a row, as `allowed_for` decides it.
 *
 * A superadmin holds every supported action, an `authenticated` catalog is
 * open to every signed-in caller, and a scoped action takes a role, resolved
 * at the row's own part of the chain, that ranks at or above the one the
 * table names. Access-profile grants are not modelled: a story that needs one
 * stubs the endpoint itself.
 */
function permits(authority: Authority, role: Role | null, superadmin: boolean): boolean {
  if (authority === "superadmin") return superadmin;
  if (authority === "authenticated") return true;
  return superadmin || (role !== null && RANK[role] >= RANK[authority]);
}

/**
 * The `resource:action` pairs `holder` may perform.
 *
 * The port of `allowed_for` — default-deny, table order, each row decided at
 * the part of the chain its `scope` names (#2376), and an action the table
 * does not define for a resource is absent for everyone.
 */
export function allowedFor(holder: Holder, superadmin = false): string[] {
  const { memberships, chain } = callerOf(holder);
  const allowed: string[] = [];
  for (const capability of CAPABILITIES) {
    const role = resolveRole(memberships, chainAt(capability.scope, chain));
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
 * `role` is the one `get_effective` resolves at the whole chain asked about,
 * while `allowed` decides each row at its own part of it — so a team admin's
 * `role` is `admin` and their `allowed` still lacks every org-scoped write.
 * A superadmin's `role` is `null` on the wire — `Principal::Superadmin` holds
 * no membership to resolve one from — and their `allowed` list is the whole
 * table rather than empty, because `allowed_for` enumerates it for them too.
 */
export function effectiveFor(holder: Holder, superadmin = false): RbacEffective {
  const caller = callerOf(superadmin ? null : holder);
  return {
    superadmin,
    role: superadmin ? null : resolveRole(caller.memberships, caller.chain),
    allowed: allowedFor(caller, superadmin),
    custom_roles: [],
    model_policy: null,
  };
}
