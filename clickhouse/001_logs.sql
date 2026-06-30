-- rolter request + cost logs (clickhouse)
--
-- high-volume, append-only log of every proxied request. written asynchronously
-- in batches off the request hot path. partitioned by day, ordered for the
-- common dashboard queries (by project + model over time).

create table if not exists request_logs (
    ts                DateTime64(3) default now64(3),
    request_id        String,
    org_id            String,
    team_id           String,
    project_id        String,
    virtual_key_id    String,
    model             String,
    provider          String,
    target            String,
    status            UInt16,
    stream            UInt8,
    cache_hit         UInt8,
    prompt_tokens     UInt32,
    completion_tokens UInt32,
    total_tokens      UInt32,
    cost_usd          Float64,
    latency_ms        UInt32,
    ttft_ms           UInt32,
    error             String
)
engine = MergeTree
partition by toYYYYMMDD(ts)
order by (ts, project_id, model)
ttl toDateTime(ts) + interval 90 day;
