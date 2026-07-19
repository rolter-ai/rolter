-- MCP tool-call event history. Arguments/results are written only when the
-- configured payload-capture policy is enabled and have already been redacted.
-- This stays independent of a specific MCP transport or proxy implementation.
create table if not exists mcp_tool_call_logs (
    ts             DateTime64(3) default now64(3),
    event_id       String,
    server         LowCardinality(String),
    tool           LowCardinality(String),
    transport      Enum8('stdio' = 1, 'sse' = 2, 'streamable_http' = 3, 'websocket' = 4),
    status         Enum8('success' = 1, 'timeout' = 2, 'auth_denied' = 3, 'transport_error' = 4, 'error' = 5),
    latency_ms     UInt32,
    org_id         String,
    team_id        String,
    project_id     String,
    virtual_key_id String,
    user_id        String,
    request_id     String,
    trace_id       String,
    arguments      String,
    result         String,
    error          String
)
engine = MergeTree
partition by toYYYYMMDD(ts)
order by (ts, server, tool, event_id)
ttl toDateTime(ts) + interval 90 day;
