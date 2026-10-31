import { useTranslation } from "react-i18next";

import { Badge, type BadgeTone } from "@/components/ui/badge";
import type { CacheReporting } from "@/lib/cache-reporting";
import { useFormat } from "@/lib/i18n/format";

// unknown is neutral on purpose: a warning tone would read as a finding about
// a provider nobody has asked yet
const TONE: Record<CacheReporting["status"], BadgeTone> = {
  reports: "success",
  none: "warning",
  unprobed: "outline",
};

/**
 * Whether a provider (or the providers behind a route or group) reports
 * prompt-cache metrics, as established by a probe.
 *
 * The three states stay distinguishable by wording and tone: "reports"
 * (success), "no cache metrics" (warning) and "not probed yet" (outline). The
 * time of the verdict sits in the badge, and the title carries the model and
 * the field spelling it rests on, so a stale verdict can be seen as such.
 */
export function CacheReportingBadge({
  state,
  className,
}: {
  /** `null` is nothing known to show (verdicts unread), and renders nothing */
  state: CacheReporting | null;
  className?: string;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  if (!state) return null;
  const label = t(`cacheReporting.${state.status}`);
  const lines = [t(`cacheReporting.hint.${state.status}`)];
  if (state.status !== "unprobed") {
    lines.push(t("cacheReporting.established", { when: fmt.dateTime(state.probedAt) }));
    if (state.model) lines.push(t("cacheReporting.model", { model: state.model }));
    if (state.spelling) lines.push(t("cacheReporting.spelling", { spelling: state.spelling }));
  }
  if (state.total > 1) {
    lines.push(
      t(state.status === "unprobed" ? "cacheReporting.coverageProbed" : "cacheReporting.coverage", {
        count: state.count,
        total: state.total,
      }),
    );
  }
  return (
    <Badge tone={TONE[state.status]} className={className} title={lines.join("\n")}>
      {state.status === "unprobed"
        ? label
        : t("cacheReporting.withTime", { label, when: fmt.relative(state.probedAt) })}
    </Badge>
  );
}
