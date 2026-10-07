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

## 2026-10-18 - Redact Upstream Connection Failures in Realtime 502 Responses

**Learning:** Rejection of WebSocket upgrade requests on `/v1/realtime` in `crates/rolter-gateway/src/realtime.rs` echoed raw upstream connection failure messages (e.g., `format!("upstream realtime connection failed: {error}")`) directly into HTTP 502 response bodies, disclosing internal infrastructure and upstream socket details.
**Prevention:** Log detailed upstream connection errors internally via `tracing::warn!` and return a sanitized static error message (such as `"upstream realtime connection failed"`) in 502 HTTP error response bodies.
