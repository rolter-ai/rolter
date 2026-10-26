-- let a database-backed provider opt out of its kind's host pin, as a file
-- provider can with `allow_custom_api_base` (ADR-0029, #1133). providers
-- already carries the bump_config_version() trigger (0003), so the change
-- reaches the snapshot without a new one
alter table providers add column if not exists allow_custom_api_base boolean not null default false;
