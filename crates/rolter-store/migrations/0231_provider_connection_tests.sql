-- the last connection test of a provider (#3649): whether the upstream
-- answered as the kind's API, how long it took and how the credential was
-- resolved, written each time "Test connection" runs. one row per provider; a
-- provider with no row has never been tested, which the dashboard shows apart
-- from a failed test.
--
-- the gateway never reads this table, so it needs no `bump_config_version()`
-- trigger, and a test does not make the data plane reload its snapshot.
create table if not exists provider_connection_tests (
    provider_id  uuid primary key references providers (id) on delete cascade,
    -- the probe demonstrated a working provider API, as the test reports it
    reachable    boolean not null,
    -- the upstream's HTTP status; null when the request never completed
    status       integer,
    latency_ms   bigint not null,
    -- how the credential was resolved: stored, env, env (unset), none, ...
    credential   text not null,
    -- how many models the upstream listed, when it answered with a catalogue
    models_found integer,
    -- the test's own explanation of a failure, never the upstream's raw body
    error        text,
    tested_at    timestamptz not null default now()
);
