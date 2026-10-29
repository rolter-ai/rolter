-- Drop an MCP tool group's assignments when their subject is deleted (#3382).
--
-- `mcp_tool_group_assignments.subject_id` names a key, a project or a team
-- depending on `subject_kind`, so it cannot carry a foreign key (0150). A
-- deleted subject therefore left a row that could never match a caller but
-- stayed in the assignments listing and the snapshot. These statement-level
-- triggers sweep the rows in the deleting transaction, as the labels sweep in
-- 0068 does; the assignments' own bump trigger (0151) then moves
-- `config_version` once. A project's keys go with the project by cascade, and
-- the cascade fires the keys' trigger too.

create or replace function delete_mcp_tool_group_assignments_for_subject()
returns trigger as $$
begin
    -- a statement trigger on the assignments fires even when no row matches,
    -- and every bump wakes every gateway, so a delete that names no subject
    -- with an assignment must not reach the table at all
    if exists (select 1 from mcp_tool_group_assignments
                where subject_kind = tg_argv[0]
                  and subject_id in (select id from deleted_subjects)) then
        delete from mcp_tool_group_assignments
         where subject_kind = tg_argv[0]
           and subject_id in (select id from deleted_subjects);
    end if;
    return null;
end;
$$ language plpgsql;

drop trigger if exists virtual_keys_delete_mcp_assignments on virtual_keys;
create trigger virtual_keys_delete_mcp_assignments
after delete on virtual_keys
referencing old table as deleted_subjects
for each statement execute function delete_mcp_tool_group_assignments_for_subject('virtual_key');

drop trigger if exists projects_delete_mcp_assignments on projects;
create trigger projects_delete_mcp_assignments
after delete on projects
referencing old table as deleted_subjects
for each statement execute function delete_mcp_tool_group_assignments_for_subject('project');

drop trigger if exists teams_delete_mcp_assignments on teams;
create trigger teams_delete_mcp_assignments
after delete on teams
referencing old table as deleted_subjects
for each statement execute function delete_mcp_tool_group_assignments_for_subject('team');

-- rows orphaned before this migration
delete from mcp_tool_group_assignments a
 where (a.subject_kind = 'virtual_key' and not exists (select 1 from virtual_keys k where k.id = a.subject_id))
    or (a.subject_kind = 'project' and not exists (select 1 from projects p where p.id = a.subject_id))
    or (a.subject_kind = 'team' and not exists (select 1 from teams t where t.id = a.subject_id));
