import { useQuery } from "@tanstack/react-query";

import { fetchClusterNodes } from "@/lib/api";
import { gatewayPickup, type Pickup } from "@/lib/gateway-pickup";

// after a save, how often the gateways are asked whether they run it yet, and
// for how long. a gateway polls every few seconds, so a minute and a half with
// one still behind is a gateway that is not picking config up
const PICKUP_POLL_MS = 4_000;
const PICKUP_WATCH_MS = 90_000;

/**
 * Which live gateways run the save made at `savedAt` ("applied to N of M",
 * #2214), from the same inventory the Cluster screen reads. Keyed on the save's
 * own time, so a read from before it is never mistaken for an answer to it, and
 * idle until there is a save to ask about.
 */
export function useSettingsPickup(savedAt: number | null): Pickup | null {
  const inventory = useQuery({
    queryKey: ["settings-pickup", savedAt],
    queryFn: fetchClusterNodes,
    enabled: savedAt !== null,
    retry: false,
    refetchInterval: (query) => {
      const { state } = gatewayPickup(query.state.data, query.state.status === "error");
      const waiting = state === "checking" || state === "lagging";
      return waiting && savedAt !== null && Date.now() - savedAt < PICKUP_WATCH_MS
        ? PICKUP_POLL_MS
        : false;
    },
  });
  if (savedAt === null) return null;
  return gatewayPickup(inventory.data, inventory.isError);
}
