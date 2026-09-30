-- alert rules gain a comparison direction and a policy for a window with no
-- data. the defaults are today's behaviour: fire at or above the threshold,
-- and leave the state alone when there is nothing to measure.
--
-- no bump_config_version() trigger: only the control plane evaluates alert
-- rules, and the data plane's snapshot never reads this table.
alter table alert_rules
    add column if not exists comparison text not null default 'above'
        check (comparison in ('above', 'below')),
    add column if not exists no_data text not null default 'ignore'
        check (no_data in ('ignore', 'fire', 'ok'));
