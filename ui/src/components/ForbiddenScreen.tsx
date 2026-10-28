import type * as React from "react";
import { useTranslation } from "react-i18next";

import { LoadError } from "@/components/LoadError";
import { PageBody } from "@/components/screen";
import { FORBIDDEN, useDeploymentGate, type Capability } from "@/lib/can";

/**
 * A screen this caller may not read, said before the request rather than after
 * it (#1183).
 *
 * The same `forbidden` LoadError a real 403 renders — the operator should not
 * be able to tell the two apart, because they mean the same thing. What
 * changes is that the deployment-scoped settings screens no longer spend a
 * request, a spinner and a wrong-looking failure to find out something the
 * effective-permissions answer already said.
 */
export function ForbiddenScreen({
  /** the translated noun for what is refused, as `LoadError` takes it */
  resource,
}: {
  resource: string;
}) {
  return (
    <PageBody>
      <LoadError error={FORBIDDEN} resource={resource} target="forbidden" />
    </PageBody>
  );
}

/**
 * Wrap a deployment-scoped settings screen so a caller the capability table
 * refuses never mounts it.
 *
 * These screens — feature flags, the runtime policy, the security settings, the
 * whole `scope: "deployment"` half of the capability table — belong to the admin
 * token and superadmins, and to the deployment role the table names for the
 * screen's `gate` (an operator, or a security auditor for the read-only ones,
 * #1834). Before this they loaded, spun, and rendered a 403 that looked like an
 * outage. Now the screen is not mounted at all, so it sends no request to be
 * refused.
 *
 * A wrapper rather than a check inside each screen: the screen keeps its hooks
 * and its shape, and the gate cannot be defeated by a query that runs above the
 * early return.
 *
 * `resourceKey` is a catalog key rather than a translated noun, because the
 * wrapping happens at module scope where there is no `t` yet — the same trick
 * `useScope` uses for its error copy. `gate` is the `resource:read` pair the
 * screen's list request is guarded by.
 */
export function superadminOnly<P extends object>(
  Screen: React.ComponentType<P>,
  resourceKey: string,
  gate: Capability,
): React.FC<P> {
  function Gated(props: P) {
    const { t } = useTranslation();
    const decision = useDeploymentGate(gate);
    // only an explicit "no" blocks: while the answer is unknown the screen
    // loads and gets its 403 the way it always did
    if (decision.blocked) return <ForbiddenScreen resource={t(resourceKey)} />;
    return <Screen {...props} />;
  }
  Gated.displayName = `SuperadminOnly(${Screen.displayName ?? Screen.name})`;
  return Gated;
}
