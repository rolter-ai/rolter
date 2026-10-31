-- which ClickHouse stores an erasure searched and which it skipped, and why
-- (#3436). an erasure of a user who holds no virtual keys cannot find request
-- logs, which are keyed by virtual key, and still erases that user's MCP
-- tool calls; without this record an operator reading the trail would take
-- the erasure for complete over every store. a list of
-- `{store, searched, reason?}`; rows from before this column hold `[]`, meaning
-- the record does not say.
--
-- the gateway never reads this table, so it needs no `bump_config_version()`
-- trigger.
alter table log_erasures
    add column if not exists stores jsonb not null default '[]'::jsonb;
