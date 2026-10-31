-- LDAP sign-in (#1826): one directory per org, the group→role mappings that
-- turn a directory group into a rolter membership, and the membership source
-- that lets a sign-in reconcile exactly the grants it produced.
--
-- Neither table is read by the data plane, so there is deliberately no
-- bump_config_version() trigger: a directory edit changes who may sign in to the
-- control plane, never what a gateway serves.

create table if not exists ldap_directories (
    id                     uuid primary key default gen_random_uuid(),
    org_id                 uuid not null references orgs (id) on delete cascade,
    name                   text not null,
    -- ldap:// or ldaps:// url of the directory
    url                    text not null,
    -- read-only service account used to find a user before binding as them
    bind_dn                text not null,
    -- sealed with the deployment KEK, exactly like provider credentials
    bind_secret_ciphertext bytea,
    bind_secret_nonce      bytea,
    -- subtree searched for users
    base_dn                text not null,
    -- search filter; {login} is replaced with the escaped login
    user_filter            text not null default '(uid={login})',
    email_attr             text not null default 'mail',
    display_name_attr      text not null default 'cn',
    -- attribute on the user entry listing their groups
    group_attr             text not null default 'memberOf',
    -- refuse a plain ldap:// url so the bind password is never sent in cleartext
    require_tls            boolean not null default true,
    -- role granted at the org when no group mapping matches; null denies login
    -- to a user the directory has not put in a mapped group
    default_role           text check (default_role in ('admin', 'member', 'viewer')),
    enabled                boolean not null default true,
    created_at             timestamptz not null default now(),
    -- sign-in has no org in hand, so one directory per org keeps a login's
    -- grants attributable to exactly one reconciliation
    constraint ldap_directories_org_unique unique (org_id),
    constraint ldap_directories_url_scheme check (url ~ '^ldaps?://.+'),
    constraint ldap_directories_secret_pair check (
        (bind_secret_ciphertext is null) = (bind_secret_nonce is null)
    )
);

-- a directory group granting a role at an org/team/project scope, with the same
-- "most specific non-null id" convention as sso_group_mappings
create table if not exists ldap_group_mappings (
    id           uuid primary key default gen_random_uuid(),
    directory_id uuid not null references ldap_directories (id) on delete cascade,
    group_name   text not null,
    org_id       uuid references orgs (id) on delete cascade,
    team_id      uuid references teams (id) on delete cascade,
    project_id   uuid references projects (id) on delete cascade,
    role         text not null check (role in ('admin', 'member', 'viewer')),
    created_at   timestamptz not null default now(),
    constraint ldap_group_mappings_unique
        unique (directory_id, group_name, org_id, team_id, project_id)
);

-- postgres treats nulls as distinct in a unique constraint, so the constraint
-- above cannot see two org-scoped rows as equal; this closes that hole
create unique index if not exists ldap_group_mappings_org_scope_unique
    on ldap_group_mappings (directory_id, group_name, role)
    where team_id is null and project_id is null;

create index if not exists ldap_group_mappings_directory_idx
    on ldap_group_mappings (directory_id, group_name);

-- a membership an ldap sign-in produced is reconciled on every sign-in, exactly
-- like an 'sso' or 'scim' row; 'manual' grants survive untouched
alter table memberships drop constraint if exists memberships_source_check;

alter table memberships
    add constraint memberships_source_check
    check (source in ('manual', 'sso', 'scim', 'ldap'));
