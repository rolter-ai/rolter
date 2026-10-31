-- #1836: scoped control-plane API tokens for automation (a service account).
-- a CI job that rotates a project key, or a pipeline that applies routes, used
-- to choose between the admin token (superadmin over everything, forever) and
-- a person's session. a token holds one role at one org, team or project, and
-- the control plane resolves it to that role, so every guard applies unchanged.
--
-- the scope is stored as the full chain: `org_id` always, `team_id` for a team
-- or project token, `project_id` for a project token, so a row names the
-- chain it is checked at without a lookup. the most specific non-null id is
-- the scope, as for `memberships`.
--
-- the role is a built-in one (`role`) or an org-defined one (`custom_role_id`),
-- exactly one of the two. a token never holds superadmin or a deployment role:
-- neither column can express them.
--
-- like sessions, virtual keys and scim tokens, only a peppered sha-256 of the
-- bearer value is stored; the plaintext is shown once, at mint or rotation.
-- `token_prefix` is the first characters of the value, for recognising a token
-- in a list. a revoked or expired token never resolves.
--
-- the data plane does not read this table, so no bump_config_version() trigger
-- is needed.

create table api_tokens (
    id             uuid primary key default gen_random_uuid(),
    org_id         uuid not null references orgs (id) on delete cascade,
    team_id        uuid references teams (id) on delete cascade,
    project_id     uuid references projects (id) on delete cascade,
    name           text not null check (char_length(btrim(name)) between 1 and 80),
    role           text check (role in ('admin', 'member', 'viewer')),
    custom_role_id uuid references custom_roles (id) on delete cascade,
    token_hash     text not null unique,
    token_prefix   text not null,
    expires_at     timestamptz,
    created_by     uuid references users (id) on delete set null,
    created_at     timestamptz not null default now(),
    rotated_at     timestamptz,
    last_used_at   timestamptz,
    revoked_at     timestamptz,
    constraint api_tokens_one_role check ((role is null) <> (custom_role_id is null)),
    constraint api_tokens_chain check (project_id is null or team_id is not null)
);

create index api_tokens_org_idx on api_tokens (org_id);
