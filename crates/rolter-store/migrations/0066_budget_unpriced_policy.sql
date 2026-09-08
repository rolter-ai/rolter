-- per-budget override for the deployment-wide unpriced-traffic policy (#996).
--
-- #974 made `unpriced_policy` a deployment-wide setting: what the gateway does
-- about traffic against a model with no price row, which accrues zero spend and
-- so can never exhaust a budget. Deployment-wide meant an org that wanted to
-- refuse unaccountable traffic had to impose that on every tenant, and a single
-- team that wanted stricter accounting than the rest could not have it.
--
-- null means "inherit the deployment-wide setting", which is what every
-- existing row gets. Resolution at request time is most-restrictive-wins
-- (ignore < warn < block) across the scope chain and against the deployment
-- setting, matching how the caps themselves compose: a project budget can
-- tighten what the org asked for, never loosen it.
--
-- no trigger is added here: `budgets` already bumps `config_version` on insert,
-- update and delete (`0004_config_version_pricing_limits.sql`), so an update to
-- this column propagates through `/internal/snapshot` like any other budget
-- change.
alter table budgets
    add column if not exists unpriced_policy text
        check (unpriced_policy in ('ignore', 'warn', 'block'));
