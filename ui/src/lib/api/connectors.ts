import { getJson, getText, sendJson } from "./request";

export interface ConnectorRow {
  id: string;
  name: string;
  kind: string;
  endpoint: string;
  enabled: boolean;
  sampling_rate: number;
  auth_secret_ref: string | null;
  auth_secret_configured: boolean;
  health_status: string;
  health_checked_at: string | null;
  health_error: string | null;
  created_at: string;
  updated_at: string;
}

export interface ConnectorInput {
  name: string;
  kind: "otlp_http";
  endpoint: string;
  enabled: boolean;
  sampling_rate: number;
  auth_secret_ref?: string | null;
  /// write-only bearer token, sealed before persistence. On update, omitted
  /// keeps the stored one and `null` removes it
  managed_auth_secret?: string | null;
}

export function fetchConnectors(): Promise<ConnectorRow[]> {
  return getJson<ConnectorRow[]>("/api/v1/connectors");
}

export function createConnector(input: ConnectorInput): Promise<ConnectorRow> {
  return sendJson<ConnectorRow>("POST", "/api/v1/connectors", input);
}

export function updateConnector(id: string, input: ConnectorInput): Promise<ConnectorRow> {
  return sendJson<ConnectorRow>("PUT", `/api/v1/connectors/${id}`, input);
}

export function deleteConnector(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/connectors/${id}`);
}

// the probe's own verdict. `health_error` is `skip_serializing_if = "none"` on
// the control plane, so it is absent on a delivered probe and carries the
// sink's refusal — the reason delivery failed — on a rejected one
export interface ConnectorTestResult {
  delivered: boolean;
  health_status: string;
  health_checked_at: string;
  health_error?: string | null;
}

export function testConnector(id: string): Promise<ConnectorTestResult> {
  return sendJson<ConnectorTestResult>("POST", `/api/v1/connectors/${id}/test`);
}

/**
 * The OpenTelemetry Collector configuration rendered from the enabled
 * connectors (`crates/rolter-control/src/collector_config.rs`).
 *
 * A YAML document, not JSON: the collector's `confmap` HTTP provider reads it
 * verbatim from `--config=http://...`. ADR-0026 put the per-destination
 * fan-out in the collector rather than in the data plane, so this document is
 * the delivery path — the connectors are only the rows it is rendered from.
 * Superadmin-only, like the rest of the connector surface.
 */
export function fetchCollectorConfig(): Promise<string> {
  return getText("/api/v1/connectors/collector-config");
}

// the full address `fetchCollectorConfig` reads (#2106): the public base from
// `fetchPublicUrl`, then the path collector_config.rs mounts. appended to the
// control plane's own answer and never to `window.location.origin`, which is
// the dashboard's address and not necessarily the one a script can call. the
// endpoint answers a superadmin only, so a collector cannot fetch it itself
export function collectorConfigUrl(base: string): string {
  return `${base}/api/v1/connectors/collector-config`;
}

// ---------------------------------------------------------------------------
// mcp tool-call logs (clickhouse-backed; 503 → AnalyticsUnavailableError)
