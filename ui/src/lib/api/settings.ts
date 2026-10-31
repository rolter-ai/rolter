import { getJson, sendJson } from "./request";

export interface SecuritySettingsDto {
  allowed_origins: string[];
  allowed_headers: string[];
  required_headers: Record<string, string>;
  auth_bypass_routes: string[];
  updated_at: string;
}

export interface UpdateSecuritySettingsInput {
  allowed_origins: string[];
  allowed_headers: string[];
  required_headers: Record<string, string>;
  auth_bypass_routes: string[];
}

export function fetchSecuritySettings(): Promise<SecuritySettingsDto> {
  return getJson<SecuritySettingsDto>("/api/v1/security-settings");
}

export function updateSecuritySettings(
  input: UpdateSecuritySettingsInput,
): Promise<SecuritySettingsDto> {
  return sendJson<SecuritySettingsDto>("PUT", "/api/v1/security-settings", input);
}

// ---------------------------------------------------------------------------
// compatibility policy: values applied when translating between the OpenAI and
// Anthropic wire formats

export interface CompatibilityPolicyDto {
  anthropic_version: string;
  default_max_tokens: number;
  updated_at: string;
  // fields that only take effect after a gateway restart; empty today, but the
  // server owns the list so the screen can warn without knowing which they are
  restart_required: string[];
}

export interface UpdateCompatibilityPolicyInput {
  anthropic_version: string;
  default_max_tokens: number;
}

export function fetchCompatibilityPolicy(): Promise<CompatibilityPolicyDto> {
  return getJson<CompatibilityPolicyDto>("/api/v1/compatibility-policy");
}

export function updateCompatibilityPolicy(
  input: UpdateCompatibilityPolicyInput,
): Promise<CompatibilityPolicyDto> {
  return sendJson<CompatibilityPolicyDto>("PUT", "/api/v1/compatibility-policy", input);
}

// ---------------------------------------------------------------------------
// runtime policy: retry, timeout and admission-queue controls

export const BACKPRESSURE_POLICIES = ["drop", "block", "error"] as const;

export type BackpressurePolicy = (typeof BACKPRESSURE_POLICIES)[number];

export interface RuntimePolicyDto {
  retry_max_retries: number;
  retry_base_ms: number;
  retry_max_ms: number;
  timeout_connect_s: number;
  timeout_request_s: number;
  queue_enabled: boolean;
  queue_capacity: number;
  queue_workers: number;
  queue_backpressure: BackpressurePolicy;
  queue_block_ms: number;
  updated_at: string;
}

export type UpdateRuntimePolicyInput = Omit<RuntimePolicyDto, "updated_at">;

export function fetchRuntimePolicy(): Promise<RuntimePolicyDto> {
  return getJson<RuntimePolicyDto>("/api/v1/runtime-policy");
}

export function updateRuntimePolicy(input: UpdateRuntimePolicyInput): Promise<RuntimePolicyDto> {
  return sendJson<RuntimePolicyDto>("PUT", "/api/v1/runtime-policy", input);
}

// ---------------------------------------------------------------------------
// client settings: advertised base URL and upstream header handling (#564)

export interface ClientSettingsDto {
  public_base_url: string | null;
  forwarded_headers: string[];
  injected_headers: Record<string, string>;
  request_id_header: string;
  updated_at: string;
  /// header names propagated whether or not they are listed
  always_propagated: string[];
  /// header names that can never be forwarded or injected
  reserved: string[];
  /// header names refused as the request id header (#2970); optional so a
  /// control plane older than the screen still loads
  request_id_refused?: string[];
}

export type UpdateClientSettingsInput = Pick<
  ClientSettingsDto,
  "public_base_url" | "forwarded_headers" | "injected_headers" | "request_id_header"
>;

export function fetchClientSettings(): Promise<ClientSettingsDto> {
  return getJson<ClientSettingsDto>("/api/v1/client-settings");
}

export function updateClientSettings(input: UpdateClientSettingsInput): Promise<ClientSettingsDto> {
  return sendJson<ClientSettingsDto>("PUT", "/api/v1/client-settings", input);
}

// ---------------------------------------------------------------------------
// model defaults: inference parameters filled in when a client omits them (#564)

export interface ModelDefaultsDto {
  enabled: boolean;
  default_model: string | null;
  default_temperature: number | null;
  default_top_p: number | null;
  default_max_tokens: number | null;
  updated_at: string;
}

