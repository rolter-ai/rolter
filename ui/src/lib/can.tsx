import { useQuery } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import {
  ApiError,
  fetchEffective,
  fetchRbacMatrix,
  type RbacAction,
  type RbacEffective,
  type RbacMatrix,
  type Role,
} from "@/lib/api";
import { useOptionalAuth } from "@/lib/auth";
import { useScope } from "@/lib/scope";

// What this caller may actually do, asked once per scope and answered by the
// control plane (#1183).
//
// Before this the dashboard gated on nothing: every create, edit and delete
// control rendered for every signed-in account, and the only feedback was the
// 403 that came back as a generic ApiError after the click. The button had
// already promised something the server was always going to refuse.
//
// `GET /api/v1/rbac/effective` (crates/rolter-control/src/rbac_matrix.rs) is
// the answer, evaluated server-side from the caller's memberships and access
// profiles. It is advisory here and authoritative only there — the guard still
// runs on every request, so this layer never has to be right for the
// deployment to be safe. It only has to be *honest*, which is why the two
// uncertain cases below both fall open.

/** A `resource:action` pair, exactly as the wire format spells it. */
export type Capability = `${string}:${RbacAction}`;

/**
 * The three answers a capability question has.
 *
 * `undefined` is not a maybe-no: it is "not known yet", and every caller
 * renders it as *allowed*. A control that starts disabled and enables itself a
 * request later reads as broken, and it would disable itself for the whole
 * page load of any deployment whose control plane is a version behind and 404s
 * the endpoint.
 */
export type Permission = boolean | undefined;

export interface CapabilityValue {
  /** null while loading, or when the question could not be answered */
  effective: RbacEffective | null;
  /** the published rules, for naming the role a denied control would take */
  matrix: RbacMatrix | null;
  /** true once the answer is in — `false` while it is still unknown */
  resolved: boolean;
}

const CapabilityContext = React.createContext<CapabilityValue | null>(null);

/**
 * Whether `effective` permits the pair, given how far the query got.
 *
 * Kept a plain function so the semantics are unit-testable without a renderer:
 * they are the whole of this module's behaviour, and they are the part that
 * decides whether an operator can press a button.
 */
export function decide(
  value: CapabilityValue | null,
  resource: string,
  action: RbacAction,
): Permission {
  // no provider above (a story, a test) or the answer is still in flight
  if (!value || !value.resolved) return undefined;
  // asked and not answered — the control plane is old, unreachable, or refused
  // the question itself. falling closed here would empty the dashboard over a
  // failure that has nothing to do with the caller's role, so the 403 stays
  // the backstop it already was
  if (!value.effective) return undefined;
  if (value.effective.superadmin) return true;
  return value.effective.allowed.includes(`${resource}:${action}`);
}

/** Whether the caller is the admin token or a superadmin; `undefined` until known. */
export function decideSuperadmin(value: CapabilityValue | null): Permission {
  if (!value || !value.resolved || !value.effective) return undefined;
  return value.effective.superadmin;
}

/**
 * What a denied pair would take, read out of the published matrix.
 *
 * `superadmin` and `null` are different answers: the first names an authority
 * nobody can be granted at a scope, the second means the matrix is not on hand
 * and the control can only say it is not permitted.
 */
export function requirementFor(
  matrix: RbacMatrix | null,
  resource: string,
  action: RbacAction,
): Role | "superadmin" | null {
  const view = matrix?.resources.find((r) => r.resource === resource);
  const entry = view?.actions.find((a) => a.action === action);
  if (!entry) return null;
  if (entry.superadmin_only) return "superadmin";
  return entry.minimum_role;
}

/**
 * One effective-capabilities query for everything below it.
 *
 * Keyed on the org/team/project chain, so switching scope re-asks — a viewer in
 * one org can be an admin in the next, and a cached "no" would follow them
 * there. Mounted inside the session so it never fires for a signed-out tab.
 */
