-- deployment-level opt-out for the dashboard UX event stream (#1748). the
-- control plane reads `logging.ui_events` from the stored config, and without a
-- column here a postgres deployment could only ever see the serde default. the
-- `logging_settings_bump_config_version` statement trigger from 0034 already
-- fires on any update of this table, so the new column needs no trigger of its own
alter table logging_settings
    add column if not exists ui_events boolean not null default true;