export type UpdateModelDefaultsInput = Omit<ModelDefaultsDto, "updated_at">;

export function fetchModelDefaults(): Promise<ModelDefaultsDto> {
  return getJson<ModelDefaultsDto>("/api/v1/model-defaults");
}

export function updateModelDefaults(input: UpdateModelDefaultsInput): Promise<ModelDefaultsDto> {
  return sendJson<ModelDefaultsDto>("PUT", "/api/v1/model-defaults", input);
}

// ---------------------------------------------------------------------------
// logging settings: request-log sampling, payload capture, retention and the
// dashboard UX event opt-out

export interface LoggingSettingsDto {
  sample_rate: number;
  payload_capture_enabled: boolean;
  payload_capture_max_bytes: number;
  payload_capture_redact_fields: string[];
  /** built-in value detectors scanning captured text (#1835) */
  payload_capture_redact_detectors: string[];
  /** operator value patterns, applied after the detectors */
  payload_capture_redact_patterns: RedactPatternDto[];
  payload_capture_models: string[];
  payload_capture_virtual_key_ids: string[];
  retention_days: number;
  payload_retention_hours: number;
  /** whether the control plane accepts dashboard UX events (#1748) */
  ui_events: boolean;
  updated_at: string;
}

export interface RedactPatternDto {
  name: string;
  pattern: string;
}

export type UpdateLoggingSettingsInput = Omit<LoggingSettingsDto, "updated_at">;

/** the built-in detector ids, in the order the server applies them */
export const REDACT_DETECTORS = [
  "private_key",
  "anthropic_key",
  "openai_key",
  "google_api_key",
  "github_token",
  "slack_token",
  "stripe_key",
  "aws_key",
  "jwt",
  "bearer_token",
  "connection_string",
] as const;

export interface RedactionPreviewInput {
  sample: string;
  redact_fields?: string[];
  redact_detectors?: string[];
  redact_patterns?: RedactPatternDto[];
}

export interface RedactionPreviewDto {
  output: string;
  report: { fields: number; detections: Record<string, number> };
}

/** dry-runs the policy; the server stores and logs nothing (#1835) */
export function previewPayloadRedaction(
  input: RedactionPreviewInput,
): Promise<RedactionPreviewDto> {
  return sendJson<RedactionPreviewDto>("POST", "/api/v1/logging-settings/redaction-preview", input);
}

export function fetchLoggingSettings(): Promise<LoggingSettingsDto> {
  return getJson<LoggingSettingsDto>("/api/v1/logging-settings");
}

/**
 * The request log's sample rate alone, which every role that reads analytics
 * may learn, unlike the rest of the logging policy (#3057).
 */
export interface LogSamplingDto {
  sample_rate: number;
}

export function fetchLogSampling(): Promise<LogSamplingDto> {
  return getJson<LogSamplingDto>("/api/v1/analytics/sampling");
}

export function updateLoggingSettings(
  input: UpdateLoggingSettingsInput,
): Promise<LoggingSettingsDto> {
  return sendJson<LoggingSettingsDto>("PUT", "/api/v1/logging-settings", input);
}

// ---------------------------------------------------------------------------
// feature flags: global switches for hot-reloadable gateway subsystems

// a flag whose subsystem this deployment cannot run. the screen renders these
// as unavailable rather than as a switch that silently does nothing (#535)
export interface UnavailableFlagDto {
  flag: FeatureFlagKey;
  reason: string;
}

export const FEATURE_FLAG_KEYS = [
  "response_cache",
  "cache_aware_routing",
  "circuit_breaker",
  "active_health_checks",
  "complexity_routing",
  "guardrails",
] as const;

export type FeatureFlagKey = (typeof FEATURE_FLAG_KEYS)[number];

export type FeatureFlagValues = Record<FeatureFlagKey, boolean>;

export type FeatureFlagsDto = FeatureFlagValues & {
  updated_at: string;
  unavailable: UnavailableFlagDto[];
};

export function fetchFeatureFlags(): Promise<FeatureFlagsDto> {
  return getJson<FeatureFlagsDto>("/api/v1/feature-flags");
}

export function updateFeatureFlags(input: FeatureFlagValues): Promise<FeatureFlagsDto> {
  return sendJson<FeatureFlagsDto>("PUT", "/api/v1/feature-flags", input);
}

// ---------------------------------------------------------------------------
// cluster inventory: nodes register themselves by identifying on their snapshot
// poll, so this is a read + drain/forget surface, never an enrolment one
