import { type DataEnvelope, getJson } from "./request";

/**
 * Which grain a health rollup row describes: `provider` for probe and
 * status-page observations, which watch the provider as a whole, `target` for
 * passive observations of one route through it. The two are never summed —
 * they count different things — so the screen nests targets under a provider
 * instead of laying them out as peers (#1257).
 */
export type HealthGrain = "provider" | "target";

export interface UptimeRow {
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
  provider: string;
  target_id: string;
  grain: HealthGrain;
  mttr_seconds: number;
  incidents: number;
}

export interface TimelineRow {
  bucket: string;
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

// --- control-plane CRUD (only reachable when rolter-control is started
// with --database-url; see crates/rolter-control/src/crud.rs) ---
