-- A post-call policy (an output guardrail or a post-response plugin) can refuse
-- to deliver a completion the provider already produced and billed. Those rows
-- carry the refusal the caller received in `status` (403) and the provider's
-- tokens and cost in the usual columns; `withheld` says the two diverged, so
-- spend on answers nobody saw is countable rather than invisible (#1478).
--
-- `usage_unknown` separates "the upstream answered and reported no usage" from
-- a genuine zero -- an OpenAI-style stream without
-- `stream_options.include_usage` is the common case.
--
-- Both default to 0, the reading every older row was written under.

alter table request_logs
    add column if not exists withheld UInt8 default 0;

alter table request_logs
    add column if not exists usage_unknown UInt8 default 0;
