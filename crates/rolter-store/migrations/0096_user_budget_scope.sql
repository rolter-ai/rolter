-- budgets and rate limits may cap a person, counted across every key they
-- minted (`virtual_keys.created_by`), so "each engineer may spend $50 a month
-- however many keys they mint" is expressible (#1830). a key cap is per key and
-- a project cap is shared by everyone in the project; neither can say that.
--
-- a person belongs to no org, so a cap on one may be narrowed to the keys they
-- minted in one org or in one project. the narrowing is what ties the row to
-- somewhere an org admin may reach: a row with neither column set counts the
-- person's keys everywhere, and only a superadmin can write that.
--
-- no trigger is added: `budgets` and `rate_limits` already bump
-- `config_version` on insert, update and delete (0004), and the cascade below
-- is a delete, so it propagates through `/internal/snapshot` like any other
-- cap change.
alter table budgets
    drop constraint if exists budgets_scope_type_check;
alter table budgets
    add constraint budgets_scope_type_check
    check (scope_type in
        ('org', 'team', 'project', 'virtual_key', 'business_unit', 'customer', 'user'));

alter table rate_limits
    drop constraint if exists rate_limits_scope_type_check;
alter table rate_limits
    add constraint rate_limits_scope_type_check
    check (scope_type in
        ('org', 'team', 'project', 'virtual_key', 'business_unit', 'customer', 'user'));

alter table budgets
    add column if not exists within_org_id uuid references orgs (id) on delete cascade,
    add column if not exists within_project_id uuid references projects (id) on delete cascade,
    add constraint budgets_within_one_place
        check (within_org_id is null or within_project_id is null),
    add constraint budgets_within_only_for_users
        check ((within_org_id is null and within_project_id is null) or scope_type = 'user');

alter table rate_limits
    add column if not exists within_org_id uuid references orgs (id) on delete cascade,
    add column if not exists within_project_id uuid references projects (id) on delete cascade,
    add constraint rate_limits_within_one_place
        check (within_org_id is null or within_project_id is null),
    add constraint rate_limits_within_only_for_users
        check ((within_org_id is null and within_project_id is null) or scope_type = 'user');
