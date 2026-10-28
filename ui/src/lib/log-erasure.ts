import type { LogErasureRow } from "@/lib/api";

// how often a pending erasure is asked how far ClickHouse has got. a mutation
// rewrites parts in the background, which takes seconds to minutes
export const ERASURE_POLL_MS = 4_000;

/**
 * react-query `refetchInterval` for one erasure record: ask again while it is
 * `pending`, stop at any answer ClickHouse will not move (`completed`,
 * `failed`, `unverified`). The server itself ends `pending` after an hour with
 * no mutation visible, so there is no client-side give-up to drift from it.
 */
export function erasureRefetchInterval(
  record: Pick<LogErasureRow, "status"> | undefined,
  pollMs: number = ERASURE_POLL_MS,
): number | false {
  return record?.status === "pending" ? pollMs : false;
}

/**
 * A log row's `ts` as the RFC 3339 instant an erasure of one request needs.
 *
 * The row shows ClickHouse's own stamp, which carries no zone when the control
 * plane forwards it as text (`2026-10-27 12:30:45.123`); that is UTC. A stamp
 * that already names a zone is kept as the instant it names. `undefined` when
 * it does not parse, so the erasure is not narrowed to a wrong instant.
 */
export function erasureTimestamp(ts: string): string | undefined {
  const zoned = /(Z|[+-]\d{2}:?\d{2})$/i.test(ts);
  const iso = zoned ? ts : `${ts.replace(" ", "T")}Z`;
  const ms = Date.parse(iso);
  return Number.isNaN(ms) ? undefined : new Date(ms).toISOString();
}
