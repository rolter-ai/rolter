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
