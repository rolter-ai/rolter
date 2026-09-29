import { Trans, useTranslation } from "react-i18next";

import { StatusDot } from "@/components/screen";
import { useFormat } from "@/lib/i18n/format";
import { HEALTH_SLA, type RouteTargetView } from "@/lib/route-targets";
import { usesWeights } from "@/lib/strategies";
import { cn } from "@/lib/utils";

/**
 * Where a route's traffic goes, one line per target (#1979).
 *
 * A weight is shown as a share of traffic only under a strategy that reads
 * weights. Under any other strategy the gateway never consults them, so the
 * list says so once instead of printing a split that does not happen.
 */
export function RouteTargetList({
  id,
  label,
  strategy,
  targets,
  health = false,
  className,
}: {
  id?: string;
  /** names the list, e.g. "Targets of gpt-4o" */
  label: string;
  strategy: string;
  targets: RouteTargetView[];
  /** add the health dot and uptime column, for a screen that fetched health */
  health?: boolean;
  className?: string;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const weighted = usesWeights(strategy);
  const total = targets.reduce((sum, tg) => sum + Math.max(tg.weight, 0), 0) || 1;
  // one template for every line, and fixed widths for the figures, so the
  // columns line up down the list whatever each line holds
  const columns = [
    health && "7px",
    "minmax(0,1fr)",
    "minmax(0,1.5fr)",
    "5.5rem",
    weighted && "7.5rem",
    health && "12.5rem",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div id={id} className={cn("space-y-2", className)}>
      {!weighted && targets.length > 1 && (
        <p className="text-xs leading-snug text-muted-foreground">
          <Trans
            i18nKey="routeTargets.weightsIgnored"
            values={{ strategy }}
            components={[<span key="strategy" className="font-mono text-foreground" />]}
          />
        </p>
      )}
      <ul aria-label={label} className="grid max-w-[60rem] gap-1.5">
        {targets.map((tg, i) => (
          <li
            key={`${tg.provider}-${tg.upstream}-${i}`}
            className="grid items-center gap-x-3 font-mono text-xs"
            style={{ gridTemplateColumns: columns }}
          >
            {health && (
              <StatusDot
                color={
                  !tg.health
                    ? "var(--zinc-500)"
                    : tg.health.breached
                      ? "var(--status-danger)"
                      : "var(--status-success)"
                }
              />
            )}
            <span className="truncate text-[color:var(--text-secondary)]">{tg.provider}</span>
            <span className="truncate text-muted-foreground">{tg.upstream}</span>
            <span className="whitespace-nowrap text-right text-[color:var(--text-secondary)]">
              {t("routeTargets.weight", { weight: fmt.number(tg.weight) })}
            </span>
            {weighted && (
              <span className="whitespace-nowrap text-right text-foreground">
                {t("routeTargets.share", {
                  share: fmt.percent(Math.max(tg.weight, 0) / total, 0),
                })}
              </span>
            )}
            {health && (
              <span
                className={cn(
                  "whitespace-nowrap text-right",
                  !tg.health
                    ? "text-[color:var(--text-subtle)]"
                    : tg.health.breached
                      ? "text-[color:var(--status-danger-text)]"
                      : "text-[color:var(--text-secondary)]",
                )}
              >
                {!tg.health
                  ? t("routeTargets.noHealth")
                  : t(tg.health.breached ? "routeTargets.belowSla" : "routeTargets.uptime", {
                      uptime: fmt.percent(tg.health.uptime, 2),
                    })}
              </span>
            )}
          </li>
        ))}
      </ul>
      {health && (
        <p className="text-[11px] leading-snug text-[color:var(--text-subtle)]">
          {t("routeTargets.healthWindow", { sla: fmt.percent(HEALTH_SLA, 0) })}
        </p>
      )}
    </div>
  );
}
