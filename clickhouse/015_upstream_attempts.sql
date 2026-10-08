-- What a request did upstream, beside what the caller was told (#2807).
--
-- `status` is the answer the caller received. When the gateway answers with an
-- error of its own after its upstreams failed, that status says the request
-- failed but not why: a provider's 429 that ran out of targets reached the
-- caller as a 429 or 503 the gateway made, and a reader of the log had to guess
-- which upstream status caused it.
--
-- `upstream_status` is the HTTP status of the last upstream attempt that
-- answered, 0 when none did (a refusal, a cache hit, the built-in model, or a
-- connection that failed before any status line). `attempts` is how many
-- upstream attempts the request made, the one that answered included, 0 when
-- it never reached an upstream.
--
-- Both default to 0, the reading every older row was written under.

alter table request_logs
    add column if not exists upstream_status UInt16 default 0;

alter table request_logs
    add column if not exists attempts UInt8 default 0;
