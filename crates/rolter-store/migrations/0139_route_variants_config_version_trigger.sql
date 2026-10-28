-- the gateway reads both variant tables through `/internal/snapshot`, so every
-- write to them bumps config_version inside its own transaction, the way
-- `0003` does for routes and route_targets. without these a variant edit would
-- never propagate and the gateway would keep serving the old split (#391)

drop trigger if exists route_variants_bump_config_version on route_variants;
create trigger route_variants_bump_config_version
    after insert or update or delete on route_variants
    for each statement execute function bump_config_version();

drop trigger if exists route_variant_targets_bump_config_version on route_variant_targets;
create trigger route_variant_targets_bump_config_version
    after insert or update or delete on route_variant_targets
    for each statement execute function bump_config_version();
