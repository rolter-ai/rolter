-- Which org's provider served a request (#1857, ADR-0040).
--
-- Two orgs may each have a provider called `openai`, so the bare name in
-- `provider` no longer says whose credential answered. `provider` keeps the
-- display name and this column records the org that owns it:
--
--   provider_org_id  the provider's org, empty for a global provider from the
--                    gateway's own config file (and for every row older than
--                    this file)
--
-- It differs from `org_id`, the key's org, when a key of an org is served by a
-- global provider. A gateway inserts with `input_format_skip_unknown_fields=1`
-- and leaves the field out of a row while it is empty, so one upgraded before
-- this is applied keeps logging and loses only the attribution.

alter table request_logs
    add column if not exists provider_org_id String default '' after provider;
