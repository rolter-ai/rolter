import * as React from "react";

import { cn } from "@/lib/utils";

export function Card({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        // DS card is border-only (translucent border, no shadow) on the zinc ground.
        // `min-w-0` because a card is almost always a grid or flex item: without
        // it the widest thing inside — a table, a donut legend — sets the track
        // width and drags the page sideways instead of scrolling in place (#1203)
        "min-w-0 rounded-lg border border-[color:var(--border-default)] bg-card text-card-foreground",
        className,
      )}
      {...props}
    />
  );
}

export function CardHeader({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("flex flex-col space-y-1.5 p-6", className)} {...props} />;
}

export function CardTitle({ className, ...props }: React.HTMLAttributes<HTMLHeadingElement>) {
  // an h2, not an h3: every screen title is the h1 in `ScreenHeader`, and a
  // card is the next level down. as an h3 it skipped a level, which the
  // assembled-shell story (#1239) caught the moment the axe gate started
  // asserting heading-order (#1244)
  return (
    <h2
      className={cn("text-lg font-semibold leading-none tracking-tight", className)}
      {...props}
    />
  );
}

export function CardDescription({
  className,
  ...props
}: React.HTMLAttributes<HTMLParagraphElement>) {
  return <p className={cn("text-sm text-muted-foreground", className)} {...props} />;
}

export function CardContent({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("p-6 pt-0", className)} {...props} />;
}
