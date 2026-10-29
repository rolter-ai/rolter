-- #3459: one route name per org, enforced by the database.
--
-- The gateway names routes by `(org, model)` (#1857, ADR-0040): a key resolves
-- a name in its own org, not its own project, so two projects of one org may
-- not both hold `gpt-4o`. Until now only `unique (project_id, model)` existed,
-- and the per-org rule was a check-then-insert in the control plane, so two
-- concurrent creates in two projects of one org could both succeed. The
-- duplicate then failed structural snapshot validation and froze every gateway
-- on its last config (#1845, reached through a race).
--
-- `routes.org_id` is a copy of the org the route's project belongs to (two
-- joins away, projects -> teams), kept in sync by triggers, so a plain unique
-- constraint can hold the rule.

alter table routes
    add column if not exists org_id uuid references orgs (id) on delete cascade;

update routes r
   set org_id = t.org_id
  from projects p
  join teams t on t.id = p.team_id
 where p.id = r.project_id
   and r.org_id is distinct from t.org_id;

-- a duplicate within one org can only come from the race above, which already
-- breaks `/internal/snapshot`. Refuse to guess which row to keep and name them
-- all; `docs/user-docs/deployment/upgrading.mdx` says how to find and resolve
-- them
do $$
declare
    dupes text;
begin
    select string_agg(format('org %s: route %L in projects %s', org_id, model, projects), '; '
                      order by org_id, model)
      into dupes
      from (select org_id, model,
                   string_agg(project_id::text, ', ' order by project_id) as projects
              from routes
             group by org_id, model
            having count(*) > 1) d;
    if dupes is not null then
        raise exception 'cannot enforce one route name per org: duplicate routes exist (%). '
                        'Rename or delete all but one of each, then restart; '
                        'see the upgrading guide', dupes
            using errcode = '23505';
    end if;
end;
$$;

alter table routes alter column org_id set not null;

alter table routes
    add constraint routes_org_model_unique unique (org_id, model);

-- a route takes its org from its project, on insert and when it moves
create or replace function routes_set_org_id() returns trigger
language plpgsql as $$
begin
    select t.org_id into new.org_id
      from projects p
      join teams t on t.id = p.team_id
     where p.id = new.project_id;
    return new;
end;
$$;

drop trigger if exists routes_set_org_id on routes;
create trigger routes_set_org_id
    before insert or update of project_id, org_id on routes
    for each row execute function routes_set_org_id();

-- the store never moves a project to another team or a team to another org,
-- but a hand-run update must not leave a stale copy behind
create or replace function projects_sync_route_org_id() returns trigger
language plpgsql as $$
begin
    update routes r
       set org_id = t.org_id
      from teams t
     where r.project_id = new.id
       and t.id = new.team_id
       and r.org_id is distinct from t.org_id;
    return null;
end;
$$;

drop trigger if exists projects_sync_route_org_id on projects;
create trigger projects_sync_route_org_id
    after update of team_id on projects
    for each row execute function projects_sync_route_org_id();

create or replace function teams_sync_route_org_id() returns trigger
language plpgsql as $$
begin
    update routes r
       set org_id = new.org_id
      from projects p
     where p.team_id = new.id
       and r.project_id = p.id
       and r.org_id is distinct from new.org_id;
    return null;
end;
$$;

drop trigger if exists teams_sync_route_org_id on teams;
create trigger teams_sync_route_org_id
    after update of org_id on teams
    for each row execute function teams_sync_route_org_id();

-- the data plane reads `routes`, and `routes_bump_config_version` (0003) is a
-- statement trigger on every insert, update and delete with no column list, so
-- the backfill and every sync above already bump `config_version`. It is kept
-- as it is.
