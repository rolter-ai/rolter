## 2026-08-01 - [ClickHouse Parameter Mismatch Vulnerability]

**Vulnerability:** Found a mismatch in a ClickHouse parameterized query where the SQL template string defined the parameter as `{event_id:String}` but the actual parameter passed to the query execution was bound as `param_event_id`.
**Learning:** In ClickHouse queries executed over the HTTP interface using the `reqwest` client in this codebase, URL query string parameterization relies on matching the parameter name bound in the Rust code (e.g., `param_event_id`) directly to the placeholder within the SQL string. A mismatch doesn't correctly substitute the parameter.
**Prevention:** Always verify that the placeholder variable name in the ClickHouse SQL string directly matches the parameter name defined in the Rust code.

## 2026-09-14 - [Replace unwrap with hex encode]

**Vulnerability:** Use of `.unwrap()` in manual hex encoding loop in `crates/rolter-gateway/src/cache.rs`.
**Learning:** While mathematically safe in this specific context (digits 0-15 mapped to hex), explicit `.unwrap()` calls in tight loops are poor practice and trigger linter/security concerns. The codebase provides a dedicated utility `rolter_auth::hex::encode` for this purpose.
**Prevention:** Prefer established, tested utility functions over manual loops, especially when they eliminate the need for `.unwrap()`, making the code safer and more maintainable.

## 2026-09-18 - [Redact Internal Store Errors in Snapshot 500 Responses]

**Vulnerability:** HTTP 500 error responses from `/internal/snapshot` in `rolter-control` echoed internal store/database error details (`err.to_string()`) directly into the JSON error body, potentially exposing internal database connection details or query errors to clients.
**Learning:** Store errors (such as Postgres connection or query failures) must be logged internally via `tracing::error!` while returning a generic message (e.g., `"failed to load config snapshot"`) in the JSON response body to prevent internal information disclosure.
**Prevention:** Always log detailed error descriptions internally with `tracing` and return sanitized, generic error messages in API HTTP response bodies.

## 2026-09-22 - [Remove expect in ZMQ Cache Telemetry Parser]

**Learning:** `expect()` or `unwrap()` calls on network message frames (like ZeroMQ telemetry messages) create potential panic points if network frames are malformed or truncated. Replacing them with `.try_into().ok()` and `message.get(...)` prevents gateway crashes.
**Prevention:** Always convert slice conversions and index accesses on network messages to safe `Option`/`Result` matching instead of using `.expect()` or `.unwrap()`.

## 2026-09-25 - [Redact Internal Store Errors in SCIM 500 Responses]

**Learning:** HTTP 500 error responses generated during SCIM error conversions (`From<rolter_core::Error>` and `From<ApiError>`) in `rolter-control` previously echoed internal error strings (`other.to_string()`) directly into the SCIM JSON error response detail, potentially leaking internal store errors or database connection details.
**Prevention:** Always log internal error details using `tracing::warn!` or `tracing::error!` and return a generic, sanitized error detail (such as `"internal server error"`) in API HTTP error response bodies.

## 2026-10-02 - Redact Internal Store Errors in Auth 500 Responses

**Learning:** `AuthError::Internal(msg)` in `rolter-control` previously echoed `msg` (which carries raw database connection or query failure strings from Postgres/SQLx) directly into the HTTP 500 JSON response body message on auth routes (`/api/v1/auth/login`, `/api/v1/auth/me`, etc.).
**Prevention:** Always log detailed internal error messages internally via `tracing::error!` and return a generic error message (such as `"an internal server error occurred"`) in HTTP 500 error response bodies to avoid leaking database internals.

## 2026-10-10 - Redact Internal Store Errors in SSO Error Messages

**Learning:** `api_error_message(err)` in `crates/rolter-control/src/sso.rs` previously converted `ApiError::Core(e)` by calling `e.to_string()`. When `e` was an `Error::Store` (carrying SQL or database connection strings), this leaked internal database details via `IdentityError::Provider(...)`.
**Prevention:** Sanitize internal errors in error message converters by checking for `Error::NotFound` / `Error::Config` vs internal errors, logging the internal detail with `tracing::error!`, and returning a generic error message like `"internal server error"`.

## 2026-10-03 - Redact Control Plane Internal Errors in Admin Proxy 502 Responses

**Learning:** `bad_gateway` in `crates/rolter-gateway/src/admin_proxy.rs` previously echoed `err` strings (containing internal transport or connection failures) directly into the JSON error response message when forwarding requests to the control plane failed.
**Prevention:** Always log detailed internal error messages via `tracing::warn!` or `tracing::error!` and return a sanitized, static error message (such as `"control plane unreachable"`) in 502 HTTP error response bodies.

## 2026-10-18 - Redact Internal Error Strings in Control Plane Analytics and Profile Endpoints

