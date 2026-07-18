-- opt-in raw request/response bodies for the invocation detail viewer
--
-- Kept out of request_logs so sensitive payloads can expire much sooner than
-- aggregate metadata. The gateway only inserts a row when [logging.payload_capture]
-- is explicitly enabled.

create table if not exists request_payloads (
    ts               DateTime64(3) default now64(3),
    request_id       String,
    request_payload  String,
    response_payload String
)
engine = MergeTree
partition by toYYYYMMDD(ts)
order by (request_id, ts)
ttl toDateTime(ts) + interval 7 day;
