import { useQuery } from "@tanstack/react-query";

import { fetchLogSampling } from "@/lib/api";

/**
 * The request log's sample rate when it is below 100 %, else `null` (#2231).
 *
 * The analytics screens read the log, so below 100 % what they show is scaled
 * up from a sample. The rate comes from `GET /api/v1/analytics/sampling`, which
 * answers every role the analytics reads serve, not only the superadmin who
 * may read the rest of the logging policy (#3057). While the answer is out,
 * and after it failed, this is `null` and the screens say nothing rather than
 * guess. The request is not retried: a note is not worth a failure banner.
 *
 * The key sits under `logging-settings`, so saving the policy on Log Settings
 * refreshes the note as well.
 */
export function useLogSampling(): number | null {
  const query = useQuery({
    queryKey: ["logging-settings", "sampling"],
    queryFn: fetchLogSampling,
    retry: false,
    staleTime: 60_000,
  });
  const rate = query.data?.sample_rate;
  return typeof rate === "number" && Number.isFinite(rate) && rate >= 0 && rate < 1 ? rate : null;
}
