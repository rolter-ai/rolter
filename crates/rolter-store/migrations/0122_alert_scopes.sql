-- scoped alert rules and org-owned channels (#1829).
--
-- Until now every channel and rule belonged to the deployment and only a
-- superadmin could touch them. A rule may now be scoped to an org, team,
-- project, virtual key, business unit or customer, and evaluates only that
-- scope's rows; a channel may belong to one org.
--
-- `org_id` is the owning org and is NULL for a deployment-wide row, which stays
-- superadmin-only. On a rule it is derived from the scope when the rule is
-- created and never changes, so a rule can be checked against its channel's
-- org without walking the tenancy tree. The scope itself is polymorphic (like
-- budgets), so `scope_id` carries no foreign key: a rule whose team or project
-- was deleted reports a scope error when evaluated instead of firing.
--
-- Rows deleted with their org: a deployment's channels and rules that name no
-- org are untouched. Neither table is read by the data plane, so no
-- bump_config_version() trigger is needed.
alter table alert_channels
    add column if not exists org_id uuid references orgs (id) on delete cascade;

alter table alert_rules
    add column if not exists org_id uuid references orgs (id) on delete cascade,
    add column if not exists scope_type text
        check (scope_type in ('org', 'team', 'project', 'virtual_key', 'business_unit', 'customer')),
    add column if not exists scope_id uuid;

alter table alert_rules
    drop constraint if exists alert_rules_scope_complete;
alter table alert_rules
    add constraint alert_rules_scope_complete
    check ((scope_type is null) = (scope_id is null) and (scope_type is null) = (org_id is null));

-- names were unique deployment-wide; two orgs may now both have "prod errors"
alter table alert_channels drop constraint if exists alert_channels_name_key;
alter table alert_rules drop constraint if exists alert_rules_name_key;

create unique index if not exists alert_channels_deployment_name_idx
    on alert_channels (name) where org_id is null;
create unique index if not exists alert_channels_org_name_idx
    on alert_channels (org_id, name) where org_id is not null;
create unique index if not exists alert_rules_deployment_name_idx
    on alert_rules (name) where org_id is null;
create unique index if not exists alert_rules_org_name_idx
    on alert_rules (org_id, name) where org_id is not null;
