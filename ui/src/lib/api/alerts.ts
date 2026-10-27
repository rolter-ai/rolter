import { getJson, sendJson } from "./request";

export const ALERT_SIGNALS = [
  "error_rate",
  "p95_latency_ms",
  "spend_velocity",
  "request_volume",
  "provider_health_flaps",
] as const;

/** which side of the threshold fires: `above` is value >= threshold, `below` is value <= threshold */
export const ALERT_COMPARISONS = ["above", "below"] as const;

/** what a rule does when its window holds no data; only `error_rate` and `p95_latency_ms` take it */
export const ALERT_NO_DATA_POLICIES = ["ignore", "fire", "ok"] as const;

export type AlertComparison = (typeof ALERT_COMPARISONS)[number];

export type AlertNoDataPolicy = (typeof ALERT_NO_DATA_POLICIES)[number];

export interface AlertChannelRow {
  id: string;
  name: string;
  kind: string;
  endpoint: string;
  enabled: boolean;
  secret_configured: boolean;
  created_at: string;
  updated_at: string;
}

export interface AlertChannelInput {
  name: string;
  endpoint: string;
  enabled: boolean;
  managed_secret?: string;
}

export interface AlertRuleRow {
  id: string;
  name: string;
  signal: string;
  threshold: number;
  comparison: AlertComparison;
  no_data: AlertNoDataPolicy;
  window_secs: number;
  channel_id: string | null;
  enabled: boolean;
  state: string;
  last_value: number | null;
  last_evaluated_at: string | null;
  last_error: string | null;
  /** stable code of `last_error`; `last_error` stays the English fallback */
  last_error_code?: string | null;
  last_error_params?: Record<string, unknown> | null;
  created_at: string;
  updated_at: string;
}

export interface AlertRuleInput {
  name: string;
  signal: string;
  threshold: number;
  /** left out, a PUT keeps the stored value */
  comparison?: AlertComparison;
  /** left out, a PUT keeps the stored value; the API answers 400 for a signal that has no data policy */
  no_data?: AlertNoDataPolicy;
  window_secs: number;
  channel_id?: string | null;
  enabled: boolean;
}

export interface AlertNotificationRow {
  id: string;
  rule_id: string;
  channel_id: string | null;
  state: string;
  delivery_status: string;
  detail: string | null;
  /** stable code of `detail`; null on rows written before codes existed */
  detail_code?: string | null;
  detail_params?: Record<string, unknown> | null;
  /** the reading the row was written for, in the signal's API unit; null for a no-data transition */
  value?: number | null;
  threshold?: number | null;
  sent_at: string;
}

export function fetchAlertChannels(): Promise<AlertChannelRow[]> {
  return getJson<AlertChannelRow[]>("/api/v1/alert-channels");
}

export function createAlertChannel(input: AlertChannelInput): Promise<AlertChannelRow> {
  return sendJson<AlertChannelRow>("POST", "/api/v1/alert-channels", input);
}

export function updateAlertChannel(id: string, input: AlertChannelInput): Promise<AlertChannelRow> {
  return sendJson<AlertChannelRow>("PUT", `/api/v1/alert-channels/${id}`, input);
}

export function deleteAlertChannel(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/alert-channels/${id}`);
}

export function fetchAlertRules(): Promise<AlertRuleRow[]> {
  return getJson<AlertRuleRow[]>("/api/v1/alert-rules");
}

export function createAlertRule(input: AlertRuleInput): Promise<AlertRuleRow> {
  return sendJson<AlertRuleRow>("POST", "/api/v1/alert-rules", input);
}

export function updateAlertRule(id: string, input: AlertRuleInput): Promise<AlertRuleRow> {
  return sendJson<AlertRuleRow>("PUT", `/api/v1/alert-rules/${id}`, input);
}

export function deleteAlertRule(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/alert-rules/${id}`);
}

/** One evaluation's result: the updated rule, and the history row it wrote
 * when the reading was a transition (`notified` is true only when that row
 * was delivered). */
export interface AlertEvaluation {
  rule: AlertRuleRow;
  notified: boolean;
  notification: AlertNotificationRow | null;
}

export function evaluateAlertRule(id: string): Promise<AlertEvaluation> {
  return sendJson<AlertEvaluation>("POST", `/api/v1/alert-rules/${id}/evaluate`);
}

export function fetchAlertHistory(
  limit = 100,
  ruleId?: string,
  filters: { state?: string; delivery_status?: string; before?: string } = {},
): Promise<AlertNotificationRow[]> {
  const params = new URLSearchParams({ limit: String(limit) });
  if (ruleId) params.set("rule_id", ruleId);
  // `before` is `<sent_at>,<id>` of the last row of the previous page
  for (const [key, value] of Object.entries(filters)) if (value) params.set(key, value);
  return getJson<AlertNotificationRow[]>(`/api/v1/alert-notifications?${params}`);
}

// ---------------------------------------------------------------------------
// observability connectors (OTLP log shipping)
