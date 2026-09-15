## 2026-08-01 - [ClickHouse Parameter Mismatch Vulnerability]
**Vulnerability:** Found a mismatch in a ClickHouse parameterized query where the SQL template string defined the parameter as `{event_id:String}` but the actual parameter passed to the query execution was bound as `param_event_id`.
**Learning:** In ClickHouse queries executed over the HTTP interface using the `reqwest` client in this codebase, URL query string parameterization relies on matching the parameter name bound in the Rust code (e.g., `param_event_id`) directly to the placeholder within the SQL string. A mismatch doesn't correctly substitute the parameter.
**Prevention:** Always verify that the placeholder variable name in the ClickHouse SQL string directly matches the parameter name defined in the Rust code.
## 2026-09-14 - [Replace unwrap with hex encode]
**Vulnerability:** Use of `.unwrap()` in manual hex encoding loop in `crates/rolter-gateway/src/cache.rs`.
**Learning:** While mathematically safe in this specific context (digits 0-15 mapped to hex), explicit `.unwrap()` calls in tight loops are poor practice and trigger linter/security concerns. The codebase provides a dedicated utility `rolter_auth::hex::encode` for this purpose.
**Prevention:** Prefer established, tested utility functions over manual loops, especially when they eliminate the need for `.unwrap()`, making the code safer and more maintainable.
