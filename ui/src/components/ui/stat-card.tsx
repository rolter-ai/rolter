import * as React from "react";

import { cn } from "@/lib/utils";

// metric tile: label, big mono value, optional unit + trend delta. mirrors the
// Rolter Design System display/StatCard
export interface StatCardProps extends React.HTMLAttributes<HTMLDivElement> {
  label: React.ReactNode;
  value: React.ReactNode;
  unit?: React.ReactNode;
  delta?: React.ReactNode;
  trend?: "up" | "down" | "flat";
}

const ARROWS: Record<string, string> = {
  up: "M12 19V5M5 12l7-7 7 7",
  down: "M12 5v14M5 12l7 7 7-7",
  flat: "M5 12h14",
};

const DELTA_TONE: Record<string, string> = {
  up: "text-emerald-400",
  down: "text-red-400",
  flat: "text-muted-foreground",
};

export function StatCard({
  label,
  value,
  unit,
  delta,
  trend = "flat",
  className,
  ...props
}: StatCardProps) {
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
        <span className={cn("inline-flex items-center gap-1 text-xs", DELTA_TONE[trend])}>
          <svg
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
          {delta}
        </span>
      )}
    </div>
  );
}
