import { useFormat } from "@/lib/i18n/format";
import { useNow } from "@/lib/use-now";

/**
 * A relative timestamp (`3m ago`) that keeps its own clock.
 *
 * The tick lives here, in a leaf, so a screen that shows relative times does not
 * re-render as a whole every second to move them (#1997). Rows on one screen
 * tick on separate timers but all re-read `Date.now()`, so they stay within a
 * tick of one another.
 */
export function RelativeTime({
  at,
  intervalMs = 1000,
  onRender,
}: {
  at: string | number | Date;
  intervalMs?: number;
  /** a render-count seam for the story that proves the parent is left alone */
  onRender?: () => void;
}) {
  const fmt = useFormat();
  const now = useNow(intervalMs);
  onRender?.();
  return <>{fmt.relative(at, now)}</>;
}
