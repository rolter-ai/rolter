-- #3476: a model label can name the organization whose model it is.
--
-- A model is addressed by name (`subject_id`), and since #1857 two
-- organizations may each serve a `gpt-4o`. With no organization on the row,
-- their catalog entries shared one label set: a label put on one org's
-- `gpt-4o` showed on the other's, and the label filter matched both.
--
-- `org_id` is null for a deployment-wide label, which is what every existing
-- row is and what a model from the gateway's config file keeps: such a model
-- belongs to no organization. A lookup for an organization's model prefers its
-- own label and falls back to the deployment-wide one of the same key
-- (`LabelRepo::list_models_for_org`). Only a model label names an
-- organization: a provider, group or route already belongs to one through its
-- row.

alter table labels
    add column if not exists org_id uuid references orgs (id) on delete cascade;

alter table labels drop constraint if exists labels_org_only_on_models;
alter table labels
    add constraint labels_org_only_on_models
        check (org_id is null or subject_type = 'model');

-- the uniqueness gains the organization, with null as one value: two
-- deployment-wide labels of one key on one model still collide, while org A's
-- and org B's `gpt-4o` may each carry their own. an index on an expression
-- rather than `unique nulls not distinct`, which needs Postgres 15: the nil
-- uuid stands in for "no organization", and no organization is issued it.
-- every `on conflict` on the table names this same expression list
-- (`LabelRepo::create_custom`, `LabelRepo::upsert_auto`)
alter table labels drop constraint if exists labels_unique_per_subject;
create unique index if not exists labels_subject_org_key_idx on labels
    (subject_type, coalesce(org_id, '00000000-0000-0000-0000-000000000000'::uuid),
     subject_id, source, key);

create index if not exists labels_org_subject_idx
    on labels (org_id, subject_type, subject_id)
    where org_id is not null;

-- the gateway reads no label today, so no snapshot depends on this column.
-- `labels_bump_config_version` (0068_labels.sql) already bumps
-- `config_version` for every write to the table, the cascade from a deleted
-- organization included, so a label the data plane reads later propagates
-- without another trigger
