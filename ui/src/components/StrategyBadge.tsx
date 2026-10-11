import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

/**
 * A balancing strategy, shown the one way everywhere (#2229).
 *
 * Verbatim and in mono, on the neutral tone: a strategy is a config value an
 * operator copies into `rolter.toml` (#1979), not a status, so it is neither
 * restyled nor tinted with a status hue.
 */
export function StrategyBadge({ strategy, className }: { strategy: string; className?: string }) {
  return (
    <Badge
      tone="neutral"
      className={cn("min-w-0 max-w-full font-mono", className)}
      title={strategy}
    >
      <span className="truncate">{strategy}</span>
    </Badge>
  );
}
