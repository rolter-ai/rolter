import { type DataEnvelope, getJson } from "./request";

/**
 * Which grain a health rollup row describes: `provider` for probe and
 * status-page observations, which watch the provider as a whole, `target` for
 * passive observations of one route through it. The two are never summed —
 * they count different things — so the screen nests targets under a provider
 * instead of laying them out as peers (#1257).
 */
export type HealthGrain = "provider" | "target";

/**
 * A provider in the health rollups is `org_id` and `provider` together: two
 * organizations may each have an `openai` (#1857), and the rollups answer one
 * series for each (#3441). `org_id` is empty for a provider from the gateway's
 * own config file, and absent from a control plane that predates the field.
 */
export interface UptimeRow {
  org_id?: string;
  provider: string;
  target_id: string;
  grain: HealthGrain;
  /** the distinct event sources behind this row, e.g. `["passive", "probe"]` */
  sources: string[];
  events: number;
  ok: number;
  errors: number;
  timeouts: number;
  uptime: number;
  failure_rate: number;
  error_budget_burn: number;
  sla_breached: number;
  last_event: string;
}

export interface MttrRow {
  /** see {@link UptimeRow} */
  org_id?: string;
  provider: string;
  target_id: string;
  grain: HealthGrain;
  mttr_seconds: number;
  incidents: number;
}

export interface TimelineRow {
  bucket: string;
  /** see {@link UptimeRow} */
  org_id?: string;
  provider: string;
  target_id: string;
  grain: HealthGrain;
  events: number;
  ok: number;
  errors: number;
  timeouts: number;
}

export function fetchUptime(sla = 0.99): Promise<UptimeRow[]> {
  return getJson<DataEnvelope<UptimeRow>>(`/api/v1/health/uptime?sla=${sla}`).then((r) => r.data);
}

export function fetchMttr(): Promise<MttrRow[]> {
  return getJson<DataEnvelope<MttrRow>>("/api/v1/health/mttr").then((r) => r.data);
}

export function fetchHealthTimeline(bucket = "hour"): Promise<TimelineRow[]> {
  return getJson<DataEnvelope<TimelineRow>>(`/api/v1/health/timeline?bucket=${bucket}`).then(
    (r) => r.data,
  );
}
