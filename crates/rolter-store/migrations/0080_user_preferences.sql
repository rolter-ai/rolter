-- #1824: a user's own preferences (dashboard language, default scope, default
-- playground model, chart time zone), served by `GET`/`PUT /api/v1/me/preferences`
-- so they follow the account to a new browser instead of living in localStorage.
--
-- a table of its own rather than a column on `users`: `users` is read by the
-- auth path on every request and by the snapshot, and a document that grows a
-- key per feature does not belong in that row. `prefs` is deliberately a
-- schemaless object: the API validates every key, and a new preference needs
-- no migration. a row exists only once the user has saved something.
--
-- the data plane does not read this table, so there is no
-- `bump_config_version()` trigger: a preference write must not make every
-- gateway re-fetch its snapshot.

create table user_preferences (
    user_id    uuid primary key references users (id) on delete cascade,
    prefs      jsonb not null default '{}'::jsonb
               constraint user_preferences_prefs_is_object
               check (jsonb_typeof(prefs) = 'object'),
    updated_at timestamptz not null default now()
);