export function CapabilityProvider({ children }: { children: React.ReactNode }) {
  const scope = useScope();
  const auth = useOptionalAuth();
  const chain = {
    orgId: scope.orgId,
    teamId: scope.teamId,
    projectId: scope.projectId,
  };
  // asking before the chain resolves would answer for the empty scope and then
  // answer again for the real one, which is a flash of "denied" on every load.
  // a signed-out tab has nothing to ask about either — but outside a session
  // provider entirely (a story, a test) there is no session to be missing, so
  // that case asks
  const signedIn = auth ? !!auth.email : true;
  const enabled = signedIn && !scope.isLoading;
  const effective = useQuery({
    queryKey: ["rbac", "effective", chain.orgId, chain.teamId, chain.projectId],
    queryFn: () => fetchEffective(chain),
    enabled,
    // a role does not change between two clicks; a minute is short enough that
    // a grant made in another tab is picked up without a reload
    staleTime: 60_000,
    retry: false,
  });
  // the rules, not the caller's answer — only ever read to name the role a
  // disabled control would take, so its failure costs a sentence, not a gate
  const matrix = useQuery({
    queryKey: ["rbac", "matrix", chain.orgId],
    queryFn: () => fetchRbacMatrix(chain.orgId),
    enabled,
    staleTime: 300_000,
    retry: false,
  });

  const value = React.useMemo<CapabilityValue>(
    () => ({
      effective: effective.data ?? null,
      matrix: matrix.data ?? null,
      resolved: enabled && !effective.isPending,
    }),
    [effective.data, effective.isPending, matrix.data, enabled],
  );

  return (
    <CapabilityContext.Provider value={value}>{children}</CapabilityContext.Provider>
  );
}

/** The raw context, for the components that need more than a yes/no. */
export function useCapabilities(): CapabilityValue | null {
  return React.useContext(CapabilityContext);
}

/**
 * `can(resource, action)` for the current scope.
 *
 * Returns `undefined` while the answer is unknown; a caller disables on an
 * explicit `false` and never on the absence of an answer.
 */
export function useCan(): (resource: string, action: RbacAction) => Permission {
  const value = useCapabilities();
  return React.useCallback(
    (resource, action) => decide(value, resource, action),
    [value],
  );
}

/**
 * What one gated control has to know: whether it is refused, and what to say.
 *
 * The two go together — "disabled" on its own is the same non-answer the 403
 * was — so every gated control reads them from here rather than each deciding
 * for itself. `GatedButton` was the first caller; the per-row icon buttons and
 * toggles are the rest of them (#1258).
 */
export interface GateState {
  /** an explicit "no" from the control plane, never a "not known yet" */
  denied: boolean;
  /** the role a refused control would take, ready for a `title` */
  reason: string | undefined;
}

/**
 * Resolve `resource:action` into a disabled flag and the sentence that explains it.
 *
 * `gate` is optional so a component can accept the prop and stay ungated when
 * it is omitted: a row control that never had a capability to check must not
 * start claiming one.
 */
export function useGate(gate: Capability | undefined): GateState {
  const { t } = useTranslation();
  const value = useCapabilities();
  const can = useCan();
  if (!gate) return { denied: false, reason: undefined };
  const [resource, action] = splitCapability(gate);
  // only an explicit "no" disables; `undefined` is "not known yet", and a
  // control that starts disabled and enables itself a request later reads as
  // broken
  const denied = can(resource, action) === false;
  if (!denied) return { denied: false, reason: undefined };
  const requirement = requirementFor(value?.matrix ?? null, resource, action);
  const reason =
    requirement === "superadmin"
      ? t("rbac.needsSuperadmin")
      : requirement
        ? t("rbac.needsRole", { role: t(`shell.roles.${requirement}`) })
        : t("rbac.needsPermission");
  return { denied: true, reason };
}

/**
 * `"provider:create"` → `["provider", "create"]`.
 *
 * A resource never contains a colon, so the split is unambiguous. Typed through
 * `Capability`, so a pair the wire format could not carry does not compile.
 */
export function splitCapability(gate: Capability): [string, RbacAction] {
  const at = gate.lastIndexOf(":");
  return [gate.slice(0, at), gate.slice(at + 1) as RbacAction];
}

/** Whether this caller is a superadmin; `undefined` until the answer is in. */
export function useIsSuperadmin(): Permission {
  return decideSuperadmin(useCapabilities());
}

/**
 * The gate a deployment-scoped settings screen puts in front of itself.
 *
 * `blocked` is only ever true on an explicit "not a superadmin", so a screen
 * whose gate could not be answered still loads and still gets its 403 the old
 * way. `error` is the value to hand `LoadError`, which classifies it as
 * `forbidden` exactly as a real refusal would be.
 */
export function useSuperadminGate(): { blocked: boolean; error: unknown } {
  return { blocked: useIsSuperadmin() === false, error: FORBIDDEN };
}

/**
 * A 403 the dashboard raised itself, so a screen it already knows is refused
 * renders the same `forbidden` LoadError without sending the request first.
 *
 * Not a lie about the server: it is the answer the server would give, said one
 * request earlier. The message is deliberately empty — `LoadError` prints it as
 * the control plane's own words, and here there are none to quote.
 */
export const FORBIDDEN: ApiError = new ApiError("", 403);
