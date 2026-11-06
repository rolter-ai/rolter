import type { UseQueryResult } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "react-router";

import { LoadingRegion } from "@/components/LoadingState";
import { Skeleton } from "@/components/ui/skeleton";
import { Tooltip } from "@/components/ui/tooltip";
import type { AnalyticsByModelRow, ClusterNodeRow, RouteRow, RouteVariantRow } from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { trafficSplit } from "@/lib/route-variants";
import { shareBarColor } from "@/lib/share-bars";
import { cn } from "@/lib/utils";

// the parts of a route the Model Catalog shows beside its row, which Routing
// Rules used to show as cards until the two screens became one (#3685)

/** how far back the catalog counts a route's requests, in hours */
export const TRAFFIC_WINDOW_HOURS = 24;

/** one route's requests and errors over the window, from the by-model rollup */
export interface RouteTraffic {
  requests: number;
  errors: number;
}

/**
 * The rollup keys on the public model name and is read with the route's
 * project filter (#3101), so two projects that define a route with one model
 * name each see their own figure; a route nothing called has no row. The name
 * alone is a safe key here even though two organizations may share it
 * (#1857): a project belongs to one organization, so the filtered read holds
 * no other organization's rows (#3460).
 */
export function trafficFor(rows: AnalyticsByModelRow[], model: string): RouteTraffic {
  const row = rows.find((r) => r.model === model);
  return { requests: Number(row?.requests ?? 0), errors: Number(row?.errors ?? 0) };
}

/** the live gateways, and how many of them have not reached the latest config */
export function gatewayConvergence(nodes: ClusterNodeRow[]): { live: number; lagging: number } {
  const live = nodes.filter(
    (n) => n.role === "gateway" && n.live && n.desired_state !== "draining",
  );
  return { live: live.length, lagging: live.filter((n) => !n.converged).length };
}

/**
 * Whether the gateways have the config the control plane holds, from the
 * cluster inventory (PRODUCT.md principle 5: say what was saved and when it
 * reached the gateway). A lagging gateway links Cluster, which names it; no
 * live gateway says nothing, since there is nothing to have caught up.
 */
export function GatewayConfigNote({ nodes }: { nodes: ClusterNodeRow[] }) {
  const { t } = useTranslation();
  const { live, lagging } = gatewayConvergence(nodes);
  if (live === 0) return null;
  return (
    <p
      role="status"
      data-testid="gateway-config"
      className={cn(
        "max-w-[72ch] text-xs leading-snug",
        lagging > 0 ? "text-[color:var(--status-warning-text)]" : "text-muted-foreground",
      )}
    >
      {lagging > 0
        ? t("pages.models.gatewaysBehind", { count: lagging, live })
        : t("pages.models.gatewaysCurrent", { count: live })}{" "}
      <Link to="/cluster" className="underline underline-offset-2">
        {t("pages.models.gatewaysLink")}
      </Link>
    </p>
  );
}

/**
 * A route's requests over the window and the share of them that failed, for
 * the catalog's traffic column.
 *
 * The rollup is read for the scope's project, so only that project's routes
 * have a figure; any other row says why it has none. While the rollup is out
 * the cell holds a skeleton, and where it could not be read the cell is a dash
 * under the screen's one note saying why, never a zero.
 */
export function RouteTrafficCell({
  traffic,
  inProject,
  model,
}: {
  traffic: UseQueryResult<AnalyticsByModelRow[]>;
  /** whether the route belongs to the scope's project, the one the rollup counts */
  inProject: boolean;
  model: string;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const dash = "font-mono text-xs text-[color:var(--text-subtle)]";
  if (!inProject) {
    return (
      <Tooltip className={dash} content={t("pages.models.traffic.outsideProject")}>
        —
      </Tooltip>
    );
  }
  if (traffic.isPending) {
    return (
      <LoadingRegion testId="route-traffic-loading">
        <Skeleton width="70%" height={14} radius={4} />
      </LoadingRegion>
    );
  }
  if (!traffic.isSuccess || !traffic.data) return <span className={dash}>—</span>;
  const { requests, errors } = trafficFor(traffic.data, model);
  if (requests === 0) {
    return (
      <span data-testid="route-traffic" className="text-xs text-[color:var(--text-subtle)]">
        {t("pages.models.traffic.none")}
      </span>
    );
  }
  return (
    <span data-testid="route-traffic" className="flex min-w-0 flex-col gap-0.5">
      <span className="font-mono text-xs text-foreground">{fmt.number(requests)}</span>
      <span className="text-2xs text-[color:var(--text-secondary)]">
        {t("pages.models.traffic.errors", { share: fmt.percent(errors / requests, 1) })}
      </span>
    </span>
  );
}

/**
 * A route that routes through weighted variants (#3343, #391).
 *
 * The gateway samples one variant by weight and tries the others in the order
 * they are declared when it fails, and while a route has variants its own
 * target list is not used. So the list shows the variants in that fallback
 * order with the share each one's weight gives it (computed the way the
 * gateway samples, `trafficSplit`), each with the targets it tries, and says
 * once how many of the route's own targets are being set aside rather than
 * drawing a split the gateway does not make. A route with variants and no
 * targets of its own never reads as having none.
 */
export function RouteVariants({
  route,
  variants,
  unusedTargets,
  providerName,
}: {
  route: RouteRow;
  variants: RouteVariantRow[];
  /** how many of the route's own targets the variants set aside */
  unusedTargets: number;
  providerName: (id: string) => string;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const ordered = [...variants].sort((a, b) => a.position - b.position);
  const shares = trafficSplit(ordered.map((v) => v.weight));
  return (
    <div className="flex max-w-[60rem] flex-col gap-2.5">
      {unusedTargets > 0 && (
        <p className="text-xs leading-snug text-muted-foreground">
          {t("pages.models.variants.targetsUnused", { count: unusedTargets })}
        </p>
      )}
      <ol
        aria-label={t("pages.models.variants.label", { model: route.model })}
        className="flex flex-col gap-2.5"
      >
        {ordered.map((v, i) => (
          <li key={v.id} className="flex flex-col gap-[5px]">
            <div className="flex items-center gap-2 font-mono text-xs">
              <span className="min-w-0 truncate text-[color:var(--text-secondary)]">{v.name}</span>
              <span className="ml-auto text-[color:var(--text-secondary)]">
                {fmt.percent(shares[i] ?? 0, 0)}
              </span>
            </div>
            <div
              data-testid="variant-share"
              className="h-[5px] overflow-hidden rounded-full bg-[color:var(--surface-subtle)]"
            >
              <div
                className="h-full rounded-full"
                style={{
                  // a CSS length, never a localized percentage
                  width: `${Math.round((shares[i] ?? 0) * 100)}%`,
                  background: shareBarColor(i),
                }}
              />
            </div>
            <p className="truncate font-mono text-2xs text-[color:var(--text-subtle)]">
              {[...v.targets]
                .sort((a, b) => a.position - b.position)
                .map(
                  (tg) => `${providerName(tg.provider_id)} → ${tg.upstream_model || route.model}`,
                )
                .join(", ")}
            </p>
          </li>
        ))}
      </ol>
    </div>
  );
}
