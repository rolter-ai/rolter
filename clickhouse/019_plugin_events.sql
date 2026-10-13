-- What each plugin did to a request (#2243).
--
-- One entry per plugin call, as `<outcome>:<slug>` where the outcome is
-- `allowed`, `blocked`, `transformed` or `annotated`, or `error:<slug>:<kind>`
-- when the call produced no verdict (`kind` is one of `refused`, `timeout`,
-- `connect`, `status_<code>` or `body`). Slugs and that fixed vocabulary only:
-- nothing a plugin answered, and no request content, reaches a log row. The
-- control plane's `GET /api/v1/analytics/plugins` counts the entries per
-- plugin, and the Plugins screen shows them on each card. It is empty for every
-- row older than the column and for a request no plugin was consulted on.
--
-- A gateway inserts with `input_format_skip_unknown_fields=1` and leaves the
-- field out of a row with no events, so one upgraded before this is applied
-- keeps logging and loses only the events. The control plane answers the
-- plugins read with `recorded: false` while the table lacks the column, so
-- applying this file is not a precondition of upgrading it.

alter table request_logs
    add column if not exists plugin_events Array(String) default [];
