-- #1919: an optional project scope for providers and provider groups.
--
-- `advanced.visibility.project_only` narrows a route to its own project but
-- not the provider or group behind it: a key from another project of the org
-- reached the same credential through `provider-slug/model`. A provider or
-- group with `project_id` set is served only to keys minted in that project,
-- through a route or through its `slug/model` address. `null` keeps today's
-- meaning, org-wide, so every existing row stays exactly as reachable as it was.
--
-- no `on delete` action (no action): the obvious alternatives both lose.
-- `set null` would silently widen a project's private credential to the whole
-- org when the project is deleted, and `cascade` would destroy the credential
-- and every route and group member behind it. `no action` refuses the delete,
-- and unlike `restrict` it checks at the end of the statement, so deleting an
-- org or team, which cascades to both the project and the provider, still works.

alter table providers
    add column if not exists project_id uuid references projects (id);
alter table provider_groups
    add column if not exists project_id uuid references projects (id);

create index if not exists idx_providers_project
    on providers (project_id) where project_id is not null;
create index if not exists idx_provider_groups_project
    on provider_groups (project_id) where project_id is not null;

-- a foreign key cannot say that the project belongs to the row's own org (the
-- org sits two joins away, projects -> teams), so a trigger does. Without it a
-- row could be scoped to another org's project, which would make the provider
-- unreachable to its own org and the scope meaningless.
create or replace function check_scope_project_in_org() returns trigger
language plpgsql as $$
begin
    if new.project_id is not null and not exists (
        select 1
          from projects p
          join teams t on t.id = p.team_id
         where p.id = new.project_id and t.org_id = new.org_id
    ) then
        raise exception 'project % does not belong to org %', new.project_id, new.org_id
            using errcode = '23514';
    end if;
    return new;
end;
$$;

drop trigger if exists providers_scope_project_in_org on providers;
create trigger providers_scope_project_in_org
    before insert or update of project_id, org_id on providers
    for each row execute function check_scope_project_in_org();

drop trigger if exists provider_groups_scope_project_in_org on provider_groups;
create trigger provider_groups_scope_project_in_org
    before insert or update of project_id, org_id on provider_groups
    for each row execute function check_scope_project_in_org();

-- the data plane reads both tables, and `providers_bump_config_version`
-- (0003) and `provider_groups_bump_config_version` (0071) are statement
-- triggers on every insert, update and delete with no column list, so a
-- change to `project_id` already bumps `config_version`. nothing to extend.
