import type * as React from "react";
import { useTranslation } from "react-i18next";

import { LoadError } from "@/components/LoadError";
import { PageBody } from "@/components/screen";
import { FORBIDDEN, useSuperadminGate } from "@/lib/can";

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
 * Wrap a deployment-scoped settings screen so a non-superadmin never mounts it.
 *
 * These screens — feature flags, the runtime policy, the security settings, the
 * whole `scope: "deployment"` half of the capability table — are the admin
 * token's alone. Before this they loaded, spun, and rendered a 403 that looked
 * like an outage. Now the screen is not mounted at all, so it sends no request
 * to be refused.
 *
 * A wrapper rather than a check inside each screen: the screen keeps its hooks
 * and its shape, and the gate cannot be defeated by a query that runs above the
 * early return.
 *
 * `resourceKey` is a catalog key rather than a translated noun, because the
 * wrapping happens at module scope where there is no `t` yet — the same trick
 * `useScope` uses for its error copy.
 */
export function superadminOnly<P extends object>(
  Screen: React.ComponentType<P>,
  resourceKey: string,
): React.FC<P> {
  function Gated(props: P) {
    const { t } = useTranslation();
    const gate = useSuperadminGate();
    // only an explicit "not a superadmin" blocks: while the answer is unknown
    // the screen loads and gets its 403 the way it always did
    if (gate.blocked) return <ForbiddenScreen resource={t(resourceKey)} />;
    return <Screen {...props} />;
  }
  Gated.displayName = `SuperadminOnly(${Screen.displayName ?? Screen.name})`;
  return Gated;
}
