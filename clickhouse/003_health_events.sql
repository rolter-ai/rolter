-- rolter provider health events (clickhouse) — ROL-197
--
-- append-only stream of per-target health observations from every signal:
-- active probes, the passive request funnel, and (later) opt-in llm-call and
-- status-page sources. written asynchronously in batches off the hot path, the
-- same plumbing as request_logs. feeds uptime %/MTTR rollups (ROL-198).
-- partitioned by day, ordered for the common per-target-over-time query.

create table if not exists provider_health_events (
    ts          DateTime64(3) default now64(3),
    target_id   String,
    provider    String,
    source      Enum8('passive' = 1, 'probe' = 2, 'llm_call' = 3, 'status_page' = 4),
    outcome     Enum8('ok' = 1, 'error' = 2, 'timeout' = 3),
    status_code Nullable(UInt16),
    latency_ms  UInt32,
    error_kind  LowCardinality(Nullable(String))
)
engine = MergeTree
partition by toYYYYMMDD(ts)
order by (ts, provider, target_id)
ttl toDateTime(ts) + interval 90 day;
