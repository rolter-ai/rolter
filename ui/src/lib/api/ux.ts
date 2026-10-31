import { sendJson } from "./request";

/**
 * A dashboard UX event (#805).
 *
 * Structural only: every field is a key, an enum, a duration or an id. There is
 * no field a form value, prompt or free-text body may travel in — `target`
 * carries the *name* of a form, control or validation rule, never what was
 * typed into it. Keep it that way; the `ui_events` table has nowhere to put a
 * value, and the schema is the privacy guarantee rather than a policy applied
 * on write.
 *
 * `user_id` is deliberately absent: the control plane fills it from the
 * authenticated session, so it cannot be forged by a caller.
 */
export interface UiEvent {
  event_id: string;
  /**
   * when the interaction happened, RFC 3339, stamped as the event is queued
   * rather than when the batch flushes. Omitted means "use ingest time"; a
   * clock too far from the server's is ignored there (#1224).
   */
  ts?: string;
  /** stable screen key (a route id), never a URL — URLs carry ids and filters */
  screen: string;
  action:
    | "screen_view"
    | "time_to_interactive"
    | "navigate"
    | "back_out"
    | "form_submit"
    | "form_abandon"
    | "validation_error"
    | "empty_state"
    | "error_state"
    | "save_confirmed"
    // the struggle signals (#1731): what the operator could not do
    | "retry_submit"
    | "refused_click"
    | "abandon_dirty";
  outcome?: "ok" | "error" | "cancelled";
  /** the name of the form, control or validation rule — never its value */
  target?: string;
  from_screen?: string;
  duration_ms?: number;
  /** joins the event to the gateway request it caused; empty when there was none */
  trace_id?: string;
  session_id?: string;
  org_id?: string;
  team_id?: string;
  project_id?: string;
  app_version?: string;
}

/** The server rejects a batch larger than this. */
export const UI_EVENTS_MAX_BATCH = 100;

/**
 * File a batch of UX events. Browsers batch these — a `visibilitychange`
 * beacon flushes whatever queued since the last one — so a batch is the normal
 * shape rather than an optimization.
 *
 * Returns 202 whether or not the deployment stores them: `logging.ui_events`
 * is a server-side opt-out, and the endpoint is inert without `clickhouse_url`.
 */
export const sendUiEvents = (events: UiEvent[]) =>
  sendJson<void>("POST", "/api/v1/ui-events", { events });

// --- single sign-on (crates/rolter-control/src/sso.rs, #240) ---
//
// an org registers an OIDC provider, and a user hits `/auth/sso/{slug}/start`
// to be bounced through the authorization-code flow. everything here is
// org-scoped and admin-gated (`sso_provider`, `sso_group_mapping` in
// rbac_matrix.rs), so a lesser principal is refused with 403 rather than
// handed an empty list.
