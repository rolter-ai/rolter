-- Marks an `mcp_tool_call_logs` row recorded for a JSON-RPC notification
-- (#3466): a `tools/call` sent without an `id`, which no reply answers. The
-- row's `status` is then only what the HTTP layer answered (`success` for a
-- `2xx`), never what the tool did, and the flag is how a reader tells the two
-- apart. `0` for every other row.
--
-- Apply by hand on an existing deployment. The gateway sends its inserts with
-- `input_format_skip_unknown_fields=1` and leaves the field out of a row that
-- is not a notification, and the control plane probes for the column, so
-- neither side has to wait for the other to be upgraded. Until it is applied a
-- notification is recorded as an ordinary `success` row.
alter table mcp_tool_call_logs
    add column if not exists notification UInt8 default 0;
