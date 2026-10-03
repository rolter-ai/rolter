-- A gateway-minted key that names one logged request (#1937).
--
-- `request_id` is the caller's `x-request-id`, so it is not unique: two
-- tenants can send the same one, and a client that sends a constant id sends
-- it on every call. Joining a captured body to its log row on
-- `(request_id, ts)` therefore still crossed bodies between requests that began
-- in the same millisecond. `log_id` is a UUID the gateway mints per request and
-- writes to both tables, which the caller can neither choose nor observe.
--
-- `request_id` keeps its column for lookup and search. `org_id` and
-- `project_id` are copied onto the payload row so the join can also require the
-- same tenancy on both sides.
--
-- All default to the empty string, which is what every row written before this
-- migration holds; the control plane falls back to `(request_id, ts)` for
-- exactly those rows.

alter table request_logs
    add column if not exists log_id String default '';

alter table request_payloads
    add column if not exists log_id String default '';

alter table request_payloads
    add column if not exists org_id String default '';

alter table request_payloads
    add column if not exists project_id String default '';
