-- The OAuth scopes an upstream asked for when it refused an MCP call with
-- `insufficient_scope` (#3375). The row's `error` says `insufficient scope`; the
-- scopes are the validated, bounded ones the gateway also returns to the caller
-- as `error.required_scopes`, so the dashboard can offer the step-up for
-- exactly those. Empty for every other row.
--
-- Apply by hand on an existing deployment. The gateway sends its inserts with
-- `input_format_skip_unknown_fields=1` and leaves the field out of a row that
-- has none, and the control plane probes for the column, so neither side has to
-- wait for the other to be upgraded.
alter table mcp_tool_call_logs
    add column if not exists error_scopes Array(String) default [];
