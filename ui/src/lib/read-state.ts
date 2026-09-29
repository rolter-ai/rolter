import type { FetchStatus } from "@tanstack/react-query";

/**
 * The part of a react-query result that says which state a list is in. A
 * `useQuery` result satisfies it as it stands, so a screen passes its query.
 *
 * A list read has four answers — still coming, failed, loaded with rows, loaded
 * with none — and the rows alone cannot tell the first two from the last: all
 * three hold an empty array. Deriving the empty state from the rows is how a
 * failed or pending read came to say "No providers yet" under its own
 * `LoadError`, and a count to say "0 teams" (#2211).
 */
export interface ReadState {
  isPending: boolean;
  isSuccess: boolean;
  fetchStatus: FetchStatus;
}

/**
 * The read has not answered and is trying to: fetching, or a retry react-query
 * parked because the tab is hidden or the browser is offline. Not `isLoading`,
 * which misses the parked retry (#1984), and not bare `isPending`, which is
 * also true of a query that is disabled — a screen with no org to read would
 * stand in a skeleton forever.
 */
export function isAwaiting(read: ReadState): boolean {
  return read.isPending && read.fetchStatus !== "idle";
}

/**
 * The read answered, and the answer was nothing. The one state an empty state
 * describes: a pending read has not said, and a failed one cannot.
 */
export function isEmptyAnswer(read: ReadState, rows: number): boolean {
  return read.isSuccess && rows === 0;
}

/** A read that answered, for data that is not fetched: a fixture, a story. */
export const ANSWERED: ReadState = { isPending: false, isSuccess: true, fetchStatus: "idle" };
