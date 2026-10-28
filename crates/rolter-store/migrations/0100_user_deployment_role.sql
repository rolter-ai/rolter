-- #1834: a delegable deployment role on the account. everything deployment-scoped
-- (cluster, runtime policy, alerting, security settings, ...) used to be
-- superadmin-only, so a person who only drains nodes or only audits settings
-- needed a superadmin account, which also reads every tenant. `deployment_role`
-- names the narrower job: `operator` (maintain the deployment) or
-- `security_auditor` (read how it is secured). null is no deployment role.
--
-- one role per account, set only by a superadmin through
-- `PUT /api/v1/users/{id}/deployment-role`. it grants nothing at any org, team
-- or project: it is read only by the guard on deployment-wide capabilities.
--
-- the data plane does not read this column: the snapshot reads only
-- `deactivated_at` and `is_superadmin` from `users`, and the
-- `users_bump_config_version` trigger (0075) is `after update of` exactly those
-- two, so changing a deployment role must not, and does not, bump
-- `config_version` and needs no new trigger.

alter table users
    add column deployment_role text;

alter table users
    add constraint users_deployment_role_known
    check (deployment_role is null or deployment_role in ('operator', 'security_auditor'));
