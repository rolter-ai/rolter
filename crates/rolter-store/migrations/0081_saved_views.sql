-- #1825: named filter presets a user saves on the LLM Logs and Dashboard
-- screens, served by `/api/v1/me/saved-views`.
--
-- a table of its own rather than a key in `user_preferences.prefs`: that
-- document is replaced whole by every `PUT`, so a list of named items nested
-- in it would be rewritten (and could be clobbered by a stale tab) on each
-- unrelated preference change, and a name cannot be unique, counted or
-- indexed inside it. one row per preset gives the per-user name uniqueness and
-- the per-surface cap a real constraint and a cheap query.
--
-- `filters` is an object whose keys the API allow-lists per surface; it is
-- private to the owner, so every query the API makes is keyed by `user_id`.
-- the name is unique per user and surface ignoring case, so "Errors" and
-- "errors" cannot both exist.
--
-- the data plane does not read this table, so there is no
-- `bump_config_version()` trigger: saving a preset must not make every
-- gateway re-fetch its snapshot.

create table saved_views (
    id         uuid primary key default gen_random_uuid(),
    user_id    uuid not null references users (id) on delete cascade,
    surface    text not null
               constraint saved_views_surface_known
               check (surface in ('llm_logs', 'dashboard')),
    name       text not null
               constraint saved_views_name_shape
               check (name = btrim(name) and char_length(name) between 1 and 80),
    filters    jsonb not null default '{}'::jsonb
               constraint saved_views_filters_is_object
               check (jsonb_typeof(filters) = 'object'),
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create unique index saved_views_user_surface_name
    on saved_views (user_id, surface, lower(name));
