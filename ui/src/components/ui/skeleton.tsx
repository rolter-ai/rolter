import * as React from "react";

import { cn } from "@/lib/utils";

// shimmering placeholder block. mirrors the Rolter Design System
// feedback/Skeleton. the shimmer keyframes live in index.css.
export interface SkeletonProps extends React.HTMLAttributes<HTMLSpanElement> {
  width?: number | string;
  height?: number | string;
  radius?: number | string;
}

export function Skeleton({ width, height = 14, radius, className, style, ...props }: SkeletonProps) {
  return (
    <span
      className={cn("rl-skeleton block rounded-sm", className)}
      style={{ width: width ?? "100%", height, borderRadius: radius, ...style }}
      {...props}
    />
  );
}
