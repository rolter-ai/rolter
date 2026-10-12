-- What each guardrail rule did to a request (#2158).
--
-- One entry per rule that acted on the request, as `<action>:<rule>` where the
-- action is `blocked`, `redacted` or `annotated` and the rule is its name, plus
-- the bare `provider_blocked` / `provider_transformed` for the active guardrail
-- provider. Names only: the matched text never reaches a log row. The control
-- plane's `GET /api/v1/analytics/guardrails` counts the entries per rule, and
-- the dashboard shows them on the Guardrail Rules and Guardrail Providers
-- screens. It is empty for every row older than the column and for a request no
-- guardrail touched.
--
-- A gateway inserts with `input_format_skip_unknown_fields=1` and leaves the
-- field out of a row with no events, so one upgraded before this is applied
-- keeps logging and loses only the events. The control plane answers the
-- guardrails read with an empty list while the table lacks the column, so
-- applying this file is not a precondition of upgrading it.

alter table request_logs
    add column if not exists guardrail_events Array(String) default [];
