import { useTranslation } from "react-i18next";

import { StatCard, type StatTone } from "@/components/ui/stat-card";
import { useGatewayHealth, type GatewayHealth } from "@/lib/gateway-health";
import { useFormat } from "@/lib/i18n/format";

// the dashboard home's gateway tile. it reads `useGatewayHealth`, the hook behind
// the header's pill, so the two cannot disagree (#3263): the cluster summary when
// the control plane counted any gateway, the `/readyz` probe when it could not
// the delta line carries what the colour alone would not say, and a probe says
// outright that it saw one gateway and could not count the fleet

const TONE: Record<GatewayHealth, StatTone> = {
  checking: "neutral",
  healthy: "good",
  // degraded is amber here as in the header pill: after every rollout a stale
  // gateway holds both there for 15 minutes, and red would call that an outage
  degraded: "warning",
  down: "bad",
  unknown: "neutral",
};

export function FleetHealthTile() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const { health, fleet, refusal } = useGatewayHealth();
  const value = fleet
    ? fmt.number(fleet.live)
    : t(`pages.dashboard.fleet.probe.${refusal ?? health}`);
  const unit = fleet ? t("pages.dashboard.fleet.liveUnit") : undefined;
  let delta: string | undefined;
  if (fleet) {
    const parts = [
      fleet.live === 0 ? t("pages.dashboard.fleet.none") : undefined,
      fleet.lagging > 0 ? t("pages.dashboard.fleet.lagging", { count: fleet.lagging }) : undefined,
      fleet.stale > 0 ? t("pages.dashboard.fleet.stale", { count: fleet.stale }) : undefined,
    ].filter(Boolean);
    delta = parts.length > 0 ? parts.join(" · ") : t("pages.dashboard.fleet.allCurrent");
  } else if (health === "unknown") {
    delta = t("pages.dashboard.fleet.unknownNote");
  } else if (health !== "checking") {
    delta = t("pages.dashboard.fleet.probeNote");
  }
  return (
    <StatCard
      data-testid="dashboard-fleet"
      label={t("pages.dashboard.statFleet")}
      value={value}
      unit={unit}
      tone={TONE[health]}
      delta={delta}
    />
  );
}
