-- one labelling primitive for providers, provider groups, routes and models
-- (#985). two kinds of fact share the table because they are displayed and
-- filtered together: an `auto` label is rolter's own observation and carries
-- when it was made and what made it, a `custom` label is an operator's
-- assertion and rolter never interprets it.

create table if not exists labels (
    id           uuid primary key default gen_random_uuid(),
    -- a model is addressed by name and everything else by id, so this is text
    -- rather than uuid; see the cleanup triggers below for what that costs
    subject_type text not null
                 check (subject_type in ('provider', 'provider_group', 'route', 'model')),
    subject_id   text not null,
    key          text not null,
    value        text,
    source       text not null check (source in ('auto', 'custom')),
    -- provenance, and only auto labels have it: an observation was made at a
    -- point in time by something nameable, and can therefore go stale
    observed_at  timestamptz,
    observation  text,
    created_at   timestamptz not null default now(),
    updated_at   timestamptz not null default now(),
    constraint labels_key_charset
        check (key ~ '^[a-z0-9][a-z0-9._:-]{0,62}$'),
    constraint labels_value_shape
        check (value is null or (length(value) between 1 and 128)),
    -- an operator must not be able to hand-write a label that looks probed
    constraint labels_provenance check (
        (source = 'auto' and observed_at is not null)
        or (source = 'custom' and observed_at is null and observation is null)
    ),
    -- source is part of the uniqueness so an auto label can never silently
    -- overwrite a custom one sharing its key, nor the other way round
    constraint labels_unique_per_subject
        unique (subject_type, subject_id, source, key)
);

create index if not exists labels_subject_idx on labels (subject_type, subject_id);
-- filtering a list by label is a lookup on the key, optionally with a value
create index if not exists labels_key_idx on labels (key, value);

-- labels are display-and-filter only for now, but the schema is deliberately
-- shaped so label-based routing can be added without rewriting it, and the
-- moment a route selects on a label the data plane reads this table. bumping
-- from the start means that change needs no migration to start propagating
drop trigger if exists labels_bump_config_version on labels;
create trigger labels_bump_config_version
after insert or update or delete on labels
for each statement execute function bump_config_version();

-- a text subject_id rules out a foreign key, so deletion has to be swept by
-- hand or a label outlives the row it describes and, worse, is inherited by
-- the next row to be issued the same id. statement-level with a transition
-- table rather than per row, so deleting n providers sweeps once instead of n
-- times and costs one config_version bump instead of n
create or replace function delete_labels_for_subject() returns trigger as $$
begin
    delete from labels
     where subject_type = tg_argv[0]
       and subject_id in (select id::text from deleted_subjects);
    return null;
end;
$$ language plpgsql;

drop trigger if exists providers_delete_labels on providers;
create trigger providers_delete_labels
after delete on providers
referencing old table as deleted_subjects
for each statement execute function delete_labels_for_subject('provider');

drop trigger if exists provider_groups_delete_labels on provider_groups;
create trigger provider_groups_delete_labels
after delete on provider_groups
referencing old table as deleted_subjects
for each statement execute function delete_labels_for_subject('provider_group');

drop trigger if exists routes_delete_labels on routes;
create trigger routes_delete_labels
after delete on routes
referencing old table as deleted_subjects
for each statement execute function delete_labels_for_subject('route');
