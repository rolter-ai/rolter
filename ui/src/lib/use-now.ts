import * as React from "react";

/**
 * The current time in milliseconds, re-read every `intervalMs`.
 *
 * One clock for every relative timestamp on a screen (`fmt.relative(at, now)`),
 * so the rows do not drift apart, and so a figure that said "just now" does not
 * keep saying it for minutes after the read that produced it went stale.
 */
export function useNow(intervalMs = 30_000): number {
  const [now, setNow] = React.useState(() => Date.now());
  React.useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);
  return now;
}
