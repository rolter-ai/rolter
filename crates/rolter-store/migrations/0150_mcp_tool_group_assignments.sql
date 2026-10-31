-- Assign an MCP tool group to a virtual key, a project or a team (#3295).
--
-- A group used to apply to every caller in its organization. A caller with an
-- assignment is now held to the groups assigned to its key, its project and its
-- team, and a caller with none keeps the organization-wide behaviour, so this
-- table being empty changes nothing for an existing deployment.
--
-- The subject is polymorphic, so it carries no foreign key: a deleted key,
-- project or team leaves a row that can never match a caller, and the control
-- plane refuses a subject outside the group's organization when it writes one.
-- The group's own foreign key removes its assignments with it.

create table if not exists mcp_tool_group_assignments (
    group_id     uuid not null references mcp_tool_groups (id) on delete cascade,
    subject_kind text not null check (subject_kind in ('virtual_key', 'project', 'team')),
    subject_id   uuid not null,
    created_at   timestamptz not null default now(),
    primary key (group_id, subject_kind, subject_id)
);

create index if not exists mcp_tool_group_assignments_subject_idx
    on mcp_tool_group_assignments (subject_kind, subject_id);
