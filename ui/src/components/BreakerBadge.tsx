import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { breakerLabel, type BreakerTargetDto } from "@/lib/api";

/**
 * A target's live circuit breaker, in words: "Breaker open on 2 gateways" or
 * "Probing on 1 gateway". It draws nothing for a closed or unlisted breaker, so
 * a healthy target keeps the row it had before breaker state existed. Danger
 * for open, warning for probing; the words carry the state, not the tone.
 */
export function BreakerBadge({
  breaker,
  className,
}: {
  breaker: BreakerTargetDto | undefined;
  className?: string;
}) {
  const { t } = useTranslation();
  const label = breakerLabel(breaker);
  if (!label || !breaker) return null;
  return (
    <Badge
      tone={breaker.state === "open" ? "danger" : "warning"}
      dot
      data-testid="breaker-badge"
      data-state={breaker.state}
      className={className}
    >
      {t(label.key, { count: label.count })}
    </Badge>
  );
}
