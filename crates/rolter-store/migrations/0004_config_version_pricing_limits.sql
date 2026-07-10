-- extend the config_version bump triggers (migration 0003) to the remaining
-- snapshot inputs: model_prices, budgets and rate_limits. without these, CRUD
-- writes to pricing/caps/limits don't bump config_version, so snapshot-polling
-- gateways keep enforcing stale values until some other tracked change bumps
-- the version (or a restart). reuses the bump_config_version() function.

drop trigger if exists model_prices_bump_config_version on model_prices;
create trigger model_prices_bump_config_version
    after insert or update or delete on model_prices
    for each statement execute function bump_config_version();

drop trigger if exists budgets_bump_config_version on budgets;
create trigger budgets_bump_config_version
    after insert or update or delete on budgets
    for each statement execute function bump_config_version();

drop trigger if exists rate_limits_bump_config_version on rate_limits;
create trigger rate_limits_bump_config_version
    after insert or update or delete on rate_limits
    for each statement execute function bump_config_version();