**Learning:** `query_failed` in `crates/rolter-control/src/analytics.rs` previously passed raw `err` strings to `tracing::warn!` without URL redaction, and `my_usage` in `crates/rolter-control/src/me.rs` echoed internal database errors (`err.to_string()`) in `500` JSON responses when fetching virtual keys failed.
**Prevention:** Redact URLs in text using `rolter_core::redact::redact_urls_in_text` when logging ClickHouse/HTTP errors, log database failures internally via `tracing::warn!`, and return a sanitized static error message (such as `"internal server error"`) in HTTP error response bodies.

## 2026-10-24 - Redact Argon2 Password Hashing Error Details in Control Plane

**Learning:** `hash_password` in `crates/rolter-control/src/crud.rs` previously wrapped Argon2 password hashing failures in `ApiError::Core(Error::Config(format!("failed to hash password: {e}")))`, which rendered as HTTP 400 Bad Request with raw Argon2 error details echoed to clients.
**Prevention:** Log internal cryptographic library errors with `tracing::error!` and return `ApiError::Curated("failed to hash password".into())` so clients receive a sanitized HTTP 500 response.

## 2026-10-24 - Redact URL Credentials in Status Page Polling Logs

**Learning:** `poll_one` in `crates/rolter-gateway/src/status_page.rs` previously logged raw `reqwest::Error` strings using `tracing::warn!`. Because `reqwest::Error` includes the request URL (which may contain query credentials or basic auth userinfo in `status_page_url`), logging raw error strings can leak operator secrets into system logs.
**Prevention:** Always pass error strings through `rolter_core::redact::redact_urls_in_text(&err.to_string())` when logging network or HTTP request failures that include URLs.


## 2026-10-23 - Redact Internal ClickHouse Error Messages in Alert Evaluation Responses

**Learning:** `describe_query_error` in `crates/rolter-control/src/alerting.rs` previously formatted non-reqwest ClickHouse query errors into `SignalError` using `truncate(&error.to_string(), ...)`. When an alert evaluation query failed, this raw ClickHouse error message (carrying SQL query details or database exception traces) was stored on the rule and echoed directly in HTTP 500 error response bodies from `POST /api/v1/alert-rules/{id}/evaluate`.
**Prevention:** Always log detailed internal query/database errors via `tracing::warn!` with URL redaction and return generic, sanitized error strings (such as `"analytics query failed"`) in API HTTP error response bodies.


## 2026-10-24 - Redact Sensitive URL Credentials in WebSocket Proxy Error Logs

**Learning:** `proxy_ws` in `crates/rolter-control/src/proxy.rs` previously logged raw `tungstenite` error messages with `tracing::warn!(%error, ...)` when WebSocket client requests failed URL parsing. Because `tungstenite` quotes the entire URL in its error message, any sensitive query parameters (like virtual keys or session tokens) were logged unredacted in server log streams.
**Prevention:** Always run error strings generated from client or upstream URL parsing through `rolter_core::redact::redact_urls_in_text` before logging them with `tracing`.

## 2026-10-24 - Redact Sensitive URL Credentials in Gateway Redis Error Logs

**Learning:** `ReconnectingRedis::new` and operations in `rate_limits.rs`, `budgets.rs`, and `cache.rs` previously logged raw `err` strings directly via `tracing::warn!`. Because Redis error messages can echo the connection URL (which may contain userinfo credentials such as `redis://user:password@host:6379/0`), logging raw error strings without URL redaction can leak Redis credentials into server logs.
**Prevention:** Always wrap error strings from Redis connection or operation failures with `rolter_core::redact::redact_urls_in_text(&err.to_string())` before passing them to `tracing::warn!`.
## 2026-10-25 - Redact Sensitive URL Credentials in MCP OAuth Discovery Errors

**Learning:** MCP OAuth discovery in `crates/rolter-control/src/mcp_oauth_discovery.rs` constructed `DiscoveryError::Refused` and `tracing::debug!` messages using raw unredacted URLs and reqwest error strings. When discovery failed against URLs containing sensitive query parameters or userinfo credentials, those secrets could be exposed in API error messages and debug logs.
**Prevention:** Always sanitize URLs and HTTP client error strings using `rolter_core::redact::redact_urls_in_text` before embedding them into `DiscoveryError::Refused` variants or `tracing` logs.
## 2026-10-25 - Consistent 409 Conflict Errors for Config-Owned Provider Groups

**Learning:** `require_group_not_config_owned` and `require_group_row_not_config_owned` in `crates/rolter-control/src/crud/provider_groups.rs` previously returned `ApiError::Core(Error::Config(...))` which rendered as HTTP 400 Bad Request, whereas mutating other config-owned resources (providers, routes, models) returned HTTP 409 Conflict.
**Prevention:** Use shared `require_not_config_owned` and `require_row_not_config_owned` helpers across all CRUD handlers to ensure mutations targeting bootstrap config-owned entities consistently answer with HTTP 409 Conflict.
