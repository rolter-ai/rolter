-- the last prompt-cache reporting probe of a provider (#984): whether a
-- completion answered by that provider carried a cache-usage field, and when
-- that was observed. one row per provider; a provider with no row has never
-- been probed, which the dashboard shows apart from "does not report".
--
-- the gateway never reads this table, so it needs no `bump_config_version()`
-- trigger, and a probe does not make the data plane reload its snapshot.
create table if not exists provider_cache_probes (
    provider_id uuid primary key references providers (id) on delete cascade,
    -- whether the probed response carried one of the spellings of
    -- `rolter_proxy::cached_prompt_tokens`, a zero included
    reports    boolean not null,
    -- the spelling that was found (a JSON pointer such as
    -- `/prompt_tokens_details/cached_tokens`); null when `reports` is false
    spelling   text,
    -- the upstream model the probe asked, so a verdict can be read against it
    model      text not null,
    probed_at  timestamptz not null default now()
);
