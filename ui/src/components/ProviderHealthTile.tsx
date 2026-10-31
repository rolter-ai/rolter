import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "react-router";

import { AnalyticsUnavailable } from "@/components/AnalyticsUnavailable";
import { LoadError } from "@/components/LoadError";
import { LoadingRegion } from "@/components/LoadingState";
import { Skeleton } from "@/components/ui/skeleton";
import { StatCard, type StatTone } from "@/components/ui/stat-card";
import { AnalyticsUnavailableError, ApiError, fetchUptime } from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { rollUpProviders } from "@/lib/provider-health";
import { HEALTH_SLA } from "@/lib/route-targets";

// the dashboard home's provider tile (#3344): of the providers that reported
// health, how many meet the SLA, and the worst one by name. one read answers it,
// `GET /api/v1/health/uptime`, which already returns every provider the caller's
// roles reach in a single rollup, so no per-provider fan-out is needed. the key
// is the Health screen's, so the two share one cached answer and cannot disagree

/** how often the tile re-reads; the rollup is hourly buckets over a week, so a minute is plenty */
const POLL_MS = 60_000;

/**
 * The health read, with a control plane that has no ClickHouse (a 503 on
 * `/api/v1/health/*`) told apart from a failure: it is a deployment shape and
 * is stated as one, not as an alert.
 */
async function readUptime() {
  try {
    // a body with no rows reads as none reported, as the other readers of this rollup do
    return (await fetchUptime(HEALTH_SLA)) ?? [];
  } catch (error) {
    if (error instanceof ApiError && error.status === 503) {
      throw new AnalyticsUnavailableError(error.message);
    }
    throw error;
  }
}

export function ProviderHealthTile({ pollMs = POLL_MS }: { pollMs?: number }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const read = useQuery({
    queryKey: ["health-uptime", HEALTH_SLA],
    queryFn: readUptime,
    refetchInterval: pollMs,
    retry: false,
  });

  if (read.isLoading) {
    return (
      <LoadingRegion className="contents">
        <Skeleton height={96} radius={10} />
      </LoadingRegion>
    );
  }
  if (read.data === undefined) {
    if (!read.isError) return null;
    if (read.error instanceof AnalyticsUnavailableError) {
      return (
        <div className="sm:col-span-2" data-testid="dashboard-providers">
          <AnalyticsUnavailable
            error={read.error}
            i18nKey="pages.dashboard.providers.noAnalytics"
          />
        </div>
      );
    }
    return (
      <div className="sm:col-span-1 xl:col-span-2" data-testid="dashboard-providers">
        <LoadError
          error={read.error}
          resource={t("errors.resources.providerHealth")}
          onRetry={() => void read.refetch()}
          target="dashboard-providers"
        />
      </div>
    );
  }

  const roll = rollUpProviders(read.data);
  const label = (
    <Link
      to="/circuit-breaker"
      className="underline-offset-2 hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
    >
      {t("pages.dashboard.statProviders")}
    </Link>
  );
  // no provider has reported an event: nothing to measure, which is not an outage
  if (roll.total === 0) {
    return (
      <StatCard
        data-testid="dashboard-providers"
        label={label}
        value={t("pages.dashboard.notMeasured")}
        tone="neutral"
        delta={t("pages.dashboard.providers.none")}
      />
    );
  }
  const tone: StatTone = roll.breaching.length === 0 ? "good" : roll.up === 0 ? "bad" : "warning";
  const worst = roll.worst;
  const delta = worst
    ? t(
        worst.breaching
          ? "pages.dashboard.providers.worstBreaching"
          : "pages.dashboard.providers.worst",
        {
          provider: worst.provider,
          uptime: fmt.percent(worst.uptime, 2),
        },
      )
    : undefined;
  return (
    <StatCard
      data-testid="dashboard-providers"
      label={label}
      value={fmt.number(roll.up)}
      unit={t("pages.dashboard.providers.ofTotal", { total: roll.total })}
      tone={tone}
      delta={
        delta && roll.breaching.length > 1
          ? `${delta} · ${t("pages.dashboard.providers.more", { count: roll.breaching.length - 1 })}`
          : delta
      }
    />
  );
}
