import * as React from "react";

import { cn } from "@/lib/utils";

/** which way a figure moved against the earlier window it was compared with */
export type StatTrend = "up" | "down" | "flat";

/** what that move means to whoever reads the tile */
export type StatTone = "good" | "bad" | "neutral";

/**
 * The grid a strip of stat cards sits in: one column on a phone, two from `sm`,
 * four from `xl`. `StatGridSkeleton` stands in for the strip with this same
 * grid, so the tiles do not change columns, and the page does not jump, when
 * the figures land (#1994).
 */
export const STAT_GRID = "grid grid-cols-1 gap-3.5 sm:grid-cols-2 xl:grid-cols-4";

// metric tile: label, big mono value, optional unit + delta
//
// direction and meaning are two props (#1974). an error rate or a latency that
// goes up is bad news, so the arrow cannot pick the colour on its own
export interface StatCardProps extends React.HTMLAttributes<HTMLDivElement> {
  label: React.ReactNode;
  value: React.ReactNode;
  unit?: React.ReactNode;
  delta?: React.ReactNode;
  /**
   * The direction of a measured change, drawn as an arrow before the delta.
   * Leave it out when the delta is not a comparison with an earlier window:
   * no arrow is drawn then, since one would claim a movement nobody measured.
   */
  trend?: StatTrend;
  /**
   * What the delta means, which picks its colour. Without it the tone follows
   * the arrow the way a count you want more of reads (up is good, down is bad,
   * anything else neutral), so pass it for a figure that is bad when it rises,
   * such as errors, latency or spend.
   */
  tone?: StatTone;
}

const ARROWS: Record<StatTrend, string> = {
  up: "M12 19V5M5 12l7-7 7 7",
  down: "M12 5v14M5 12l7 7 7-7",
  flat: "M5 12h14",
};

// the `-text` half of each status pair: the delta is a glyph, not a shape
const TONE_CLASS: Record<StatTone, string> = {
  good: "text-[color:var(--status-success-text)]",
  bad: "text-[color:var(--status-danger-text)]",
  neutral: "text-muted-foreground",
};

const TONE_OF_TREND: Record<StatTrend, StatTone> = {
  up: "good",
  down: "bad",
  flat: "neutral",
};

export function StatCard({
  label,
  value,
  unit,
  delta,
  trend,
  tone,
  className,
  ...props
}: StatCardProps) {
  const deltaTone = tone ?? (trend ? TONE_OF_TREND[trend] : "neutral");
  return (
    <div
      className={cn(
        "flex flex-col gap-1.5 rounded-lg border border-[color:var(--border-default)] bg-card p-4",
        className,
      )}
      {...props}
    >
      <span className="flex items-center gap-1.5 text-xs text-muted-foreground">{label}</span>
      <span className="font-mono text-[1.75rem] font-medium leading-none tracking-tight text-foreground">
        {value}
        {unit && <span className="ml-0.5 text-base text-muted-foreground">{unit}</span>}
      </span>
      {delta != null && (
        <span className={cn("inline-flex items-center gap-1 text-xs", TONE_CLASS[deltaTone])}>
          {trend && (
            <svg
              aria-hidden="true"
              className="h-3 w-3"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2.5"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d={ARROWS[trend]} />
            </svg>
          )}
          {delta}
        </span>
      )}
    </div>
  );
}
