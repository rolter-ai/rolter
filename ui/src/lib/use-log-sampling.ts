import { useQuery } from "@tanstack/react-query";

import { fetchLoggingSettings } from "@/lib/api";
import { useCan } from "@/lib/can";

/**
 * The request log's sample rate when it is below 100 %, else `null` (#2231).
 *
 * The analytics screens read the log, so below 100 % what they show is scaled
 * up from a sample. Only a role that can read the logging settings can ask
 * what the rate is, so for anyone else, while the answer is out and after it
 * failed, this is `null` and the screens say nothing rather than guess. The
 * request is not retried: a note is not worth a failure banner.
 */
export function useLogSampling(): number | null {
  const can = useCan();
  const query = useQuery({
    queryKey: ["logging-settings", "sampling"],
    queryFn: fetchLoggingSettings,
    enabled: can("logging_settings", "read") === true,
    retry: false,
    staleTime: 60_000,
  });
  const rate = query.data?.sample_rate;
  return typeof rate === "number" && Number.isFinite(rate) && rate >= 0 && rate < 1 ? rate : null;
}
