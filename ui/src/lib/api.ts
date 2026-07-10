// typed fetch helpers for the rolter control api (proxied at /api in dev)

export interface TargetDto {
  provider: string;
  model?: string | null;
  weight: number;
}

export interface RouteDto {
  model: string;
  strategy: string;
  targets: TargetDto[];
}

export interface VirtualKeyDto {
  key: string;
  name?: string | null;
  models: string[];
}

export interface ProviderDto {
  name: string;
  kind: string;
  api_base: string;
}

export interface GatewayConfigDto {
  providers: ProviderDto[];
  routes: RouteDto[];
  virtual_keys: VirtualKeyDto[];
}

async function getJson<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(`request failed: ${res.status}`);
  }
  return (await res.json()) as T;
}

export function fetchConfig(): Promise<GatewayConfigDto> {
  return getJson<GatewayConfigDto>("/api/v1/config");
}

export function fetchRoles(): Promise<string[]> {
  return getJson<string[]>("/api/v1/roles");
}

// provider stability rollups over provider_health_events (ROL-198)

export interface UptimeRow {
  provider: string;
  target_id: string;
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
  mttr_seconds: number;
  incidents: number;
}

export interface TimelineRow {
  bucket: string;
  provider: string;
  target_id: string;
  events: number;
  ok: number;
  errors: number;
  timeouts: number;
}

interface DataEnvelope<T> {
  data: T[];
}

export function fetchUptime(sla = 0.99): Promise<UptimeRow[]> {
  return getJson<DataEnvelope<UptimeRow>>(
    `/api/v1/health/uptime?sla=${sla}`,
  ).then((r) => r.data);
}

export function fetchMttr(): Promise<MttrRow[]> {
  return getJson<DataEnvelope<MttrRow>>("/api/v1/health/mttr").then(
    (r) => r.data,
  );
}

export function fetchHealthTimeline(bucket = "hour"): Promise<TimelineRow[]> {
  return getJson<DataEnvelope<TimelineRow>>(
    `/api/v1/health/timeline?bucket=${bucket}`,
  ).then((r) => r.data);
}
