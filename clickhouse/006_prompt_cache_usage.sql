-- Provider-native prompt-cache usage is separate from Rolter response-cache
-- hits, allowing cost/reporting to distinguish reads from cache creation.

alter table request_logs
    add column if not exists cache_read_tokens UInt32 default 0;

alter table request_logs
    add column if not exists cache_write_tokens UInt32 default 0;
