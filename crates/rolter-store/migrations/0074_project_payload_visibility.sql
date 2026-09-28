-- who may read the request and response bodies payload capture stored for a
-- project's traffic (#1820). `member` is the default, so a read-only viewer sees
-- a request-log row without the prompt and completion inside it; a project
-- admin may lower the bar to `viewer` for a project whose viewers debug beside
-- its engineers. the control plane reads this when it filters analytics and the
-- gateway never does, so no `bump_config_version()` trigger is needed
alter table projects
    add column if not exists payload_min_role text not null default 'member'
        check (payload_min_role in ('member', 'viewer'));
